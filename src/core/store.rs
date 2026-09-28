use super::{
    timing::{core_lock_error, unix_time},
    Core, CoreError, CoreResult, Snapshot, StatusSnapshot,
};
use crate::{
    integrations::backend::Backend,
    killmail::{report_state, ReportState},
    models::Store,
    persistence::storage::{self, LockError},
};
#[cfg(any(feature = "gui", feature = "dev-tools", test))]
use std::sync::atomic::{AtomicBool, Ordering};
use std::{
    marker::PhantomData,
    path::PathBuf,
    sync::{Arc, Mutex},
};

#[derive(Clone)]
pub(super) enum Persistence {
    File(PathBuf),
    #[cfg(any(feature = "gui", feature = "dev-tools", test))]
    Memory {
        store: Arc<Mutex<Store>>,
        service_active: Arc<AtomicBool>,
        #[cfg(test)]
        persist_budget: Option<Arc<std::sync::atomic::AtomicIsize>>,
    },
}

pub(crate) struct ServiceGuard {
    inner: ServiceGuardInner,
}

enum ServiceGuardInner {
    File {
        _lock: storage::ServiceLock,
    },
    #[cfg(any(feature = "gui", feature = "dev-tools", test))]
    Memory(Arc<AtomicBool>),
}

impl Drop for ServiceGuard {
    fn drop(&mut self) {
        match &self.inner {
            ServiceGuardInner::File { .. } => {}
            #[cfg(any(feature = "gui", feature = "dev-tools", test))]
            ServiceGuardInner::Memory(active) => active.store(false, Ordering::Release),
        }
    }
}

pub(super) struct LockedStore<'a> {
    inner: LockedStoreInner<'a>,
}

enum LockedStoreInner<'a> {
    File {
        _lock: storage::OperationLock,
        path: PathBuf,
        store: Box<Store>,
        _lifetime: PhantomData<&'a ()>,
    },
    #[cfg(any(feature = "gui", feature = "dev-tools", test))]
    Memory {
        store: std::sync::MutexGuard<'a, Store>,
        #[cfg(test)]
        persist_budget: Option<Arc<std::sync::atomic::AtomicIsize>>,
    },
}

impl LockedStore<'_> {
    pub(super) fn store(&self) -> &Store {
        match &self.inner {
            LockedStoreInner::File { store, .. } => store,
            #[cfg(any(feature = "gui", feature = "dev-tools", test))]
            LockedStoreInner::Memory { store, .. } => store,
        }
    }

    pub(super) fn store_mut(&mut self) -> &mut Store {
        match &mut self.inner {
            LockedStoreInner::File { store, .. } => store,
            #[cfg(any(feature = "gui", feature = "dev-tools", test))]
            LockedStoreInner::Memory { store, .. } => store,
        }
    }

    pub(super) fn persist(&self) -> CoreResult<()> {
        match &self.inner {
            LockedStoreInner::File { path, store, .. } => {
                let data = serde_json::to_vec_pretty(store)
                    .map_err(|error| CoreError::Persistence(error.to_string()))?;
                storage::persist_to_path(path, &data).map_err(|error| {
                    CoreError::Persistence(format!(
                        "could not atomically write {}: {error}",
                        path.display()
                    ))
                })
            }
            #[cfg(any(feature = "gui", feature = "dev-tools", test))]
            LockedStoreInner::Memory {
                #[cfg(test)]
                persist_budget,
                ..
            } => {
                #[cfg(test)]
                if let Some(budget) = persist_budget {
                    if budget.fetch_sub(1, Ordering::AcqRel) <= 0 {
                        return Err(CoreError::Persistence(
                            "injected persistence failure".into(),
                        ));
                    }
                }
                Ok(())
            }
        }
    }
}
impl Core {
    pub(crate) fn live(backend: Arc<dyn Backend>) -> CoreResult<Self> {
        let path = storage::store_path().map_err(CoreError::Persistence)?;
        Ok(Self::at_path(backend, path))
    }

    pub(crate) fn at_path(backend: Arc<dyn Backend>, path: PathBuf) -> Self {
        Self {
            backend,
            persistence: Persistence::File(path),
            events: None,
            session_reports: Arc::new(Mutex::new(Vec::new())),
        }
    }

