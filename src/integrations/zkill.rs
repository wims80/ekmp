use crate::models::{ApiCooldown, Killmail};
use reqwest::blocking::{Client, Response};
use reqwest::header::{
    HeaderMap, HeaderValue, ACCEPT_ENCODING, AGE, DATE, EXPIRES, RETRY_AFTER, USER_AGENT,
};
use serde::Deserialize;
use std::{
    sync::{Mutex, OnceLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const API: &str = "https://zkillboard.com/api";
pub const KILLMAILS_PER_PAGE: usize = 200;
pub const WITHHOLDING_WINDOW_SECS: u64 = 5 * 60;
const QUERY_VALIDITY_SECS: u64 = 60 * 60;
const USER_AGENT_VALUE: &str = concat!(
    "ekmp/",
    env!("CARGO_PKG_VERSION"),
    " EVE Killmail Publisher (+https://github.com/wims80/ekmp)"
);
static OBSERVED_COOLDOWNS: OnceLock<Mutex<Vec<ApiCooldown>>> = OnceLock::new();

pub fn take_api_cooldowns() -> Vec<ApiCooldown> {
    let mut cooldowns = OBSERVED_COOLDOWNS
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    std::mem::take(&mut *cooldowns)
}

fn record_retry_after(headers: &HeaderMap, received_at: u64, scope: &str, reason: &str) {
    let Some(seconds) = retry_after_seconds(headers.get(RETRY_AFTER), received_at) else {
        return;
    };
    OBSERVED_COOLDOWNS
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push(ApiCooldown {
            source: "zkillboard".into(),
            scope: Some(scope.into()),
            until: received_at.saturating_add(seconds),
            reason: Some(reason.into()),
        });
}

#[derive(Debug, PartialEq, Eq)]
pub struct PostOutcome {
    pub new: bool,
    pub url: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LookupPage {
    pub entries: Vec<KillEntry>,
    /// Time represented by the source response, rather than local cache-read time.
    pub observed_at: u64,
    pub valid_until: u64,
}

pub fn character_killmail_page(character_id: u64, page: usize) -> Result<LookupPage, String> {
    character_mail_page_at(API, character_id, "kills", page)
}

pub fn character_loss_killmail_page(character_id: u64, page: usize) -> Result<LookupPage, String> {
    character_mail_page_at(API, character_id, "losses", page)
}

fn client() -> Result<Client, String> {
    Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|_| "Could not configure the HTTP client".to_string())
}

fn character_mail_page_at(
    api: &str,
    character_id: u64,
    mail_type: &str,
    page: usize,
) -> Result<LookupPage, String> {
    let response = client()?
        .get(format!(
            "{api}/{mail_type}/characterID/{character_id}/page/{page}/"
        ))
        .header(USER_AGENT, USER_AGENT_VALUE)
        .header(ACCEPT_ENCODING, "gzip")
        .send()
        .map_err(|error| transport_error("zKillboard lookup", &error))?;
    let received_at = unix_time();
    decode_lookup_response(response, received_at)
}

pub fn post(mail: &Killmail) -> Result<PostOutcome, String> {
    post_at(API, mail)
}

fn post_at(api: &str, mail: &Killmail) -> Result<PostOutcome, String> {
    let response = client()?
        .post(format!("{api}/killmail/add/{}/{}/", mail.id, mail.hash))
        .header(USER_AGENT, USER_AGENT_VALUE)
        .header(ACCEPT_ENCODING, "gzip")
        .send()
        .map_err(|error| transport_error("zKillboard submission", &error))?;
    let received_at = unix_time();
    if !response.status().is_success() {
        record_retry_after(
            response.headers(),
            received_at,
            "submission",
            "submission cooldown",
        );
    }
    let body: PostResponse = decode_response(response, "zKillboard submission")?;
    if body.status != "success" {
        return Err("zKillboard submission returned a non-success result".into());
    }
    Ok(PostOutcome {
        new: body.new,
        url: format!("https://zkillboard.com/kill/{}/", mail.id),
    })
}

fn decode_lookup_response(response: Response, received_at: u64) -> Result<LookupPage, String> {
    let status = response.status();
    let (observed_at, valid_until) = lookup_metadata(response.headers(), received_at);
    if !status.is_success() {
        record_retry_after(response.headers(), received_at, "query", "query cooldown");
        let retry = retry_after_seconds(response.headers().get(RETRY_AFTER), received_at)
            .map(|seconds| format!("; retry after {seconds} seconds"))
            .unwrap_or_default();
        return Err(format!(
            "zKillboard lookup failed (HTTP {}){retry}",
            status.as_u16()
        ));
    }
    let entries = response
        .json()
        .map_err(|_| "zKillboard lookup returned an invalid response".to_string())?;
    Ok(LookupPage {
        entries,
        observed_at,
        valid_until,
    })
}

fn decode_response<T: for<'de> Deserialize<'de>>(
    response: Response,
    operation: &str,
) -> Result<T, String> {
    let status = response.status();
    if !status.is_success() {
        return Err(format!("{operation} failed (HTTP {})", status.as_u16()));
    }
    response
        .json()
        .map_err(|_| format!("{operation} returned an invalid response"))
}

#[cfg(test)]
fn decode_body<T: for<'de> Deserialize<'de>>(
    status: u16,
    body: &str,
    operation: &str,
) -> Result<T, String> {
    if !(200..300).contains(&status) {
        return Err(format!("{operation} failed (HTTP {status})"));
    }
    serde_json::from_str(body).map_err(|_| format!("{operation} returned an invalid response"))
}

fn lookup_metadata(headers: &HeaderMap, received_at: u64) -> (u64, u64) {
    let date = date_header(headers.get(DATE));
    let age_observation =
        seconds_header(headers.get(AGE)).map(|age| received_at.saturating_sub(age));
    let observed_at = match (date, age_observation) {
        (Some(date), Some(age_observation)) => date.min(age_observation),
        (Some(date), None) => date.min(received_at),
        (None, Some(age_observation)) => age_observation,
        (None, None) => return (0, 0),
    };
    let maximum = observed_at.saturating_add(QUERY_VALIDITY_SECS);
    let valid_until = date_header(headers.get(EXPIRES))
        .unwrap_or(maximum)
        .min(maximum);
    (observed_at, valid_until)
}

fn date_header(value: Option<&HeaderValue>) -> Option<u64> {
    httpdate::parse_http_date(value?.to_str().ok()?)
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
}

fn seconds_header(value: Option<&HeaderValue>) -> Option<u64> {
    value?.to_str().ok()?.trim().parse().ok()
}

fn retry_after_seconds(value: Option<&HeaderValue>, received_at: u64) -> Option<u64> {
    let value = value?.to_str().ok()?;
    value.trim().parse().ok().or_else(|| {
        httpdate::parse_http_date(value)
            .ok()?
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|deadline| deadline.as_secs().saturating_sub(received_at))
    })
}

fn transport_error(operation: &str, error: &reqwest::Error) -> String {
    let reason = if error.is_timeout() {
        "timed out"
    } else if error.is_connect() {
        "could not connect"
    } else {
        "transport failed"
    };
    format!("{operation} {reason}")
}

fn unix_time() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct KillEntry {
    pub killmail_id: u64,
    pub killmail_time: String,
}

#[derive(Debug, Deserialize)]
struct PostResponse {
    status: String,
    new: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::CharacterSource;
    use httpmock::prelude::*;

    fn mail() -> Killmail {
        Killmail {
            id: 42,
            hash: "sentinel-secret-hash".into(),
            sources: vec![CharacterSource {
                id: 1,
                name: "Pilot".into(),
            }],
            victim_id: Some(2),
            victim_corporation_id: Some(3),
            victim: "Victim".into(),
            ship: "Ship".into(),
            time: "2026-08-16T10:00:00Z".into(),
            estimated_value_isk: None,
            detail: None,
        }
    }

    #[test]
    fn response_errors_hide_bodies() {
        let error =
            decode_body::<PostResponse>(422, r#"{"error":"sentinel"}"#, "post").unwrap_err();
        assert_eq!(error, "post failed (HTTP 422)");
        let error = decode_body::<PostResponse>(200, "sentinel", "post").unwrap_err();
        assert_eq!(error, "post returned an invalid response");
    }

    #[test]
    fn metadata_uses_source_time_and_caps_validity() {
        let mut headers = HeaderMap::new();
        headers.insert(
            DATE,
            HeaderValue::from_static("Thu, 01 Jan 2026 00:00:00 GMT"),
        );
        headers.insert(
            EXPIRES,
            HeaderValue::from_static("Thu, 01 Jan 2026 02:00:00 GMT"),
        );
        let (observed, valid) = lookup_metadata(&headers, 99);
        assert_eq!(valid, observed + 3600);
    }

    #[test]
    fn metadata_without_date_accounts_for_age() {
        let mut headers = HeaderMap::new();
        headers.insert(AGE, HeaderValue::from_static("120"));
        assert_eq!(lookup_metadata(&headers, 1_000), (880, 4_480));
    }

    #[test]
    fn metadata_without_source_headers_cannot_prove_freshness() {
        assert_eq!(lookup_metadata(&HeaderMap::new(), 1_000), (0, 0));
    }

    #[test]
    fn lookup_returns_entries_and_metadata() {
        let server = MockServer::start();
        let request = server.mock(|when, then| {
            when.method(GET)
                .path("/kills/characterID/7/page/2/")
                .header("accept-encoding", "gzip");
            then.status(200)
                .header("content-type", "application/json")
                .header("date", "Thu, 01 Jan 2026 00:00:00 GMT")
                .body(r#"[{"killmail_id":42,"killmail_time":"2026-08-16T10:00:00Z"}]"#);
        });
        let page = character_mail_page_at(&server.base_url(), 7, "kills", 2).unwrap();
        assert_eq!(page.entries[0].killmail_id, 42);
        assert_eq!(page.valid_until, page.observed_at + 3600);
        request.assert();
    }

    #[test]
    fn submission_error_hides_hash_and_body() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST)
                .path("/killmail/add/42/sentinel-secret-hash/");
            then.status(500).body("sentinel response");
        });
        let error = post_at(&server.base_url(), &mail()).unwrap_err();
        assert_eq!(error, "zKillboard submission failed (HTTP 500)");
        assert!(!error.contains("sentinel"));
    }
    #[test]
    fn submissions_are_not_replayed_through_redirects() {
        for status in [307, 308] {
            let server = MockServer::start();
            let original = server.mock(|when, then| {
                when.method(POST)
                    .path("/killmail/add/42/sentinel-secret-hash/");
                then.status(status).header(
                    "location",
                    format!("{}/sentinel-replayed", server.base_url()),
                );
            });
            let replayed = server.mock(|when, then| {
                when.method(POST).path("/sentinel-replayed");
                then.status(200)
                    .json_body(serde_json::json!({"status":"success","new":true,"url":"ignored"}));
            });
            let error = post_at(&server.base_url(), &mail()).unwrap_err();
            assert!(error.contains(&format!("HTTP {status}")));
            assert!(!error.contains("sentinel"));
            original.assert_calls(1);
            replayed.assert_calls(0);
        }
    }
}
