use super::{timing::unix_time, Cancellation};
use crate::{
    integrations::{
        backend::{Backend, LoadKillmailsOutcome},
        zkill, ApiResult,
    },
    models::{
        Character, CharacterSource, ProtectedVictim, ProtectedVictimKind, Store, ZkillPage,
        ZkillStatus,
    },
};
use std::{
    collections::HashSet,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Mutex,
    },
    time::Duration,
};

pub(super) struct TestBackend {
    pub(super) posts: AtomicUsize,
    pub(super) lookups: AtomicUsize,
    pub(super) cancel_after_post: Mutex<Option<Cancellation>>,
    pub(super) authenticated: Mutex<Option<Character>>,
    pub(super) spacing_ms: AtomicUsize,
}

impl TestBackend {
    pub(super) fn new() -> Self {
        Self {
            posts: AtomicUsize::new(0),
            lookups: AtomicUsize::new(0),
            cancel_after_post: Mutex::new(None),
            authenticated: Mutex::new(None),
            spacing_ms: AtomicUsize::new(0),
        }
    }
}

impl Backend for TestBackend {
    fn authenticate(
        &self,
        _cancelled: &AtomicBool,
        _flow: crate::integrations::auth::AuthFlow,
        _on_authorization_url: &dyn Fn(&str),
    ) -> ApiResult<Character> {
        self.authenticated
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
            .ok_or_else(|| "unused".into())
    }

    fn refresh_character_affiliation(
        &self,
        _character: &mut Character,
        _cancelled: &AtomicBool,
    ) -> ApiResult<()> {
        Ok(())
    }

    fn load_killmails(
        &self,
        _characters: &[Character],
        cached_killmails: &[crate::models::Killmail],
        _reported_ids: &HashSet<u64>,
        _cancelled: &AtomicBool,
        _on_character_updated: &mut dyn FnMut(&Character) -> ApiResult<()>,
    ) -> ApiResult<LoadKillmailsOutcome> {
        Ok(LoadKillmailsOutcome {
            killmails: cached_killmails.to_vec(),
            character_failures: Vec::new(),
        })
    }

    fn resolve_protected_victim(
        &self,
        kind: ProtectedVictimKind,
        query: &str,
    ) -> ApiResult<ProtectedVictim> {
        Ok(ProtectedVictim {
            id: query.parse().unwrap_or(2),
            name: match kind {
                ProtectedVictimKind::Character => "Pilot",
                ProtectedVictimKind::Corporation => "Corp",
            }
            .into(),
        })
    }

    fn killmail_page(
        &self,
        _kind: zkill::MailKind,
        _character_id: u64,
        _page: usize,
    ) -> ApiResult<ZkillPage> {
        self.lookups.fetch_add(1, Ordering::Relaxed);
        Ok(empty_page())
    }

    fn post(&self, mail: &crate::models::Killmail) -> ApiResult<zkill::PostOutcome> {
        self.posts.fetch_add(1, Ordering::Relaxed);
        if let Some(cancellation) = self
            .cancel_after_post
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            cancellation.cancel();
        }
        Ok(zkill::PostOutcome {
            new: true,
            url: format!("https://zkillboard.com/kill/{}/", mail.id),
        })
    }

    fn save_refresh_token(&self, _character_id: u64, _token: &str) -> ApiResult<()> {
        Ok(())
    }

    fn delete_refresh_token(&self, _character_id: u64) -> ApiResult<()> {
        Ok(())
    }

    fn request_spacing(&self) -> Duration {
        Duration::from_millis(self.spacing_ms.load(Ordering::Relaxed) as u64)
    }
}

pub(super) fn empty_page() -> ZkillPage {
    let now = unix_time();
    ZkillPage {
        entries: Vec::new(),
        observed_at: now,
        valid_until: now + 3_600,
    }
}

pub(super) fn mail(id: u64) -> crate::models::Killmail {
    crate::models::Killmail {
        id,
        hash: "secret-hash".into(),
        sources: vec![CharacterSource {
            id: 1,
            name: "Pilot".into(),
        }],
        victim_id: Some(2),
        victim_corporation_id: None,
        victim: "Victim".into(),
        ship: "Ship".into(),
        time: "2020-01-01T00:00:00Z".into(),
        estimated_value_isk: None,
        detail: None,
    }
}

pub(super) fn postable_store() -> Store {
    let now = unix_time();
    let mut store = Store {
        characters: vec![Character {
            id: 1,
            name: "Pilot".into(),
            refresh_token: None,
            corporation_id: None,
            corporation_name: None,
        }],
        cached_killmails: vec![mail(42)],
        ..Store::default()
    };
    store.zkill_status.insert(
        42,
        ZkillStatus::Unreported {
            valid_until: now + 3_600,
        },
    );
    store
}
