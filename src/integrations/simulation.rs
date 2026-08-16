use crate::{
    integrations::{backend::Backend, zkill},
    models::{Character, Killmail, ProtectedVictim, ProtectedVictimKind, Store, ZkillCacheEntry},
};
use serde::Deserialize;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{atomic::AtomicBool, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
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
    known_characters: Vec<Character>,
    connect_characters: Mutex<VecDeque<Character>>,
    killmails: Vec<Killmail>,
    resolved_characters: Vec<ProtectedVictim>,
    resolved_corporations: Vec<ProtectedVictim>,
    reported_kills: HashMap<u64, Vec<u64>>,
    reported_losses: HashMap<u64, Vec<u64>>,
    post_results: HashMap<u64, ScenarioPostResult>,
    load_error: Option<String>,
    status_error: Option<String>,
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

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    for id in &scenario.confirmed_unreported_ids {
        scenario.initial_store.zkill_cache.insert(
            *id,
            ZkillCacheEntry {
                reported: false,
                checked_at: now,
            },
        );
    }

    let mut known_characters = scenario.initial_store.characters.clone();
    known_characters.extend(scenario.connect_characters.iter().cloned());
    Ok(LoadedScenario {
        name: scenario.name,
        store: scenario.initial_store,
        backend: SimulatorBackend {
            known_characters,
            connect_characters: Mutex::new(scenario.connect_characters.into()),
            killmails: scenario.killmails,
            resolved_characters: scenario.resolved_characters,
            resolved_corporations: scenario.resolved_corporations,
            reported_kills: scenario.reported_kills,
            reported_losses: scenario.reported_losses,
            post_results: scenario.post_results,
            load_error: scenario.load_error,
            status_error: scenario.status_error,
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
    ) -> Result<Vec<zkill::KillEntry>, String> {
        if let Some(error) = &self.status_error {
            return Err(error.clone());
        }
        if page != 1 {
            return Ok(Vec::new());
        }
        Ok(reported
            .get(&character_id)
            .into_iter()
            .flatten()
            .map(|id| zkill::KillEntry {
                killmail_id: *id,
                killmail_time: self
                    .killmails
                    .iter()
                    .find(|mail| mail.id == *id)
                    .map(|mail| mail.time.clone())
                    .unwrap_or_else(|| "2000-01-01T00:00:00Z".into()),
            })
            .collect())
    }

    #[cfg(test)]
    pub(crate) fn posted_ids(&self) -> Vec<u64> {
        self.posted_ids.lock().unwrap().clone()
    }
}

impl Backend for SimulatorBackend {
    fn authenticate(&self, _cancelled: &AtomicBool) -> Result<Character, String> {
        self.connect_characters
            .lock()
            .map_err(|_| "simulation character queue is unavailable".to_string())?
            .pop_front()
            .ok_or_else(|| "the simulation has no more characters to connect".into())
    }

    fn refresh_character_affiliation(&self, character: &mut Character) -> Result<(), String> {
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

    fn load_killmails(&self, _characters: &[Character]) -> Result<Vec<Killmail>, String> {
        match &self.load_error {
            Some(error) => Err(error.clone()),
            None => Ok(self.killmails.clone()),
        }
    }

    fn resolve_protected_victim(
        &self,
        kind: ProtectedVictimKind,
        query: &str,
    ) -> Result<ProtectedVictim, String> {
        let candidates = match kind {
            ProtectedVictimKind::Character => &self.resolved_characters,
            ProtectedVictimKind::Corporation => &self.resolved_corporations,
        };
        candidates
            .iter()
            .find(|victim| {
                victim.name.eq_ignore_ascii_case(query) || query == victim.id.to_string()
            })
            .cloned()
            .ok_or_else(|| format!("the simulation has no exact protected victim {query:?}"))
    }

    fn character_killmail_page(
        &self,
        character_id: u64,
        page: usize,
    ) -> Result<Vec<zkill::KillEntry>, String> {
        self.status_page(&self.reported_kills, character_id, page)
    }

    fn character_loss_killmail_page(
        &self,
        character_id: u64,
        page: usize,
    ) -> Result<Vec<zkill::KillEntry>, String> {
        self.status_page(&self.reported_losses, character_id, page)
    }

    fn post(&self, mail: &Killmail) -> Result<zkill::PostOutcome, String> {
        self.posted_ids
            .lock()
            .map_err(|_| "simulation post ledger is unavailable".to_string())?
            .push(mail.id);
        match self.post_results.get(&mail.id) {
            Some(ScenarioPostResult::New) => Ok(zkill::PostOutcome {
                new: true,
                url: format!("https://example.invalid/kill/{}/", mail.id),
            }),
            Some(ScenarioPostResult::Existing) => Ok(zkill::PostOutcome {
                new: false,
                url: format!("https://example.invalid/kill/{}/", mail.id),
            }),
            Some(ScenarioPostResult::Error { message }) => Err(message.clone()),
            None => Err(format!(
                "simulation has no configured post result for killmail {}",
                mail.id
            )),
        }
    }

    fn save_refresh_token(&self, _character_id: u64, _token: &str) -> Result<(), String> {
        Ok(())
    }

    fn delete_refresh_token(&self, _character_id: u64) -> Result<(), String> {
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