    #[cfg(any(feature = "gui", feature = "dev-tools", test))]
    pub(crate) fn in_memory(backend: Arc<dyn Backend>, store: Store) -> Self {
        Self {
            backend,
            persistence: Persistence::Memory {
                store: Arc::new(Mutex::new(store)),
                service_active: Arc::new(AtomicBool::new(false)),
                #[cfg(test)]
                persist_budget: None,
            },
            events: None,
            session_reports: Arc::new(Mutex::new(Vec::new())),
        }
    }

    #[cfg(test)]
    pub(crate) fn in_memory_with_persist_budget(
        backend: Arc<dyn Backend>,
        store: Store,
        successful_persists: isize,
    ) -> Self {
        Self {
            backend,
            persistence: Persistence::Memory {
                store: Arc::new(Mutex::new(store)),
                service_active: Arc::new(AtomicBool::new(false)),
                persist_budget: Some(Arc::new(std::sync::atomic::AtomicIsize::new(
                    successful_persists,
                ))),
            },
            events: None,
            session_reports: Arc::new(Mutex::new(Vec::new())),
        }
    }

    #[cfg(any(test, feature = "dev-tools"))]
    pub(crate) fn initialize(&self, initial: Store) -> CoreResult<()> {
        let Persistence::File(path) = &self.persistence else {
            return Ok(());
        };
        let _lock = storage::try_operation_lock_for(path).map_err(core_lock_error)?;
        if storage::store_exists_or_recoverable(path) {
            return Ok(());
        }
        let data = serde_json::to_vec_pretty(&initial)
            .map_err(|error| CoreError::Persistence(error.to_string()))?;
        storage::persist_to_path(path, &data).map_err(|error| {
            CoreError::Persistence(format!("could not initialize {}: {error}", path.display()))
        })
    }

    pub(crate) fn snapshot(&self) -> CoreResult<Snapshot> {
        let store = match &self.persistence {
            Persistence::File(path) => {
                let _lock = storage::try_snapshot_lock_for(path).map_err(core_lock_error)?;
                storage::load_from_path(path).map_err(CoreError::Persistence)?
            }
            #[cfg(any(feature = "gui", feature = "dev-tools", test))]
            Persistence::Memory { store, .. } => {
                store.try_lock().map_err(|_| CoreError::Busy)?.clone()
            }
        };
        let status = self.status_for(&store);
        Ok(Snapshot { store, status })
    }

    pub(crate) fn try_service_guard(&self) -> CoreResult<super::ServiceGuard> {
        let inner = match &self.persistence {
            Persistence::File(path) => ServiceGuardInner::File {
                _lock: storage::try_service_lock_for(path).map_err(core_lock_error)?,
            },
            #[cfg(any(feature = "gui", feature = "dev-tools", test))]
            Persistence::Memory { service_active, .. } => {
                service_active
                    .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                    .map_err(|_| CoreError::Busy)?;
                ServiceGuardInner::Memory(Arc::clone(service_active))
            }
        };
        Ok(ServiceGuard { inner })
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
            Persistence::File(path) => {
                let lock = storage::try_operation_lock_for(path).map_err(core_lock_error)?;
                let store = storage::load_from_path(path).map_err(CoreError::Persistence)?;
                Ok(LockedStore {
                    inner: LockedStoreInner::File {
                        _lock: lock,
                        path: path.clone(),
                        store: Box::new(store),
                        _lifetime: PhantomData,
                    },
                })
            }
            #[cfg(any(feature = "gui", feature = "dev-tools", test))]
            Persistence::Memory {
                store,
                #[cfg(test)]
                persist_budget,
                ..
            } => store
                .try_lock()
                .map(|store| LockedStore {
                    inner: LockedStoreInner::Memory {
                        store,
                        #[cfg(test)]
                        persist_budget: persist_budget.clone(),
                    },
                })
                .map_err(|_| CoreError::Busy),
        }
    }

    fn status_for(&self, store: &Store) -> StatusSnapshot {
        let now = unix_time();
        let reported = store
            .zkill_cache
            .values()
            .filter(|entry| entry.reported)
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
            Persistence::File(path) => match storage::try_service_lock_for(path) {
                Ok(lock) => {
                    drop(lock);
                    false
                }
                Err(LockError::Busy) => true,
                Err(LockError::Io(_)) => false,
            },
            #[cfg(any(feature = "gui", feature = "dev-tools", test))]
            Persistence::Memory { service_active, .. } => service_active.load(Ordering::Acquire),
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
        let path = directory.join("ekmp.json");
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
