use super::{
    worker::unix_time, App, Operation, SessionReport, SessionReportStatus, SubmissionMode,
};
use crate::{
    killmail::remove_killmails_for_removed_character,
    models::{ProtectedVictimKind, ZkillCacheEntry},
};

impl App {
    pub(super) fn poll_worker(&mut self) {
        let events = self
            .active_operation
            .as_ref()
            .map(|active| active.events.try_iter().collect::<Vec<_>>())
            .unwrap_or_default();
        for event in events {
            self.handle_event(event);
        }
    }

    fn handle_event(&mut self, event: super::WorkerEvent) {
        match event {
            super::WorkerEvent::Status(message) => self.log(message),
            super::WorkerEvent::Character(character) => {
                let name = character.name.clone();
                self.store.characters.retain(|old| old.id != character.id);
                self.store.characters.push(character);
                self.persist_or_log_error();
                self.log(format!("Character {name} authenticated"));
            }
            super::WorkerEvent::CharactersRefreshed(characters) => {
                self.store.characters = characters;
                self.persist_or_log_error();
            }
            super::WorkerEvent::RefreshTokensMigrated(characters) => {
                self.store.characters = characters;
                self.persist_or_log_error();
            }
            super::WorkerEvent::CharacterRemoved {
                id,
                name,
                credential_error,
            } => {
                self.store.characters.retain(|character| character.id != id);
                let store = self.store.clone();
                let removed_count = remove_killmails_for_removed_character(
                    &store,
                    &mut self.store.cached_killmails,
                    id,
                );
                self.persist_or_log_error();
                match credential_error {
                    Some(error) => self.log(format!(
                        "Removed {name}, {removed_count} cached killmails, but could not delete its system credential: {error}"
                    )),
                    None => self.log(format!(
                        "Removed {name}, its refresh token, and {removed_count} cached killmails"
                    )),
                }
            }
            super::WorkerEvent::ProtectedVictimResolved { kind, victim } => {
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
            super::WorkerEvent::KillmailsLoaded(killmails) => {
                let count = killmails.len();
                self.store.cached_killmails = killmails;
                self.prune_persisted_reported_killmails();
                self.persist_or_log_error();
                self.log(format!(
                    "Loaded {count} unique recent killmails; checking zKillboard status..."
                ));
            }
            super::WorkerEvent::LookupComplete {
                character_name,
                checked_ids,
                reported_ids,
                checked_at,
            } => {
                let checked_count = checked_ids.len();
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
                    "Completed status check for {character_name} ({checked_count} killmails checked)"
                ));
            }
            super::WorkerEvent::LookupFailed {
                character_name,
                error,
            } => self.log(format!(
                "Could not check zKillboard status for {character_name}: {error}"
            )),
            super::WorkerEvent::LookupIncomplete { character_name } => self.log(format!(
                "Could not confirm zKillboard status for {character_name} within the three-page lookup limit"
            )),
            super::WorkerEvent::PostComplete {
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
            super::WorkerEvent::Finished => self.finish_operation(),
            super::WorkerEvent::Failed(error) => {
                self.log(error);
                self.active_operation = None;
            }
        }
    }

    fn finish_operation(&mut self) {
        let operation = self.active_operation.take().map(|active| active.kind);
        match operation {
            Some(Operation::Authenticate) => {}
            Some(Operation::MigrateRefreshTokens) => self.check_cached_statuses_on_startup(),
            Some(Operation::RemoveCharacter) => {}
            Some(Operation::AddProtectedVictim) => {}
            Some(Operation::CheckCachedStatuses) | Some(Operation::Load) => {}
            Some(Operation::Post(mode)) => {
                let label = match mode {
                    SubmissionMode::Bulk => "Bulk submission",
                    SubmissionMode::Individual => "Submission",
                };
                self.log(format!(
                    "{label} complete: {} requests completed, {} failed out of {}",
                    self.post_stats.new + self.post_stats.existing,
                    self.post_stats.failed,
                    self.post_stats.total
                ));
            }
            None => {}
        }
    }
}
