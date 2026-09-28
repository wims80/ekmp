use super::{store::LockedStore, CoreError, CoreResult};
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
    time::{Duration, SystemTime, UNIX_EPOCH},
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
}

impl Default for Cancellation {
    fn default() -> Self {
        Self::new()
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
        cancellation_aware_wait(Duration::from_millis(delay_ms), cancelled)?;
    }
    check_cancelled(cancelled)?;
    let spacing_ms = u64::try_from(spacing.as_millis()).unwrap_or(u64::MAX);
    locked.store_mut().zkill_next_request_at_ms = unix_time_millis().saturating_add(spacing_ms);
    // Reserve durably before sending so a crash or competing process cannot bypass spacing.
    locked.persist()
}

pub(super) fn unix_time() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub(super) fn unix_time_millis() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(u64::MAX)
}

pub(super) fn check_cancelled(cancelled: &Cancellation) -> CoreResult<()> {
    if cancelled.is_cancelled() {
        Err(CoreError::Cancelled)
    } else {
        Ok(())
    }
}

fn cancellation_aware_wait(duration: Duration, cancelled: &Cancellation) -> CoreResult<()> {
    let deadline = std::time::Instant::now() + duration;
    loop {
        check_cancelled(cancelled)?;
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return Ok(());
        }
        thread::sleep(remaining.min(Duration::from_millis(100)));
    }
}
