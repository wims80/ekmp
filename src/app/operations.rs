use super::{ActiveOperation, App, Completion, OperationUpdate, SNAPSHOT_POLL_INTERVAL};
use crate::{
    clock::unix_time,
    core::{
        Cancellation, Core, CoreError, CoreEvent, CoreResult, PostResultStatus, PostSelection,
        PreparedPost,
    },
    models::ProtectedVictimKind,
};
use std::{
    sync::mpsc::{self, Sender},
    thread,
    time::Instant,
};

/// What a background operation may use besides the core.
struct OperationContext {
    cancellation: Cancellation,
    updates: Sender<OperationUpdate>,
}

/// Boxes the GUI update applied once an operation succeeds.
fn then(update: impl FnOnce(&mut App) + Send + 'static) -> CoreResult<Completion> {
    Ok(Box::new(update))
}

impl App {
    pub(super) fn migrate_refresh_tokens(&mut self) {
        self.start_operation(
            "Securing character credentials",
            "Moving refresh tokens to the system credential store...",
            |core, _| {
                let result = core.migrate_refresh_tokens()?;
                then(move |app| {
                    app.log(format!(
                        "Secured {} refresh token{} in the system credential store",
                        result.migrated,
                        if result.migrated == 1 { "" } else { "s" }
                    ));
                    for warning in result.warnings {
                        app.log(warning);
                    }
                    app.refresh_after_snapshot = true;
                })
            },
        );
    }

    pub(super) fn begin_auth(&mut self) {
        self.start_operation(
            "Waiting for EVE authorization",
            "Authorize the character in your browser...",
            |core, context| {
                let updates = context.updates.clone();
                let character = core.authenticate(&context.cancellation, true, &move |url| {
                    let _ = updates.send(OperationUpdate::AuthorizationUrl(url.to_owned()));
                })?;
                then(move |app| {
                    app.log(format!("Character {} authenticated", character.name));
                    app.refresh_after_snapshot = true;
                })
            },
        );
        if let Some(active) = &mut self.active_operation {
            active.cancel_label = Some("Cancel connection");
        }
    }

    pub(super) fn cancel_operation(&mut self) {
        let Some(active) = &mut self.active_operation else {
            return;
        };
        if active.cancel_label.is_none() || active.cancellation.is_cancelled() {
            return;
        }
        active.cancellation.cancel();
        self.authorization_url = None;
        self.log("Cancelling...");
    }

    pub(super) fn remove_character(&mut self, id: u64) {
        self.start_operation(
            "Disconnecting character",
            format!("Removing character {id}..."),
            move |core, _| {
                let result = core.remove_character(id)?;
                then(move |app| {
                    app.log(format!(
                        "Removed {}, its refresh token, and {} cached killmail(s)",
                        result.name, result.removed_killmails
                    ));
                    if let Some(warning) = result.credential_warning {
                        app.log(format!("Could not delete the system credential: {warning}"));
                    }
                })
            },
        );
    }

    pub(super) fn refresh_killmails(&mut self) {
        if self.store.characters.is_empty() {
            self.log("Authenticate at least one character first");
            return;
        }
        self.start_operation(
            "Loading recent killmails",
            "Loading recent killmails...",
            |core, context| {
                let result = core.refresh(&context.cancellation)?;
                then(move |app| {
                    for message in result.messages {
                        app.log(message);
                    }
                    if result.idle {
                        app.log("Refresh is idle because no characters are authenticated");
                    } else if let Some(until) = result.deferred_until {
                        app.log(format!(
                            "Refresh deferred until {}",
                            super::relative_time_label(until, unix_time())
                        ));
                    } else {
                        app.log(format!(
                            "Refresh complete: {} cached killmails, {} reported found",
                            result.fetched_killmails, result.reported_found
                        ));
                    }
                })
            },
        );
    }

