use super::{
    worker::unix_time, App, Operation, SessionReport, SessionReportStatus, SubmissionMode,
};
use crate::{
    killmail::remove_killmails_for_removed_character,
    models::{Character, Killmail, ProtectedVictimKind, ZkillCacheEntry},
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
                if self.protected_victim_already_present(kind, victim.id) {
                    self.new_protected_victim_query.clear();
                    self.log(format!(
                        "{} ({}) is already a protected victim",
                        victim.name, victim.id
                    ));
                    return;
                }
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
                self.new_protected_victim_query.clear();
                self.persist_or_log_error();
                self.log(format!(
                    "Added protected {label}: {} ({})",
                    victim.name, victim.id
                ));
            }
            super::WorkerEvent::KillmailsLoaded(killmails) => {
                let fetched_count = killmails.len();
                let source_summaries = self
                    .store
                    .characters
                    .iter()
                    .map(|character| character_killmail_summary(character, &killmails))
                    .collect::<Vec<_>>();
                let source_count = self.store.characters.len();
                self.store.cached_killmails = killmails;
                self.expanded_killmail_ids.clear();
                self.prune_persisted_reported_killmails();
                let retained_count = self.store.cached_killmails.len();
                let removed_reported = fetched_count.saturating_sub(retained_count);
                self.persist_or_log_error();
                for summary in source_summaries {
                    self.log(summary);
                }
                self.log(format!(
                    "Review queue - Combined {fetched_count} unique killmail IDs from {source_count} authenticated character{}. Duplicate IDs visible to multiple characters are counted once. Retained {retained_count}; removed {removed_reported} already known as reported.",
                    if source_count == 1 { "" } else { "s" },
                ));
            }
            super::WorkerEvent::LookupComplete {
                character_name,
                character_id,
                checked_ids,
                reported_ids,
                checked_at,
            } => {
                let checked_count = checked_ids.len();
                let reported_count = reported_ids.len();
                let unreported_count = checked_count.saturating_sub(reported_count);
                let ids = formatted_killmail_ids(checked_ids.iter().copied());
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
                    "zKillboard - {character_name} ({character_id}) - Checked {checked_count} killmail IDs: {reported_count} already reported and {unreported_count} confirmed unreported. Killmail IDs: {ids}"
                ));
            }
            super::WorkerEvent::LookupFailed {
                character_name,
                character_id,
                error,
            } => self.log(format!(
                "zKillboard - {character_name} ({character_id}) - Status check failed; affected killmails remain unavailable for posting. {error}"
            )),
            super::WorkerEvent::LookupIncomplete {
                character_name,
                character_id,
            } => self.log(format!(
                "zKillboard - {character_name} ({character_id}) - Status check reached the three-page lookup limit; affected killmails remain unavailable for posting"
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
            Some(Operation::CheckCachedStatuses) => self.refresh_killmails_on_startup(),
            Some(Operation::Load) => {}
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

fn character_killmail_summary(character: &Character, killmails: &[Killmail]) -> String {
    let character_mails = killmails
        .iter()
        .filter(|mail| mail.sources.iter().any(|source| source.id == character.id))
        .collect::<Vec<_>>();
    let losses = character_mails
        .iter()
        .filter(|mail| mail.victim_id == Some(character.id))
        .count();
    let kills = character_mails.len() - losses;
    let ids = formatted_killmail_ids(character_mails.iter().map(|mail| mail.id));
    format!(
        "ESI - {} ({}) - Returned {} recent killmails: {kills} kills and {losses} losses. Killmail IDs: {ids}",
        character.name,
        character.id,
        character_mails.len(),
    )
}

fn formatted_killmail_ids(ids: impl IntoIterator<Item = u64>) -> String {
    let mut ids = ids.into_iter().collect::<Vec<_>>();
    ids.sort_unstable();
    ids.into_iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::CharacterSource;

    fn mail(id: u64, source_id: u64, victim_id: Option<u64>) -> Killmail {
        Killmail {
            id,
            hash: "hash".into(),
            sources: vec![CharacterSource {
                id: source_id,
                name: "Pilot".into(),
            }],
            victim_id,
            victim_corporation_id: None,
            victim: "Victim".into(),
            ship: "Ship".into(),
            time: "Time".into(),
            estimated_value_isk: None,
            detail: None,
        }
    }

    #[test]
    fn character_activity_summary_identifies_source_counts_and_killmail_ids() {
        let character = Character {
            id: 95_742_577,
            name: "Pilot".into(),
            refresh_token: None,
            corporation_id: None,
            corporation_name: None,
        };
        let killmails = vec![
            mail(30, character.id, None),
            mail(20, character.id, Some(character.id)),
            mail(10, 123, None),
        ];

        let summary = character_killmail_summary(&character, &killmails);

        assert!(summary.contains("Pilot (95742577)"));
        assert!(summary.contains("2 recent killmails: 1 kills and 1 losses"));
        assert!(summary.contains("Killmail IDs: 20, 30"));
        assert!(!summary.contains("10,"));
    }
}
