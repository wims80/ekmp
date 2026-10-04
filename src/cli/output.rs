//! Rendering command results.

use super::text::{fields, isk, killmail_time, table, yes_no};
use crate::{
    core::CoreError,
    killmail::{is_bulk_candidate, is_eligible_for_bulk_posting, report_state, ReportState},
    models::{CharacterSource, Killmail, KillmailDetail, Store},
};
use serde::Serialize;
use serde_json::{json, Value};

/// A command's result: JSON for `--json`, text otherwise, and the exit code.
///
/// Both forms are built from the same data. The JSON schema is the stable interface for
/// scripts; the text is for people and may change.
pub(super) struct Output {
    pub(super) json: Value,
    pub(super) text: String,
    pub(super) code: u8,
}

impl Output {
    pub(super) fn new(json: Value, text: impl Into<String>) -> Self {
        Self {
            json,
            text: text.into(),
            code: 0,
        }
    }

    /// Output for a command that has already reported everything on stderr.
    pub(super) fn none() -> Self {
        Self::new(Value::Null, "")
    }

    pub(super) fn with_code(mut self, code: u8) -> Self {
        self.code = code;
        self
    }
}

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
pub(super) fn emit(json_output: bool, output: &Output) {
    if json_output {
        if !output.json.is_null() {
            println!("{}", output.json);
        }
    } else if !output.text.is_empty() {
        print!("{}", output.text);
        if !output.text.ends_with('\n') {
            println!();
        }
    }
}

fn status_text(state: ReportState) -> &'static str {
    match state {
        ReportState::Reported => "reported",
        ReportState::Unreported => "unreported",
        ReportState::Unknown => "awaiting status",
    }
}

/// Whether the killmail can be bulk posted, or why not.
fn bulk_text(store: &Store, mail: &Killmail, now: u64) -> &'static str {
    if !is_eligible_for_bulk_posting(store, mail) {
        "protected"
    } else if is_bulk_candidate(store, mail, now) {
        "yes"
    } else {
        "no"
    }
}

pub(super) fn killmail_table(store: &Store, mails: &[&Killmail], now: u64) -> String {
    let rows: Vec<Vec<String>> = mails
        .iter()
        .map(|mail| {
            vec![
                mail.id.to_string(),
                killmail_time(&mail.time),
                mail.victim.clone(),
                mail.ship.clone(),
                isk(mail.estimated_value_isk),
                status_text(report_state(store, mail.id, now)).into(),
                bulk_text(store, mail, now).into(),
            ]
        })
        .collect();
    table(
        &[
            "ID",
            "TIME (UTC)",
            "VICTIM",
            "SHIP",
            "VALUE",
            "STATUS",
            "BULK",
        ],
        &rows,
    )
}

pub(super) fn killmail_details(store: &Store, mail: &Killmail, now: u64) -> String {
    let mut victim = mail.victim.clone();
    let detail = mail.detail.as_ref();
    if let Some(corporation) = detail.and_then(|detail| detail.victim.corporation_name.as_deref()) {
        victim = format!("{victim} ({corporation})");
    }
    let mut pairs = vec![
        ("Killmail", mail.id.to_string()),
        ("Time", format!("{} UTC", killmail_time(&mail.time))),
        ("Victim", victim),
        ("Ship", mail.ship.clone()),
    ];
    if let Some(location) = detail.map(|detail| &detail.location) {
        let region = location
            .region_name
            .as_deref()
            .map(|region| format!(" ({region})"))
            .unwrap_or_default();
        pairs.push((
            "Location",
            format!("{}{region}", location.solar_system_name),
        ));
    }
    pairs.push(("Value", format!("{} ISK", isk(mail.estimated_value_isk))));
    if let Some(attackers) = detail.map(|detail| &detail.attackers) {
        let final_blow = attackers
            .iter()
            .find(|attacker| attacker.final_blow)
            .and_then(|attacker| attacker.character_name.as_deref())
            .map(|name| format!("; final blow: {name}"))
            .unwrap_or_default();
        pairs.push(("Attackers", format!("{}{final_blow}", attackers.len())));
    }
    let sources = mail
        .sources
        .iter()
        .map(|source| source.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    pairs.extend([
        ("Found by", sources),
        (
            "Status",
            status_text(report_state(store, mail.id, now)).into(),
        ),
        (
            "Protected",
            yes_no(!is_eligible_for_bulk_posting(store, mail)),
        ),
        (
            "Eligible for bulk posting",
            yes_no(is_bulk_candidate(store, mail, now)),
        ),
    ]);
    fields(&pairs)
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
