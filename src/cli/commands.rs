//! Command handlers that change or read the store.

use super::{
    args::{
        Characters, Cli, Config, ConfigKey, PostArgs, Protect, ProtectAdd, ProtectRemove, Setting,
    },
    output::{to_json, Output},
    prompt::{confirm, read_line},
    text::{duration, table},
};
use crate::{
    core::{Cancellation, Core, CoreError, PostBatchResult, PostResultStatus, PostSelection},
    integrations::auth::AuthFlow,
    models::{Character, ProtectedVictimKind, Store},
};
use serde::Serialize;
use serde_json::json;
use std::fmt::Write;

#[derive(Serialize)]
struct CharacterOutput<'a> {
    id: u64,
    name: &'a str,
    corporation_id: Option<u64>,
    corporation_name: Option<&'a str>,
}

impl<'a> From<&'a Character> for CharacterOutput<'a> {
    fn from(character: &'a Character) -> Self {
        Self {
            id: character.id,
            name: &character.name,
            corporation_id: character.corporation_id,
            corporation_name: character.corporation_name.as_deref(),
        }
    }
}

pub(super) fn characters(
    command: &Characters,
    core: &Core,
    cancel: &Cancellation,
) -> Result<Output, CoreError> {
    match command {
        Characters::List => {
            let store = core.snapshot()?.store;
            let output = store
                .characters
                .iter()
                .map(CharacterOutput::from)
                .collect::<Vec<_>>();
            let text = if store.characters.is_empty() {
                "No characters are authenticated. Add one with `ekmp characters add`.".into()
            } else {
                let rows: Vec<Vec<String>> = store
                    .characters
                    .iter()
                    .map(|character| {
                        vec![
                            character.id.to_string(),
                            character.name.clone(),
                            character.corporation_name.clone().unwrap_or_default(),
                        ]
                    })
                    .collect();
                table(&["ID", "NAME", "CORPORATION"], &rows)
            };
            Ok(Output::new(json!(output), text))
        }
        Characters::Add { no_browser, paste } => {
            let read_pasted_url = || {
                eprint!("Paste the full URL from the browser's address bar: ");
                read_line(cancel).ok()
            };
            let (flow, instructions) = if *paste {
                (AuthFlow::Paste(&read_pasted_url), PASTE_INSTRUCTIONS)
            } else {
                let open_browser = !no_browser && has_display();
                (AuthFlow::Loopback { open_browser }, LOOPBACK_INSTRUCTIONS)
            };
            let character = core.authenticate(cancel, flow, &|url| {
                eprintln!("{instructions}\n\n{url}\n");
            })?;
            let text = format!("Added {} ({}).", character.name, character.id);
            Ok(Output::new(
                json!({"id": character.id, "name": character.name}),
                text,
            ))
        }
        Characters::Remove { id, yes } => {
            confirm(
                *yes,
                &format!("Remove character {id}, credentials and unshared cached killmails?"),
                cancel,
            )?;
            let result = core.remove_character(*id)?;
            let mut text = format!(
                "Removed {} ({}) and {} cached killmails.",
                result.name, result.id, result.removed_killmails
            );
            if let Some(warning) = &result.credential_warning {
                let _ = write!(text, "\nWarning: {warning}");
            }
            let code = u8::from(result.credential_warning.is_some());
            Ok(Output::new(to_json(result)?, text).with_code(code))
        }
    }
}

/// The stored state with this invocation's protected-visibility override applied.
pub(super) fn listing_store(cli: &Cli, core: &Core) -> Result<Store, CoreError> {
    let mut store = core.snapshot()?.store;
    if cli.show_protected {
        store.show_protected_killmails = true;
    }
    if cli.hide_protected {
        store.show_protected_killmails = false;
    }
    Ok(store)
}

pub(super) fn post(
    args: &PostArgs,
    core: &Core,
    cancel: &Cancellation,
) -> Result<Output, CoreError> {
    let selection = match args.id {
        Some(id) if !args.all => PostSelection::One {
            id,
            post_anyway: args.post_anyway,
        },
        _ => PostSelection::All,
    };
    let prepared = core.prepare_post(selection, cancel)?;
    if prepared.ids.is_empty() {
        return Err(CoreError::Operational(
            "no selected killmails are eligible for posting".into(),
        ));
    }
    confirm(
        args.yes,
        &format!(
            "Submit these {} killmail IDs to zKillboard: {:?}?",
            prepared.ids.len(),
            prepared.ids
        ),
        cancel,
    )?;
    let result = match core.post(&prepared, cancel) {
        Ok(result) => result,
        Err(error) => {
            let completed = core.session_reports();
            if completed.is_empty() {
                return Err(error);
            }
            eprintln!("ekmp: {error}");
            let code = if cancel.is_cancelled() { 130 } else { 1 };
            let text: String = completed
                .iter()
                .map(|report| format!("{}  submitted  {}\n", report.killmail_id, report.url))
                .collect();
            let output =
                json!({"completed": completed, "error": error.to_string(), "exit_code": code});
            return Ok(Output::new(output, text).with_code(code));
        }
    };
    let code = if result.cancelled {
        130
    } else {
        u8::from(result.has_failures())
    };
    let text = post_text(&result);
    Ok(Output::new(to_json(result)?, text).with_code(code))
}

