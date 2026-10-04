use crate::clock::unix_time;
use crate::{
    integrations::{
        backend::{Backend, LoadKillmailsOutcome},
        zkill, ApiError, ApiResult,
    },
    models::{
        Character, Killmail, ProtectedVictim, ProtectedVictimKind, Store, ZkillEntry, ZkillPage,
        ZkillStatus,
    },
};
use serde::Deserialize;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{atomic::AtomicBool, Mutex},
    time::Duration,
};

#[derive(Deserialize)]
struct Scenario {
    name: String,
    #[serde(default)]
    initial_store: Store,
    #[serde(default)]
    connect_characters: Vec<Character>,
    #[serde(default)]
    killmails: Vec<Killmail>,
    #[serde(default)]
    resolved_characters: Vec<ProtectedVictim>,
    #[serde(default)]
    resolved_corporations: Vec<ProtectedVictim>,
    #[serde(default)]
    reported_kills: HashMap<u64, Vec<u64>>,
    #[serde(default)]
    reported_losses: HashMap<u64, Vec<u64>>,
    #[serde(default)]
    confirmed_unreported_ids: Vec<u64>,
    #[serde(default)]
    post_results: HashMap<u64, ScenarioPostResult>,
    load_error: Option<String>,
    status_error: Option<String>,
}

#[derive(Clone, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
enum ScenarioPostResult {
    New,
    Existing,
    Error { message: String },
}

pub(crate) struct LoadedScenario {
    pub name: String,
    pub store: Store,
    pub backend: SimulatorBackend,
}

pub(crate) struct SimulatorBackend {
    /// The simulated world; its initial store and connection queue are moved out on load.
    scenario: Scenario,
    known_characters: Vec<Character>,
    connect_characters: Mutex<VecDeque<Character>>,
    posted_ids: Mutex<Vec<u64>>,
}

pub(crate) fn load(name: &str) -> Result<LoadedScenario, String> {
    let json = match name {
        "mixed" => include_str!("../../dev/scenarios/mixed.json"),
        "errors" => include_str!("../../dev/scenarios/errors.json"),
        _ => {
            return Err(format!(
                "unknown simulation scenario {name:?}; use mixed or errors"
            ))
        }
    };
    load_json(json)
}

fn load_json(json: &str) -> Result<LoadedScenario, String> {
    let mut scenario: Scenario =
        serde_json::from_str(json).map_err(|error| format!("invalid scenario JSON: {error}"))?;
    validate(&scenario)?;

    let now = unix_time();
    for id in &scenario.confirmed_unreported_ids {
        scenario.initial_store.zkill_status.insert(
            *id,
            ZkillStatus::Unreported {
                valid_until: now + 60 * 60,
            },
        );
    }

    let store = std::mem::take(&mut scenario.initial_store);
    let connect_characters = std::mem::take(&mut scenario.connect_characters);
    let mut known_characters = store.characters.clone();
    known_characters.extend(connect_characters.iter().cloned());
    Ok(LoadedScenario {
        name: scenario.name.clone(),
        store,
        backend: SimulatorBackend {
            scenario,
            known_characters,
            connect_characters: Mutex::new(connect_characters.into()),
            posted_ids: Mutex::new(Vec::new()),
        },
    })
}

fn validate(scenario: &Scenario) -> Result<(), String> {
    let mut ids = HashSet::new();
    for mail in &scenario.killmails {
        if !ids.insert(mail.id) {
            return Err(format!(
                "scenario contains duplicate killmail ID {}",
                mail.id
            ));
        }
    }
    if let Some(id) = scenario.post_results.keys().find(|id| !ids.contains(id)) {
        return Err(format!(
            "scenario post result references missing killmail ID {id}"
        ));
    }
    Ok(())
}

impl SimulatorBackend {
    fn status_page(
        &self,
        reported: &HashMap<u64, Vec<u64>>,
        character_id: u64,
        page: usize,
    ) -> ApiResult<ZkillPage> {
        if let Some(error) = &self.scenario.status_error {
            return Err(error.clone().into());
        }
        let entries = reported
            .get(&character_id)
            .filter(|_| page == 1)
            .into_iter()
            .flatten()
            .map(|id| ZkillEntry {
                killmail_id: *id,
                killmail_time: self
                    .scenario
                    .killmails
                    .iter()
                    .find(|mail| mail.id == *id)
                    .map(|mail| mail.time.clone())
                    .unwrap_or_else(|| "2000-01-01T00:00:00Z".into()),
            })
            .collect();
        let observed_at = unix_time();
        Ok(ZkillPage {
            entries,
            observed_at,
            valid_until: observed_at + 60 * 60,
        })
    }

    #[cfg(all(test, feature = "gui"))]
    pub(crate) fn posted_ids(&self) -> Vec<u64> {
        self.posted_ids.lock().unwrap().clone()
    }
}

