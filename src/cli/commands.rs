//! Command handlers that change or read the store.

use super::{
    args::{
        Characters, Cli, Config, ConfigKey, PostArgs, Protect, ProtectAdd, ProtectRemove, Setting,
    },
    output::to_json,
    prompt::{confirm, read_line},
    Output,
};
use crate::{
    core::{Cancellation, Core, CoreError, PostSelection},
    integrations::auth::AuthFlow,
    models::{Character, ProtectedVictimKind, Store},
};
use serde::Serialize;
use serde_json::{json, Value};

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
            Ok((json!(output), 0))
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
            Ok((json!({"id": character.id, "name": character.name}), 0))
        }
        Characters::Remove { id, yes } => {
            confirm(
                *yes,
                &format!("Remove character {id}, credentials and unshared cached killmails?"),
                cancel,
            )?;
            let result = core.remove_character(*id)?;
            let code = u8::from(result.credential_warning.is_some());
            Ok((to_json(result)?, code))
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
            let output =
                json!({"completed": completed, "error": error.to_string(), "exit_code": code});
            return Ok((output, code));
        }
    };
    let code = if result.cancelled {
        130
    } else {
        u8::from(result.has_failures())
    };
    Ok((to_json(result)?, code))
}

pub(super) fn protect(command: &Protect, core: &Core) -> Result<Value, CoreError> {
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
            Ok(json!({
                "characters": store.manually_protected_characters,
                "corporations": store.manually_protected_corporations,
                "killmail_ids": store.manually_protected_killmail_ids,
                "automatic_characters": automatic,
            }))
        }
        Protect::Add(target) => {
            match target {
                ProtectAdd::Character { query } => {
                    core.add_protected_victim(Character, query)?;
                }
                ProtectAdd::Corporation { query } => {
                    core.add_protected_victim(Corporation, query)?;
                }
                ProtectAdd::Killmail { id } => core.set_killmail_protection(*id, true)?,
            }
            Ok(json!({"protected": true}))
        }
        Protect::Remove(target) => {
            match target {
                ProtectRemove::Character { id } => {
                    core.remove_protected_victim(Character, *id)?;
                }
                ProtectRemove::Corporation { id } => {
                    core.remove_protected_victim(Corporation, *id)?;
                }
                ProtectRemove::Killmail { id } => core.set_killmail_protection(*id, false)?,
            }
            Ok(json!({"removed": true}))
        }
    }
}

pub(super) fn config(command: &Config, core: &Core) -> Result<Value, CoreError> {
    match command {
        Config::Get { key } => {
            let store = core.snapshot()?.store;
            Ok(match key {
                Some(ConfigKey::RefreshInterval) => {
                    json!({"refresh_interval_secs": store.refresh_interval_secs})
                }
                Some(ConfigKey::ShowProtectedKillmails) => {
                    json!({"show_protected_killmails": store.show_protected_killmails})
                }
                None => json!({
                    "refresh_interval_secs": store.refresh_interval_secs,
                    "show_protected_killmails": store.show_protected_killmails,
                }),
            })
        }
        Config::Set(setting) => {
            match setting {
                Setting::RefreshInterval { value } => core.set_refresh_interval(*value)?,
                Setting::ShowProtectedKillmails { value } => core.set_show_protected(*value)?,
            }
            Ok(json!({"saved": true}))
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
