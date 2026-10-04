use crate::clock::unix_time;
mod operations;
mod ui;
mod worker;

use crate::{
    core::{Core, CoreError, CoreEvent, PreparedPost, SessionReport, Snapshot, StatusSnapshot},
    integrations::backend::{Backend, LiveBackend},
    models::{Character, ProtectedVictimKind, Store},
};
use eframe::egui;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::Path,
    sync::{mpsc::Receiver, Arc},
    time::{Duration, Instant},
};
use worker::{IdentityImageEvent, IdentityImageKey};

const STATUS_HISTORY_LIMIT: usize = 200;
const SNAPSHOT_POLL_INTERVAL: Duration = Duration::from_secs(2);

struct PendingCharacterRemoval {
    id: u64,
    name: String,
}

/// The GUI update applied when an operation succeeds.
type Completion = Box<dyn FnOnce(&mut App) + Send>;

struct ActiveOperation {
    /// Status shown while the operation runs.
    label: &'static str,
    updates: Receiver<OperationUpdate>,
    cancellation: crate::core::Cancellation,
    /// Button text when the user may cancel this operation.
    cancel_label: Option<&'static str>,
}

enum OperationUpdate {
    AuthorizationUrl(String),
    Completed(Result<Completion, CoreError>),
}

/// Loaded identity images by key.
type Images = HashMap<IdentityImageKey, IdentityImageState>;

enum IdentityImageState {
    Loading,
    Ready(egui::TextureHandle),
    Failed,
}

pub struct App {
    core: Core,
    core_events: Receiver<CoreEvent>,
    store: Store,
    core_status: StatusSnapshot,
    latest_status: String,
    status_history: VecDeque<String>,
    active_operation: Option<ActiveOperation>,
    snapshot_result: Option<Receiver<Result<Snapshot, CoreError>>>,
    last_snapshot_poll: Instant,
    refresh_after_snapshot: bool,
    pending_post: Option<PreparedPost>,
    pending_character_removal: Option<PendingCharacterRemoval>,
    authorization_url: Option<String>,
    new_protected_victim_query: String,
    new_protected_victim_kind: ProtectedVictimKind,
    session_reports: Vec<SessionReport>,
    expanded_killmail_ids: HashSet<u64>,
    persistence_blocked: Option<String>,
    identity_image_requests: std::sync::mpsc::Sender<IdentityImageKey>,
    identity_image_events: Receiver<IdentityImageEvent>,
    identity_images: Images,
    simulation_name: Option<String>,
    run_jobs_inline: bool,
}

impl App {
    pub fn new() -> Self {
        let backend: Arc<dyn Backend> = Arc::new(LiveBackend::default());
        let (core, persistence_blocked) = match Core::live(Arc::clone(&backend)) {
            Ok(core) => (core, None),
            Err(error) => (
                Core::in_memory(backend, Store::default()),
                Some(error.to_string()),
            ),
        };
        Self::build(core, persistence_blocked, None, false)
    }

    #[cfg(test)]
    fn simulated(store: Store, backend: Arc<dyn Backend>, scenario_name: String) -> Self {
        Self::build(
            Core::in_memory(backend, store),
            None,
            Some(scenario_name),
            true,
        )
    }

