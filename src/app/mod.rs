mod ui;
mod worker;

use crate::{
    killmail::{
        character_summaries, remove_reported_killmails, report_state, submission_candidates,
        ReportState,
    },
    models::{Killmail, Store, ZkillCacheEntry},
    storage,
};
use std::{
    collections::VecDeque,
    sync::mpsc::{self, Receiver},
    thread,
};
use worker::{unix_time, WorkerEvent};

const STATUS_HISTORY_LIMIT: usize = 200;

#[derive(Clone, Copy)]
enum Operation {
    Authenticate,
    MigrateRefreshTokens,
    Load,
    CheckCachedStatuses,
    AddProtectedVictim,
    Post { bulk: bool },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProtectedVictimKind {
    Character,
    Corporation,
}

#[derive(Default)]
struct PostStats {
    total: usize,
    new: usize,
    existing: usize,
    failed: usize,
}

struct SessionReport {
    killmail_id: u64,
    url: String,
    status: SessionReportStatus,
}

#[derive(Clone, Copy)]
enum SessionReportStatus {
    Submitted,
    AlreadyPresent,
}

pub struct App {
    store: Store,
    killmails: Vec<Killmail>,
    latest_status: String,
    status_history: VecDeque<String>,
    event_rx: Option<Receiver<WorkerEvent>>,
    operation: Option<Operation>,
    post_stats: PostStats,
    pending_bulk: Option<Vec<Killmail>>,
    new_protected_victim_id: String,
    new_protected_victim_kind: ProtectedVictimKind,
    session_reports: Vec<SessionReport>,
}

impl App {
    pub fn new() -> Self {
        let mut store = storage::load();
        let removed_reported =
            remove_reported_killmails(&store.zkill_cache, &mut store.cached_killmails);
        let killmails = store.cached_killmails.clone();
        let mut app = Self {
            store,
            killmails,
            latest_status: "Ready".into(),
            status_history: VecDeque::from(["Ready".into()]),
            event_rx: None,
            operation: None,
            post_stats: PostStats::default(),
            pending_bulk: None,
            new_protected_victim_id: String::new(),
            new_protected_victim_kind: ProtectedVictimKind::Character,
            session_reports: Vec::new(),
        };
        if removed_reported > 0 {
            app.persist_or_log_error();
        }
        if app
            .store
            .characters
            .iter()
            .any(|character| character.uses_json_refresh_token_fallback())
        {
            app.migrate_refresh_tokens();
        } else {
            app.check_cached_statuses_on_startup();
        }
        app
    }

    fn is_busy(&self) -> bool {
        self.operation.is_some()
    }

