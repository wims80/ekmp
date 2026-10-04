use super::{store::LockedStore, Core, CoreError, CoreResult};
pub(super) use crate::clock::{unix_time, unix_time_millis};
use crate::{
    models::{ApiCooldown, Store},
    persistence::storage::LockError,
};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

#[derive(Clone, Debug)]
pub(crate) struct Cancellation(Arc<AtomicBool>);

impl Cancellation {
    pub(crate) fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }

    pub(crate) fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    pub(crate) fn as_atomic(&self) -> &AtomicBool {
        &self.0
    }

    /// Sleeps for `duration` unless cancelled first; returns whether it was cancelled.
    pub(crate) fn wait(&self, duration: Duration) -> bool {
        let deadline = std::time::Instant::now() + duration;
        loop {
            if self.is_cancelled() {
                return true;
            }
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return false;
            }
            thread::sleep(remaining.min(Duration::from_millis(100)));
        }
    }
}

impl Default for Cancellation {
    fn default() -> Self {
        Self::new()
    }
}

impl Core {
    /// Persists the backend's observed cooldowns into `store` and reports its warnings.
    pub(super) fn absorb_api_observations(&self, store: &mut Store) {
        merge_api_cooldowns(store, self.backend.take_api_cooldowns());
        for warning in self.backend.take_api_warnings() {
            self.progress(warning);
        }
    }
}

pub(super) fn merge_api_cooldowns(store: &mut Store, new_cooldowns: Vec<ApiCooldown>) {
    let now = unix_time();
    store.api_cooldowns.retain(|cooldown| cooldown.until > now);
    for cooldown in new_cooldowns {
        if cooldown.until <= now {
            continue;
        }
        if let Some(existing) = store
            .api_cooldowns
            .iter_mut()
            .find(|existing| existing.source == cooldown.source && existing.scope == cooldown.scope)
        {
            if cooldown.until >= existing.until {
                *existing = cooldown;
            }
        } else {
            store.api_cooldowns.push(cooldown);
        }
    }
}

pub(super) fn active_api_cooldown(store: &Store, source: &str) -> Option<u64> {
    let now = unix_time();
    store
        .api_cooldowns
        .iter()
        .filter(|cooldown| cooldown.source.eq_ignore_ascii_case(source) && cooldown.until > now)
        .map(|cooldown| cooldown.until)
        .max()
}

pub(super) fn core_lock_error(error: LockError) -> CoreError {
    match error {
        LockError::Busy => CoreError::Busy,
        LockError::Io(error) => CoreError::Persistence(error),
    }
}

pub(super) fn reserve_zkill_request(
    locked: &mut LockedStore<'_>,
    spacing: Duration,
    cancelled: &Cancellation,
) -> CoreResult<()> {
    if spacing.is_zero() {
        return check_cancelled(cancelled);
    }
    let now_ms = unix_time_millis();
    let delay_ms = locked
        .store()
        .zkill_next_request_at_ms
        .saturating_sub(now_ms);
    if delay_ms > 0 {
        cancelled.wait(Duration::from_millis(delay_ms));
    }
    check_cancelled(cancelled)?;
    let spacing_ms = u64::try_from(spacing.as_millis()).unwrap_or(u64::MAX);
    locked.store_mut().zkill_next_request_at_ms = unix_time_millis().saturating_add(spacing_ms);
    // Reserve durably before sending so a crash or competing process cannot bypass spacing.
    locked.persist()
}

pub(super) fn check_cancelled(cancelled: &Cancellation) -> CoreResult<()> {
    if cancelled.is_cancelled() {
        Err(CoreError::Cancelled)
    } else {
        Ok(())
    }
}
