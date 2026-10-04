use crate::models::{Killmail, Store, ZkillStatus};
use serde::Serialize;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
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
#[cfg(any(feature = "gui", test))]
pub(crate) struct PostingSummary {
    pub eligible_for_bulk_posting: usize,
    pub protected: usize,
    pub awaiting_status: usize,
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
        if let Some(character) = store
            .characters
            .iter()
            .find(|character| character.corporation_id == Some(corporation_id))
        {
            reasons.push(ProtectionReason::AuthenticatedCorporation(
                character
                    .corporation_name
                    .clone()
                    .unwrap_or_else(|| format!("Corporation {corporation_id}")),
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

pub(crate) fn is_reported(zkill_status: &HashMap<u64, ZkillStatus>, killmail_id: u64) -> bool {
    zkill_status.get(&killmail_id) == Some(&ZkillStatus::Reported)
}

pub(crate) fn remove_reported_killmails(
    zkill_status: &HashMap<u64, ZkillStatus>,
    killmails: &mut Vec<Killmail>,
) -> usize {
    let previous_len = killmails.len();
    killmails.retain(|mail| !is_reported(zkill_status, mail.id));
    previous_len - killmails.len()
}

pub(crate) fn remove_reported_killmail_flags(
    zkill_status: &HashMap<u64, ZkillStatus>,
    protected_killmail_ids: &mut Vec<u64>,
) -> usize {
    let previous_len = protected_killmail_ids.len();
    protected_killmail_ids.retain(|id| !is_reported(zkill_status, *id));
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

pub(crate) fn report_state(store: &Store, killmail_id: u64, now: u64) -> ReportState {
    match store.zkill_status.get(&killmail_id) {
        Some(ZkillStatus::Reported) => ReportState::Reported,
        Some(ZkillStatus::Unreported { valid_until }) if *valid_until > now => {
            ReportState::Unreported
        }
        _ => ReportState::Unknown,
    }
}

pub(crate) fn is_bulk_candidate(store: &Store, mail: &Killmail, now: u64) -> bool {
    is_eligible_for_bulk_posting(store, mail)
        && report_state(store, mail.id, now) == ReportState::Unreported
}

#[cfg(any(feature = "gui", test))]
pub(crate) fn posting_summary(store: &Store, killmails: &[Killmail], now: u64) -> PostingSummary {
    let mut summary = PostingSummary {
        eligible_for_bulk_posting: 0,
        protected: 0,
        awaiting_status: 0,
    };
    for mail in killmails {
        match report_state(store, mail.id, now) {
            ReportState::Reported => {}
            _ if !is_eligible_for_bulk_posting(store, mail) => summary.protected += 1,
            ReportState::Unreported => summary.eligible_for_bulk_posting += 1,
            ReportState::Unknown => summary.awaiting_status += 1,
        }
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Character, CharacterSource, ProtectedVictim};

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

    fn cache_unreported(store: &mut Store, id: u64, valid_until: u64) {
        store
            .zkill_status
            .insert(id, ZkillStatus::Unreported { valid_until });
    }

    fn cache_reported(store: &mut Store, id: u64) {
        store.zkill_status.insert(id, ZkillStatus::Reported);
    }

    #[test]
    fn report_state_distinguishes_fresh_stale_and_reported_entries() {
        let mut store = store();
        cache_unreported(&mut store, 1, 1_000);
        cache_reported(&mut store, 2);

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
        cache_unreported(&mut store, 10, 1_000);
        cache_reported(&mut store, 14);
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
            }
        );
    }

    #[test]
    fn bulk_candidates_exclude_reported_unknown_and_authenticated_losses() {
        let mut store = store();
        cache_unreported(&mut store, 10, 1_000);
        cache_reported(&mut store, 11);
        cache_unreported(&mut store, 13, 1_000);
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
        cache_unreported(&mut store, 10, 1_000);
        let protected = mail(10, &[1], Some(2));

        assert!(!is_bulk_candidate(&store, &protected, 100));
        assert_eq!(report_state(&store, 10, 100), ReportState::Unreported);

        store.manually_protected_killmail_ids.push(11);
        let protected_by_flag = mail(11, &[1], None);
        cache_unreported(&mut store, 11, 1_000);

        assert!(!is_bulk_candidate(&store, &protected_by_flag, 100));
        assert_eq!(report_state(&store, 11, 100), ReportState::Unreported);
    }

    #[test]
    fn killmail_visibility_respects_reported_and_protected_preferences() {
        let mut store = store();
        store.manually_protected_characters.push(ProtectedVictim {
            id: 2,
            name: "Protected Pilot".into(),
        });
        for id in [10, 11] {
            cache_unreported(&mut store, id, 0);
        }
        cache_reported(&mut store, 12);
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
        cache_unreported(&mut store, 10, 0);
        let protected = mail(10, &[1], None);

        assert!(!is_killmail_visible(&store, &protected, 100));
        assert_eq!(
            posting_summary(&store, std::slice::from_ref(&protected), 100),
            PostingSummary {
                eligible_for_bulk_posting: 0,
                protected: 1,
                awaiting_status: 0,
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
            cache_unreported(&mut store, id, 0);
        }
        for id in [10, 13] {
            cache_reported(&mut store, id);
        }
        let mut killmails = vec![
            mail(10, &[1], None),
            mail(11, &[1], None),
            mail(12, &[1], None),
            mail(13, &[1], None),
            mail(14, &[1], None),
        ];

        let removed = remove_reported_killmails(&store.zkill_status, &mut killmails);
        let ids = killmails.iter().map(|mail| mail.id).collect::<Vec<_>>();

        assert_eq!(removed, 2);
        assert_eq!(ids, vec![11, 12, 14]);
    }

    #[test]
    fn protection_flags_are_removed_only_for_reported_killmails() {
        let mut store = store();
        cache_reported(&mut store, 10);
        cache_unreported(&mut store, 11, 0);
        let mut protected_ids = vec![10, 11, 12];

        assert_eq!(
            remove_reported_killmail_flags(&store.zkill_status, &mut protected_ids),
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
            cache_unreported(&mut store, id, 0);
        }
        let mut killmails = vec![
            mail(10, &[1], None),
            mail(11, &[1, 2], None),
            mail(12, &[1], None),
            mail(13, &[1], None),
        ];
        cache_reported(&mut store, 12);
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

    #[test]
    fn authenticated_corporation_id_is_protected_without_a_resolved_name() {
        let mut store = store();
        store.characters[0].corporation_name = None;
        let mut mail = mail(10, &[1], Some(9));
        mail.victim_corporation_id = Some(100);

        assert!(!is_eligible_for_bulk_posting(&store, &mail));
        assert_eq!(
            protection_reasons(&store, &mail),
            vec![ProtectionReason::AuthenticatedCorporation(
                "Corporation 100".into()
            )]
        );
    }
}