    pub(super) fn begin_add_protected_victim(&mut self) {
        let query = self.new_protected_victim_query.trim().to_owned();
        if query.is_empty() {
            self.log("Enter an exact EVE character or corporation name, or a numeric EVE ID");
            return;
        }
        if query.parse::<u64>().is_ok_and(|id| id == 0) {
            self.log("Enter a positive numeric EVE ID");
            return;
        }
        let kind = self.new_protected_victim_kind;
        self.start_operation(
            "Adding protected victim",
            format!("Resolving protected victim {query}..."),
            move |core, _| {
                let victim = core.add_protected_victim(kind, &query)?;
                then(move |app| {
                    app.new_protected_victim_query.clear();
                    app.log(format!(
                        "Added protected {}: {} ({})",
                        kind.label(),
                        victim.name,
                        victim.id
                    ));
                })
            },
        );
    }

    pub(super) fn remove_protected_victim(&mut self, kind: ProtectedVictimKind, id: u64) {
        self.start_operation(
            "Removing protected victim",
            format!("Removing protected victim {id}..."),
            move |core, _| {
                let removed = core.remove_protected_victim(kind, id)?;
                let label = kind.label();
                then(move |app| {
                    app.log(if removed {
                        format!("Removed protected {label} with EVE ID {id}")
                    } else {
                        format!("Protected {label} {id} was already absent")
                    });
                })
            },
        );
    }

    pub(super) fn set_show_protected(&mut self, show: bool) {
        self.start_operation(
            "Saving protection settings",
            "Saving protected-killmail visibility...",
            move |core, _| {
                core.set_show_protected(show)?;
                then(move |app| {
                    app.log(if show {
                        "Protected killmails are visible"
                    } else {
                        "Protected killmails are hidden"
                    });
                })
            },
        );
    }

    pub(super) fn set_killmail_protection(&mut self, id: u64, protected: bool) {
        self.start_operation(
            "Saving protection settings",
            if protected {
                format!("Protecting killmail {id}...")
            } else {
                format!("Removing protection from killmail {id}...")
            },
            move |core, _| {
                core.set_killmail_protection(id, protected)?;
                then(move |app| {
                    app.log(if protected {
                        format!("Flagged killmail {id} for protection")
                    } else {
                        format!("Removed protection flag from killmail {id}")
                    });
                })
            },
        );
    }

    pub(super) fn request_bulk_post(&mut self) {
        self.prepare_post(PostSelection::All);
    }

    pub(super) fn request_individual_post(&mut self, id: u64, post_anyway: bool) {
        self.prepare_post(PostSelection::One { id, post_anyway });
    }

    /// Checks eligibility only; posting requires a separate confirmation of `pending_post`.
    fn prepare_post(&mut self, selection: PostSelection) {
        self.start_operation(
            "Checking posting eligibility",
            "Checking current zKillboard status and posting eligibility...",
            move |core, context| {
                let prepared = core.prepare_post(selection, &context.cancellation)?;
                then(move |app| {
                    if prepared.ids.is_empty() {
                        app.log("There are no eligible confirmed unreported killmails to submit");
                    } else {
                        app.pending_post = Some(prepared);
                    }
                })
            },
        );
    }

    pub(super) fn post_prepared(&mut self, prepared: PreparedPost) {
        self.start_operation(
            "Posting confirmed killmails",
            format!("Posting {} confirmed killmail(s)...", prepared.ids.len()),
            move |core, context| {
                let result = core.post(&prepared, &context.cancellation)?;
                then(move |app| {
                    let submitted = result
                        .results
                        .iter()
                        .filter(|entry| {
                            matches!(
                                entry.status,
                                PostResultStatus::Submitted | PostResultStatus::AlreadyPresent
                            )
                        })
                        .count();
                    for entry in &result.results {
                        if let Some(message) = &entry.message {
                            app.log(format!("Killmail {}: {message}", entry.killmail_id));
                        }
                    }
                    app.log(format!(
                        "Submission complete: {submitted} requests completed, {} failed or skipped out of {}",
                        result.results.len().saturating_sub(submitted),
                        result.results.len()
                    ));
                })
            },
        );
    }