    fn build(
        core: Core,
        mut persistence_blocked: Option<String>,
        simulation_name: Option<String>,
        run_jobs_inline: bool,
    ) -> Self {
        let (core_event_tx, core_events) = std::sync::mpsc::channel();
        let core = core.with_events(core_event_tx);
        let fallback_snapshot = || Snapshot {
            store: Store::default(),
            status: StatusSnapshot::default(),
        };
        let (snapshot, refresh_after_snapshot) = match core.snapshot() {
            Ok(snapshot) => (snapshot, false),
            Err(CoreError::Busy) => (fallback_snapshot(), true),
            Err(error) => {
                persistence_blocked.get_or_insert_with(|| error.to_string());
                (fallback_snapshot(), false)
            }
        };
        let (identity_image_requests, identity_image_events) =
            worker::start_identity_image_worker(simulation_name.is_none());
        let session_reports = core.session_reports();
        let mut app = Self {
            core,
            core_events,
            store: snapshot.store,
            core_status: snapshot.status,
            latest_status: "Ready to load recent killmails.".into(),
            status_history: VecDeque::from(["Ready to load recent killmails.".into()]),
            active_operation: None,
            snapshot_result: None,
            last_snapshot_poll: Instant::now()
                .checked_sub(SNAPSHOT_POLL_INTERVAL)
                .unwrap_or_else(Instant::now),
            refresh_after_snapshot,
            pending_post: None,
            pending_character_removal: None,
            authorization_url: None,
            new_protected_victim_query: String::new(),
            new_protected_victim_kind: ProtectedVictimKind::Character,
            session_reports,
            expanded_killmail_ids: HashSet::new(),
            persistence_blocked,
            identity_image_requests,
            identity_image_events,
            identity_images: HashMap::new(),
            simulation_name,
            run_jobs_inline,
        };
        if let Some(error) = app.persistence_blocked.clone() {
            app.log(format!(
                "Could not safely load local state; operations are disabled: {error}"
            ));
        } else if app.has_json_refresh_token_fallback() {
            app.migrate_refresh_tokens();
        } else if !app.store.characters.is_empty() {
            app.refresh_killmails();
        }
        app
    }

    fn is_busy(&self) -> bool {
        self.active_operation.is_some()
    }

    fn status_pill_text(&self) -> String {
        if let Some(active) = &self.active_operation {
            return active.label.into();
        }
        if self.persistence_blocked.is_some() {
            return "Local state unavailable - See warning".into();
        }
        if self.core_status.last_error.is_some() {
            return "Action needed - See activity log".into();
        }
        if self.store.characters.is_empty() {
            return "Connect a character to begin".into();
        }
        if self.store.cached_killmails.is_empty() {
            return "Ready to load recent killmails".into();
        }
        if self.core_status.awaiting_status > 0 {
            format!(
                "Checking {} killmail statuses",
                self.core_status.awaiting_status
            )
        } else if self.core_status.unreported > 0 {
            format!(
                "Review queue - {} confirmed unreported",
                self.core_status.unreported
            )
        } else {
            "Review queue is up to date".into()
        }
    }

    fn refresh_status_text(&self) -> String {
        let now = unix_time();
        let mut parts = Vec::new();
        if let Some(last_attempt) = self.core_status.last_refresh_attempt_at {
            parts.push(format!(
                "Last refresh attempt {}",
                relative_time_label(last_attempt, now)
            ));
        } else {
            parts.push("Refresh has not run yet".into());
        }
        if let Some(last_success) = self.core_status.last_refresh_success_at {
            parts.push(format!(
                "last success {}",
                relative_time_label(last_success, now)
            ));
        }
        if let Some(next) = self.core_status.next_eligible_refresh_at {
            parts.push(format!("next eligible {}", relative_time_label(next, now)));
        }
        if self.core_status.stale {
            parts.push("cached data is stale".into());
        }
        parts.push(if self.core_status.service_running {
            "refresh service running".into()
        } else {
            "refresh service stopped".into()
        });
        if !self.core_status.api_cooldowns.is_empty() {
            parts.push(format!(
                "{} API cooldown{} active",
                self.core_status.api_cooldowns.len(),
                if self.core_status.api_cooldowns.len() == 1 {
                    ""
                } else {
                    "s"
                }
            ));
        }
        parts.join(" · ")
    }

    fn persisted_controls_enabled(&self) -> bool {
        !self.is_busy() && self.persistence_blocked.is_none()
    }

