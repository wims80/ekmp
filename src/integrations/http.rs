use super::{ApiError, ApiResult};
use crate::{
    clock::{http_date, unix_time},
    models::ApiCooldown,
};
use reqwest::blocking::{Client, ClientBuilder, Response};
use serde::de::DeserializeOwned;
use std::{
    sync::{Mutex, OnceLock},
    time::Duration,
};

pub(crate) const USER_AGENT: &str = concat!(
    "ekmp/",
    env!("CARGO_PKG_VERSION"),
    " EVE Killmail Publisher (+https://github.com/wims80/ekmp)"
);

/// The shared HTTP client for idempotent requests.
pub(crate) fn client() -> ApiResult<&'static Client> {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    shared(&CLIENT, |builder| builder)
}

/// A client for consequential requests that must never be redirected or retried.
pub(crate) fn single_shot_client() -> ApiResult<&'static Client> {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    shared(&CLIENT, |builder| {
        builder
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
    })
}

fn shared(
    cell: &'static OnceLock<Client>,
    configure: fn(ClientBuilder) -> ClientBuilder,
) -> ApiResult<&'static Client> {
    if let Some(client) = cell.get() {
        return Ok(client);
    }
    let client = configure(
        Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30)),
    )
    .build()
    .map_err(|_| "Could not configure the HTTP client")?;
    Ok(cell.get_or_init(|| client))
}

pub(crate) fn transport_error(description: &str, error: &reqwest::Error) -> ApiError {
    let reason = if error.is_timeout() {
        "timed out"
    } else if error.is_connect() {
        "could not connect"
    } else {
        "transport failed"
    };
    format!("{description} {reason}").into()
}

/// Decodes a successful JSON response without echoing an error body.
pub(crate) fn decode_json<T: DeserializeOwned>(
    response: Response,
    description: &str,
) -> ApiResult<T> {
    let status = response.status();
    if !status.is_success() {
        return Err(format!("{description} failed (HTTP {})", status.as_u16()).into());
    }
    response
        .json()
        .map_err(|_| format!("{description} returned an invalid response").into())
}

/// Parses a `Retry-After` value given either as seconds or as an HTTP date.
pub(crate) fn retry_after_secs(value: &str, now: u64) -> Option<u64> {
    value
        .trim()
        .parse()
        .ok()
        .or_else(|| http_date(value).map(|deadline| deadline.saturating_sub(now)))
}

/// API cooldowns observed during one backend operation.
///
/// Core drains the log after each backend call and persists the cooldowns. Until then, an
/// observed cooldown also stops further requests to the same source.
#[derive(Default)]
pub(crate) struct CooldownLog(Mutex<Vec<ApiCooldown>>);

impl CooldownLog {
    pub(crate) fn record(&self, cooldown: ApiCooldown) {
        self.lock().push(cooldown);
    }

    pub(crate) fn take(&self) -> Vec<ApiCooldown> {
        std::mem::take(&mut *self.lock())
    }

    /// The remaining seconds of the longest observed cooldown for `source`.
    pub(crate) fn remaining(&self, source: &str) -> Option<u64> {
        let now = unix_time();
        self.lock()
            .iter()
            .filter(|cooldown| cooldown.source == source && cooldown.until > now)
            .map(|cooldown| cooldown.until - now)
            .max()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<ApiCooldown>> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_agent_identifies_the_application_and_source() {
        assert!(USER_AGENT.starts_with("ekmp/"));
        assert!(USER_AGENT.contains("https://github.com/wims80/ekmp"));
    }

    #[test]
    fn retry_after_accepts_seconds_and_http_dates() {
        assert_eq!(retry_after_secs(" 30 ", 0), Some(30));
        assert_eq!(
            retry_after_secs("Thu, 01 Jan 1970 00:01:00 GMT", 20),
            Some(40)
        );
        assert_eq!(retry_after_secs("soon", 0), None);
    }

    #[test]
    fn cooldown_log_reports_the_longest_active_cooldown_until_taken() {
        let log = CooldownLog::default();
        let now = unix_time();
        for (source, until) in [
            ("esi", now + 10),
            ("esi", now + 60),
            ("zkillboard", now + 600),
        ] {
            log.record(ApiCooldown {
                source: source.into(),
                scope: None,
                until,
                reason: None,
            });
        }

        assert!(log.remaining("esi").is_some_and(|seconds| seconds > 10));
        assert_eq!(log.take().len(), 3);
        assert_eq!(log.remaining("esi"), None);
    }
}
