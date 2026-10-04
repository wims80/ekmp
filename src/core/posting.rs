use super::{
    status::prune_reported,
    timing::{
        active_api_cooldown, check_cancelled, merge_api_cooldowns, reserve_zkill_request, unix_time,
    },
    Cancellation, Core, CoreError, CoreResult, PostBatchResult, PostMode, PostResult,
    PostResultStatus, PostSelection, PreparedPost, SessionReport,
};
use crate::{
    killmail::{is_bulk_candidate, is_eligible_for_bulk_posting, report_state, ReportState},
    models::{Killmail, Store, ZkillStatus},
};

const MAX_SESSION_REPORTS: usize = 200;

impl Core {
    pub(crate) fn session_reports(&self) -> Vec<SessionReport> {
        self.session_reports
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
    pub(crate) fn prepare_post(
        &self,
        selection: PostSelection,
        cancelled: &Cancellation,
    ) -> CoreResult<PreparedPost> {
        let mut locked = self.begin_operation()?;
        check_cancelled(cancelled)?;
        let now = unix_time();
        let needs_status = match selection {
            PostSelection::One { id, .. } => locked.store().cached_killmails.iter().any(|mail| {
                mail.id == id && report_state(locked.store(), id, now) == ReportState::Unknown
            }),
            PostSelection::All => locked.store().cached_killmails.iter().any(|mail| {
                is_eligible_for_bulk_posting(locked.store(), mail)
                    && report_state(locked.store(), mail.id, now) == ReportState::Unknown
            }),
        };
        if needs_status {
            self.progress("Checking zKillboard status before confirmation");
            let _ = self.refresh_statuses_locked(&mut locked, cancelled)?;
            locked.persist()?;
            self.changed();
        }
        let now = unix_time();
        let (ids, mode) = match selection {
            PostSelection::One { id, post_anyway } => {
                let mail = locked
                    .store()
                    .cached_killmails
                    .iter()
                    .find(|mail| mail.id == id)
                    .ok_or_else(|| {
                        CoreError::Operational(format!("killmail {id} is not in the cache"))
                    })?;
                match report_state(locked.store(), id, now) {
                    ReportState::Reported => {
                        return Err(CoreError::Operational(format!(
                            "killmail {id} is already reported"
                        )))
                    }
                    ReportState::Unknown => {
                        return Err(CoreError::Operational(format!(
                            "killmail {id} could not be confirmed as unreported"
                        )))
                    }
                    ReportState::Unreported => {}
                }
                let protected = !is_eligible_for_bulk_posting(locked.store(), mail);
                if protected && !post_anyway {
                    return Err(CoreError::Operational(format!(
                        "killmail {id} has a protected victim; use the individual post-anyway action"
                    )));
                }
                (
                    vec![id],
                    if protected {
                        PostMode::ProtectedIndividual
                    } else {
                        PostMode::Individual
                    },
                )
            }
            PostSelection::All => (
                locked
                    .store()
                    .cached_killmails
                    .iter()
                    .filter(|mail| is_bulk_candidate(locked.store(), mail, now))
                    .map(|mail| mail.id)
                    .collect(),
                PostMode::Bulk,
            ),
        };
        Ok(PreparedPost { ids, mode })
    }

    pub(crate) fn post(
        &self,
        prepared: &PreparedPost,
        cancelled: &Cancellation,
    ) -> CoreResult<PostBatchResult> {
        let mut locked = self.begin_operation()?;
        let mut batch = PostBatchResult {
            results: Vec::new(),
            cancelled: false,
        };
        for id in prepared.ids.iter().copied() {
            if cancelled.is_cancelled() {
                batch.cancelled = true;
                break;
            }
            let now = unix_time();
            if let Some(until) = active_api_cooldown(locked.store(), "zkillboard") {
                batch
                    .results
                    .push(skipped(id, format!("API cooldown is active until {until}")));
                break;
            }
            let Some(mail) = locked
                .store()
                .cached_killmails
                .iter()
                .find(|mail| mail.id == id)
                .cloned()
            else {
                batch
                    .results
                    .push(skipped(id, "killmail is no longer in the cache"));
                continue;
            };
            if let Err(reason) = post_permission(locked.store(), &mail, prepared, now) {
                batch.results.push(skipped(id, reason));
                continue;
            }

            reserve_zkill_request(&mut locked, self.backend.request_spacing(), cancelled)?;
            let dispatch_at = unix_time();
            if post_permission(locked.store(), &mail, prepared, dispatch_at).is_err() {
                batch.results.push(skipped(
                    id,
                    "killmail eligibility expired while waiting for request spacing",
                ));
                continue;
            }
            self.progress(format!("Submitting killmail {id} to zKillboard"));
            locked.store_mut().zkill_status.insert(
                id,
                ZkillStatus::PostAttempted {
                    attempted_at: dispatch_at,
                },
            );
            // A durable attempt marker is required before the consequential request.
            locked.persist()?;
            if cancelled.is_cancelled() {
                batch.cancelled = true;
                break;
            }

            let outcome = self.backend.post(&mail);
            merge_api_cooldowns(locked.store_mut(), self.backend.take_api_cooldowns());
            match outcome {
                Ok(outcome) => {
                    locked
                        .store_mut()
                        .zkill_status
                        .insert(id, ZkillStatus::Reported);
                    prune_reported(locked.store_mut());
                    // The authoritative reported ID must be durable before another POST.
                    locked.persist()?;
                    let status = if outcome.new {
                        PostResultStatus::Submitted
                    } else {
                        PostResultStatus::AlreadyPresent
                    };
                    batch.results.push(PostResult {
                        killmail_id: id,
                        status,
                        url: Some(outcome.url.clone()),
                        message: None,
                    });
                    self.push_session_report(SessionReport {
                        killmail_id: id,
                        url: outcome.url,
                        new: outcome.new,
                    });
                    self.changed();
                }
                Err(error) => {
                    // The attempt marker and invalidated negative evidence remain persisted.
                    locked.persist()?;
                    batch.results.push(PostResult {
                        killmail_id: id,
                        status: PostResultStatus::Failed,
                        url: None,
                        message: Some(error.to_string()),
                    });
                }
            }
        }
        Ok(batch)
    }

    fn push_session_report(&self, report: SessionReport) {
        let mut reports = self
            .session_reports
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        reports.retain(|old| old.killmail_id != report.killmail_id);
        reports.push(report);
        if reports.len() > MAX_SESSION_REPORTS {
            let drain = reports.len() - MAX_SESSION_REPORTS;
            reports.drain(..drain);
        }
    }
}
/// Revalidates a confirmed post against the current store immediately before submission.
fn post_permission(
    store: &Store,
    mail: &Killmail,
    prepared: &PreparedPost,
    now: u64,
) -> Result<(), &'static str> {
    let protected = !is_eligible_for_bulk_posting(store, mail);
    match report_state(store, mail.id, now) {
        ReportState::Reported => Err("killmail is already reported"),
        ReportState::Unknown => Err("killmail status is no longer confirmed"),
        ReportState::Unreported => match prepared.mode {
            PostMode::Bulk | PostMode::Individual if protected => {
                Err("victim protection changed after confirmation")
            }
            PostMode::Bulk | PostMode::Individual => Ok(()),
            PostMode::ProtectedIndividual if prepared.ids.len() == 1 => Ok(()),
            PostMode::ProtectedIndividual => Err("killmail is no longer eligible"),
        },
    }
}

