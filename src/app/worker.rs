use crate::{
    integrations::{backend::Backend, images, zkill},
    killmail::{report_state, ReportState},
    models::{Character, Killmail, ProtectedVictim, ProtectedVictimKind, Store},
    persistence::image_cache,
};
use std::{
    collections::HashSet,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
        Arc,
    },
    thread,
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_ZKILL_STATUS_PAGES: usize = 3;

pub(super) use images::IdentityImageKey;

pub(super) struct DecodedIdentityImage {
    pub key: IdentityImageKey,
    pub size: [usize; 2],
    pub rgba: Vec<u8>,
}

pub(super) enum IdentityImageEvent {
    Loaded(DecodedIdentityImage),
    Failed(IdentityImageKey),
}

pub(super) fn start_identity_image_worker(
    load_images: bool,
) -> (Sender<IdentityImageKey>, Receiver<IdentityImageEvent>) {
    let (request_tx, request_rx) = mpsc::channel();
    let (event_tx, event_rx) = mpsc::channel();
    thread::spawn(move || {
        for key in request_rx {
            let event = if load_images {
                match load_identity_image(key) {
                    Ok(image) => IdentityImageEvent::Loaded(image),
                    Err(()) => IdentityImageEvent::Failed(key),
                }
            } else {
                IdentityImageEvent::Failed(key)
            };
            if event_tx.send(event).is_err() {
                break;
            }
        }
    });
    (request_tx, event_rx)
}

fn load_identity_image(key: IdentityImageKey) -> Result<DecodedIdentityImage, ()> {
    let cached = image_cache::load(key).ok().flatten();
    if let Some(cached) = &cached {
        if cached.fresh {
            if let Ok(image) = decode_identity_image(key, &cached.bytes) {
                return Ok(image);
            }
        }
    }

    match images::fetch(key) {
        Ok(bytes) => {
            let image = decode_identity_image(key, &bytes)?;
            let _ = image_cache::store(key, &bytes);
            Ok(image)
        }
        Err(_) => cached
            .and_then(|cached| decode_identity_image(key, &cached.bytes).ok())
            .ok_or(()),
    }
}

