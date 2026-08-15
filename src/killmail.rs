use crate::models::{Killmail, Store, ZkillCacheEntry};
use std::collections::{BTreeMap, HashMap};

const NEGATIVE_CACHE_TTL_SECS: u64 = 15 * 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReportState {
    Reported,
    Unreported,
    Unknown,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct PostingSummary {
    pub eligible_for_bulk_posting: usize,
    pub protected: usize,
    pub awaiting_status: usize,
    pub protection_reasons: Vec<(String, usize)>,
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
    has_authenticated_source(store, mail)
        && report_state(store, mail.id, now) != ReportState::Reported
        && (store.show_protected_killmails || is_eligible_for_bulk_posting(store, mail))
}

fn has_authenticated_source(store: &Store, mail: &Killmail) -> bool {
    mail.sources.iter().any(|source| {
        store
            .characters
            .iter()
            .any(|character| character.id == source.id)
    })
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

pub(crate) fn remove_killmails_for_removed_character(
    store: &Store,
    killmails: &mut Vec<Killmail>,
    character_id: u64,
) -> usize {
    let previous_len = killmails.len();
    killmails.retain_mut(|mail| {
        let sourced_by_removed_character =
            mail.sources.iter().any(|source| source.id == character_id);
        let still_sourced_by_authenticated_character = mail.sources.iter().any(|source| {
            store
                .characters
                .iter()
                .any(|character| character.id == source.id)
        });
        if sourced_by_removed_character && still_sourced_by_authenticated_character {
            mail.sources.retain(|source| {
                store
                    .characters
                    .iter()
                    .any(|character| character.id == source.id)
            });
        }
        !sourced_by_removed_character || still_sourced_by_authenticated_character
    });
    previous_len - killmails.len()
}

pub(crate) fn remove_killmails_without_authenticated_sources(
    store: &Store,
    killmails: &mut Vec<Killmail>,
) -> usize {
    let previous_len = killmails.len();
    killmails.retain(|mail| has_authenticated_source(store, mail));
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

pub(crate) fn posting_summary(store: &Store, killmails: &[Killmail], now: u64) -> PostingSummary {
    let mut summary = PostingSummary {
        eligible_for_bulk_posting: 0,
        protected: 0,
        awaiting_status: 0,
        protection_reasons: Vec::new(),
    };
    let mut protection_reasons = BTreeMap::new();

    for mail in killmails {
        if report_state(store, mail.id, now) == ReportState::Reported {
            continue;
        }

        let reasons = protected_victim_reasons(store, mail);
        if reasons.is_empty() {
            match report_state(store, mail.id, now) {
                ReportState::Unreported => summary.eligible_for_bulk_posting += 1,
                ReportState::Unknown => summary.awaiting_status += 1,
                ReportState::Reported => {}
            }
        } else {
            summary.protected += 1;
            for reason in reasons {
                *protection_reasons.entry(reason).or_default() += 1;
            }
        }
    }

    summary.protection_reasons = protection_reasons.into_iter().collect();
    summary
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
    fn posting_summary_explains_bulk_eligibility_and_protection() {
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
        store.zkill_cache.insert(
            14,
            ZkillCacheEntry {
                reported: true,
                checked_at: 100,
            },
        );
        let mut protected_corporation = mail(13, &[1], None);
        protected_corporation.victim_corporation_id = Some(100);
        let killmails = vec![
            mail(10, &[1], None),
            mail(11, &[1], None),
            mail(12, &[1], Some(2)),
            protected_corporation,
            mail(14, &[1], Some(2)),
        ];

        assert_eq!(
            posting_summary(&store, &killmails, 100),
            PostingSummary {
                eligible_for_bulk_posting: 1,
                protected: 2,
                awaiting_status: 1,
                protection_reasons: vec![
                    ("authenticated corporation Pilot Corp".into(), 1),
                    ("character Protected Pilot".into(), 1),
                ],
            }
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
    fn removing_a_character_removes_all_of_its_unshared_killmails() {
        let mut store = store();
        store.characters.push(Character {
            id: 2,
            name: "Pilot 2".into(),
            refresh_token: None,
            corporation_id: None,
            corporation_name: None,
        });
        for id in [10, 11, 12] {
            store.zkill_cache.insert(
                id,
                ZkillCacheEntry {
                    reported: false,
                    checked_at: 100,
                },
            );
        }
        let mut killmails = vec![
            mail(10, &[1], None),
            mail(11, &[1, 2], None),
            mail(12, &[1], None),
            mail(13, &[1], None),
        ];
        store.zkill_cache.insert(
            12,
            ZkillCacheEntry {
                reported: true,
                checked_at: 100,
            },
        );
        store.characters.retain(|character| character.id != 1);

        let removed = remove_killmails_for_removed_character(&store, &mut killmails, 1);

        assert_eq!(removed, 3);
        assert_eq!(
            killmails.iter().map(|mail| mail.id).collect::<Vec<_>>(),
            vec![11]
        );
        assert_eq!(
            killmails[0].sources,
            vec![CharacterSource {
                id: 2,
                name: "Pilot 2".into(),
            }]
        );
    }

    #[test]
    fn killmails_without_authenticated_sources_are_removed() {
        let store = Store::default();
        let mut killmails = vec![mail(10, &[1], None)];

        assert_eq!(
            remove_killmails_without_authenticated_sources(&store, &mut killmails),
            1
        );
        assert!(killmails.is_empty());
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
