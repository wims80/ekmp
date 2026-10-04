use super::{COMPATIBILITY_DATE, ESI};
use crate::{
    clock::unix_time,
    integrations::{
        http::{self, ApiLog},
        ApiError, ApiResult,
    },
    models::ApiCooldown,
    persistence::esi_cache::{CachedResponse, EsiCache},
};
use reqwest::{
    blocking::{Client, Response},
    header::{
        HeaderMap, HeaderName, CACHE_CONTROL, ETAG, EXPIRES, IF_MODIFIED_SINCE, IF_NONE_MATCH,
        LAST_MODIFIED, RETRY_AFTER, WARNING,
    },
    StatusCode,
};
use serde::{de::DeserializeOwned, Serialize};

const COMPATIBILITY_DATE_HEADER: &str = "X-Compatibility-Date";

/// An ESI client with the local response cache and the operation's cooldown log.
pub(super) struct Esi<'a> {
    base: &'a str,
    http: &'static Client,
    cache: Option<EsiCache>,
    cooldowns: &'a ApiLog,
}

impl<'a> Esi<'a> {
    pub(super) fn live(cooldowns: &'a ApiLog) -> ApiResult<Self> {
        Self::new(ESI, EsiCache::open().ok(), cooldowns)
    }

    pub(super) fn new(
        base: &'a str,
        cache: Option<EsiCache>,
        cooldowns: &'a ApiLog,
    ) -> ApiResult<Self> {
        Ok(Self {
            base,
            http: http::client()?,
            cache,
            cooldowns,
        })
    }

    /// A cached, unauthenticated GET.
    pub(super) fn get<T: DeserializeOwned>(&self, path: &str, description: &str) -> ApiResult<T> {
        self.fetch(path, true, None, || Ok(None), description)
    }

    /// An unauthenticated GET that bypasses the response cache.
    pub(super) fn get_uncached<T: DeserializeOwned>(
        &self,
        path: &str,
        description: &str,
    ) -> ApiResult<T> {
        self.fetch(path, false, None, || Ok(None), description)
    }

    /// A cached GET whose bearer token is only requested when the network is used.
    pub(super) fn get_authed<T: DeserializeOwned>(
        &self,
        path: &str,
        rate_limit_scope: &str,
        token: impl FnOnce() -> ApiResult<String>,
        description: &str,
    ) -> ApiResult<T> {
        self.fetch(
            path,
            true,
            Some(rate_limit_scope),
            || token().map(Some),
            description,
        )
    }

    pub(super) fn post<T: DeserializeOwned, B: Serialize + ?Sized>(
        &self,
        path: &str,
        body: &B,
        description: &str,
    ) -> ApiResult<T> {
        self.check_cooldown()?;
        let response = self
            .http
            .post(self.url(path))
            .header(COMPATIBILITY_DATE_HEADER, COMPATIBILITY_DATE)
            .json(body)
            .send()
            .map_err(|error| http::transport_error(description, &error))?;
        self.observe_rate_limit(&response, None);
        self.observe_deprecation(&response, description);
        if let Some(error) = esi_limit_error(&response, description) {
            return Err(error);
        }
        http::decode_json(response, description)
    }

