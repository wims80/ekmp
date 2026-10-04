use crate::{
    clock::{http_date, unix_time},
    integrations::{
        http::{self, CooldownLog},
        ApiResult,
    },
    models::{ApiCooldown, Killmail, ZkillPage},
};
use reqwest::{
    blocking::Response,
    header::{HeaderMap, ACCEPT_ENCODING, AGE, DATE, EXPIRES, RETRY_AFTER},
};
use serde::Deserialize;

const API: &str = "https://zkillboard.com/api";
pub const KILLMAILS_PER_PAGE: usize = 200;
pub const WITHHOLDING_WINDOW_SECS: u64 = 5 * 60;
const QUERY_VALIDITY_SECS: u64 = 60 * 60;

#[derive(Debug, PartialEq, Eq)]
pub struct PostOutcome {
    pub new: bool,
    pub url: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MailKind {
    Kills,
    Losses,
}

impl MailKind {
    pub fn path_segment(self) -> &'static str {
        match self {
            Self::Kills => "kills",
            Self::Losses => "losses",
        }
    }
}

pub fn killmail_page(
    kind: MailKind,
    character_id: u64,
    page: usize,
    cooldowns: &CooldownLog,
) -> ApiResult<ZkillPage> {
    killmail_page_at(API, kind, character_id, page, cooldowns)
}

fn killmail_page_at(
    api: &str,
    kind: MailKind,
    character_id: u64,
    page: usize,
    cooldowns: &CooldownLog,
) -> ApiResult<ZkillPage> {
    const DESCRIPTION: &str = "zKillboard lookup";
    let response = http::single_shot_client()?
        .get(format!(
            "{api}/{}/characterID/{character_id}/page/{page}/",
            kind.path_segment()
        ))
        .header(ACCEPT_ENCODING, "gzip")
        .send()
        .map_err(|error| http::transport_error(DESCRIPTION, &error))?;
    let received_at = unix_time();
    let (observed_at, valid_until) = lookup_metadata(response.headers(), received_at);
    if !response.status().is_success() {
        let retry = record_retry_after(&response, received_at, "query", cooldowns)
            .map(|seconds| format!("; retry after {seconds} seconds"))
            .unwrap_or_default();
        return Err(format!(
            "{DESCRIPTION} failed (HTTP {}){retry}",
            response.status().as_u16()
        )
        .into());
    }
    Ok(ZkillPage {
        entries: http::decode_json(response, DESCRIPTION)?,
        observed_at,
        valid_until,
    })
}

pub fn post(mail: &Killmail, cooldowns: &CooldownLog) -> ApiResult<PostOutcome> {
    post_at(API, mail, cooldowns)
}

fn post_at(api: &str, mail: &Killmail, cooldowns: &CooldownLog) -> ApiResult<PostOutcome> {
    const DESCRIPTION: &str = "zKillboard submission";
    let response = http::single_shot_client()?
        .post(format!("{api}/killmail/add/{}/{}/", mail.id, mail.hash))
        .header(ACCEPT_ENCODING, "gzip")
        .send()
        .map_err(|error| http::transport_error(DESCRIPTION, &error))?;
    if !response.status().is_success() {
        record_retry_after(&response, unix_time(), "submission", cooldowns);
    }
    let body: PostResponse = http::decode_json(response, DESCRIPTION)?;
    if body.status != "success" {
        return Err("zKillboard submission returned a non-success result".into());
    }
    Ok(PostOutcome {
        new: body.new,
        url: format!("https://zkillboard.com/kill/{}/", mail.id),
    })
}

/// Records a `Retry-After` cooldown for `scope` and returns its length in seconds.
fn record_retry_after(
    response: &Response,
    received_at: u64,
    scope: &str,
    cooldowns: &CooldownLog,
) -> Option<u64> {
    let value = response.headers().get(RETRY_AFTER)?.to_str().ok()?;
    let seconds = http::retry_after_secs(value, received_at)?;
    cooldowns.record(ApiCooldown {
        source: "zkillboard".into(),
        scope: Some(scope.into()),
        until: received_at.saturating_add(seconds),
        reason: Some(format!("{scope} cooldown")),
    });
    Some(seconds)
}

fn lookup_metadata(headers: &HeaderMap, received_at: u64) -> (u64, u64) {
    let header = |name| headers.get(name).and_then(|value| value.to_str().ok());
    let date = header(DATE).and_then(http_date);
    let age_observation = header(AGE)
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(|age| received_at.saturating_sub(age));
    let observed_at = match (date, age_observation) {
        (Some(date), Some(age_observation)) => date.min(age_observation),
        (Some(date), None) => date.min(received_at),
        (None, Some(age_observation)) => age_observation,
        (None, None) => return (0, 0),
    };
    let maximum = observed_at.saturating_add(QUERY_VALIDITY_SECS);
    let valid_until = header(EXPIRES)
        .and_then(http_date)
        .unwrap_or(maximum)
        .min(maximum);
    (observed_at, valid_until)
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
    use reqwest::header::HeaderValue;

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
        let page = killmail_page_at(
            &server.base_url(),
            MailKind::Kills,
            7,
            2,
            &CooldownLog::default(),
        )
        .unwrap();
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
        let error = post_at(&server.base_url(), &mail(), &CooldownLog::default())
            .unwrap_err()
            .to_string();
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
            let error = post_at(&server.base_url(), &mail(), &CooldownLog::default())
                .unwrap_err()
                .to_string();
            assert!(error.contains(&format!("HTTP {status}")));
            assert!(!error.contains("sentinel"));
            original.assert_calls(1);
            replayed.assert_calls(0);
        }
    }
}
