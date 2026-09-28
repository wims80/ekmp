use super::{
    active_cooldown_error, rate_limit_scope, record_cooldown, unix_time, ApiCooldown,
    CachedResponse, Client, DeserializeOwned, EsiCache, HeaderMap, StatusCode, CACHE_CONTROL, ETAG,
    EXPIRES, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED, RETRY_AFTER, USER_AGENT,
    USER_AGENT_VALUE,
};

pub(super) fn cached_get_json<T: DeserializeOwned>(
    client: &Client,
    cache: &mut Option<EsiCache>,
    url: String,
    cacheable: bool,
    bearer_token: impl FnOnce() -> Result<Option<String>, String>,
    request_description: &str,
) -> Result<T, String> {
    if let Some(error) = active_cooldown_error() {
        return Err(error);
    }
    let cached = if cacheable {
        cache
            .as_ref()
            .and_then(|cache| cache.load(&url).ok())
            .flatten()
    } else {
        None
    };
    if let Some(entry) = cached.as_ref().filter(|entry| entry.fresh) {
        return deserialize_cached_response(entry, request_description);
    }

    let mut request = client.get(&url).header(USER_AGENT, USER_AGENT_VALUE);
    if let Some(token) = bearer_token()? {
        request = request.bearer_auth(token);
    }
    if let Some(etag) = cached.as_ref().and_then(|entry| entry.etag.as_deref()) {
        request = request.header(IF_NONE_MATCH, etag);
    } else if let Some(last_modified) = cached
        .as_ref()
        .and_then(|entry| entry.last_modified.as_deref())
    {
        request = request.header(IF_MODIFIED_SINCE, last_modified);
    }

    let response = match request.send() {
        Ok(response) => response,
        Err(error) => {
            if let Some(entry) = cached.as_ref() {
                return deserialize_cached_response(entry, request_description);
            }
            return Err(transport_error(request_description, &error));
        }
    };
    observe_rate_limit(&response);
    if response.status() == StatusCode::NOT_MODIFIED {
        let expires = header_value(response.headers(), EXPIRES);
        let etag = header_value(response.headers(), ETAG);
        let last_modified = header_value(response.headers(), LAST_MODIFIED);
        let Some(entry) = cached.as_ref() else {
            return Err(format!(
                "{request_description} returned 304 without a cached response"
            ));
        };
        if cacheable {
            if let Some(cache) = cache.as_ref() {
                let _ = cache.revalidate(
                    &url,
                    expires.as_deref(),
                    etag.as_deref(),
                    last_modified.as_deref(),
                );
            }
        }
        return deserialize_cached_response(entry, request_description);
    }
    if let Some(error) = esi_limit_error(&response, request_description) {
        return Err(error);
    }
    if response.status().is_server_error() {
        if let Some(entry) = cached.as_ref() {
            return deserialize_cached_response(entry, request_description);
        }
    }
    if !response.status().is_success() {
        return Err(format!(
            "{request_description} failed (HTTP {})",
            response.status().as_u16()
        ));
    }
    let expires = header_value(response.headers(), EXPIRES);
    let etag = header_value(response.headers(), ETAG);
    let last_modified = header_value(response.headers(), LAST_MODIFIED);
    let allows_storage = !header_value(response.headers(), CACHE_CONTROL)
        .is_some_and(|value| value.to_ascii_lowercase().contains("no-store"));
    let body = response
        .bytes()
        .map_err(|_| format!("{request_description} response body could not be read"))?;
    if cacheable && allows_storage {
        if let Some(cache) = cache.as_ref() {
            let _ = cache.store(
                &url,
                &body,
                expires.as_deref(),
                etag.as_deref(),
                last_modified.as_deref(),
            );
        }
    }
    serde_json::from_slice(&body)
        .map_err(|error| format!("{request_description} response invalid: {error}"))
}