fn decode_identity_image(key: IdentityImageKey, bytes: &[u8]) -> Result<DecodedIdentityImage, ()> {
    let decoded = image::load_from_memory(bytes).map_err(|_| ())?.into_rgba8();
    let size = [decoded.width() as usize, decoded.height() as usize];
    Ok(DecodedIdentityImage {
        key,
        size,
        rgba: decoded.into_raw(),
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KillmailRole {
    Kill,
    Loss,
}

#[derive(Debug, PartialEq, Eq)]
struct ZkillStatusCheck {
    character_name: String,
    character_id: u64,
    candidates: Vec<StatusCandidate>,
    role: KillmailRole,
}

#[derive(Debug, PartialEq, Eq)]
struct StatusCandidate {
    id: u64,
    time: String,
}

pub(super) enum WorkerEvent {
    Status(String),
    Character(Character),
    CharactersRefreshed(Vec<Character>),
    RefreshTokensMigrated(Vec<Character>),
    CharacterRemoved {
        id: u64,
        name: String,
        credential_error: Option<String>,
    },
    ProtectedVictimResolved {
        kind: ProtectedVictimKind,
        victim: ProtectedVictim,
    },
    KillmailsLoaded(Vec<Killmail>),
    LookupComplete {
        character_name: String,
        character_id: u64,
        checked_ids: Vec<u64>,
        reported_ids: HashSet<u64>,
        checked_at: u64,
    },
    LookupFailed {
        character_name: String,
        character_id: u64,
        error: String,
    },
    LookupIncomplete {
        character_name: String,
        character_id: u64,
    },
    PostComplete {
        killmail_id: u64,
        result: Result<zkill::PostOutcome, String>,
    },
    Finished,
    Failed(String),
}

pub(super) fn authenticate(
    backend: Arc<dyn Backend>,
    tx: Sender<WorkerEvent>,
    cancelled: Arc<AtomicBool>,
) {
    match backend.authenticate(&cancelled) {
        Ok(mut character) => {
            if cancelled.load(Ordering::Relaxed) {
                return;
            }
            if let Err(error) = backend.refresh_character_affiliation(&mut character) {
                let _ = tx.send(WorkerEvent::Status(format!(
                    "Character authenticated, but corporation lookup failed: {error}"
                )));
            }
            if cancelled.load(Ordering::Relaxed) {
                return;
            }
            save_refresh_token_or_keep_fallback(backend.as_ref(), &mut character, &tx);
            let _ = tx.send(WorkerEvent::Character(character));
            let _ = tx.send(WorkerEvent::Finished);
        }
        Err(error) => {
            let _ = tx.send(WorkerEvent::Failed(error));
        }
    }
}

pub(super) fn migrate_refresh_tokens(
    backend: Arc<dyn Backend>,
    mut characters: Vec<Character>,
    tx: Sender<WorkerEvent>,
) {
    for character in &mut characters {
        if character.refresh_token.is_some() {
            save_refresh_token_or_keep_fallback(backend.as_ref(), character, &tx);
        }
    }
    let _ = tx.send(WorkerEvent::RefreshTokensMigrated(characters));
    let _ = tx.send(WorkerEvent::Finished);
}

pub(super) fn remove_character(
    backend: Arc<dyn Backend>,
    character: Character,
    tx: Sender<WorkerEvent>,
) {
    let credential_error = if character.uses_json_refresh_token_fallback() {
        None
    } else {
        backend.delete_refresh_token(character.id).err()
    };
    let _ = tx.send(WorkerEvent::CharacterRemoved {
        id: character.id,
        name: character.name,
        credential_error,
    });
    let _ = tx.send(WorkerEvent::Finished);
}

fn save_refresh_token_or_keep_fallback(
    backend: &dyn Backend,
    character: &mut Character,
    tx: &Sender<WorkerEvent>,
) {
    let Some(token) = character.refresh_token.as_deref() else {
        return;
    };
    let result = backend.save_refresh_token(character.id, token);
    save_refresh_token_or_keep_fallback_with(character, result, tx);
}

fn save_refresh_token_or_keep_fallback_with(
    character: &mut Character,
    result: Result<(), String>,
    tx: &Sender<WorkerEvent>,
) {
    match result {
        Ok(()) => character.refresh_token = None,
        Err(error) => {
            let _ = tx.send(WorkerEvent::Status(format!(
                "Could not use the system credential store for {}: {error}. Its refresh token is stored in the local JSON configuration.",
                character.name
            )));
        }
    }
}

pub(super) fn resolve_protected_victim(
    backend: Arc<dyn Backend>,
    kind: ProtectedVictimKind,
    query: String,
    tx: Sender<WorkerEvent>,
) {
    let result = backend.resolve_protected_victim(kind, &query);
    match result {
        Ok(victim) => {
            let _ = tx.send(WorkerEvent::ProtectedVictimResolved { kind, victim });
            let _ = tx.send(WorkerEvent::Finished);
        }
        Err(error) => {
            let _ = tx.send(WorkerEvent::Failed(format!(
                "Could not add protected victim: {error}"
            )));
        }
    }
}

pub(super) fn post_killmails(
    backend: Arc<dyn Backend>,
    mails: Vec<Killmail>,
    tx: Sender<WorkerEvent>,
) {
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
        let result = backend.post(mail);
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
            thread::sleep(backend.request_spacing());
        }
    }
    let _ = tx.send(WorkerEvent::Finished);
}

pub(super) fn load_killmails_and_statuses(
    backend: Arc<dyn Backend>,
    mut store: Store,
    tx: Sender<WorkerEvent>,
) {
    for character in &mut store.characters {
        if let Err(error) = backend.refresh_character_affiliation(character) {
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

    let killmails = match backend.load_killmails(&store.characters) {
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

    check_zkill_statuses(backend.as_ref(), &store, &killmails, &tx);
    let _ = tx.send(WorkerEvent::Finished);
}

pub(super) fn check_zkill_statuses(
    backend: &dyn Backend,
    store: &Store,
    killmails: &[Killmail],
    tx: &Sender<WorkerEvent>,
) {
    let checks = zkill_status_checks(store, killmails, unix_time());

    for (index, check) in checks.iter().enumerate() {
        let mail_type = match check.role {
            KillmailRole::Kill => "kills",
            KillmailRole::Loss => "losses",
        };
        if tx
            .send(WorkerEvent::Status(format!(
                "zKillboard · {} ({}) · Checking whether {} recent {mail_type} {} already reported (batch {}/{}). Killmail IDs: {}",
                check.character_name,
                check.character_id,
                check.candidates.len(),
                if check.candidates.len() == 1 { "is" } else { "are" },
                index + 1,
                checks.len(),
                formatted_candidate_ids(&check.candidates),
            )))
            .is_err()
        {
            return;
        }
        match check_zkill_status(backend, check) {
            Ok(Some(reported_ids)) => {
                if tx
                    .send(WorkerEvent::LookupComplete {
                        character_name: check.character_name.clone(),
                        character_id: check.character_id,
                        checked_ids: check
                            .candidates
                            .iter()
                            .map(|candidate| candidate.id)
                            .collect(),
                        reported_ids,
                        checked_at: unix_time(),
                    })
                    .is_err()
                {
                    return;
                }
            }
            Ok(None) => {
                if tx
                    .send(WorkerEvent::LookupIncomplete {
                        character_name: check.character_name.clone(),
                        character_id: check.character_id,
                    })
                    .is_err()
                {
                    return;
                }
            }
            Err(error) => {
                if tx
                    .send(WorkerEvent::LookupFailed {
                        character_name: check.character_name.clone(),
                        character_id: check.character_id,
                        error,
                    })
                    .is_err()
                {
                    return;
                }
            }
        }
        if index + 1 < checks.len() {
            thread::sleep(backend.request_spacing());
        }
    }
}

fn formatted_candidate_ids(candidates: &[StatusCandidate]) -> String {
    candidates
        .iter()
        .map(|candidate| candidate.id.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

fn check_zkill_status(
    backend: &dyn Backend,
    check: &ZkillStatusCheck,
) -> Result<Option<HashSet<u64>>, String> {
    let oldest_candidate_time = check
        .candidates
        .iter()
        .map(|candidate| candidate.time.as_str())
        .min()
        .unwrap_or_default();
    let candidate_ids = check
        .candidates
        .iter()
        .map(|candidate| candidate.id)
        .collect::<HashSet<_>>();
    let mut reported_ids = HashSet::new();

    for page in 1..=MAX_ZKILL_STATUS_PAGES {
        let entries = match check.role {
            KillmailRole::Loss => backend.character_loss_killmail_page(check.character_id, page),
            KillmailRole::Kill => backend.character_killmail_page(check.character_id, page),
        }?;
        reported_ids.extend(
            entries
                .iter()
                .filter(|entry| candidate_ids.contains(&entry.killmail_id))
                .map(|entry| entry.killmail_id),
        );
        if status_page_covers_oldest_candidate(&entries, oldest_candidate_time) {
            return Ok(Some(reported_ids));
        }
        thread::sleep(backend.request_spacing());
    }
    Ok(None)
}

fn status_page_covers_oldest_candidate(
    entries: &[zkill::KillEntry],
    oldest_candidate_time: &str,
) -> bool {
    entries.len() < zkill::KILLMAILS_PER_PAGE
        || entries
            .last()
            .is_some_and(|entry| entry.killmail_time.as_str() <= oldest_candidate_time)
}

fn zkill_status_checks(store: &Store, killmails: &[Killmail], now: u64) -> Vec<ZkillStatusCheck> {
    let mut checks = Vec::new();
    for character in &store.characters {
        let character_mails = killmails
            .iter()
            .filter(|mail| mail.sources.iter().any(|source| source.id == character.id));
        let (losses, kills): (Vec<_>, Vec<_>) =
            character_mails.partition(|mail| mail.victim_id == Some(character.id));
        for (role, candidates) in [(KillmailRole::Kill, kills), (KillmailRole::Loss, losses)] {
            let candidate_ids = candidates
                .into_iter()
                .filter(|mail| report_state(store, mail.id, now) == ReportState::Unknown)
                .map(|mail| StatusCandidate {
                    id: mail.id,
                    time: mail.time.clone(),
                })
                .collect::<Vec<_>>();
            if !candidate_ids.is_empty() {
                checks.push(ZkillStatusCheck {
                    character_name: character.name.clone(),
                    character_id: character.id,
                    candidates: candidate_ids,
                    role,
                });
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
            estimated_value_isk: None,
        }
    }

    #[test]
    fn zkill_status_checks_include_only_unknown_cached_killmails() {
        let mut store = Store {
            characters: vec![Character {
                id: 1,
                name: "Pilot 1".into(),
                refresh_token: None,
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
                ZkillStatusCheck {
                    character_name: "Pilot 1".into(),
                    character_id: 1,
                    candidates: vec![StatusCandidate {
                        id: 12,
                        time: "Time".into(),
                    }],
                    role: KillmailRole::Kill,
                },
                ZkillStatusCheck {
                    character_name: "Pilot 1".into(),
                    character_id: 1,
                    candidates: vec![StatusCandidate {
                        id: 13,
                        time: "Time".into(),
                    }],
                    role: KillmailRole::Loss,
                },
            ]
        );
    }

    #[test]
    fn status_pages_cover_candidates_only_after_reaching_their_oldest_time() {
        let recent_entry = zkill::KillEntry {
            killmail_id: 1,
            killmail_time: "2026-08-16T12:00:00Z".into(),
        };
        let old_entry = zkill::KillEntry {
            killmail_id: 2,
            killmail_time: "2026-05-24T19:39:44Z".into(),
        };
        let full_recent_page = vec![recent_entry.clone(); zkill::KILLMAILS_PER_PAGE];

        assert!(!status_page_covers_oldest_candidate(
            &full_recent_page,
            "2026-05-24T19:39:44Z"
        ));
        assert!(status_page_covers_oldest_candidate(
            &[recent_entry.clone(), old_entry],
            "2026-05-24T19:39:44Z"
        ));
        assert!(status_page_covers_oldest_candidate(
            &[recent_entry],
            "2026-05-24T19:39:44Z"
        ));
    }

    #[test]
    fn successful_token_migration_removes_the_json_fallback() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut character = Character {
            id: 1,
            name: "Pilot".into(),
            refresh_token: Some("token".into()),
            corporation_id: None,
            corporation_name: None,
        };

        save_refresh_token_or_keep_fallback_with(&mut character, Ok(()), &tx);

        assert!(character.refresh_token.is_none());
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn failed_token_migration_keeps_the_json_fallback_and_warns() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut character = Character {
            id: 1,
            name: "Pilot".into(),
            refresh_token: Some("token".into()),
            corporation_id: None,
            corporation_name: None,
        };

        save_refresh_token_or_keep_fallback_with(&mut character, Err("unavailable".into()), &tx);

        assert_eq!(character.refresh_token.as_deref(), Some("token"));
        assert!(
            matches!(rx.recv().unwrap(), WorkerEvent::Status(message) if message.contains("local JSON configuration"))
        );
    }
}