    pub(super) fn check_cooldown(&self) -> ApiResult<()> {
        match self.cooldowns.remaining("esi") {
            Some(seconds) => Err(ApiError::RateLimited(format!(
                "ESI rate budget requires retry after {seconds} seconds"
            ))),
            None => Ok(()),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    fn fetch<T: DeserializeOwned>(
        &self,
        path: &str,
        cacheable: bool,
        rate_limit_scope: Option<&str>,
        bearer_token: impl FnOnce() -> ApiResult<Option<String>>,
        description: &str,
    ) -> ApiResult<T> {
        self.check_cooldown()?;
        let url = self.url(path);
        let cache = self.cache.as_ref().filter(|_| cacheable);
        let cache_key = cache_key(&url);
        let cached = cache
            .and_then(|cache| cache.load(&cache_key).ok())
            .flatten();
        if let Some(entry) = cached.as_ref().filter(|entry| entry.fresh) {
            return deserialize_cached_response(entry, description);
        }

        let mut request = self
            .http
            .get(&url)
            .header(COMPATIBILITY_DATE_HEADER, COMPATIBILITY_DATE);
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
                    return deserialize_cached_response(entry, description);
                }
                return Err(http::transport_error(description, &error));
            }
        };
        self.observe_rate_limit(&response, rate_limit_scope);
        self.observe_deprecation(&response, description);
        let headers = response.headers();
        let expires = header_value(headers, EXPIRES);
        let etag = header_value(headers, ETAG);
        let last_modified = header_value(headers, LAST_MODIFIED);
        if response.status() == StatusCode::NOT_MODIFIED {
            let Some(entry) = cached.as_ref() else {
                return Err(format!("{description} returned 304 without a cached response").into());
            };
            if let Some(cache) = cache {
                let _ = cache.revalidate(
                    &cache_key,
                    expires.as_deref(),
                    etag.as_deref(),
                    last_modified.as_deref(),
                );
            }
            return deserialize_cached_response(entry, description);
        }
        if let Some(error) = esi_limit_error(&response, description) {
            return Err(error);
        }
        if response.status().is_server_error() {
            if let Some(entry) = cached.as_ref() {
                return deserialize_cached_response(entry, description);
            }
        }
        if !response.status().is_success() {
            return Err(
                format!("{description} failed (HTTP {})", response.status().as_u16()).into(),
            );
        }
        let allows_storage = !header_value(headers, CACHE_CONTROL)
            .is_some_and(|value| value.to_ascii_lowercase().contains("no-store"));
        let body = response
            .bytes()
            .map_err(|_| format!("{description} response body could not be read"))?;
        if let Some(cache) = cache.filter(|_| allows_storage) {
            let _ = cache.store(
                &cache_key,
                &body,
                expires.as_deref(),
                etag.as_deref(),
                last_modified.as_deref(),
            );
        }
        serde_json::from_slice(&body)
            .map_err(|error| format!("{description} response invalid: {error}").into())
    }

    /// Reports a `299` deprecation warning once per kind of request.
    fn observe_deprecation(&self, response: &Response, description: &str) {
        let Some(warning) = header_value(response.headers(), WARNING) else {
            return;
        };
        if warning.trim_start().starts_with("299") {
            self.cooldowns.warn_once(description, || {
                format!("ESI reports that the {description} route is deprecated: {warning}")
            });
        }
    }

    fn observe_rate_limit(&self, response: &Response, request_scope: Option<&str>) {
        let headers = response.headers();
        let group = headers
            .get("x-ratelimit-group")
            .and_then(|value| value.to_str().ok());
        let scope = match (group, request_scope) {
            (Some(group), Some(request_scope)) => Some(format!("{group}:{request_scope}")),
            (Some(scope), None) | (None, Some(scope)) => Some(scope.to_owned()),
            (None, None) => headers
                .contains_key("x-esi-error-limit-remain")
                .then(|| "error-limit".to_string()),
        };
        let error_limited = response.status().as_u16() == 420;
        let record = |scope: Option<String>, seconds: u64, reason: &str| {
            self.cooldowns.record(ApiCooldown {
                source: "esi".into(),
                scope,
                until: unix_time().saturating_add(seconds),
                reason: Some(reason.into()),
            });
        };

        let retry_seconds = header_value(headers, RETRY_AFTER)
            .and_then(|value| http::retry_after_secs(&value, unix_time()))
            .or_else(|| {
                error_limited
                    .then(|| numeric_header(headers, "x-esi-error-limit-reset"))
                    .flatten()
            });
        if let Some(seconds) = retry_seconds {
            let reason = if error_limited {
                "error limit"
            } else {
                "rate limit"
            };
            record(scope.clone(), seconds, reason);
        }

        if numeric_header(headers, "x-esi-error-limit-remain") == Some(0) {
            if let Some(seconds) = numeric_header(headers, "x-esi-error-limit-reset") {
                let scope = scope.clone().or_else(|| Some("error-limit".into()));
                record(scope, seconds, "error budget exhausted");
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
            record(scope, seconds, "low rate budget");
        }
    }
}