pub(super) fn esi_limit_error(
    response: &reqwest::blocking::Response,
    description: &str,
) -> Option<String> {
    match response.status().as_u16() {
        420 => {
            let reset = response
                .headers()
                .get("x-esi-error-limit-reset")
                .and_then(|value| value.to_str().ok())
                .unwrap_or("an unspecified interval");
            Some(format!(
                "{description} reached ESI's error limit; retry after {reset} seconds"
            ))
        }
        429 => {
            let retry_after = header_value(response.headers(), RETRY_AFTER)
                .unwrap_or_else(|| "an unspecified delay".into());
            Some(format!(
                "{description} was rate limited; retry after {retry_after} seconds"
            ))
        }
        _ => None,
    }
}

pub(super) fn observe_rate_limit(response: &reqwest::blocking::Response) {
    let headers = response.headers();
    let group = headers
        .get("x-ratelimit-group")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let request_scope = rate_limit_scope();
    let scope = match (group, request_scope) {
        (Some(group), Some(request_scope)) => Some(format!("{group}:{request_scope}")),
        (Some(group), None) => Some(group),
        (None, Some(request_scope)) => Some(request_scope),
        (None, None) => headers
            .contains_key("x-esi-error-limit-remain")
            .then(|| "error-limit".to_string()),
    };

    let retry_seconds = header_value(headers, RETRY_AFTER)
        .as_deref()
        .and_then(parse_retry_after)
        .or_else(|| {
            (response.status().as_u16() == 420)
                .then(|| numeric_header(headers, "x-esi-error-limit-reset"))
                .flatten()
        });
    if let Some(seconds) = retry_seconds {
        record_cooldown(ApiCooldown {
            source: "esi".into(),
            scope: scope.clone(),
            until: unix_time().saturating_add(seconds),
            reason: Some(if response.status().as_u16() == 420 {
                "error limit".into()
            } else {
                "rate limit".into()
            }),
        });
    }

    let error_remaining = numeric_header(headers, "x-esi-error-limit-remain");
    if error_remaining == Some(0) {
        if let Some(seconds) = numeric_header(headers, "x-esi-error-limit-reset") {
            record_cooldown(ApiCooldown {
                source: "esi".into(),
                scope: scope.clone().or_else(|| Some("error-limit".into())),
                until: unix_time().saturating_add(seconds),
                reason: Some("error budget exhausted".into()),
            });
        }
    }

    // ESI exposes a group/user budget. Defer the next request as the remaining
    // share falls so a large enrichment run does not consume the whole group.
    let limit = headers
        .get("x-ratelimit-limit")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split('/').next())
        .and_then(|value| value.parse::<u64>().ok());
    let remaining = numeric_header(headers, "x-ratelimit-remaining");
    let delay = match limit.zip(remaining) {
        Some((limit, remaining)) if limit > 0 && remaining * 10 <= limit => Some(2),
        Some((limit, remaining)) if limit > 0 && remaining * 4 <= limit => Some(1),
        _ => None,
    };
    if let Some(seconds) = delay {
        record_cooldown(ApiCooldown {
            source: "esi".into(),
            scope,
            until: unix_time().saturating_add(seconds),
            reason: Some("low rate budget".into()),
        });
    }
}

fn numeric_header(headers: &HeaderMap, name: &'static str) -> Option<u64> {
    headers.get(name)?.to_str().ok()?.trim().parse().ok()
}

fn parse_retry_after(value: &str) -> Option<u64> {
    value.trim().parse().ok().or_else(|| {
        httpdate::parse_http_date(value)
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .map(|deadline| deadline.as_secs().saturating_sub(unix_time()))
    })
}

fn transport_error(description: &str, error: &reqwest::Error) -> String {
    let reason = if error.is_timeout() {
        "timed out"
    } else if error.is_connect() {
        "could not connect"
    } else {
        "transport failed"
    };
    format!("{description} {reason}")
}

fn deserialize_cached_response<T: DeserializeOwned>(
    response: &CachedResponse,
    request_description: &str,
) -> Result<T, String> {
    serde_json::from_slice(&response.body)
        .map_err(|error| format!("cached {request_description} response invalid: {error}"))
}

fn header_value(headers: &HeaderMap, name: reqwest::header::HeaderName) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}
