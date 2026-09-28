use super::{
    status::prune_reported,
    store::LockedStore,
    timing::{check_cancelled, merge_api_cooldowns, unix_time},
    Cancellation, Core, CoreError, CoreResult, RefreshResult,
};
use crate::models::{Character, Store};
use std::{collections::HashSet, time::Duration};

impl Core {
    pub(crate) fn next_refresh_delay(
        &self,
        interval_override: Option<Duration>,
    ) -> CoreResult<Option<Duration>> {
        let snapshot = self.snapshot()?;
        if snapshot.store.characters.is_empty() {
            return Ok(None);
        }
        let interval = interval_override
            .map(|duration| duration.as_secs())
            .unwrap_or(snapshot.store.refresh_interval_secs);
        let next = refresh_not_before(&snapshot.store, interval);
        Ok(Some(Duration::from_secs(next.saturating_sub(unix_time()))))
    }

    pub(crate) fn refresh_due(
        &self,
        interval_override: Option<Duration>,
        cancelled: &Cancellation,
    ) -> CoreResult<RefreshResult> {
        let now = unix_time();
        let snapshot = self.snapshot()?;
        if snapshot.store.characters.is_empty() {
            return Ok(RefreshResult::idle());
        }
        let interval = interval_override
            .map(|duration| duration.as_secs())
            .unwrap_or(snapshot.store.refresh_interval_secs);
        let next = refresh_not_before(&snapshot.store, interval);
        if next > now {
            let mut result = RefreshResult::idle();
            result.idle = false;
            result.deferred_until = Some(next);
            return Ok(result);
        }
        self.refresh_with_interval(cancelled, interval)
    }

    pub(crate) fn refresh(&self, cancelled: &Cancellation) -> CoreResult<RefreshResult> {
        let interval = self.snapshot()?.store.refresh_interval_secs;
        self.refresh_with_interval(cancelled, interval)
    }

    pub(crate) fn set_refresh_interval(&self, interval: Duration) -> CoreResult<()> {
        let seconds = interval.as_secs();
        if seconds == 0 {
            return Err(CoreError::Operational(
                "refresh interval must be greater than zero".into(),
            ));
        }
        self.simple_mutation(|store| store.refresh_interval_secs = seconds)
    }

    fn refresh_with_interval(
        &self,
        cancelled: &Cancellation,
        interval_secs: u64,
    ) -> CoreResult<RefreshResult> {
        let mut locked = self.begin_operation()?;
        check_cancelled(cancelled)?;
        let started = unix_time();
        let not_before = refresh_not_before(locked.store(), interval_secs);
        if not_before > started {
            return Ok(RefreshResult {
                fetched_killmails: 0,
                reported_found: 0,
                status_checks_incomplete: 0,
                idle: false,
                deferred_until: Some(not_before),
                messages: Vec::new(),
                has_failures: false,
            });
        }
        locked.store_mut().refresh_schedule.last_attempt_at = Some(started);
        locked.persist()?;

        if locked.store().characters.is_empty() {
            let completed = unix_time();
            let schedule = &mut locked.store_mut().refresh_schedule;
            schedule.last_completed_at = Some(completed);
            schedule.next_eligible_at = Some(completed.saturating_add(interval_secs));
            schedule.last_error = None;
            locked.persist()?;
            return Ok(RefreshResult::idle());
        }

        let operation = self.perform_refresh(&mut locked, cancelled);
        merge_api_cooldowns(locked.store_mut(), self.backend.take_api_cooldowns());
        let completed = unix_time();
        let active_cooldown = locked
            .store()
            .api_cooldowns
            .iter()
            .map(|cooldown| cooldown.until)
            .max()
            .unwrap_or(0);
        match operation {
            Ok(result) => {
                if result.has_failures() {
                    let failures = locked
                        .store()
                        .refresh_schedule
                        .consecutive_failures
                        .saturating_add(1);
                    let backoff = refresh_backoff(failures, interval_secs);
                    let schedule = &mut locked.store_mut().refresh_schedule;
                    schedule.last_completed_at = Some(completed);
                    schedule.next_eligible_at =
                        Some(completed.saturating_add(backoff).max(active_cooldown));
                    schedule.consecutive_failures = failures;
                    schedule.last_error = Some(result.messages.join("; "));
                } else {
                    let schedule = &mut locked.store_mut().refresh_schedule;
                    schedule.last_success_at = Some(completed);
                    schedule.last_completed_at = Some(completed);
                    schedule.next_eligible_at =
                        Some(completed.saturating_add(interval_secs).max(active_cooldown));
                    schedule.consecutive_failures = 0;
                    schedule.last_error = None;
                }
                locked.persist()?;
                self.changed();
                Ok(result)
            }
            Err(error @ CoreError::Cancelled) => {
                let schedule = &mut locked.store_mut().refresh_schedule;
                schedule.last_completed_at = Some(completed);
                schedule.next_eligible_at = Some(completed.max(active_cooldown));
                schedule.last_error = Some(error.to_string());
                locked.persist()?;
                Err(error)
            }
            Err(error) => {
                let failures = locked
                    .store()
                    .refresh_schedule
                    .consecutive_failures
                    .saturating_add(1);
                let backoff = refresh_backoff(failures, interval_secs);
                let schedule = &mut locked.store_mut().refresh_schedule;
                schedule.last_completed_at = Some(completed);
                schedule.next_eligible_at =
                    Some(completed.saturating_add(backoff).max(active_cooldown));
                schedule.consecutive_failures = failures;
                schedule.last_error = Some(error.to_string());
                // Preserve completed partial results and the failure schedule. If this fails,
                // report persistence as the primary error.
                locked.persist()?;
                self.changed();
                Err(error)
            }
        }
    }

