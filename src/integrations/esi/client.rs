use super::{
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
            return Err(format!("{request_description} failed: {error}"));
        }
    };
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
    let response = response
        .error_for_status()
        .map_err(|error| format!("{request_description} failed: {error}"))?;
    let expires = header_value(response.headers(), EXPIRES);
    let etag = header_value(response.headers(), ETAG);
    let last_modified = header_value(response.headers(), LAST_MODIFIED);
    let allows_storage = !header_value(response.headers(), CACHE_CONTROL)
        .is_some_and(|value| value.to_ascii_lowercase().contains("no-store"));
    let body = response
        .bytes()
        .map_err(|error| format!("{request_description} response body failed: {error}"))?;
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
