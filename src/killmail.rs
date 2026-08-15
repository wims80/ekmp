use crate::models::{Killmail, Store, ZkillCacheEntry};
use std::collections::HashMap;

const NEGATIVE_CACHE_TTL_SECS: u64 = 15 * 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReportState {
    Reported,
    Unreported,
    Unknown,
}

pub(crate) fn protected_victim_reasons(store: &Store, mail: &Killmail) -> Vec<String> {
    let mut reasons = Vec::new();
    if let Some(victim_id) = mail.victim_id {
        if let Some(character) = store
            .characters
            .iter()
            .find(|character| character.id == victim_id)
        {
            reasons.push(format!("authenticated character {}", character.name));
        }
        if let Some(character) = store
            .manually_protected_characters
            .iter()
            .find(|character| character.id == victim_id)
        {
            reasons.push(format!("character {}", character.name));
        }
    }
    if let Some(corporation_id) = mail.victim_corporation_id {
        if let Some(character) = store.characters.iter().find(|character| {
            character.corporation_id == Some(corporation_id) && character.corporation_name.is_some()
        }) {
            reasons.push(format!(
                "authenticated corporation {}",
                character.corporation_name.as_deref().unwrap_or_default()
            ));
        }
        if let Some(corporation) = store
            .manually_protected_corporations
            .iter()
            .find(|corporation| corporation.id == corporation_id)
        {
            reasons.push(format!("corporation {}", corporation.name));
        }
    }
    reasons
}

pub(crate) fn is_eligible_for_bulk_posting(store: &Store, mail: &Killmail) -> bool {
    protected_victim_reasons(store, mail).is_empty()
}

fn is_killmail_visible(store: &Store, mail: &Killmail, now: u64) -> bool {
    report_state(store, mail.id, now) != ReportState::Reported
        && (store.show_protected_killmails || is_eligible_for_bulk_posting(store, mail))
}

pub(crate) fn displayed_killmails<'a>(
    store: &Store,
    killmails: &'a [Killmail],
    now: u64,
) -> Vec<&'a Killmail> {
    killmails
        .iter()
        .filter(|mail| is_killmail_visible(store, mail, now))
        .collect()
}

pub(crate) fn remove_reported_killmails(
    zkill_cache: &HashMap<u64, ZkillCacheEntry>,
    killmails: &mut Vec<Killmail>,
) -> usize {
    let previous_len = killmails.len();
    killmails.retain(|mail| {
        !zkill_cache
            .get(&mail.id)
            .is_some_and(|entry| entry.reported)
    });
    previous_len - killmails.len()
}

pub(crate) fn report_state(store: &Store, killmail_id: u64, now: u64) -> ReportState {
    match store.zkill_cache.get(&killmail_id).copied() {
        Some(entry) if entry.reported => ReportState::Reported,
        Some(entry) if entry.is_fresh(now, NEGATIVE_CACHE_TTL_SECS) => ReportState::Unreported,
        _ => ReportState::Unknown,
    }
}

pub(crate) fn is_bulk_candidate(store: &Store, mail: &Killmail, now: u64) -> bool {
    is_eligible_for_bulk_posting(store, mail)
        && report_state(store, mail.id, now) == ReportState::Unreported
}

pub(crate) fn submission_candidates(
    store: &Store,
    mut mails: Vec<Killmail>,
    bulk: bool,
    now: u64,
) -> Vec<Killmail> {
    if bulk {
        mails.retain(|mail| is_bulk_candidate(store, mail, now));
    }
    mails
}