    fn perform_refresh(
        &self,
        locked: &mut LockedStore<'_>,
        cancelled: &Cancellation,
    ) -> CoreResult<RefreshResult> {
        let mut messages = Vec::new();
        let mut affiliation_failures = 0;
        for character in &mut locked.store_mut().characters {
            check_cancelled(cancelled)?;
            if let Err(error) = self
                .backend
                .refresh_character_affiliation(character, cancelled.as_atomic())
            {
                affiliation_failures += 1;
                messages.push(format!(
                    "Could not refresh corporation for {}: {error}",
                    character.name
                ));
            }
        }
        check_cancelled(cancelled)?;
        self.progress("Loading recent killmails from ESI");
        let reported_ids = locked
            .store()
            .zkill_cache
            .iter()
            .filter_map(|(id, entry)| entry.reported.then_some(*id))
            .collect::<HashSet<_>>();
        let characters = locked.store().characters.clone();
        let cached_killmails = locked.store().cached_killmails.clone();
        let mut callback_persistence_error = None;
        let mut on_character_updated = |updated: &Character| -> Result<(), String> {
            if let Some(character) = locked
                .store_mut()
                .characters
                .iter_mut()
                .find(|character| character.id == updated.id)
            {
                *character = updated.clone();
            }
            match locked.persist() {
                Ok(()) => Ok(()),
                Err(error) => {
                    callback_persistence_error = Some(error);
                    cancelled.cancel();
                    Err("could not persist rotated character credentials".into())
                }
            }
        };
        let outcome = self.backend.load_killmails(
            &characters,
            &cached_killmails,
            &reported_ids,
            cancelled.as_atomic(),
            &mut on_character_updated,
        );
        if let Some(error) = callback_persistence_error {
            return Err(error);
        }
        let outcome = outcome.map_err(|error| {
            if cancelled.is_cancelled() {
                CoreError::Cancelled
            } else {
                CoreError::Operational(format!("could not load recent killmails: {error}"))
            }
        })?;
        let fetched_killmails = outcome.killmails.len();
        let character_failures = outcome.character_failures.len();
        messages.extend(outcome.character_failures.into_iter().map(|failure| {
            format!(
                "Could not refresh killmails for {} ({}): {}",
                failure.character_name, failure.character_id, failure.error
            )
        }));
        locked.store_mut().cached_killmails = outcome.killmails;
        prune_reported(locked.store_mut());
        locked.persist()?;
        self.changed();

        let status = self.refresh_statuses_locked(locked, cancelled)?;
        messages.extend(status.messages);
        Ok(RefreshResult {
            fetched_killmails,
            reported_found: status.reported_found,
            status_checks_incomplete: status.incomplete,
            idle: false,
            deferred_until: None,
            messages,
            has_failures: affiliation_failures > 0
                || character_failures > 0
                || status.incomplete > 0,
        })
    }
}
fn refresh_not_before(store: &Store, interval_secs: u64) -> u64 {
    let scheduled = if store.refresh_schedule.consecutive_failures > 0 {
        store.refresh_schedule.next_eligible_at.unwrap_or(0)
    } else {
        store
            .refresh_schedule
            .last_completed_at
            .map(|completed| completed.saturating_add(interval_secs))
            .unwrap_or(0)
    };
    store
        .api_cooldowns
        .iter()
        .map(|cooldown| cooldown.until)
        .fold(scheduled, u64::max)
}

fn refresh_backoff(failures: u32, interval_secs: u64) -> u64 {
    let exponent = failures.saturating_sub(1).min(10);
    60_u64
        .saturating_mul(1_u64 << exponent)
        .min(interval_secs.max(60))
}
