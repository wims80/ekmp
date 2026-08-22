mod events;
mod operations;
mod ui;
mod worker;

use crate::{
    integrations::backend::{Backend, LiveBackend},
    killmail::{
        posting_summary, remove_killmails_without_authenticated_sources,
        remove_reported_killmail_flags, remove_reported_killmails,
    },
    models::{Character, Killmail, ProtectedVictimKind, Store, ZKILL_STATUS_CACHE_VERSION},
    persistence::storage,
};
use eframe::egui;
#[cfg(any(test, feature = "dev-tools"))]
use std::path::PathBuf;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{
        atomic::AtomicBool,
        mpsc::{Receiver, Sender},
        Arc,
    },
};
use worker::{IdentityImageEvent, IdentityImageKey, WorkerEvent};

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
    cancellation: Option<Arc<AtomicBool>>,
}

enum IdentityImageState {
    Loading,
    Ready(egui::TextureHandle),
    Failed,
}

pub struct App {
    store: Store,
    latest_status: String,
    status_history: VecDeque<String>,
    active_operation: Option<ActiveOperation>,
    post_stats: PostStats,
    pending_bulk: Option<Vec<Killmail>>,
    pending_character_removal: Option<Character>,
    new_protected_victim_query: String,
    new_protected_victim_kind: ProtectedVictimKind,
    session_reports: Vec<SessionReport>,
    expanded_killmail_ids: HashSet<u64>,
    persistence_blocked: Option<String>,
    identity_image_requests: Sender<IdentityImageKey>,
    identity_image_events: Receiver<IdentityImageEvent>,
    identity_images: HashMap<IdentityImageKey, IdentityImageState>,
    backend: Arc<dyn Backend>,
    persistence: PersistenceTarget,
    simulation_name: Option<String>,
    run_jobs_inline: bool,
}

pub(crate) enum PersistenceTarget {
    Live,
    #[cfg(any(test, feature = "dev-tools"))]
    Disabled,
    #[cfg(any(test, feature = "dev-tools"))]
    File(PathBuf),
}

impl App {
    pub fn new() -> Self {
        let (mut store, persistence_blocked) = match storage::load() {
            Ok(store) => (store, None),
            Err(error) => (Store::default(), Some(error)),
        };
        Self::build(
            &mut store,
            persistence_blocked,
            Arc::new(LiveBackend),
            PersistenceTarget::Live,
            None,
            false,
        )
    }

    #[cfg(any(test, feature = "dev-tools"))]
    pub(crate) fn simulated(
        mut store: Store,
        backend: Arc<dyn Backend>,
        scenario_name: String,
        state_path: Option<PathBuf>,
        run_jobs_inline: bool,
    ) -> Self {
        let persistence = match state_path {
            Some(path) => PersistenceTarget::File(path),
            None => PersistenceTarget::Disabled,
        };
        Self::build(
            &mut store,
            None,
            backend,
            persistence,
            Some(scenario_name),
            run_jobs_inline,
        )
    }