fn skipped(id: u64, message: impl Into<String>) -> PostResult {
    PostResult {
        killmail_id: id,
        status: PostResultStatus::Skipped,
        url: None,
        message: Some(message.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::test_support::*;
    use std::sync::{atomic::Ordering, Arc};

    #[test]
    fn protection_change_after_confirmation_is_revalidated() {
        let backend = Arc::new(TestBackend::new());
        let core = Core::in_memory(backend.clone(), postable_store());
        let cancellation = Cancellation::new();
        let prepared = core
            .prepare_post(
                PostSelection::One {
                    id: 42,
                    post_anyway: false,
                },
                &cancellation,
            )
            .unwrap();

        core.set_killmail_protection(42, true).unwrap();
        let result = core.post(&prepared, &cancellation).unwrap();

        assert_eq!(result.results[0].status, PostResultStatus::Skipped);
        assert_eq!(backend.posts.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn individual_override_posts_only_the_confirmed_protected_killmail() {
        let backend = Arc::new(TestBackend::new());
        let core = Core::in_memory(backend.clone(), postable_store());
        core.set_killmail_protection(42, true).unwrap();
        let cancellation = Cancellation::new();
        let prepared = core
            .prepare_post(
                PostSelection::One {
                    id: 42,
                    post_anyway: true,
                },
                &cancellation,
            )
            .unwrap();

        let result = core.post(&prepared, &cancellation).unwrap();

        assert_eq!(result.results[0].status, PostResultStatus::Submitted);
        assert_eq!(backend.posts.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn persistence_failure_before_first_post_sends_no_request() {
        let backend = Arc::new(TestBackend::new());
        let core = Core::in_memory_with_persist_budget(backend.clone(), postable_store(), 0);
        let prepared = PreparedPost {
            ids: vec![42],
            mode: PostMode::Individual,
        };

        let error = core.post(&prepared, &Cancellation::new()).unwrap_err();

        assert!(matches!(error, CoreError::Persistence(_)));
        assert_eq!(backend.posts.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn eligibility_expiring_during_request_spacing_stops_the_post() {
        let backend = Arc::new(TestBackend::new());
        backend.spacing_ms.store(1_000, Ordering::Relaxed);
        let now = unix_time();
        let mut store = postable_store();
        store.zkill_status.insert(
            42,
            ZkillStatus::Unreported {
                valid_until: now + 1,
            },
        );
        store.zkill_next_request_at_ms = now.saturating_add(1).saturating_mul(1_000) + 25;
        let core = Core::in_memory(backend.clone(), store);
        let prepared = core
            .prepare_post(
                PostSelection::One {
                    id: 42,
                    post_anyway: false,
                },
                &Cancellation::new(),
            )
            .unwrap();

        let result = core.post(&prepared, &Cancellation::new()).unwrap();

        assert_eq!(result.results[0].status, PostResultStatus::Skipped);
        assert_eq!(backend.posts.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn persistence_failure_after_first_post_stops_the_batch() {
        let backend = Arc::new(TestBackend::new());
        let mut store = postable_store();
        let now = unix_time();
        store.cached_killmails.push(mail(43));
        store.zkill_status.insert(
            43,
            ZkillStatus::Unreported {
                valid_until: now + 3_600,
            },
        );
        let core = Core::in_memory_with_persist_budget(backend.clone(), store, 1);
        let prepared = PreparedPost {
            ids: vec![42, 43],
            mode: PostMode::Bulk,
        };

        let error = core.post(&prepared, &Cancellation::new()).unwrap_err();

        assert!(matches!(error, CoreError::Persistence(_)));
        assert_eq!(backend.posts.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn cancellation_after_in_flight_post_saves_it_and_stops_the_next_request() {
        let backend = Arc::new(TestBackend::new());
        let cancellation = Cancellation::new();
        *backend
            .cancel_after_post
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(cancellation.clone());
        let mut store = postable_store();
        let now = unix_time();
        store.cached_killmails.push(mail(43));
        store.zkill_status.insert(
            43,
            ZkillStatus::Unreported {
                valid_until: now + 3_600,
            },
        );
        let core = Core::in_memory(backend.clone(), store);

        let result = core
            .post(
                &PreparedPost {
                    ids: vec![42, 43],
                    mode: PostMode::Bulk,
                },
                &cancellation,
            )
            .unwrap();

        assert!(result.cancelled);
        assert_eq!(result.results.len(), 1);
        assert_eq!(result.results[0].status, PostResultStatus::Submitted);
        assert_eq!(backend.posts.load(Ordering::Relaxed), 1);
        assert_eq!(
            core.snapshot().unwrap().store.zkill_status[&42],
            ZkillStatus::Reported
        );
    }
}
