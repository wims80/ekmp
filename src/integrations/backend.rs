use crate::{
    integrations::{auth, esi, zkill},
    models::{ApiCooldown, Character, Killmail, ProtectedVictim, ProtectedVictimKind},
    persistence::secrets,
};
use std::{collections::HashSet, sync::atomic::AtomicBool, time::Duration};

#[derive(Debug)]
pub(crate) struct CharacterRefreshFailure {
    pub character_id: u64,
    pub character_name: String,
    pub error: String,
}

pub(crate) struct LoadKillmailsOutcome {
    pub killmails: Vec<Killmail>,
    pub character_failures: Vec<CharacterRefreshFailure>,
}

pub(crate) trait Backend: Send + Sync {
    fn authenticate(
        &self,
        cancelled: &AtomicBool,
        open_browser: bool,
        on_authorization_url: &dyn Fn(&str),
    ) -> Result<Character, String>;
    fn refresh_character_affiliation(
        &self,
        character: &mut Character,
        cancelled: &AtomicBool,
    ) -> Result<(), String>;
    fn load_killmails(
        &self,
        characters: &[Character],
        cached_killmails: &[Killmail],
        reported_ids: &HashSet<u64>,
        cancelled: &AtomicBool,
        on_character_updated: &mut dyn FnMut(&Character) -> Result<(), String>,
    ) -> Result<LoadKillmailsOutcome, String>;
    fn resolve_protected_victim(
        &self,
        kind: ProtectedVictimKind,
        query: &str,
    ) -> Result<ProtectedVictim, String>;
    fn character_killmail_page(
        &self,
        character_id: u64,
        page: usize,
    ) -> Result<zkill::LookupPage, String>;
    fn character_loss_killmail_page(
        &self,
        character_id: u64,
        page: usize,
    ) -> Result<zkill::LookupPage, String>;
    fn post(&self, mail: &Killmail) -> Result<zkill::PostOutcome, String>;
    fn save_refresh_token(&self, character_id: u64, token: &str) -> Result<(), String>;
    fn delete_refresh_token(&self, character_id: u64) -> Result<(), String>;

    fn take_api_cooldowns(&self) -> Vec<ApiCooldown> {
        Vec::new()
    }

    fn request_spacing(&self) -> Duration {
        Duration::from_secs(1)
    }
}

#[derive(Default)]
pub(crate) struct LiveBackend;

impl Backend for LiveBackend {
    fn authenticate(
        &self,
        cancelled: &AtomicBool,
        open_browser: bool,
        on_authorization_url: &dyn Fn(&str),
    ) -> Result<Character, String> {
        auth::authenticate(cancelled, open_browser, on_authorization_url)
    }

    fn refresh_character_affiliation(
        &self,
        character: &mut Character,
        cancelled: &AtomicBool,
    ) -> Result<(), String> {
        esi::refresh_character_affiliation(character, cancelled)
    }

    fn load_killmails(
        &self,
        characters: &[Character],
        cached_killmails: &[Killmail],
        reported_ids: &HashSet<u64>,
        cancelled: &AtomicBool,
        on_character_updated: &mut dyn FnMut(&Character) -> Result<(), String>,
    ) -> Result<LoadKillmailsOutcome, String> {
        esi::load_killmails(
            characters,
            cached_killmails,
            reported_ids,
            cancelled,
            on_character_updated,
        )
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
    ) -> Result<zkill::LookupPage, String> {
        zkill::character_killmail_page(character_id, page)
    }

    fn character_loss_killmail_page(
        &self,
        character_id: u64,
        page: usize,
    ) -> Result<zkill::LookupPage, String> {
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

    fn take_api_cooldowns(&self) -> Vec<ApiCooldown> {
        let mut cooldowns = esi::take_api_cooldowns();
        cooldowns.extend(zkill::take_api_cooldowns());
        cooldowns
    }
}
