mod events;
mod operations;
mod ui;
mod worker;

use crate::{
    killmail::{remove_killmails_without_authenticated_sources, remove_reported_killmails},
    models::{Character, Killmail, ProtectedVictimKind, Store},
    storage,
};
use std::{collections::VecDeque, sync::mpsc::Receiver};
use worker::WorkerEvent;

const STATUS_HISTORY_LIMIT: usize = 200;

#[derive(Clone, Copy)]
enum Operation {
    Authenticate,
    MigrateRefreshTokens,
    RemoveCharacter,
    Load,
    CheckCachedStatuses,
    AddProtectedVictim,
    Post(SubmissionMode),
}

#[derive(Clone, Copy)]
enum SubmissionMode {
    Individual,
    Bulk,
}

#[derive(Default)]
struct PostStats {
    total: usize,
    new: usize,
    existing: usize,
    failed: usize,
}

struct SessionReport {
    killmail_id: u64,
    url: String,
    status: SessionReportStatus,
}

#[derive(Clone, Copy)]
enum SessionReportStatus {
    Submitted,
    AlreadyPresent,
}

struct ActiveOperation {
    kind: Operation,
    events: Receiver<WorkerEvent>,
}

pub struct App {
    store: Store,
    latest_status: String,
    status_history: VecDeque<String>,
    active_operation: Option<ActiveOperation>,
    post_stats: PostStats,
    pending_bulk: Option<Vec<Killmail>>,
    pending_character_removal: Option<Character>,
    new_protected_victim_id: String,
    new_protected_victim_kind: ProtectedVictimKind,
    session_reports: Vec<SessionReport>,
    persistence_blocked: Option<String>,
}

impl App {
    pub fn new() -> Self {
        let (mut store, persistence_blocked) = match storage::load() {
            Ok(store) => (store, None),
            Err(error) => (Store::default(), Some(error)),
        };
        let removed_reported =
            remove_reported_killmails(&store.zkill_cache, &mut store.cached_killmails);
        let store_view = store.clone();
        let removed_orphaned = remove_killmails_without_authenticated_sources(
            &store_view,
            &mut store.cached_killmails,
        );
        let mut app = Self {
            store,
            latest_status: "Ready to load recent killmails.".into(),
            status_history: VecDeque::from(["Ready to load recent killmails.".into()]),
            active_operation: None,
            post_stats: PostStats::default(),
            pending_bulk: None,
            pending_character_removal: None,
            new_protected_victim_id: String::new(),
            new_protected_victim_kind: ProtectedVictimKind::Character,
            session_reports: Vec::new(),
            persistence_blocked,
        };
        if let Some(error) = app.persistence_blocked.clone() {
            app.log(format!(
                "Could not safely load local state; saving is disabled: {error}"
            ));
        } else if removed_reported > 0 || removed_orphaned > 0 {
            app.persist_or_log_error();
        }
        if app
            .store
            .characters
            .iter()
            .any(|character| character.uses_json_refresh_token_fallback())
        {
            app.migrate_refresh_tokens();
        } else {
            app.check_cached_statuses_on_startup();
        }
        app
    }

    fn is_busy(&self) -> bool {
        self.active_operation.is_some()
    }

    fn persisted_controls_enabled(&self) -> bool {
        !self.is_busy() && self.persistence_blocked.is_none()
    }

    fn has_json_refresh_token_fallback(&self) -> bool {
        self.store
            .characters
            .iter()
            .any(|character| character.uses_json_refresh_token_fallback())
    }

    fn log(&mut self, message: impl Into<String>) {
        let message = message.into();
        self.latest_status.clone_from(&message);
        if self.status_history.len() == STATUS_HISTORY_LIMIT {
            self.status_history.pop_front();
        }
        self.status_history.push_back(message);
    }

    fn persist_or_log_error(&mut self) {
        if self.persistence_blocked.is_some() {
            self.log("Local state was not saved because it could not be loaded safely at startup");
            return;
        }
        if let Err(error) = storage::persist(&self.store) {
            self.log(format!("Could not save local state: {error}"));
        }
    }

    fn prune_persisted_reported_killmails(&mut self) {
        remove_reported_killmails(&self.store.zkill_cache, &mut self.store.cached_killmails);
    }
}
