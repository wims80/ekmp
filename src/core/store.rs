use super::{
    timing::{core_lock_error, unix_time},
    Core, CoreError, CoreResult, Snapshot, StatusSnapshot,
};
use crate::{
    integrations::backend::Backend,
    killmail::{report_state, ReportState},
    models::{Store, ZkillStatus},
    persistence::storage::{self, FileLock, LockError, StorePaths},
};
#[cfg(any(feature = "gui", feature = "dev-tools", test))]
use std::sync::{
    atomic::{AtomicBool, Ordering},
    MutexGuard,
};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub(super) enum Persistence {
    File(StorePaths),
    #[cfg(any(feature = "gui", feature = "dev-tools", test))]
    Memory(Arc<MemoryStore>),
}

#[cfg(any(feature = "gui", feature = "dev-tools", test))]
pub(crate) struct MemoryStore {
    store: Mutex<Store>,
    service_active: AtomicBool,
    /// Number of persists that succeed before failures are injected.
    #[cfg(test)]
    persist_budget: Option<std::sync::atomic::AtomicIsize>,
}

/// Holds the cross-process service lock until dropped.
pub(crate) enum ServiceGuard {
    File {
        _lock: FileLock,
    },
    #[cfg(any(feature = "gui", feature = "dev-tools", test))]
    Memory(Arc<MemoryStore>),
}

impl Drop for ServiceGuard {
    fn drop(&mut self) {
        #[cfg(any(feature = "gui", feature = "dev-tools", test))]
        if let Self::Memory(memory) = self {
            memory.service_active.store(false, Ordering::Release);
        }
    }
}

/// A working copy of the store, held under the exclusive operation lock.
///
/// Changes become visible to other readers only through `persist`.
pub(super) struct LockedStore<'a> {
    store: Store,
    target: PersistTarget<'a>,
}

enum PersistTarget<'a> {
    File {
        _lock: FileLock,
        paths: &'a StorePaths,
    },
    #[cfg(any(feature = "gui", feature = "dev-tools", test))]
    Memory {
        shared: MutexGuard<'a, Store>,
        #[cfg(test)]
        memory: &'a MemoryStore,
    },
}

impl LockedStore<'_> {
    pub(super) fn store(&self) -> &Store {
        &self.store
    }

    pub(super) fn store_mut(&mut self) -> &mut Store {
        &mut self.store
    }

    pub(super) fn persist(&mut self) -> CoreResult<()> {
        match &mut self.target {
            PersistTarget::File { paths, .. } => save(paths, &self.store),
            #[cfg(any(feature = "gui", feature = "dev-tools", test))]
            PersistTarget::Memory {
                shared,
                #[cfg(test)]
                memory,
            } => {
                #[cfg(test)]
                if let Some(budget) = &memory.persist_budget {
                    if budget.fetch_sub(1, Ordering::AcqRel) <= 0 {
                        return Err(CoreError::Persistence(
                            "injected persistence failure".into(),
                        ));
                    }
                }
                shared.clone_from(&self.store);
                Ok(())
            }
        }
    }
}

fn save(paths: &StorePaths, store: &Store) -> CoreResult<()> {
    storage::save(paths, store).map_err(CoreError::Persistence)
}

impl Core {
    pub(crate) fn live(backend: Arc<dyn Backend>) -> CoreResult<Self> {
        let paths = StorePaths::live().map_err(CoreError::Persistence)?;
        Ok(Self::with_persistence(backend, Persistence::File(paths)))
    }

    #[cfg(any(test, feature = "dev-tools"))]
    /// A core whose files are kept beside the state file at `path`.
    pub(crate) fn at_path(backend: Arc<dyn Backend>, path: std::path::PathBuf) -> Self {
        Self::with_persistence(backend, Persistence::File(StorePaths::beside(path)))
    }

    #[cfg(any(feature = "gui", feature = "dev-tools", test))]
    pub(crate) fn in_memory(backend: Arc<dyn Backend>, store: Store) -> Self {
        Self::with_persistence(
            backend,
            Persistence::Memory(Arc::new(MemoryStore {
                store: Mutex::new(store),
                service_active: AtomicBool::new(false),
                #[cfg(test)]
                persist_budget: None,
            })),
        )
    }

    #[cfg(test)]
    pub(crate) fn in_memory_with_persist_budget(
        backend: Arc<dyn Backend>,
        store: Store,
        successful_persists: isize,
    ) -> Self {
        Self::with_persistence(
            backend,
            Persistence::Memory(Arc::new(MemoryStore {
                store: Mutex::new(store),
                service_active: AtomicBool::new(false),
                persist_budget: Some(std::sync::atomic::AtomicIsize::new(successful_persists)),
            })),
        )
    }

    fn with_persistence(backend: Arc<dyn Backend>, persistence: Persistence) -> Self {
        Self {
            backend,
            persistence,
            events: None,
            session_reports: Arc::new(Mutex::new(Vec::new())),
        }
    }

