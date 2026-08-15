use super::ProtectedVictimKind;
use crate::{
    auth, esi,
    killmail::{report_state, ReportState},
    models::{Character, Killmail, ProtectedVictim, Store},
    zkill,
};
use std::{
    collections::HashSet,
    sync::mpsc::Sender,
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const REQUEST_SPACING: Duration = Duration::from_secs(1);

pub(super) enum WorkerEvent {
    Status(String),
    Character(Character),
    CharactersRefreshed(Vec<Character>),
    ProtectedVictimResolved {
        kind: ProtectedVictimKind,
        victim: ProtectedVictim,
    },
    KillmailsLoaded(Vec<Killmail>),
    LookupComplete {
        character_name: String,
        checked_ids: Vec<u64>,
        reported_ids: HashSet<u64>,
        checked_at: u64,
    },
    LookupFailed {
        character_name: String,
        error: String,
    },
    PostComplete {
        killmail_id: u64,
        result: Result<zkill::PostOutcome, String>,
    },
    Finished,
    Failed(String),
}

pub(super) fn authenticate(tx: Sender<WorkerEvent>) {
    match auth::authenticate() {
        Ok(mut character) => {
            if let Err(error) = esi::refresh_character_affiliation(&mut character) {
                let _ = tx.send(WorkerEvent::Status(format!(
                    "Character authenticated, but corporation lookup failed: {error}"
                )));
            }
            let _ = tx.send(WorkerEvent::Character(character));
            let _ = tx.send(WorkerEvent::Finished);
        }
        Err(error) => {
            let _ = tx.send(WorkerEvent::Failed(error));
        }
    }
}

pub(super) fn resolve_protected_victim(
    kind: ProtectedVictimKind,
    id: u64,
    tx: Sender<WorkerEvent>,
) {
    let result = match kind {
        ProtectedVictimKind::Character => esi::resolve_character_name(id),
        ProtectedVictimKind::Corporation => esi::resolve_corporation_name(id),
    };
    match result {
        Ok(name) => {
            let _ = tx.send(WorkerEvent::ProtectedVictimResolved {
                kind,
                victim: ProtectedVictim { id, name },
            });
            let _ = tx.send(WorkerEvent::Finished);
        }
        Err(error) => {
            let _ = tx.send(WorkerEvent::Failed(format!(
                "Could not add protected victim: {error}"
            )));
        }
    }
}

pub(super) fn post_killmails(mails: Vec<Killmail>, tx: Sender<WorkerEvent>) {
    let total = mails.len();
    for (index, mail) in mails.iter().enumerate() {
        if tx
            .send(WorkerEvent::Status(format!(
                "Submitting killmail {} to zKillboard ({}/{total})...",
                mail.id,
                index + 1
            )))
            .is_err()
        {
            return;
        }
        let result = zkill::post(mail);
        if tx
            .send(WorkerEvent::PostComplete {
                killmail_id: mail.id,
                result,
            })
            .is_err()
        {
            return;
        }
        if index + 1 < total {
            thread::sleep(REQUEST_SPACING);
        }
    }
    let _ = tx.send(WorkerEvent::Finished);
}

pub(super) fn load_killmails_and_statuses(mut store: Store, tx: Sender<WorkerEvent>) {
    for character in &mut store.characters {
        if let Err(error) = esi::refresh_character_affiliation(character) {
            let _ = tx.send(WorkerEvent::Status(format!(
                "Could not refresh corporation for {}: {error}",
                character.name
            )));
        }
    }
    if tx
        .send(WorkerEvent::CharactersRefreshed(store.characters.clone()))
        .is_err()
    {
        return;
    }

    let killmails = match esi::load_killmails(&store.characters) {
        Ok(killmails) => killmails,
        Err(error) => {
            let _ = tx.send(WorkerEvent::Failed(format!(
                "Could not load recent killmails: {error}"
            )));
            return;
        }
    };
    if tx
        .send(WorkerEvent::KillmailsLoaded(killmails.clone()))
        .is_err()
    {
        return;
    }

    check_zkill_statuses(&store, &killmails, &tx);
    let _ = tx.send(WorkerEvent::Finished);
}

pub(super) fn check_zkill_statuses(
    store: &Store,
    killmails: &[Killmail],
    tx: &Sender<WorkerEvent>,
) {
    let checks = zkill_status_checks(store, killmails, unix_time());

    for (index, (name, id, candidate_ids, is_loss)) in checks.iter().enumerate() {
        let mail_type = if *is_loss { "losses" } else { "kills" };
        if tx
            .send(WorkerEvent::Status(format!(
                "Checking recent {mail_type} for {name} on zKillboard ({}/{})...",
                index + 1,
                checks.len()
            )))
            .is_err()
        {
            return;
        }
        let result = if *is_loss {
            zkill::character_loss_ids(*id)
        } else {
            zkill::character_kill_ids(*id)
        };
        match result {
            Ok(reported_ids) => {
                if tx
                    .send(WorkerEvent::LookupComplete {
                        character_name: name.clone(),
                        checked_ids: candidate_ids.clone(),
                        reported_ids,
                        checked_at: unix_time(),
                    })
                    .is_err()
                {
                    return;
                }
            }
            Err(error) => {
                if tx
                    .send(WorkerEvent::LookupFailed {
                        character_name: name.clone(),
                        error,
                    })
                    .is_err()
                {
                    return;
                }
            }
        }
        if index + 1 < checks.len() {
            thread::sleep(REQUEST_SPACING);
        }
    }
}

fn zkill_status_checks(
    store: &Store,
    killmails: &[Killmail],
    now: u64,
) -> Vec<(String, u64, Vec<u64>, bool)> {
    let mut checks = Vec::new();
    for character in &store.characters {
        let character_mails = killmails
            .iter()
            .filter(|mail| mail.sources.iter().any(|source| source.id == character.id));
        let (losses, kills): (Vec<_>, Vec<_>) =
            character_mails.partition(|mail| mail.victim_id == Some(character.id));
        for (is_loss, candidates) in [(false, kills), (true, losses)] {
            let candidate_ids = candidates
                .into_iter()
                .filter(|mail| report_state(store, mail.id, now) == ReportState::Unknown)
                .map(|mail| mail.id)
                .collect::<Vec<_>>();
            if !candidate_ids.is_empty() {
                checks.push((character.name.clone(), character.id, candidate_ids, is_loss));
            }
        }
    }
    checks
}

pub(super) fn unix_time() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{CharacterSource, ZkillCacheEntry};

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

    #[test]
    fn zkill_status_checks_include_only_unknown_cached_killmails() {
        let mut store = Store {
            characters: vec![Character {
                id: 1,
                name: "Pilot 1".into(),
                refresh_token: String::new(),
                corporation_id: Some(100),
                corporation_name: Some("Pilot Corp".into()),
            }],
            ..Store::default()
        };
        for (id, reported, checked_at) in [(10, false, 200), (11, true, 0), (13, false, 0)] {
            store.zkill_cache.insert(
                id,
                ZkillCacheEntry {
                    reported,
                    checked_at,
                },
            );
        }
        let killmails = vec![
            mail(10, &[1], None),
            mail(11, &[1], None),
            mail(12, &[1], None),
            mail(13, &[1], Some(1)),
            mail(14, &[2], None),
        ];

        assert_eq!(
            zkill_status_checks(&store, &killmails, 1_000),
            vec![
                ("Pilot 1".into(), 1, vec![12], false),
                ("Pilot 1".into(), 1, vec![13], true),
            ]
        );
    }
}
