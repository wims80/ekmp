use crate::{
    integrations::{auth, esi, http::CooldownLog, zkill, ApiResult},
    models::{ApiCooldown, Character, Killmail, ProtectedVictim, ProtectedVictimKind, ZkillPage},
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
    ) -> ApiResult<Character>;
    fn refresh_character_affiliation(
        &self,
        character: &mut Character,
        cancelled: &AtomicBool,
    ) -> ApiResult<()>;
    fn load_killmails(
        &self,
        characters: &[Character],
        cached_killmails: &[Killmail],
        reported_ids: &HashSet<u64>,
        cancelled: &AtomicBool,
        on_character_updated: &mut dyn FnMut(&Character) -> ApiResult<()>,
    ) -> ApiResult<LoadKillmailsOutcome>;
    fn resolve_protected_victim(
        &self,
        kind: ProtectedVictimKind,
        query: &str,
    ) -> ApiResult<ProtectedVictim>;
    fn killmail_page(
        &self,
        kind: zkill::MailKind,
        character_id: u64,
        page: usize,
    ) -> ApiResult<ZkillPage>;
    fn post(&self, mail: &Killmail) -> ApiResult<zkill::PostOutcome>;
    fn save_refresh_token(&self, character_id: u64, token: &str) -> ApiResult<()>;
    fn delete_refresh_token(&self, character_id: u64) -> ApiResult<()>;

    fn take_api_cooldowns(&self) -> Vec<ApiCooldown> {
        Vec::new()
    }

    fn request_spacing(&self) -> Duration {
        Duration::from_secs(1)
    }
}

#[derive(Default)]
pub(crate) struct LiveBackend {
    cooldowns: CooldownLog,
}

impl Backend for LiveBackend {
    fn authenticate(
        &self,
        cancelled: &AtomicBool,
        open_browser: bool,
        on_authorization_url: &dyn Fn(&str),
    ) -> ApiResult<Character> {
        auth::authenticate(cancelled, open_browser, on_authorization_url)
    }

    fn refresh_character_affiliation(
        &self,
        character: &mut Character,
        cancelled: &AtomicBool,
    ) -> ApiResult<()> {
        esi::refresh_character_affiliation(character, cancelled, &self.cooldowns)
    }

    fn load_killmails(
        &self,
        characters: &[Character],
        cached_killmails: &[Killmail],
        reported_ids: &HashSet<u64>,
        cancelled: &AtomicBool,
        on_character_updated: &mut dyn FnMut(&Character) -> ApiResult<()>,
    ) -> ApiResult<LoadKillmailsOutcome> {
        esi::load_killmails(
            characters,
            cached_killmails,
            reported_ids,
            cancelled,
            &self.cooldowns,
            on_character_updated,
        )
    }

    fn resolve_protected_victim(
        &self,
        kind: ProtectedVictimKind,
        query: &str,
    ) -> ApiResult<ProtectedVictim> {
        esi::resolve_protected_victim(kind, query, &self.cooldowns)
    }

    fn killmail_page(
        &self,
        kind: zkill::MailKind,
        character_id: u64,
        page: usize,
    ) -> ApiResult<ZkillPage> {
        zkill::killmail_page(kind, character_id, page, &self.cooldowns)
    }

    fn post(&self, mail: &Killmail) -> ApiResult<zkill::PostOutcome> {
        zkill::post(mail, &self.cooldowns)
    }

    fn save_refresh_token(&self, character_id: u64, token: &str) -> ApiResult<()> {
        Ok(secrets::save_refresh_token(character_id, token)?)
    }

    fn delete_refresh_token(&self, character_id: u64) -> ApiResult<()> {
        Ok(secrets::delete_refresh_token(character_id)?)
    }

    fn take_api_cooldowns(&self) -> Vec<ApiCooldown> {
        self.cooldowns.take()
    }
}