pub(crate) fn character_summaries(store: &Store, killmails: &[Killmail], now: u64) -> Vec<String> {
    store
        .characters
        .iter()
        .map(|character| {
            let eligible = killmails
                .iter()
                .filter(|mail| {
                    is_eligible_for_bulk_posting(store, mail)
                        && mail
                            .sources
                            .iter()
                            .any(|source| source.id == character.id)
                })
                .collect::<Vec<_>>();
            if eligible.is_empty() {
                return format!(
                    "{} has no recent killmails eligible for bulk posting",
                    character.name
                );
            }
            let unknown = eligible
                .iter()
                .filter(|mail| report_state(store, mail.id, now) == ReportState::Unknown)
                .count();
            if unknown > 0 {
                return format!(
                    "Could not determine zKillboard status for {unknown} of {} eligible recent killmails for {}",
                    eligible.len(),
                    character.name
                );
            }
            let unreported = eligible
                .iter()
                .filter(|mail| report_state(store, mail.id, now) == ReportState::Unreported)
                .count();
            if unreported == 0 {
                format!(
                    "All {} eligible recent killmails for {} are reported to zKillboard",
                    eligible.len(),
                    character.name
                )
            } else {
                format!(
                    "{} has {unreported} of {} eligible recent killmails still unreported",
                    character.name,
                    eligible.len()
                )
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Character, CharacterSource, ProtectedVictim, ZkillCacheEntry};

    fn mail(id: u64, source_ids: &[u64], victim_id: Option<u64>) -> Killmail {
        Killmail {
            id,
            hash: "hash".into(),
            sources: source_ids
                .iter()
                .map(|id| CharacterSource {
                    id: *id,
                    name: format!("Pilot {id}"),
                })
                .collect(),
            victim_id,
            victim_corporation_id: None,
            victim: "Victim".into(),
            ship: "Ship".into(),
            time: "Time".into(),
        }
    }

    fn store() -> Store {
        Store {
            characters: vec![Character {
                id: 1,
                name: "Pilot 1".into(),
                refresh_token: None,
                corporation_id: Some(100),
                corporation_name: Some("Pilot Corp".into()),
            }],
            ..Store::default()
        }
    }

    #[test]
    fn report_state_distinguishes_fresh_stale_and_reported_entries() {
        let mut store = store();
        store.zkill_cache.insert(
            1,
            ZkillCacheEntry {
                reported: false,
                checked_at: 100,
            },
        );
        store.zkill_cache.insert(
            2,
            ZkillCacheEntry {
                reported: true,
                checked_at: 0,
            },
        );

        assert_eq!(report_state(&store, 1, 999), ReportState::Unreported);
        assert_eq!(report_state(&store, 1, 1_000), ReportState::Unknown);
        assert_eq!(report_state(&store, 2, u64::MAX), ReportState::Reported);
        assert_eq!(report_state(&store, 3, 100), ReportState::Unknown);
    }

    #[test]
    fn summaries_exclude_authenticated_character_losses() {
        let mut store = store();
        store.zkill_cache.insert(
            10,
            ZkillCacheEntry {
                reported: true,
                checked_at: 0,
            },
        );
        let killmails = vec![mail(10, &[1], None), mail(11, &[1], Some(1))];

        assert_eq!(
            character_summaries(&store, &killmails, 1),
            vec!["All 1 eligible recent killmails for Pilot 1 are reported to zKillboard"]
        );
    }

    #[test]
    fn summaries_report_partial_and_unknown_states() {
        let mut store = store();
        store.zkill_cache.insert(
            10,
            ZkillCacheEntry {
                reported: false,
                checked_at: 100,
            },
        );
        let killmails = vec![mail(10, &[1], None), mail(11, &[1], None)];

        assert!(character_summaries(&store, &killmails, 100)[0]
            .starts_with("Could not determine zKillboard status for 1 of 2"));
        store.zkill_cache.insert(
            11,
            ZkillCacheEntry {
                reported: true,
                checked_at: 100,
            },
        );
        assert_eq!(
            character_summaries(&store, &killmails, 100),
            vec!["Pilot 1 has 1 of 2 eligible recent killmails still unreported"]
        );
    }

    #[test]
    fn bulk_candidates_exclude_reported_unknown_and_authenticated_losses() {
        let mut store = store();
        for (id, reported) in [(10, false), (11, true), (13, false)] {
            store.zkill_cache.insert(
                id,
                ZkillCacheEntry {
                    reported,
                    checked_at: 100,
                },
            );
        }
        let killmails = [
            mail(10, &[1], None),
            mail(11, &[1], None),
            mail(12, &[1], None),
            mail(13, &[1], Some(1)),
        ];

        let candidates = killmails
            .iter()
            .filter(|mail| is_bulk_candidate(&store, mail, 100))
            .map(|mail| mail.id)
            .collect::<Vec<_>>();

        assert_eq!(candidates, vec![10]);
    }

    #[test]
    fn protected_killmails_require_individual_submission() {
        let mut store = store();
        store.manually_protected_characters.push(ProtectedVictim {
            id: 2,
            name: "Protected Pilot".into(),
        });
        store.zkill_cache.insert(
            10,
            ZkillCacheEntry {
                reported: false,
                checked_at: 100,
            },
        );
        let protected = mail(10, &[1], Some(2));

        assert!(submission_candidates(&store, vec![protected.clone()], true, 100).is_empty());
        assert_eq!(
            submission_candidates(&store, vec![protected], false, 100)[0].id,
            10
        );
    }

    #[test]
    fn killmail_visibility_respects_reported_and_protected_preferences() {
        let mut store = store();
        store.manually_protected_characters.push(ProtectedVictim {
            id: 2,
            name: "Protected Pilot".into(),
        });
        for id in [10, 11] {
            store.zkill_cache.insert(
                id,
                ZkillCacheEntry {
                    reported: false,
                    checked_at: 100,
                },
            );
        }
        store.zkill_cache.insert(
            12,
            ZkillCacheEntry {
                reported: true,
                checked_at: 100,
            },
        );
        let visible = mail(10, &[1], None);
        let protected = mail(11, &[1], Some(2));
        let reported = mail(12, &[1], None);

        assert!(is_killmail_visible(&store, &visible, 100));
        assert!(!is_killmail_visible(&store, &protected, 100));
        assert!(!is_killmail_visible(&store, &reported, 100));

        store.show_protected_killmails = true;
        assert!(is_killmail_visible(&store, &protected, 100));
        assert!(!is_killmail_visible(&store, &reported, 100));

        assert!(!is_killmail_visible(&store, &reported, 100));
    }

    #[test]
    fn reported_killmails_are_removed_from_cached_snapshots() {
        let mut store = store();
        for id in [12, 14] {
            store.zkill_cache.insert(
                id,
                ZkillCacheEntry {
                    reported: false,
                    checked_at: 100,
                },
            );
        }
        for id in [10, 13] {
            store.zkill_cache.insert(
                id,
                ZkillCacheEntry {
                    reported: true,
                    checked_at: 100,
                },
            );
        }
        let mut killmails = vec![
            mail(10, &[1], None),
            mail(11, &[1], None),
            mail(12, &[1], None),
            mail(13, &[1], None),
            mail(14, &[1], None),
        ];

        let removed = remove_reported_killmails(&store.zkill_cache, &mut killmails);
        let ids = killmails.iter().map(|mail| mail.id).collect::<Vec<_>>();

        assert_eq!(removed, 2);
        assert_eq!(ids, vec![11, 12, 14]);
    }

    #[test]
    fn automatically_and_manually_protected_victims_match_characters_and_corporations() {
        let mut store = store();
        store.manually_protected_characters.push(ProtectedVictim {
            id: 2,
            name: "Protected Pilot".into(),
        });
        store.manually_protected_corporations.push(ProtectedVictim {
            id: 200,
            name: "Protected Corp".into(),
        });
        let authenticated_character = mail(1, &[1], Some(1));
        let mut authenticated_corporation = mail(2, &[1], Some(9));
        authenticated_corporation.victim_corporation_id = Some(100);
        let manually_protected_character = mail(3, &[1], Some(2));
        let mut manually_protected_corporation = mail(4, &[1], Some(9));
        manually_protected_corporation.victim_corporation_id = Some(200);
        let unrelated = mail(5, &[1], Some(9));

        assert!(!is_eligible_for_bulk_posting(
            &store,
            &authenticated_character
        ));
        assert!(!is_eligible_for_bulk_posting(
            &store,
            &authenticated_corporation
        ));
        assert!(!is_eligible_for_bulk_posting(
            &store,
            &manually_protected_character
        ));
        assert!(!is_eligible_for_bulk_posting(
            &store,
            &manually_protected_corporation
        ));
        assert!(is_eligible_for_bulk_posting(&store, &unrelated));
    }
}