    fn build(
        store: &mut Store,
        persistence_blocked: Option<String>,
        backend: Arc<dyn Backend>,
        persistence: PersistenceTarget,
        simulation_name: Option<String>,
        run_jobs_inline: bool,
    ) -> Self {
        let (identity_image_requests, identity_image_events) =
            worker::start_identity_image_worker(simulation_name.is_none());
        let invalidated_unreported = invalidate_outdated_negative_statuses(store);
        let removed_reported =
            remove_reported_killmails(&store.zkill_cache, &mut store.cached_killmails);
        let removed_reported_flags = remove_reported_killmail_flags(
            &store.zkill_cache,
            &mut store.manually_protected_killmail_ids,
        );
        let store_view = store.clone();
        let removed_orphaned = remove_killmails_without_authenticated_sources(
            &store_view,
            &mut store.cached_killmails,
        );
        let mut app = Self {
            store: std::mem::take(store),
            latest_status: "Ready to load recent killmails.".into(),
            status_history: VecDeque::from(["Ready to load recent killmails.".into()]),
            active_operation: None,
            post_stats: PostStats::default(),
            pending_bulk: None,
            pending_character_removal: None,
            new_protected_victim_query: String::new(),
            new_protected_victim_kind: ProtectedVictimKind::Character,
            session_reports: Vec::new(),
            expanded_killmail_ids: HashSet::new(),
            persistence_blocked,
            identity_image_requests,
            identity_image_events,
            identity_images: HashMap::new(),
            backend,
            persistence,
            simulation_name,
            run_jobs_inline,
        };
        if let Some(error) = app.persistence_blocked.clone() {
            app.log(format!(
                "Could not safely load local state; saving is disabled: {error}"
            ));
        } else if invalidated_unreported > 0
            || removed_reported > 0
            || removed_reported_flags > 0
            || removed_orphaned > 0
        {
            app.persist_or_log_error();
        }
        if invalidated_unreported > 0 {
            app.log(format!(
                "Rechecking {invalidated_unreported} cached killmail status{} with the updated zKillboard lookup",
                if invalidated_unreported == 1 { "" } else { "es" }
            ));
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

    fn is_authenticating(&self) -> bool {
        self.active_operation
            .as_ref()
            .is_some_and(|operation| matches!(operation.kind, Operation::Authenticate))
    }

    fn status_pill_text(&self) -> String {
        if let Some(active) = &self.active_operation {
            return match active.kind {
                Operation::Authenticate => "Waiting for EVE authorization".into(),
                Operation::MigrateRefreshTokens => "Securing character credentials".into(),
                Operation::RemoveCharacter => "Disconnecting character".into(),
                Operation::Load => "Loading recent killmails".into(),
                Operation::CheckCachedStatuses => "Checking zKillboard status".into(),
                Operation::AddProtectedVictim => "Adding protected victim".into(),
                Operation::Post(SubmissionMode::Individual) => "Posting one killmail".into(),
                Operation::Post(SubmissionMode::Bulk) => "Posting eligible killmails".into(),
            };
        }
        if self.persistence_blocked.is_some() {
            return "Local state unavailable - See warning".into();
        }

        let latest_status = self.latest_status.to_ascii_lowercase();
        if latest_status.contains("failed")
            || latest_status.contains("could not")
            || latest_status.contains("unavailable")
        {
            return "Action needed - See activity log".into();
        }
        if self.store.characters.is_empty() {
            return "Connect a character to begin".into();
        }
        if self.store.cached_killmails.is_empty() {
            return "Ready to load recent killmails".into();
        }

        let summary = posting_summary(
            &self.store,
            &self.store.cached_killmails,
            worker::unix_time(),
        );
        if summary.awaiting_status > 0 {
            format!("Checking {} killmail statuses", summary.awaiting_status)
        } else if summary.eligible_for_bulk_posting > 0 {
            format!(
                "Ready - {} eligible for posting",
                summary.eligible_for_bulk_posting
            )
        } else if summary.protected > 0 {
            format!("Review queue - {} protected", summary.protected)
        } else {
            "Review queue is up to date".into()
        }
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

    fn protected_victim_already_present(&self, kind: ProtectedVictimKind, id: u64) -> bool {
        match kind {
            ProtectedVictimKind::Character => {
                self.store.characters.iter().any(|entry| entry.id == id)
                    || self
                        .store
                        .manually_protected_characters
                        .iter()
                        .any(|entry| entry.id == id)
            }
            ProtectedVictimKind::Corporation => {
                self.store
                    .characters
                    .iter()
                    .any(|entry| entry.corporation_id == Some(id))
                    || self
                        .store
                        .manually_protected_corporations
                        .iter()
                        .any(|entry| entry.id == id)
            }
        }
    }

    fn queue_identity_image(&mut self, key: IdentityImageKey) {
        if self.identity_images.contains_key(&key) {
            return;
        }
        let state = if self.identity_image_requests.send(key).is_ok() {
            IdentityImageState::Loading
        } else {
            IdentityImageState::Failed
        };
        self.identity_images.insert(key, state);
    }

    fn poll_identity_images(&mut self, ctx: &egui::Context) {
        for event in self.identity_image_events.try_iter() {
            match event {
                IdentityImageEvent::Loaded(image) => {
                    let color_image =
                        egui::ColorImage::from_rgba_unmultiplied(image.size, &image.rgba);
                    let texture = ctx.load_texture(
                        image.key.texture_name(),
                        color_image,
                        egui::TextureOptions::LINEAR,
                    );
                    self.identity_images
                        .insert(image.key, IdentityImageState::Ready(texture));
                }
                IdentityImageEvent::Failed(key) => {
                    self.identity_images.insert(key, IdentityImageState::Failed);
                }
            }
        }
    }

    fn identity_images_loading(&self) -> bool {
        self.identity_images
            .values()
            .any(|state| matches!(state, IdentityImageState::Loading))
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
        let result = match &self.persistence {
            PersistenceTarget::Live => storage::persist(&self.store),
            #[cfg(any(test, feature = "dev-tools"))]
            PersistenceTarget::Disabled => return,
            #[cfg(any(test, feature = "dev-tools"))]
            PersistenceTarget::File(path) => {
                let ensure_parent = path
                    .parent()
                    .filter(|parent| !parent.as_os_str().is_empty())
                    .map_or(Ok(()), std::fs::create_dir_all)
                    .map_err(|error| error.to_string());
                ensure_parent.and_then(|()| {
                    serde_json::to_vec_pretty(&self.store)
                        .map_err(|error| error.to_string())
                        .and_then(|data| {
                            storage::persist_to_path(path, &data).map_err(|error| {
                                format!("could not atomically write {}: {error}", path.display())
                            })
                        })
                })
            }
        };
        if let Err(error) = result {
            self.log(format!("Could not save local state: {error}"));
        }
    }

    fn prune_persisted_reported_killmails(&mut self) {
        remove_reported_killmails(&self.store.zkill_cache, &mut self.store.cached_killmails);
        remove_reported_killmail_flags(
            &self.store.zkill_cache,
            &mut self.store.manually_protected_killmail_ids,
        );
    }
}

fn invalidate_outdated_negative_statuses(store: &mut Store) -> usize {
    if store.zkill_status_cache_version >= ZKILL_STATUS_CACHE_VERSION {
        return 0;
    }
    let previous_len = store.zkill_cache.len();
    store.zkill_cache.retain(|_, entry| entry.reported);
    store.zkill_status_cache_version = ZKILL_STATUS_CACHE_VERSION;
    previous_len - store.zkill_cache.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ZkillCacheEntry;

    #[test]
    fn updated_lookup_invalidates_only_old_negative_cache_entries() {
        let mut store = Store::default();
        store.zkill_cache.insert(
            1,
            ZkillCacheEntry {
                reported: true,
                checked_at: 10,
            },
        );
        store.zkill_cache.insert(
            2,
            ZkillCacheEntry {
                reported: false,
                checked_at: 10,
            },
        );

        assert_eq!(invalidate_outdated_negative_statuses(&mut store), 1);
        assert!(store.zkill_cache.contains_key(&1));
        assert!(!store.zkill_cache.contains_key(&2));
        assert_eq!(store.zkill_status_cache_version, ZKILL_STATUS_CACHE_VERSION);
        assert_eq!(invalidate_outdated_negative_statuses(&mut store), 0);
    }
}
