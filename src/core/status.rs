use super::{
    store::LockedStore,
    timing::{
        active_api_cooldown, check_cancelled, merge_api_cooldowns, reserve_zkill_request, unix_time,
    },
    Cancellation, Core, CoreError, CoreResult,
};
use crate::{
    integrations::{backend::Backend, zkill},
    killmail::{
        remove_reported_killmail_flags, remove_reported_killmails, report_state, ReportState,
    },
    models::{Store, ZkillCacheEntry, ZkillQueryCacheEntry, ZkillQueryKillmail},
};
use std::collections::HashSet;

const MAX_ZKILL_STATUS_PAGES: usize = 3;

impl Core {
    pub(super) fn refresh_statuses_locked(
        &self,
        locked: &mut LockedStore<'_>,
        cancelled: &Cancellation,
    ) -> CoreResult<StatusRefreshResult> {
        let checks = status_checks(locked.store(), unix_time());
        let mut result = StatusRefreshResult::default();
        for check in &checks {
            check_cancelled(cancelled)?;
            self.progress(format!(
                "Checking zKillboard status for {} ({})",
                check.character_name, check.character_id
            ));
            match lookup_status(self.backend.as_ref(), locked, check, cancelled) {
                Ok(lookup) => {
                    result.reported_found += lookup.reported.len();
                    if !lookup.complete {
                        result.incomplete += 1;
                        let message = format!(
                            "zKillboard status coverage was incomplete for {} ({})",
                            check.character_name, check.character_id
                        );
                        self.progress(message.clone());
                        result.messages.push(message);
                    }
                    apply_lookup(locked.store_mut(), check, &lookup);
                    locked.persist()?;
                    self.changed();
                }
                Err(CoreError::Cancelled) => return Err(CoreError::Cancelled),
                Err(error @ CoreError::Persistence(_)) => return Err(error),
                Err(error) => {
                    result.incomplete += 1;
                    let message = format!(
                        "zKillboard status check failed for {} ({}): {error}",
                        check.character_name, check.character_id
                    );
                    self.progress(message.clone());
                    result.messages.push(message);
                }
            }
            merge_api_cooldowns(locked.store_mut(), self.backend.take_api_cooldowns());
        }
        prune_reported(locked.store_mut());
        Ok(result)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KillmailRole {
    Kill,
    Loss,
}

#[derive(Clone, Debug)]
struct StatusCandidate {
    id: u64,
    time: String,
}

#[derive(Clone, Debug)]
struct StatusCheck {
    character_name: String,
    character_id: u64,
    role: KillmailRole,
    candidates: Vec<StatusCandidate>,
}

#[derive(Default)]
pub(super) struct StatusRefreshResult {
    pub(super) reported_found: usize,
    pub(super) incomplete: usize,
    pub(super) messages: Vec<String>,
}

struct StatusLookup {
    reported: HashSet<u64>,
    complete: bool,
    observed_at: u64,
    valid_until: u64,
}

fn status_checks(store: &Store, now: u64) -> Vec<StatusCheck> {
    let mut checks = Vec::new();
    for character in &store.characters {
        let character_mails = store
            .cached_killmails
            .iter()
            .filter(|mail| mail.sources.iter().any(|source| source.id == character.id));
        let (losses, kills): (Vec<_>, Vec<_>) =
            character_mails.partition(|mail| mail.victim_id == Some(character.id));
        for (role, mails) in [(KillmailRole::Kill, kills), (KillmailRole::Loss, losses)] {
            let candidates = mails
                .into_iter()
                .filter(|mail| report_state(store, mail.id, now) == ReportState::Unknown)
                .map(|mail| StatusCandidate {
                    id: mail.id,
                    time: mail.time.clone(),
                })
                .collect::<Vec<_>>();
            if !candidates.is_empty() {
                checks.push(StatusCheck {
                    character_name: character.name.clone(),
                    character_id: character.id,
                    role,
                    candidates,
                });
            }
        }
    }
    checks
}

fn lookup_status(
    backend: &dyn Backend,
    locked: &mut LockedStore<'_>,
    check: &StatusCheck,
    cancelled: &Cancellation,
) -> CoreResult<StatusLookup> {
    let candidate_ids = check
        .candidates
        .iter()
        .map(|candidate| candidate.id)
        .collect::<HashSet<_>>();
    let oldest_time = check
        .candidates
        .iter()
        .filter_map(|candidate| source_time(&candidate.time))
        .min()
        .ok_or_else(|| {
            CoreError::Operational("cached killmail has an invalid source timestamp".into())
        })?;
    let mut reported = HashSet::new();
    let mut observed_at = u64::MAX;
    let mut valid_until = u64::MAX;
    let mut complete = false;
    for page_number in 1..=MAX_ZKILL_STATUS_PAGES {
        check_cancelled(cancelled)?;
        let cache_key = query_cache_key(check.role, check.character_id, page_number);
        let now = unix_time();
        let page = match locked
            .store()
            .zkill_query_cache
            .get(&cache_key)
            .filter(|entry| entry.valid_until > now)
        {
            Some(cached) => zkill::LookupPage {
                entries: cached
                    .entries
                    .iter()
                    .map(|entry| zkill::KillEntry {
                        killmail_id: entry.id,
                        killmail_time: entry.time.clone(),
                    })
                    .collect(),
                observed_at: cached.observed_at,
                valid_until: cached.valid_until,
            },
            None => {
                if let Some(until) = active_api_cooldown(locked.store(), "zkillboard") {
                    return Err(CoreError::Operational(format!(
                        "zKillboard cooldown is active until {until}"
                    )));
                }
                reserve_zkill_request(locked, backend.request_spacing(), cancelled)?;
                let page = match check.role {
                    KillmailRole::Kill => {
                        backend.character_killmail_page(check.character_id, page_number)
                    }
                    KillmailRole::Loss => {
                        backend.character_loss_killmail_page(check.character_id, page_number)
                    }
                }
                .map_err(CoreError::Operational)?;
                locked.store_mut().zkill_query_cache.insert(
                    cache_key,
                    ZkillQueryCacheEntry {
                        entries: page
                            .entries
                            .iter()
                            .map(|entry| ZkillQueryKillmail {
                                id: entry.killmail_id,
                                time: entry.killmail_time.clone(),
                            })
                            .collect(),
                        observed_at: page.observed_at,
                        valid_until: page.valid_until,
                    },
                );
                page
            }
        };
        observed_at = observed_at.min(page.observed_at);
        valid_until = valid_until.min(page.valid_until);
        reported.extend(
            page.entries
                .iter()
                .filter(|entry| candidate_ids.contains(&entry.killmail_id))
                .map(|entry| entry.killmail_id),
        );
        complete = page.entries.len() < zkill::KILLMAILS_PER_PAGE
            || page
                .entries
                .last()
                .and_then(|entry| source_time(&entry.killmail_time))
                .is_some_and(|last_time| last_time <= oldest_time);
        if complete {
            break;
        }
    }
    if observed_at == u64::MAX {
        observed_at = 0;
    }
    if valid_until == u64::MAX {
        valid_until = 0;
    }
    Ok(StatusLookup {
        reported,
        complete,
        observed_at,
        valid_until,
    })
}

fn apply_lookup(store: &mut Store, check: &StatusCheck, lookup: &StatusLookup) {
    let now = unix_time();
    for candidate in &check.candidates {
        if store
            .zkill_cache
            .get(&candidate.id)
            .is_some_and(|entry| entry.reported)
        {
            continue;
        }
        if lookup.reported.contains(&candidate.id) {
            store.zkill_cache.insert(
                candidate.id,
                ZkillCacheEntry {
                    reported: true,
                    checked_at: lookup.observed_at,
                },
            );
            store.zkill_valid_until.remove(&candidate.id);
            store.post_attempts.remove(&candidate.id);
            continue;
        }
        if !lookup.complete || lookup.valid_until <= now {
            continue;
        }
        let Some(killmail_time) = source_time(&candidate.time) else {
            continue;
        };
        if killmail_time
            > lookup
                .observed_at
                .saturating_sub(zkill::WITHHOLDING_WINDOW_SECS)
        {
            continue;
        }
        if store
            .post_attempts
            .get(&candidate.id)
            .is_some_and(|attempt| lookup.observed_at <= attempt.attempted_at)
        {
            continue;
        }
        store.zkill_cache.insert(
            candidate.id,
            ZkillCacheEntry {
                reported: false,
                checked_at: lookup.observed_at,
            },
        );
        store
            .zkill_valid_until
            .insert(candidate.id, lookup.valid_until);
        store.post_attempts.remove(&candidate.id);
    }
}

fn query_cache_key(role: KillmailRole, character_id: u64, page: usize) -> String {
    let role = match role {
        KillmailRole::Kill => "kills",
        KillmailRole::Loss => "losses",
    };
    format!("{role}:{character_id}:{page}")
}

pub(super) fn prune_reported(store: &mut Store) {
    remove_reported_killmails(&store.zkill_cache, &mut store.cached_killmails);
    remove_reported_killmail_flags(
        &store.zkill_cache,
        &mut store.manually_protected_killmail_ids,
    );
}

fn source_time(time: &str) -> Option<u64> {
    let (date, time) = time.split_once('T')?;
    let mut date = date.split('-').map(|part| part.parse::<i64>().ok());
    let year = date.next()??;
    let month = date.next()??;
    let day = date.next()??;
    let time = time.strip_suffix('Z')?;
    let mut time = time.split(':');
    let hour = time.next()?.parse::<i64>().ok()?;
    let minute = time.next()?.parse::<i64>().ok()?;
    let second = time.next()?.split('.').next()?.parse::<i64>().ok()?;
    let days = days_from_civil(year, month, day)?;
    let seconds = days
        .checked_mul(86_400)?
        .checked_add(hour.checked_mul(3_600)?)?
        .checked_add(minute.checked_mul(60)?)?
        .checked_add(second)?;
    u64::try_from(seconds).ok()
}

fn days_from_civil(year: i64, month: i64, day: i64) -> Option<i64> {
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let adjusted_month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * adjusted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    Some(era * 146_097 + day_of_era - 719_468)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::test_support::*;
    use crate::models::PostAttempt;
    use std::sync::{atomic::Ordering, Arc};

    #[test]
    fn absent_shared_source_lookup_never_overwrites_positive_status() {
        let now = unix_time();
        let mut store = Store::default();
        store.zkill_cache.insert(
            42,
            ZkillCacheEntry {
                reported: true,
                checked_at: now - 10,
            },
        );
        let check = StatusCheck {
            character_name: "Other source".into(),
            character_id: 2,
            role: KillmailRole::Kill,
            candidates: vec![StatusCandidate {
                id: 42,
                time: "2020-01-01T00:00:00Z".into(),
            }],
        };
        apply_lookup(
            &mut store,
            &check,
            &StatusLookup {
                reported: HashSet::new(),
                complete: true,
                observed_at: now,
                valid_until: now + 3_600,
            },
        );

        assert!(store.zkill_cache[&42].reported);
    }

    #[test]
    fn uncertain_attempt_requires_later_source_observation() {
        let now = unix_time();
        let check = StatusCheck {
            character_name: "Pilot".into(),
            character_id: 1,
            role: KillmailRole::Kill,
            candidates: vec![StatusCandidate {
                id: 42,
                time: "2020-01-01T00:00:00Z".into(),
            }],
        };
        let mut store = Store::default();
        store
            .post_attempts
            .insert(42, PostAttempt { attempted_at: now });
        let mut lookup = StatusLookup {
            reported: HashSet::new(),
            complete: true,
            observed_at: now,
            valid_until: now + 3_600,
        };

        apply_lookup(&mut store, &check, &lookup);
        assert_eq!(report_state(&store, 42, now), ReportState::Unknown);

        lookup.observed_at += 1;
        apply_lookup(&mut store, &check, &lookup);
        assert_eq!(report_state(&store, 42, now), ReportState::Unreported);
        assert!(!store.post_attempts.contains_key(&42));
    }

    #[test]
    fn fresh_killmail_inside_withholding_window_remains_unknown() {
        let now = unix_time();
        let check = StatusCheck {
            character_name: "Pilot".into(),
            character_id: 1,
            role: KillmailRole::Kill,
            candidates: vec![StatusCandidate {
                id: 42,
                time: "9999-01-01T00:00:00Z".into(),
            }],
        };
        let mut store = Store::default();
        apply_lookup(
            &mut store,
            &check,
            &StatusLookup {
                reported: HashSet::new(),
                complete: true,
                observed_at: now,
                valid_until: now + 3_600,
            },
        );

        assert_eq!(report_state(&store, 42, now), ReportState::Unknown);
    }

    #[test]
    fn incomplete_lookup_does_not_establish_negative_status() {
        let now = unix_time();
        let check = StatusCheck {
            character_name: "Pilot".into(),
            character_id: 1,
            role: KillmailRole::Kill,
            candidates: vec![StatusCandidate {
                id: 42,
                time: "2020-01-01T00:00:00Z".into(),
            }],
        };
        let mut store = Store::default();

        apply_lookup(
            &mut store,
            &check,
            &StatusLookup {
                reported: HashSet::new(),
                complete: false,
                observed_at: now,
                valid_until: now + 3_600,
            },
        );

        assert_eq!(report_state(&store, 42, now), ReportState::Unknown);
    }

    #[test]
    fn cached_lookup_reuses_original_observation_and_expiry_without_network() {
        let now = unix_time();
        let backend = Arc::new(TestBackend::new());
        let mut store = postable_store();
        store.zkill_cache.remove(&42);
        store.zkill_valid_until.remove(&42);
        let original_expiry = now + 600;
        store.zkill_query_cache.insert(
            query_cache_key(KillmailRole::Kill, 1, 1),
            ZkillQueryCacheEntry {
                entries: Vec::new(),
                observed_at: now,
                valid_until: original_expiry,
            },
        );
        let core = Core::in_memory(backend.clone(), store);
        let mut locked = core.begin_operation().unwrap();
        let check = status_checks(locked.store(), now).remove(0);

        let lookup =
            lookup_status(backend.as_ref(), &mut locked, &check, &Cancellation::new()).unwrap();

        assert_eq!(lookup.observed_at, now);
        assert_eq!(lookup.valid_until, original_expiry);
        assert_eq!(backend.lookups.load(Ordering::Relaxed), 0);
        assert_eq!(
            locked
                .store()
                .zkill_query_cache
                .values()
                .next()
                .unwrap()
                .valid_until,
            original_expiry
        );
    }

    #[test]
    fn lookup_uses_oldest_observation_and_expiry_from_every_coverage_page() {
        let now = unix_time();
        let backend = Arc::new(TestBackend::new());
        let mut store = postable_store();
        store.zkill_cache.remove(&42);
        store.zkill_valid_until.remove(&42);
        store.zkill_query_cache.insert(
            query_cache_key(KillmailRole::Kill, 1, 1),
            ZkillQueryCacheEntry {
                entries: vec![
                    ZkillQueryKillmail {
                        id: 9_999,
                        time: "2021-01-01T00:00:00Z".into(),
                    };
                    zkill::KILLMAILS_PER_PAGE
                ],
                observed_at: now + 10,
                valid_until: now + 500,
            },
        );
        store.zkill_query_cache.insert(
            query_cache_key(KillmailRole::Kill, 1, 2),
            ZkillQueryCacheEntry {
                entries: Vec::new(),
                observed_at: now,
                valid_until: now + 400,
            },
        );
        let core = Core::in_memory(backend.clone(), store);
        let mut locked = core.begin_operation().unwrap();
        let check = status_checks(locked.store(), now).remove(0);

        let lookup =
            lookup_status(backend.as_ref(), &mut locked, &check, &Cancellation::new()).unwrap();

        assert!(lookup.complete);
        assert_eq!(lookup.observed_at, now);
        assert_eq!(lookup.valid_until, now + 400);
        assert_eq!(backend.lookups.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn uncertain_attempt_survives_restart_and_rejects_old_evidence() {
        let now = unix_time();
        let mut original = Store::default();
        original
            .post_attempts
            .insert(42, PostAttempt { attempted_at: now });
        let encoded = serde_json::to_vec(&original).unwrap();
        let mut restored: Store = serde_json::from_slice(&encoded).unwrap();
        let check = StatusCheck {
            character_name: "Pilot".into(),
            character_id: 1,
            role: KillmailRole::Kill,
            candidates: vec![StatusCandidate {
                id: 42,
                time: "2020-01-01T00:00:00Z".into(),
            }],
        };

        apply_lookup(
            &mut restored,
            &check,
            &StatusLookup {
                reported: HashSet::new(),
                complete: true,
                observed_at: now,
                valid_until: now + 3_600,
            },
        );

        assert_eq!(report_state(&restored, 42, now), ReportState::Unknown);
        assert!(restored.post_attempts.contains_key(&42));
    }

    #[test]
    fn withholding_boundary_is_inclusive_only_after_five_minutes() {
        let observed_at = source_time("2099-01-01T00:05:00Z").unwrap();
        let mut store = Store::default();
        let lookup = StatusLookup {
            reported: HashSet::new(),
            complete: true,
            observed_at,
            valid_until: observed_at + 3_600,
        };
        let at_boundary = StatusCheck {
            character_name: "Pilot".into(),
            character_id: 1,
            role: KillmailRole::Kill,
            candidates: vec![StatusCandidate {
                id: 42,
                time: "2099-01-01T00:00:00Z".into(),
            }],
        };
        let inside_boundary = StatusCheck {
            candidates: vec![StatusCandidate {
                id: 43,
                time: "2099-01-01T00:00:01Z".into(),
            }],
            ..at_boundary.clone()
        };

        apply_lookup(&mut store, &at_boundary, &lookup);
        apply_lookup(&mut store, &inside_boundary, &lookup);

        assert_eq!(
            report_state(&store, 42, observed_at),
            ReportState::Unreported
        );
        assert_eq!(report_state(&store, 43, observed_at), ReportState::Unknown);
    }
}