/// The response-cache key for `url`. ESI varies responses by compatibility date, so the
/// cache must too.
pub(super) fn cache_key(url: &str) -> String {
    format!("{url}#{COMPATIBILITY_DATE}")
}

fn esi_limit_error(response: &Response, description: &str) -> Option<ApiError> {
    let message = match response.status().as_u16() {
        420 => {
            let reset = response
                .headers()
                .get("x-esi-error-limit-reset")
                .and_then(|value| value.to_str().ok())
                .unwrap_or("an unspecified interval");
            format!("{description} reached ESI's error limit; retry after {reset} seconds")
        }
        429 => {
            let retry_after = header_value(response.headers(), RETRY_AFTER)
                .unwrap_or_else(|| "an unspecified delay".into());
            format!("{description} was rate limited; retry after {retry_after} seconds")
        }
        _ => return None,
    };
    Some(ApiError::RateLimited(message))
}

fn numeric_header(headers: &HeaderMap, name: &'static str) -> Option<u64> {
    headers.get(name)?.to_str().ok()?.trim().parse().ok()
}

fn deserialize_cached_response<T: DeserializeOwned>(
    response: &CachedResponse,
    description: &str,
) -> ApiResult<T> {
    serde_json::from_slice(&response.body)
        .map_err(|error| format!("cached {description} response invalid: {error}").into())
}

fn header_value(headers: &HeaderMap, name: HeaderName) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;
    use serde_json::Value;

    #[test]
    fn get_and_post_send_the_compatibility_date() {
        let server = MockServer::start();
        let get = server.mock(|when, then| {
            when.method(GET)
                .path("/status/")
                .header("x-compatibility-date", COMPATIBILITY_DATE);
            then.status(200).body("{}");
        });
        let post = server.mock(|when, then| {
            when.method(POST)
                .path("/universe/names/")
                .header("x-compatibility-date", COMPATIBILITY_DATE);
            then.status(200).body("[]");
        });
        let (base_url, log) = (server.base_url(), ApiLog::default());
        let esi = Esi::new(&base_url, None, &log).unwrap();

        esi.get::<Value>("/status/", "status").unwrap();
        esi.post::<Value, _>("/universe/names/", &[1], "names")
            .unwrap();

        get.assert();
        post.assert();
    }

    #[test]
    fn deprecation_warning_is_reported_once_per_route() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/status/");
            then.status(200)
                .header("warning", "299 - This route is deprecated")
                .body("{}");
        });
        let (base_url, log) = (server.base_url(), ApiLog::default());
        let esi = Esi::new(&base_url, None, &log).unwrap();

        esi.get::<Value>("/status/", "status").unwrap();
        esi.get::<Value>("/status/", "status").unwrap();

        let warnings = log.take_warnings();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("status route is deprecated"));
    }

    #[test]
    fn rate_limited_response_records_a_cooldown_for_its_group() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/killmails/1/hash/");
            then.status(429)
                .header("retry-after", "30")
                .header("x-ratelimit-group", "killmail")
                .header("x-ratelimit-limit", "3600/15m")
                .header("x-ratelimit-remaining", "0");
        });
        let (base_url, log) = (server.base_url(), ApiLog::default());
        let esi = Esi::new(&base_url, None, &log).unwrap();

        let result = esi.get::<Value>("/killmails/1/hash/", "killmail");

        assert!(matches!(result, Err(ApiError::RateLimited(_))));
        let cooldowns = log.take_cooldowns();
        let longest = cooldowns
            .iter()
            .max_by_key(|cooldown| cooldown.until)
            .unwrap();
        assert_eq!(longest.source, "esi");
        assert_eq!(longest.scope.as_deref(), Some("killmail"));
        assert!(longest.until >= unix_time() + 29);
    }
}