fn post_text(result: &PostBatchResult) -> String {
    let rows: Vec<Vec<String>> = result
        .results
        .iter()
        .map(|item| {
            let status = match item.status {
                PostResultStatus::Submitted => "submitted",
                PostResultStatus::AlreadyPresent => "already reported",
                PostResultStatus::Skipped => "skipped",
                PostResultStatus::Failed => "failed",
            };
            let detail = item
                .url
                .as_deref()
                .or(item.message.as_deref())
                .unwrap_or_default();
            vec![item.killmail_id.to_string(), status.into(), detail.into()]
        })
        .collect();
    let mut text = table(&["ID", "RESULT", "DETAIL"], &rows);
    if result.cancelled {
        text.push_str("Cancelled; the remaining killmails were not submitted.\n");
    }
    text
}

pub(super) fn protect(command: &Protect, core: &Core) -> Result<Output, CoreError> {
    use ProtectedVictimKind::{Character, Corporation};
    match command {
        Protect::List => {
            let store = core.snapshot()?.store;
            let automatic = store
                .characters
                .iter()
                .map(|character| {
                    json!({"id": character.id, "name": character.name, "corporation_id": character.corporation_id})
                })
                .collect::<Vec<_>>();
            let text = protection_text(&store);
            Ok(Output::new(
                json!({
                    "characters": store.manually_protected_characters,
                    "corporations": store.manually_protected_corporations,
                    "killmail_ids": store.manually_protected_killmail_ids,
                    "automatic_characters": automatic,
                }),
                text,
            ))
        }
        Protect::Add(target) => {
            let text = match target {
                ProtectAdd::Character { query } => {
                    let victim = core.add_protected_victim(Character, query)?;
                    format!("Protected character {} ({}).", victim.name, victim.id)
                }
                ProtectAdd::Corporation { query } => {
                    let victim = core.add_protected_victim(Corporation, query)?;
                    format!("Protected corporation {} ({}).", victim.name, victim.id)
                }
                ProtectAdd::Killmail { id } => {
                    core.set_killmail_protection(*id, true)?;
                    format!("Protected killmail {id}.")
                }
            };
            Ok(Output::new(json!({"protected": true}), text))
        }
        Protect::Remove(target) => {
            let text = match target {
                ProtectRemove::Character { id } => {
                    core.remove_protected_victim(Character, *id)?;
                    format!("Character {id} is no longer protected.")
                }
                ProtectRemove::Corporation { id } => {
                    core.remove_protected_victim(Corporation, *id)?;
                    format!("Corporation {id} is no longer protected.")
                }
                ProtectRemove::Killmail { id } => {
                    core.set_killmail_protection(*id, false)?;
                    format!("Killmail {id} is no longer protected.")
                }
            };
            Ok(Output::new(json!({"removed": true}), text))
        }
    }
}

fn protection_text(store: &Store) -> String {
    let mut text = String::from(
        "Automatically protected (authenticated characters and their corporations):\n",
    );
    if store.characters.is_empty() {
        text.push_str("  none\n");
    }
    for character in &store.characters {
        let corporation = character
            .corporation_name
            .as_deref()
            .map(|name| format!(", {name}"))
            .unwrap_or_default();
        let _ = writeln!(text, "  {} ({}){corporation}", character.name, character.id);
    }
    for (heading, victims) in [
        ("Protected characters", &store.manually_protected_characters),
        (
            "Protected corporations",
            &store.manually_protected_corporations,
        ),
    ] {
        let _ = writeln!(text, "{heading}:");
        if victims.is_empty() {
            text.push_str("  none\n");
        }
        for victim in victims {
            let _ = writeln!(text, "  {} ({})", victim.name, victim.id);
        }
    }
    let killmails = &store.manually_protected_killmail_ids;
    let ids = if killmails.is_empty() {
        "none".into()
    } else {
        killmails
            .iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    };
    let _ = writeln!(text, "Protected killmails: {ids}");
    text
}

pub(super) fn config(command: &Config, core: &Core) -> Result<Output, CoreError> {
    match command {
        Config::Get { key } => {
            let store = core.snapshot()?.store;
            let interval = (
                "refresh_interval_secs",
                json!(store.refresh_interval_secs),
                format!(
                    "refresh-interval: {}",
                    duration(store.refresh_interval_secs)
                ),
            );
            let show = (
                "show_protected_killmails",
                json!(store.show_protected_killmails),
                format!(
                    "show-protected-killmails: {}",
                    store.show_protected_killmails
                ),
            );
            let selected = match key {
                Some(ConfigKey::RefreshInterval) => vec![interval],
                Some(ConfigKey::ShowProtectedKillmails) => vec![show],
                None => vec![interval, show],
            };
            let mut json = serde_json::Map::new();
            let mut text = String::new();
            for (name, value, line) in selected {
                json.insert(name.into(), value);
                let _ = writeln!(text, "{line}");
            }
            Ok(Output::new(json.into(), text))
        }
        Config::Set(setting) => {
            let text = match setting {
                Setting::RefreshInterval { value } => {
                    core.set_refresh_interval(*value)?;
                    format!("Saved refresh-interval: {}.", duration(value.as_secs()))
                }
                Setting::ShowProtectedKillmails { value } => {
                    core.set_show_protected(*value)?;
                    format!("Saved show-protected-killmails: {value}.")
                }
            };
            Ok(Output::new(json!({"saved": true}), text))
        }
    }
}

const LOOPBACK_INSTRUCTIONS: &str = "\
Sign in with this URL in a browser on this machine. From another machine, forward the
callback first (ssh -L 17842:127.0.0.1:17842 HOST) or use `ekmp characters add --paste`.";

const PASTE_INSTRUCTIONS: &str = "\
Open this URL in a browser on any machine and sign in. The browser is then sent to a
127.0.0.1 address that fails to load; that is expected. Copy that address and paste it here.";

/// Whether a graphical session is available to open a browser in.
fn has_display() -> bool {
    ["DISPLAY", "WAYLAND_DISPLAY"]
        .iter()
        .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()))
}