impl Backend for SimulatorBackend {
    fn authenticate(
        &self,
        _cancelled: &AtomicBool,
        _open_browser: bool,
        _on_authorization_url: &dyn Fn(&str),
    ) -> ApiResult<Character> {
        self.connect_characters
            .lock()
            .map_err(|_| "simulation character queue is unavailable".to_string())?
            .pop_front()
            .ok_or_else(|| "the simulation has no more characters to connect".into())
    }

    fn refresh_character_affiliation(
        &self,
        character: &mut Character,
        cancelled: &AtomicBool,
    ) -> ApiResult<()> {
        if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(ApiError::Cancelled);
        }
        let known = self
            .known_characters
            .iter()
            .find(|known| known.id == character.id)
            .ok_or_else(|| format!("simulation has no character {}", character.id))?;
        character.name.clone_from(&known.name);
        character.corporation_id = known.corporation_id;
        character
            .corporation_name
            .clone_from(&known.corporation_name);
        Ok(())
    }

    fn load_killmails(
        &self,
        _characters: &[Character],
        _cached_killmails: &[Killmail],
        _reported_ids: &HashSet<u64>,
        cancelled: &AtomicBool,
        _on_character_updated: &mut dyn FnMut(&Character) -> ApiResult<()>,
    ) -> ApiResult<LoadKillmailsOutcome> {
        if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(ApiError::Cancelled);
        }
        match &self.scenario.load_error {
            Some(error) => Err(error.clone().into()),
            None => Ok(LoadKillmailsOutcome {
                killmails: self.scenario.killmails.clone(),
                character_failures: Vec::new(),
            }),
        }
    }

    fn resolve_protected_victim(
        &self,
        kind: ProtectedVictimKind,
        query: &str,
    ) -> ApiResult<ProtectedVictim> {
        let candidates = match kind {
            ProtectedVictimKind::Character => &self.scenario.resolved_characters,
            ProtectedVictimKind::Corporation => &self.scenario.resolved_corporations,
        };
        candidates
            .iter()
            .find(|victim| {
                victim.name.eq_ignore_ascii_case(query) || query == victim.id.to_string()
            })
            .cloned()
            .ok_or_else(|| format!("the simulation has no exact protected victim {query:?}").into())
    }

    fn killmail_page(
        &self,
        kind: zkill::MailKind,
        character_id: u64,
        page: usize,
    ) -> ApiResult<ZkillPage> {
        let reported = match kind {
            zkill::MailKind::Kills => &self.scenario.reported_kills,
            zkill::MailKind::Losses => &self.scenario.reported_losses,
        };
        self.status_page(reported, character_id, page)
    }

    fn post(&self, mail: &Killmail) -> ApiResult<zkill::PostOutcome> {
        self.posted_ids
            .lock()
            .map_err(|_| "simulation post ledger is unavailable".to_string())?
            .push(mail.id);
        match self.scenario.post_results.get(&mail.id) {
            Some(result @ (ScenarioPostResult::New | ScenarioPostResult::Existing)) => {
                Ok(zkill::PostOutcome {
                    new: matches!(result, ScenarioPostResult::New),
                    url: format!("https://example.invalid/kill/{}/", mail.id),
                })
            }
            Some(ScenarioPostResult::Error { message }) => Err(message.clone().into()),
            None => Err(format!(
                "simulation has no configured post result for killmail {}",
                mail.id
            )
            .into()),
        }
    }

    fn save_refresh_token(&self, _character_id: u64, _token: &str) -> ApiResult<()> {
        Ok(())
    }

    fn delete_refresh_token(&self, _character_id: u64) -> ApiResult<()> {
        Ok(())
    }

    fn request_spacing(&self) -> Duration {
        Duration::ZERO
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_scenarios_are_valid() {
        assert_eq!(load("mixed").unwrap().name, "mixed");
        assert_eq!(load("errors").unwrap().name, "errors");
    }

    #[test]
    fn failed_simulated_submission_is_recorded_without_changing_cached_state() {
        let loaded = load("errors").unwrap();
        assert!(!loaded.store.characters.is_empty());
        let mail = &loaded.backend.scenario.killmails[0];
        assert!(loaded.backend.post(mail).is_err());
        assert_eq!(*loaded.backend.posted_ids.lock().unwrap(), vec![mail.id]);
        assert!(matches!(
            loaded.store.zkill_status[&mail.id],
            ZkillStatus::Unreported { .. }
        ));
    }

    #[test]
    fn duplicate_killmail_ids_are_rejected() {
        let error = load_json(
            r#"{
                "name":"bad",
                "killmails":[
                    {"id":1,"hash":"one","sources":[],"victim_id":null,"victim_corporation_id":null,"victim":"A","ship":"A","time":"2026-01-01T00:00:00Z"},
                    {"id":1,"hash":"two","sources":[],"victim_id":null,"victim_corporation_id":null,"victim":"B","ship":"B","time":"2026-01-01T00:00:00Z"}
                ]
            }"#,
        )
        .err()
        .unwrap();

        assert!(error.contains("duplicate killmail ID 1"));
    }
}
