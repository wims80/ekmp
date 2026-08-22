use crate::models::{Killmail, Store, ZkillCacheEntry};
use std::collections::{BTreeMap, HashMap};

const NEGATIVE_CACHE_TTL_SECS: u64 = 15 * 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReportState {
    Reported,
    Unreported,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ProtectionReason {
    AuthenticatedCharacter(String),
    AuthenticatedCorporation(String),
    ManuallyProtectedCharacter(String),
    ManuallyProtectedCorporation(String),
    ManuallyProtectedKillmail,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct PostingSummary {
    pub eligible_for_bulk_posting: usize,
    pub protected: usize,
    pub awaiting_status: usize,
    pub protection_reasons: Vec<(ProtectionReason, usize)>,
}

pub(crate) fn protection_reasons(store: &Store, mail: &Killmail) -> Vec<ProtectionReason> {
    let mut reasons = Vec::new();
    if store.manually_protected_killmail_ids.contains(&mail.id) {
        reasons.push(ProtectionReason::ManuallyProtectedKillmail);
    }
    if let Some(victim_id) = mail.victim_id {
        if let Some(character) = store
            .characters
            .iter()
            .find(|character| character.id == victim_id)
        {
            reasons.push(ProtectionReason::AuthenticatedCharacter(
                character.name.clone(),
            ));
        }
        if let Some(character) = store
            .manually_protected_characters
            .iter()
            .find(|character| character.id == victim_id)
        {
            reasons.push(ProtectionReason::ManuallyProtectedCharacter(
                character.name.clone(),
            ));
        }
    }
    if let Some(corporation_id) = mail.victim_corporation_id {
        if let Some(character) = store.characters.iter().find(|character| {
            character.corporation_id == Some(corporation_id) && character.corporation_name.is_some()
        }) {
            reasons.push(ProtectionReason::AuthenticatedCorporation(
                character.corporation_name.clone().unwrap_or_default(),
            ));
        }
        if let Some(corporation) = store
            .manually_protected_corporations
            .iter()
            .find(|corporation| corporation.id == corporation_id)
        {
            reasons.push(ProtectionReason::ManuallyProtectedCorporation(
                corporation.name.clone(),
            ));
        }
    }
    reasons
}

pub(crate) fn is_eligible_for_bulk_posting(store: &Store, mail: &Killmail) -> bool {
    protection_reasons(store, mail).is_empty()
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

pub(crate) fn remove_reported_killmail_flags(
    zkill_cache: &HashMap<u64, ZkillCacheEntry>,
    protected_killmail_ids: &mut Vec<u64>,
) -> usize {
    let previous_len = protected_killmail_ids.len();
    protected_killmail_ids.retain(|id| !zkill_cache.get(id).is_some_and(|entry| entry.reported));
    previous_len - protected_killmail_ids.len()
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
    let mut protection_reason_counts = BTreeMap::new();

    for mail in killmails {
        if report_state(store, mail.id, now) == ReportState::Reported {
            continue;
        }

        let reasons = protection_reasons(store, mail);
        if reasons.is_empty() {
            match report_state(store, mail.id, now) {
                ReportState::Unreported => summary.eligible_for_bulk_posting += 1,
                ReportState::Unknown => summary.awaiting_status += 1,
                ReportState::Reported => {}
            }
        } else {
            summary.protected += 1;
            for reason in reasons {
                *protection_reason_counts.entry(reason).or_default() += 1;
            }
        }
    }

    summary.protection_reasons = protection_reason_counts.into_iter().collect();
    summary
}

pub(crate) fn bulk_submission_candidates(
    store: &Store,
    mut mails: Vec<Killmail>,
    now: u64,
) -> Vec<Killmail> {
    mails.retain(|mail| is_bulk_candidate(store, mail, now));
    mails
}

pub(crate) fn individual_submission_candidate(
    store: &Store,
    mail: Killmail,
    now: u64,
) -> Option<Killmail> {
    (report_state(store, mail.id, now) == ReportState::Unreported).then_some(mail)
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
            estimated_value_isk: None,
            detail: None,
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
                    (
                        ProtectionReason::AuthenticatedCorporation("Pilot Corp".into()),
                        1,
                    ),
                    (
                        ProtectionReason::ManuallyProtectedCharacter("Protected Pilot".into()),
                        1,
                    ),
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

        assert!(bulk_submission_candidates(&store, vec![protected.clone()], 100).is_empty());
        assert_eq!(
            individual_submission_candidate(&store, protected, 100)
                .unwrap()
                .id,
            10
        );

        store.manually_protected_killmail_ids.push(11);
        let protected_by_flag = mail(11, &[1], None);
        store.zkill_cache.insert(
            11,
            ZkillCacheEntry {
                reported: false,
                checked_at: 100,
            },
        );

        assert!(
            bulk_submission_candidates(&store, vec![protected_by_flag.clone()], 100).is_empty()
        );
        assert_eq!(
            individual_submission_candidate(&store, protected_by_flag, 100)
                .unwrap()
                .id,
            11
        );
    }

    #[test]
    fn individual_submission_requires_a_confirmed_unreported_status() {
        let mut store = store();
        store.zkill_cache.insert(
            10,
            ZkillCacheEntry {
                reported: false,
                checked_at: 100,
            },
        );
        store.zkill_cache.insert(
            11,
            ZkillCacheEntry {
                reported: true,
                checked_at: 100,
            },
        );

        assert!(individual_submission_candidate(&store, mail(10, &[1], None), 100).is_some());
        assert!(individual_submission_candidate(&store, mail(11, &[1], None), 100).is_none());
        assert!(individual_submission_candidate(&store, mail(12, &[1], None), 100).is_none());
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
    fn manually_protected_killmail_is_hidden_and_summarized_as_protected() {
        let mut store = store();
        store.manually_protected_killmail_ids.push(10);
        store.zkill_cache.insert(
            10,
            ZkillCacheEntry {
                reported: false,
                checked_at: 100,
            },
        );
        let protected = mail(10, &[1], None);

        assert!(!is_killmail_visible(&store, &protected, 100));
        assert_eq!(
            posting_summary(&store, std::slice::from_ref(&protected), 100),
            PostingSummary {
                eligible_for_bulk_posting: 0,
                protected: 1,
                awaiting_status: 0,
                protection_reasons: vec![(ProtectionReason::ManuallyProtectedKillmail, 1)],
            }
        );

        store.show_protected_killmails = true;
        assert!(is_killmail_visible(&store, &protected, 100));
    }

    #[test]
    fn removing_a_killmail_flag_does_not_override_protected_victim_policy() {
        let mut store = store();
        store.manually_protected_characters.push(ProtectedVictim {
            id: 2,
            name: "Protected Pilot".into(),
        });
        store.manually_protected_killmail_ids.push(10);
        let protected = mail(10, &[1], Some(2));

        assert_eq!(protection_reasons(&store, &protected).len(), 2);

        store.manually_protected_killmail_ids.clear();

        assert_eq!(
            protection_reasons(&store, &protected),
            vec![ProtectionReason::ManuallyProtectedCharacter(
                "Protected Pilot".into()
            )]
        );
        assert!(!is_eligible_for_bulk_posting(&store, &protected));
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
    fn protection_flags_are_removed_only_for_reported_killmails() {
        let mut store = store();
        store.zkill_cache.insert(
            10,
            ZkillCacheEntry {
                reported: true,
                checked_at: 100,
            },
        );
        store.zkill_cache.insert(
            11,
            ZkillCacheEntry {
                reported: false,
                checked_at: 100,
            },
        );
        let mut protected_ids = vec![10, 11, 12];

        assert_eq!(
            remove_reported_killmail_flags(&store.zkill_cache, &mut protected_ids),
            1
        );
        assert_eq!(protected_ids, vec![11, 12]);
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
