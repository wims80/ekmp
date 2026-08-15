use crate::models::Killmail;
use reqwest::blocking::Client;
use reqwest::header::{ACCEPT_ENCODING, USER_AGENT};
use serde::Deserialize;
use std::collections::HashSet;

const API: &str = "https://zkillboard.com/api";
const USER_AGENT_VALUE: &str =
    concat!("akmp/", env!("CARGO_PKG_VERSION"), " EVE killmail reporter");

#[derive(Debug, PartialEq, Eq)]
pub struct PostOutcome {
    pub new: bool,
    pub url: String,
}

pub fn character_kill_ids(character_id: u64) -> Result<HashSet<u64>, String> {
    let response = Client::new()
        .get(format!("{API}/kills/characterID/{character_id}/"))
        .header(USER_AGENT, USER_AGENT_VALUE)
        .header(ACCEPT_ENCODING, "gzip")
        .send()
        .map_err(|e| format!("zKillboard lookup failed: {e}"))?;
    let entries: Vec<KillEntry> = decode_response(response, "zKillboard lookup")?;
    Ok(entries.into_iter().map(|entry| entry.killmail_id).collect())
}

pub fn post(mail: &Killmail) -> Result<PostOutcome, String> {
    let response = Client::new()
        .post(format!("{API}/killmail/add/{}/{}/", mail.id, mail.hash))
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

#[derive(Deserialize)]
struct KillEntry {
    killmail_id: u64,
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
            r#"[{"killmail_id":42,"zkb":{"hash":"ignored"}},{"killmail_id":43}]"#,
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
}
