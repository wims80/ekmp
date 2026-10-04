//! Rendering command results.

use crate::{
    core::CoreError,
    killmail::{is_bulk_candidate, is_eligible_for_bulk_posting, report_state, ReportState},
    models::{CharacterSource, Killmail, KillmailDetail, Store},
};
use serde::Serialize;
use serde_json::{json, Value};

pub(super) fn to_json(value: impl Serialize) -> Result<Value, CoreError> {
    serde_json::to_value(value)
        .map_err(|_| CoreError::Operational("could not encode output".into()))
}

#[derive(Serialize)]
struct KillmailOutput<'a> {
    id: u64,
    sources: &'a [CharacterSource],
    victim_id: Option<u64>,
    victim_corporation_id: Option<u64>,
    victim: &'a str,
    ship: &'a str,
    time: &'a str,
    estimated_value_isk: Option<f64>,
    status: ReportState,
    protected: bool,
    eligible_for_bulk_posting: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<&'a Option<KillmailDetail>>,
}
pub(super) fn mail_output(store: &Store, mail: &Killmail, now: u64, details: bool) -> Value {
    json!(KillmailOutput {
        id: mail.id,
        sources: &mail.sources,
        victim_id: mail.victim_id,
        victim_corporation_id: mail.victim_corporation_id,
        victim: &mail.victim,
        ship: &mail.ship,
        time: &mail.time,
        estimated_value_isk: mail.estimated_value_isk,
        status: report_state(store, mail.id, now),
        protected: !is_eligible_for_bulk_posting(store, mail),
        eligible_for_bulk_posting: is_bulk_candidate(store, mail, now),
        detail: details.then_some(&mail.detail),
    })
}
pub(super) fn emit(json_output: bool, value: &Value) {
    if value.is_null() {
        return;
    }
    if json_output {
        println!("{value}");
    } else {
        println!(
            "{}",
            serde_json::to_string_pretty(value).unwrap_or_default()
        );
    }
}
pub(super) fn emit_error(json_output: bool, message: &str, code: u8) {
    if json_output {
        println!("{}", json!({"error":message,"exit_code":code}));
    }
    eprintln!("ekmp: {message}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::unix_time;
    #[test]
    fn killmail_output_excludes_hashes_and_credentials() {
        let mail = Killmail {
            id: 1,
            hash: "sentinel-private-hash".into(),
            sources: vec![],
            victim_id: None,
            victim_corporation_id: None,
            victim: "Victim".into(),
            ship: "Ship".into(),
            time: "2026-01-01T00:00:00Z".into(),
            estimated_value_isk: None,
            detail: None,
        };
        let mut store = Store::default();
        store.characters.push(crate::models::Character {
            id: 2,
            name: "Pilot".into(),
            refresh_token: Some("sentinel-private-token".into()),
            corporation_id: None,
            corporation_name: None,
        });
        let output = mail_output(&store, &mail, unix_time(), true).to_string();
        assert!(!output.contains("sentinel"));
        assert!(!output.contains("hash"));
        assert!(!output.contains("refresh_token"));
    }
}