    fn check_cached_statuses_on_startup(&mut self) {
        let now = unix_time();
        let unknown_count = self
            .killmails
            .iter()
            .filter(|mail| report_state(&self.store, mail.id, now) == ReportState::Unknown)
            .count();
        if unknown_count == 0 || self.store.characters.is_empty() {
            return;
        }

        let store = self.store.clone();
        let killmails = self.killmails.clone();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            worker::check_zkill_statuses(&store, &killmails, &tx);
            let _ = tx.send(WorkerEvent::Finished);
        });
        self.log(format!(
            "Checking zKillboard status for {unknown_count} cached killmails..."
        ));
        self.event_rx = Some(rx);
        self.operation = Some(Operation::CheckCachedStatuses);
    }

    fn migrate_refresh_tokens(&mut self) {
        let characters = self.store.characters.clone();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || worker::migrate_refresh_tokens(characters, tx));
        self.log("Moving refresh tokens to the system credential store...");
        self.event_rx = Some(rx);
        self.operation = Some(Operation::MigrateRefreshTokens);
    }

    fn has_json_refresh_token_fallback(&self) -> bool {
        self.store
            .characters
            .iter()
            .any(|character| character.uses_json_refresh_token_fallback())
    }

    fn log(&mut self, message: impl Into<String>) {
        let message = message.into();
        self.latest_status.clone_from(&message);
        if self.status_history.len() == STATUS_HISTORY_LIMIT {
            self.status_history.pop_front();
        }
        self.status_history.push_back(message);
    }

    fn begin_auth(&mut self) {
        if self.is_busy() {
            self.log("Another operation is already in progress");
            return;
        }
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || worker::authenticate(tx));
        self.log("Authorize the character in your browser...");
        self.event_rx = Some(rx);
        self.operation = Some(Operation::Authenticate);
    }

    fn refresh_killmails(&mut self) {
        if self.is_busy() {
            self.log("Another operation is already in progress");
            return;
        }
        if self.store.characters.is_empty() {
            self.log("Authenticate at least one character first");
            return;
        }
        let store = self.store.clone();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || worker::load_killmails_and_statuses(store, tx));
        self.log("Loading recent killmails from ESI...");
        self.event_rx = Some(rx);
        self.operation = Some(Operation::Load);
    }

    fn request_bulk_post(&mut self) {
        let mails = submission_candidates(&self.store, self.killmails.clone(), true, unix_time());
        if mails.is_empty() {
            self.log("There are no confirmed unreported killmails to submit");
        } else {
            self.pending_bulk = Some(mails);
        }
    }

    fn begin_add_protected_victim(&mut self) {
        if self.is_busy() {
            self.log("Another operation is already in progress");
            return;
        }
        let id = match self.new_protected_victim_id.trim().parse::<u64>() {
            Ok(id) if id > 0 => id,
            _ => {
                self.log("Enter a valid numeric EVE character or corporation ID");
                return;
            }
        };
        let kind = self.new_protected_victim_kind;
        let automatically_present = match kind {
            ProtectedVictimKind::Character => {
                self.store.characters.iter().any(|entry| entry.id == id)
            }
            ProtectedVictimKind::Corporation => self
                .store
                .characters
                .iter()
                .any(|entry| entry.corporation_id == Some(id)),
        };
        let manually_present = match kind {
            ProtectedVictimKind::Character => self
                .store
                .manually_protected_characters
                .iter()
                .any(|entry| entry.id == id),
            ProtectedVictimKind::Corporation => self
                .store
                .manually_protected_corporations
                .iter()
                .any(|entry| entry.id == id),
        };
        if automatically_present || manually_present {
            self.log("That protected victim is already in the list");
            return;
        }

        let (tx, rx) = mpsc::channel();
        thread::spawn(move || worker::resolve_protected_victim(kind, id, tx));
        self.log(format!("Resolving EVE ID {id}..."));
        self.event_rx = Some(rx);
        self.operation = Some(Operation::AddProtectedVictim);
    }

    fn start_posts(&mut self, mails: Vec<Killmail>, bulk: bool) {
        if self.is_busy() {
            self.log("Another operation is already in progress");
            return;
        }
        let mails = submission_candidates(&self.store, mails, bulk, unix_time());
        if mails.is_empty() {
            self.log("There are no eligible unreported killmails to submit");
            return;
        }
        let total = mails.len();
        let first_id = mails[0].id;
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || worker::post_killmails(mails, tx));
        self.post_stats = PostStats {
            total,
            ..PostStats::default()
        };
        self.log(if bulk {
            format!("Starting submission of {total} unreported killmails...")
        } else {
            format!("Starting submission of killmail {first_id}...")
        });
        self.event_rx = Some(rx);
        self.operation = Some(Operation::Post { bulk });
    }

    fn poll_worker(&mut self) {
        let events = self
            .event_rx
            .as_ref()
            .map(|rx| rx.try_iter().collect::<Vec<_>>())
            .unwrap_or_default();
        for event in events {
            self.handle_event(event);
        }
    }

    fn handle_event(&mut self, event: WorkerEvent) {
        match event {
            WorkerEvent::Status(message) => self.log(message),
            WorkerEvent::Character(character) => {
                let name = character.name.clone();
                self.store.characters.retain(|old| old.id != character.id);
                self.store.characters.push(character);
                self.persist_or_log_error();
                self.log(format!("Character {name} authenticated"));
            }
            WorkerEvent::CharactersRefreshed(characters) => {
                self.store.characters = characters;
                self.persist_or_log_error();
            }
            WorkerEvent::RefreshTokensMigrated(characters) => {
                self.store.characters = characters;
                self.persist_or_log_error();
            }
            WorkerEvent::ProtectedVictimResolved { kind, victim } => {
                let label = match kind {
                    ProtectedVictimKind::Character => {
                        self.store
                            .manually_protected_characters
                            .push(victim.clone());
                        "character"
                    }
                    ProtectedVictimKind::Corporation => {
                        self.store
                            .manually_protected_corporations
                            .push(victim.clone());
                        "corporation"
                    }
                };
                self.new_protected_victim_id.clear();
                self.persist_or_log_error();
                self.log(format!(
                    "Added protected {label}: {} ({})",
                    victim.name, victim.id
                ));
            }
            WorkerEvent::KillmailsLoaded(killmails) => {
                let count = killmails.len();
                self.store.cached_killmails.clone_from(&killmails);
                self.prune_persisted_reported_killmails();
                self.killmails = killmails;
                self.persist_or_log_error();
                self.log(format!(
                    "Loaded {count} unique recent killmails; checking zKillboard status..."
                ));
            }
            WorkerEvent::LookupComplete {
                character_name,
                checked_ids,
                reported_ids,
                checked_at,
            } => {
                let reported_count = checked_ids
                    .iter()
                    .filter(|id| reported_ids.contains(id))
                    .count();
                for id in checked_ids {
                    let already_reported = self
                        .store
                        .zkill_cache
                        .get(&id)
                        .is_some_and(|entry| entry.reported);
                    self.store.zkill_cache.insert(
                        id,
                        ZkillCacheEntry {
                            reported: already_reported || reported_ids.contains(&id),
                            checked_at,
                        },
                    );
                }
                self.prune_persisted_reported_killmails();
                self.persist_or_log_error();
                self.log(format!(
                    "Checked {character_name} on zKillboard: found {reported_count} reported killmails"
                ));
            }
            WorkerEvent::LookupFailed {
                character_name,
                error,
            } => self.log(format!(
                "Could not check zKillboard status for {character_name}: {error}"
            )),
            WorkerEvent::PostComplete {
                killmail_id,
                result,
            } => match result {
                Ok(outcome) => {
                    self.store.zkill_cache.insert(
                        killmail_id,
                        ZkillCacheEntry {
                            reported: true,
                            checked_at: unix_time(),
                        },
                    );
                    self.prune_persisted_reported_killmails();
                    self.session_reports
                        .retain(|report| report.killmail_id != killmail_id);
                    self.session_reports.push(SessionReport {
                        killmail_id,
                        url: outcome.url.clone(),
                        status: if outcome.new {
                            SessionReportStatus::Submitted
                        } else {
                            SessionReportStatus::AlreadyPresent
                        },
                    });
                    self.persist_or_log_error();
                    if outcome.new {
                        self.post_stats.new += 1;
                        self.log(format!(
                            "Killmail {killmail_id} was reported successfully: {}",
                            outcome.url
                        ));
                    } else {
                        self.post_stats.existing += 1;
                        self.log(format!(
                            "Killmail {killmail_id} was already on zKillboard: {}",
                            outcome.url
                        ));
                    }
                }
                Err(error) => {
                    if self
                        .store
                        .zkill_cache
                        .get(&killmail_id)
                        .is_some_and(|entry| !entry.reported)
                    {
                        self.store.zkill_cache.remove(&killmail_id);
                        self.persist_or_log_error();
                    }
                    self.post_stats.failed += 1;
                    self.log(format!("Killmail {killmail_id} submission failed: {error}"));
                }
            },
            WorkerEvent::Finished => self.finish_operation(),
            WorkerEvent::Failed(error) => {
                self.log(error);
                self.operation = None;
                self.event_rx = None;
            }
        }
    }

    fn finish_operation(&mut self) {
        let operation = self.operation.take();
        self.event_rx = None;
        match operation {
            Some(Operation::Authenticate) => {}
            Some(Operation::MigrateRefreshTokens) => self.check_cached_statuses_on_startup(),
            Some(Operation::AddProtectedVictim) => {}
            Some(Operation::CheckCachedStatuses) | Some(Operation::Load) => {
                self.log_character_summaries();
            }
            Some(Operation::Post { bulk }) => {
                self.log_character_summaries();
                let label = if bulk {
                    "Bulk submission"
                } else {
                    "Submission"
                };
                self.log(format!(
                    "{label} complete: {} new, {} already present, {} failed out of {}",
                    self.post_stats.new,
                    self.post_stats.existing,
                    self.post_stats.failed,
                    self.post_stats.total
                ));
            }
            None => {}
        }
    }

    fn persist_or_log_error(&mut self) {
        if let Err(error) = storage::persist(&self.store) {
            self.log(format!("Could not save local state: {error}"));
        }
    }

    fn prune_persisted_reported_killmails(&mut self) {
        remove_reported_killmails(&self.store.zkill_cache, &mut self.store.cached_killmails);
    }

    fn log_character_summaries(&mut self) {
        let messages = character_summaries(&self.store, &self.killmails, unix_time());
        for message in messages {
            self.log(message);
        }
    }
}