    fn start_operation(
        &mut self,
        label: &'static str,
        status: impl Into<String>,
        work: impl FnOnce(Core, OperationContext) -> CoreResult<Completion> + Send + 'static,
    ) {
        if self.is_busy() {
            self.log("Another operation is already in progress");
            return;
        }
        if self.persistence_blocked.is_some() {
            self.log("The operation cannot start because local state could not be loaded safely");
            return;
        }
        let (updates_tx, updates) = mpsc::channel();
        let cancellation = Cancellation::new();
        let context = OperationContext {
            cancellation: cancellation.clone(),
            updates: updates_tx.clone(),
        };
        let core = self.core.clone();
        self.spawn_job(move || {
            let _ = updates_tx.send(OperationUpdate::Completed(work(core, context)));
        });
        self.log(status);
        self.active_operation = Some(ActiveOperation {
            label,
            updates,
            cancellation,
            cancel_label: None,
        });
    }

    fn spawn_job(&self, job: impl FnOnce() + Send + 'static) {
        if self.run_jobs_inline {
            job();
        } else {
            thread::spawn(job);
        }
    }

    pub(super) fn poll_core(&mut self) {
        let events = self.core_events.try_iter().collect::<Vec<_>>();
        for event in events {
            match event {
                CoreEvent::Progress(message) => self.log(message),
                CoreEvent::SnapshotChanged => self.request_snapshot_reload(),
            }
        }

        let updates = self
            .active_operation
            .as_ref()
            .map(|active| active.updates.try_iter().collect::<Vec<_>>())
            .unwrap_or_default();
        for update in updates {
            match update {
                OperationUpdate::AuthorizationUrl(url) => self.authorization_url = Some(url),
                OperationUpdate::Completed(result) => self.finish_operation(result),
            }
        }

        self.poll_snapshot_reload();
        if self.last_snapshot_poll.elapsed() >= SNAPSHOT_POLL_INTERVAL {
            self.request_snapshot_reload();
        }
    }

    fn finish_operation(&mut self, result: CoreResult<Completion>) {
        self.active_operation = None;
        self.authorization_url = None;
        match result {
            Ok(completion) => completion(self),
            Err(CoreError::Busy) => self.log("Another ekmp operation is already in progress"),
            Err(CoreError::Cancelled) => self.log("Operation cancelled"),
            Err(error) => self.log(format!("Operation failed: {error}")),
        }
        self.session_reports = self.core.session_reports();
        self.request_snapshot_reload();
    }

    fn request_snapshot_reload(&mut self) {
        if self.snapshot_result.is_some() {
            return;
        }
        self.last_snapshot_poll = Instant::now();
        let core = self.core.clone();
        let (tx, rx) = mpsc::channel();
        self.spawn_job(move || {
            let _ = tx.send(core.snapshot());
        });
        self.snapshot_result = Some(rx);
    }

    fn poll_snapshot_reload(&mut self) {
        let result = self
            .snapshot_result
            .as_ref()
            .and_then(|receiver| receiver.try_recv().ok());
        let Some(result) = result else {
            return;
        };
        self.snapshot_result = None;
        match result {
            Ok(snapshot) => {
                let cached_ids = snapshot
                    .store
                    .cached_killmails
                    .iter()
                    .map(|mail| mail.id)
                    .collect::<std::collections::HashSet<_>>();
                self.expanded_killmail_ids
                    .retain(|id| cached_ids.contains(id));
                self.store = snapshot.store;
                self.core_status = snapshot.status;
                if self.refresh_after_snapshot && !self.store.characters.is_empty() {
                    self.refresh_after_snapshot = false;
                    self.refresh_killmails();
                }
            }
            Err(CoreError::Busy) => {}
            Err(error) => self.log(format!("Could not reload local state: {error}")),
        }
    }
}
