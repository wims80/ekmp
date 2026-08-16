use crate::{
    integrations::{auth, esi, zkill},
    models::{Character, Killmail, ProtectedVictim, ProtectedVictimKind},
    persistence::secrets,
};
use std::{collections::HashSet, sync::atomic::AtomicBool, time::Duration};

pub(crate) trait Backend: Send + Sync {
    fn authenticate(&self, cancelled: &AtomicBool) -> Result<Character, String>;
    fn refresh_character_affiliation(&self, character: &mut Character) -> Result<(), String>;
    fn load_killmails(
        &self,
        characters: &[Character],
        cached_killmails: &[Killmail],
        reported_ids: &HashSet<u64>,
    ) -> Result<Vec<Killmail>, String>;
    fn resolve_protected_victim(
        &self,
        kind: ProtectedVictimKind,
        query: &str,
    ) -> Result<ProtectedVictim, String>;
    fn character_killmail_page(
        &self,
        character_id: u64,
        page: usize,
    ) -> Result<Vec<zkill::KillEntry>, String>;
    fn character_loss_killmail_page(
        &self,
        character_id: u64,
        page: usize,
    ) -> Result<Vec<zkill::KillEntry>, String>;
    fn post(&self, mail: &Killmail) -> Result<zkill::PostOutcome, String>;
    fn save_refresh_token(&self, character_id: u64, token: &str) -> Result<(), String>;
    fn delete_refresh_token(&self, character_id: u64) -> Result<(), String>;

    fn request_spacing(&self) -> Duration {
        Duration::from_secs(1)
    }
}

#[derive(Default)]
pub(crate) struct LiveBackend;

impl Backend for LiveBackend {
    fn authenticate(&self, cancelled: &AtomicBool) -> Result<Character, String> {
        auth::authenticate(cancelled)
    }

    fn refresh_character_affiliation(&self, character: &mut Character) -> Result<(), String> {
        esi::refresh_character_affiliation(character)
    }

    fn load_killmails(
        &self,
        characters: &[Character],
        cached_killmails: &[Killmail],
        reported_ids: &HashSet<u64>,
    ) -> Result<Vec<Killmail>, String> {
        esi::load_killmails(characters, cached_killmails, reported_ids)
    }

    fn resolve_protected_victim(
        &self,
        kind: ProtectedVictimKind,
        query: &str,
    ) -> Result<ProtectedVictim, String> {
        match query.parse::<u64>() {
            Ok(id) if id > 0 => {
                let name = match kind {
                    ProtectedVictimKind::Character => esi::resolve_character_name(id),
                    ProtectedVictimKind::Corporation => esi::resolve_corporation_name(id),
                }?;
                Ok(ProtectedVictim { id, name })
            }
            _ => esi::resolve_protected_victim_name(kind, query)
                .map(|(id, name)| ProtectedVictim { id, name }),
        }
    }

    fn character_killmail_page(
        &self,
        character_id: u64,
        page: usize,
    ) -> Result<Vec<zkill::KillEntry>, String> {
        zkill::character_killmail_page(character_id, page)
    }

    fn character_loss_killmail_page(
        &self,
        character_id: u64,
        page: usize,
    ) -> Result<Vec<zkill::KillEntry>, String> {
        zkill::character_loss_killmail_page(character_id, page)
    }

    fn post(&self, mail: &Killmail) -> Result<zkill::PostOutcome, String> {
        zkill::post(mail)
    }

    fn save_refresh_token(&self, character_id: u64, token: &str) -> Result<(), String> {
        secrets::save_refresh_token(character_id, token)
    }

    fn delete_refresh_token(&self, character_id: u64) -> Result<(), String> {
        secrets::delete_refresh_token(character_id)
    }
}