    fn has_json_refresh_token_fallback(&self) -> bool {
        self.store
            .characters
            .iter()
            .any(Character::uses_json_refresh_token_fallback)
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
}

/// Opens the desktop interface, optionally on an offline scenario.
pub(crate) fn run(scenario: Option<&str>, dev_state: Option<&Path>) -> Result<(), String> {
    let inspection = std::env::var("EGUI_INSPECTION")
        .is_ok_and(|value| !value.is_empty() && value != "0" && value != "false");
    if inspection && scenario.is_none() {
        return Err(
            "EGUI_INSPECTION may only be enabled together with a simulation scenario".into(),
        );
    }
    let app = match scenario {
        None => App::new(),
        Some(name) => {
            let (core, name) =
                crate::cli::scenario_core(name, dev_state).map_err(|error| error.to_string())?;
            App::build(core, None, Some(name), false)
        }
    };
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../../assets/app-icon.png"))
        .map_err(|_| "could not decode application icon")?;
    eframe::run_native(
        "EVE Killmail Publisher",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_app_id("ekmp")
                .with_inner_size([1180.0, 760.0])
                .with_min_inner_size([900.0, 620.0])
                .with_icon(icon),
            ..Default::default()
        },
        Box::new(move |_| Ok(Box::new(app))),
    )
    .map_err(|_| "could not open desktop interface".into())
}

fn relative_time_label(timestamp: u64, now: u64) -> String {
    if timestamp > now {
        format!("in {}s", timestamp - now)
    } else {
        format!("{}s ago", now - timestamp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{Cancellation, PostMode};
    use crate::integrations::simulation;
    use crate::persistence::storage;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_PATH: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn busy_initial_snapshot_is_retried_without_disabling_operations() {
        let sequence = NEXT_PATH.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "ekmp-gui-busy-snapshot-{}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("ekmp.json");
        let core = Core::at_path(Arc::new(LiveBackend::default()), path.clone());
        core.initialize(Store {
            show_protected_killmails: true,
            ..Store::default()
        })
        .unwrap();
        let lock = storage::try_operation_lock_for(&path).unwrap();

        let mut app = App::build(core, None, Some("test".into()), true);

        assert!(app.persistence_blocked.is_none());
        assert!(!app.store.show_protected_killmails);
        drop(lock);
        app.last_snapshot_poll = Instant::now()
            .checked_sub(SNAPSHOT_POLL_INTERVAL)
            .unwrap_or_else(Instant::now);
        app.poll_core();
        app.poll_core();
        assert!(app.store.show_protected_killmails);

        drop(app);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn background_snapshot_poll_observes_external_core_edits() {
        let core = Core::in_memory(Arc::new(LiveBackend::default()), Store::default());
        let external_core = core.clone();
        let mut app = App::build(core, None, Some("test".into()), true);
        assert!(!app.store.show_protected_killmails);

        external_core.set_show_protected(true).unwrap();
        assert!(!app.store.show_protected_killmails);

        app.poll_core();
        app.poll_core();

        assert!(app.store.show_protected_killmails);
    }

    #[test]
    fn completed_session_reports_survive_a_later_post_persistence_error() {
        let loaded = simulation::load("mixed").unwrap();
        let backend = Arc::new(loaded.backend);
        let setup = Core::in_memory(backend.clone(), loaded.store);
        setup.refresh(&Cancellation::new()).unwrap();
        let mut store = setup.snapshot().unwrap().store;
        store.characters.clear();
        store
            .cached_killmails
            .retain(|mail| matches!(mail.id, 9001 | 9006));
        let core = Core::in_memory_with_persist_budget(backend.clone(), store, 3);
        let mut app = App::build(core, None, Some("test".into()), true);

        app.post_prepared(PreparedPost {
            ids: vec![9001, 9006],
            mode: PostMode::Bulk,
        });
        app.poll_core();

        assert_eq!(backend.posted_ids(), vec![9001, 9006]);
        assert_eq!(app.session_reports.len(), 1);
        assert_eq!(app.session_reports[0].killmail_id, 9001);
        assert!(app.latest_status.contains("injected persistence failure"));
    }
}