    #[cfg(any(test, feature = "dev-tools"))]
    pub(crate) fn initialize(&self, initial: Store) -> CoreResult<()> {
        let Persistence::File(paths) = &self.persistence else {
            return Ok(());
        };
        let _lock = storage::try_operation_lock_for(&paths.state).map_err(core_lock_error)?;
        if storage::state_exists(paths) {
            return Ok(());
        }
        save(paths, &initial)
    }

    pub(crate) fn snapshot(&self) -> CoreResult<Snapshot> {
        let store = match &self.persistence {
            Persistence::File(paths) => {
                let _lock =
                    storage::try_snapshot_lock_for(&paths.state).map_err(core_lock_error)?;
                storage::load(paths).map_err(CoreError::Persistence)?
            }
            #[cfg(any(feature = "gui", feature = "dev-tools", test))]
            Persistence::Memory(memory) => memory
                .store
                .try_lock()
                .map_err(|_| CoreError::Busy)?
                .clone(),
        };
        let status = self.status_for(&store);
        Ok(Snapshot { store, status })
    }

    pub(crate) fn try_service_guard(&self) -> CoreResult<ServiceGuard> {
        match &self.persistence {
            Persistence::File(paths) => Ok(ServiceGuard::File {
                _lock: storage::try_service_lock_for(&paths.state).map_err(core_lock_error)?,
            }),
            #[cfg(any(feature = "gui", feature = "dev-tools", test))]
            Persistence::Memory(memory) => {
                memory
                    .service_active
                    .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                    .map_err(|_| CoreError::Busy)?;
                Ok(ServiceGuard::Memory(Arc::clone(memory)))
            }
        }
    }

    pub(super) fn simple_mutation(&self, mutation: impl FnOnce(&mut Store)) -> CoreResult<()> {
        let mut locked = self.begin_operation()?;
        mutation(locked.store_mut());
        locked.persist()?;
        self.changed();
        Ok(())
    }

    pub(super) fn begin_operation(&self) -> CoreResult<LockedStore<'_>> {
        match &self.persistence {
            Persistence::File(paths) => {
                let lock =
                    storage::try_operation_lock_for(&paths.state).map_err(core_lock_error)?;
                Ok(LockedStore {
                    store: storage::load(paths).map_err(CoreError::Persistence)?,
                    target: PersistTarget::File { _lock: lock, paths },
                })
            }
            #[cfg(any(feature = "gui", feature = "dev-tools", test))]
            Persistence::Memory(memory) => {
                let shared = memory.store.try_lock().map_err(|_| CoreError::Busy)?;
                Ok(LockedStore {
                    store: shared.clone(),
                    target: PersistTarget::Memory {
                        shared,
                        #[cfg(test)]
                        memory,
                    },
                })
            }
        }
    }

    fn status_for(&self, store: &Store) -> StatusSnapshot {
        let now = unix_time();
        let reported = store
            .zkill_status
            .values()
            .filter(|status| **status == ZkillStatus::Reported)
            .count();
        let mut unreported = 0;
        let mut awaiting_status = 0;
        for mail in &store.cached_killmails {
            match report_state(store, mail.id, now) {
                ReportState::Reported => {}
                ReportState::Unreported => unreported += 1,
                ReportState::Unknown => awaiting_status += 1,
            }
        }
        let service_running = match &self.persistence {
            Persistence::File(paths) => match storage::try_service_lock_for(&paths.state) {
                Ok(lock) => {
                    drop(lock);
                    false
                }
                Err(LockError::Busy) => true,
                Err(LockError::Io(_)) => false,
            },
            #[cfg(any(feature = "gui", feature = "dev-tools", test))]
            Persistence::Memory(memory) => memory.service_active.load(Ordering::Acquire),
        };
        let stale = store
            .refresh_schedule
            .last_success_at
            .is_none_or(|success| now >= success.saturating_add(store.refresh_interval_secs));
        StatusSnapshot {
            authenticated_characters: store.characters.len(),
            cached_killmails: store.cached_killmails.len(),
            reported,
            unreported,
            awaiting_status,
            last_refresh_attempt_at: store.refresh_schedule.last_attempt_at,
            last_refresh_success_at: store.refresh_schedule.last_success_at,
            last_refresh_completed_at: store.refresh_schedule.last_completed_at,
            next_eligible_refresh_at: store.refresh_schedule.next_eligible_at,
            refresh_interval_secs: store.refresh_interval_secs,
            stale,
            last_error: store.refresh_schedule.last_error.clone(),
            service_running,
            api_cooldowns: store
                .api_cooldowns
                .iter()
                .filter(|cooldown| cooldown.until > now)
                .cloned()
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{test_support::*, timing::unix_time_millis};
    use std::sync::Arc;

    #[test]
    fn initialize_does_not_overwrite_existing_state() {
        let sequence = unix_time_millis();
        let directory = std::env::temp_dir().join(format!(
            "ekmp-core-initialize-{}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("state.json");
        let core = Core::at_path(Arc::new(TestBackend::new()), path);
        core.initialize(Store {
            show_protected_killmails: true,
            ..Store::default()
        })
        .unwrap();

        core.initialize(Store::default()).unwrap();

        assert!(core.snapshot().unwrap().store.show_protected_killmails);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
