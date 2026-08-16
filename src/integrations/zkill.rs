use crate::models::Killmail;
use reqwest::blocking::Client;
use reqwest::header::{ACCEPT_ENCODING, USER_AGENT};
use serde::Deserialize;

const API: &str = "https://zkillboard.com/api";
pub const KILLMAILS_PER_PAGE: usize = 200;
const USER_AGENT_VALUE: &str = concat!(
    "ekmp/",
    env!("CARGO_PKG_VERSION"),
    " EVE Killmail Publisher"
);

#[derive(Debug, PartialEq, Eq)]
pub struct PostOutcome {
    pub new: bool,
    pub url: String,
}

pub fn character_killmail_page(character_id: u64, page: usize) -> Result<Vec<KillEntry>, String> {
    character_mail_page(character_id, "kills", page)
}

pub fn character_loss_killmail_page(
    character_id: u64,
    page: usize,
) -> Result<Vec<KillEntry>, String> {
    character_mail_page(character_id, "losses", page)
}

fn character_mail_page(
    character_id: u64,
    mail_type: &str,
    page: usize,
) -> Result<Vec<KillEntry>, String> {
    character_mail_page_at(API, character_id, mail_type, page)
}

fn character_mail_page_at(
    api: &str,
    character_id: u64,
    mail_type: &str,
    page: usize,
) -> Result<Vec<KillEntry>, String> {
    let response = Client::new()
        .get(format!(
            "{api}/{mail_type}/characterID/{character_id}/page/{page}/"
        ))
        .header(USER_AGENT, USER_AGENT_VALUE)
        .header(ACCEPT_ENCODING, "gzip")
        .send()
        .map_err(|e| format!("zKillboard lookup failed: {e}"))?;
    decode_response(response, "zKillboard lookup")
}

pub fn post(mail: &Killmail) -> Result<PostOutcome, String> {
    post_at(API, mail)
}

fn post_at(api: &str, mail: &Killmail) -> Result<PostOutcome, String> {
    let response = Client::new()
        .post(format!("{api}/killmail/add/{}/{}/", mail.id, mail.hash))
        .header(USER_AGENT, USER_AGENT_VALUE)
        .header(ACCEPT_ENCODING, "gzip")
        .send()
        .map_err(|e| format!("zKillboard submission failed: {e}"))?;
    let body: PostResponse = decode_response(response, "zKillboard submission")?;
    if body.status != "success" {
        return Err("zKillboard submission returned a non-success result".into());
    }
    Ok(PostOutcome {
        new: body.new,
        url: body.url,
    })
}

fn decode_response<T: for<'de> Deserialize<'de>>(
    response: reqwest::blocking::Response,
    operation: &str,
) -> Result<T, String> {
    let status = response.status();
    let body = response
        .text()
        .map_err(|e| format!("{operation} response could not be read: {e}"))?;
    decode_body(status.as_u16(), &body, operation)
}

fn decode_body<T: for<'de> Deserialize<'de>>(
    status: u16,
    body: &str,
    operation: &str,
) -> Result<T, String> {
    if !(200..300).contains(&status) {
        return Err(format!("{operation} failed (HTTP {status}): {body}"));
    }
    serde_json::from_str(body)
        .map_err(|e| format!("{operation} returned invalid JSON ({e}): {body}"))
}

#[derive(Clone, Debug, Deserialize)]
pub struct KillEntry {
    pub killmail_id: u64,
    pub killmail_time: String,
}

#[derive(Debug, Deserialize)]
struct PostResponse {
    status: String,
    new: bool,
    url: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::CharacterSource;
    use httpmock::prelude::*;

    fn mail() -> Killmail {
        Killmail {
            id: 42,
            hash: "fixture-hash".into(),
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
        }
    }

    #[test]
    fn parses_new_and_existing_post_outcomes() {
        let new: PostResponse = decode_body(
            200,
            r#"{"status":"success","new":true,"url":"https://zkillboard.com/kill/42/"}"#,
            "post",
        )
        .unwrap();
        let existing: PostResponse = decode_body(
            200,
            r#"{"status":"success","new":false,"url":"https://zkillboard.com/kill/42/"}"#,
            "post",
        )
        .unwrap();

        assert!(new.new);
        assert!(!existing.new);
    }

    #[test]
    fn preserves_http_error_details() {
        let result = decode_body::<PostResponse>(422, r#"{"error":"invalid hash"}"#, "post");

        assert_eq!(
            result.unwrap_err(),
            r#"post failed (HTTP 422): {"error":"invalid hash"}"#
        );
    }

    #[test]
    fn rejects_malformed_success_responses() {
        let result = decode_body::<PostResponse>(200, "not json", "post");

        assert!(result.unwrap_err().contains("returned invalid JSON"));
    }

    #[test]
    fn parses_lightweight_killmail_lookup_results() {
        let entries: Vec<KillEntry> = decode_body(
            200,
            r#"[{"killmail_id":42,"killmail_time":"2026-01-01T00:00:00Z","zkb":{"hash":"ignored"}},{"killmail_id":43,"killmail_time":"2026-01-02T00:00:00Z"}]"#,
            "lookup",
        )
        .unwrap();

        assert_eq!(
            entries
                .into_iter()
                .map(|entry| entry.killmail_id)
                .collect::<Vec<_>>(),
            vec![42, 43]
        );
    }

    #[test]
    fn lookup_uses_the_expected_http_path_and_headers() {
        let server = MockServer::start();
        let request = server.mock(|when, then| {
            when.method(GET)
                .path("/kills/characterID/7/page/2/")
                .header("accept-encoding", "gzip");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"[{"killmail_id":42,"killmail_time":"2026-08-16T10:00:00Z"}]"#);
        });

        let entries = character_mail_page_at(&server.base_url(), 7, "kills", 2).unwrap();

        assert_eq!(entries[0].killmail_id, 42);
        request.assert();
    }

    #[test]
    fn submission_uses_the_expected_http_path_and_decodes_the_outcome() {
        let server = MockServer::start();
        let request = server.mock(|when, then| {
            when.method(POST).path("/killmail/add/42/fixture-hash/");
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    r#"{"status":"success","new":true,"url":"https://example.invalid/kill/42/"}"#,
                );
        });

        let outcome = post_at(&server.base_url(), &mail()).unwrap();

        assert!(outcome.new);
        request.assert();
    }
}
