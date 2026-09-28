use super::{
    unix_time, ActiveOperation, App, Operation, OperationOutcome, OperationUpdate,
    SNAPSHOT_POLL_INTERVAL,
};
use crate::{
    core::{Cancellation, Core, CoreError, CoreEvent, PostSelection},
    models::ProtectedVictimKind,
};
use std::{sync::mpsc, thread, time::Instant};

impl App {
    pub(super) fn migrate_refresh_tokens(&mut self) {
        self.start_operation(
            Operation::MigrateRefreshTokens,
            "Moving refresh tokens to the system credential store...",
            None,
            move |core, _| {
                core.migrate_refresh_tokens()
                    .map(OperationOutcome::RefreshTokensMigrated)
            },
        );
    }

    pub(super) fn begin_auth(&mut self) {
        let cancellation = Cancellation::new();
        let worker_cancellation = cancellation.clone();
        self.start_operation(
            Operation::Authenticate,
            "Authorize the character in your browser...",
            Some(cancellation),
            move |core, updates| {
                let authorization_updates = updates.clone();
                core.authenticate(&worker_cancellation, true, &move |url| {
                    let _ = authorization_updates
                        .send(OperationUpdate::AuthorizationUrl(url.to_owned()));
                })
                .map(OperationOutcome::Authenticated)
            },
        );
    }

    pub(super) fn cancel_authentication(&mut self) {
        let Some(active) = self.active_operation.as_mut() else {
            return;
        };
        if !matches!(active.kind, Operation::Authenticate) || active.cancellation_requested {
            return;
        }
        if let Some(cancellation) = &active.cancellation {
            cancellation.cancel();
        }
        active.cancellation_requested = true;
        self.authorization_url = None;
        self.log("Cancelling character connection...");
    }

    pub(super) fn remove_character(&mut self, id: u64) {
        self.start_operation(
            Operation::RemoveCharacter,
            format!("Removing character {id}..."),
            None,
            move |core, _| {
                core.remove_character(id)
                    .map(OperationOutcome::CharacterRemoved)
            },
        );
    }

    pub(super) fn refresh_killmails(&mut self) {
        if self.store.characters.is_empty() {
            self.log("Authenticate at least one character first");
            return;
        }
        let cancellation = Cancellation::new();
        let worker_cancellation = cancellation.clone();
        self.start_operation(
            Operation::Refresh,
            "Loading recent killmails...",
            Some(cancellation),
            move |core, _| {
                core.refresh(&worker_cancellation)
                    .map(OperationOutcome::Refreshed)
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
            Operation::AddProtectedVictim,
            format!("Resolving protected victim {query}..."),
            None,
            move |core, _| {
                core.add_protected_victim(kind, &query)
                    .map(|victim| OperationOutcome::ProtectedVictimAdded { kind, victim })
            },
        );
    }

    pub(super) fn remove_protected_victim(&mut self, kind: ProtectedVictimKind, id: u64) {
        self.start_operation(
            Operation::RemoveProtectedVictim,
            format!("Removing protected victim {id}..."),
            None,
            move |core, _| {
                core.remove_protected_victim(kind, id)
                    .map(|removed| OperationOutcome::ProtectedVictimRemoved { kind, id, removed })
            },
        );
    }

    pub(super) fn set_show_protected(&mut self, show: bool) {
        self.start_operation(
            Operation::SetShowProtected,
            "Saving protected-killmail visibility...",
            None,
            move |core, _| {
                core.set_show_protected(show)
                    .map(|()| OperationOutcome::ShowProtectedSet(show))
            },
        );
    }

    pub(super) fn set_killmail_protection(&mut self, id: u64, protected: bool) {
        self.start_operation(
            Operation::SetKillmailProtection,
            if protected {
                format!("Protecting killmail {id}...")
            } else {
                format!("Removing protection from killmail {id}...")
            },
            None,
            move |core, _| {
                core.set_killmail_protection(id, protected)
                    .map(|()| OperationOutcome::KillmailProtectionSet { id, protected })
            },
        );
    }

    pub(super) fn request_bulk_post(&mut self) {
        self.prepare_post(PostSelection::All);
    }

    pub(super) fn request_individual_post(&mut self, id: u64, post_anyway: bool) {
        self.prepare_post(PostSelection::One { id, post_anyway });
    }

    fn prepare_post(&mut self, selection: PostSelection) {
        let cancellation = Cancellation::new();
        let worker_cancellation = cancellation.clone();
        self.start_operation(
            Operation::PreparePost,
            "Checking current zKillboard status and posting eligibility...",
            Some(cancellation),
            move |core, _| {
                core.prepare_post(selection, &worker_cancellation)
                    .map(OperationOutcome::PostPrepared)
            },
        );
    }

    pub(super) fn post_prepared(&mut self, prepared: crate::core::PreparedPost) {
        let cancellation = Cancellation::new();
        let worker_cancellation = cancellation.clone();
        self.start_operation(
            Operation::Post,
            format!("Posting {} confirmed killmail(s)...", prepared.ids.len()),
            Some(cancellation),
            move |core, _| {
                core.post(&prepared, &worker_cancellation)
                    .map(OperationOutcome::Posted)
            },
        );
    }

    fn start_operation(
        &mut self,
        kind: Operation,
        status: impl Into<String>,
        cancellation: Option<Cancellation>,
        work: impl FnOnce(Core, mpsc::Sender<OperationUpdate>) -> Result<OperationOutcome, CoreError>
            + Send
            + 'static,
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
        let completion_tx = updates_tx.clone();
        let core = self.core.clone();
        let job = move || {
            let result = work(core, updates_tx);
            let _ = completion_tx.send(OperationUpdate::Completed(result));
        };
        if self.run_jobs_inline {
            job();
        } else {
            thread::spawn(job);
        }
        self.log(status);
        self.active_operation = Some(ActiveOperation {
            kind,
            updates,
            cancellation,
            cancellation_requested: false,
        });
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

    fn finish_operation(&mut self, result: Result<OperationOutcome, CoreError>) {
        self.active_operation = None;
        self.authorization_url = None;
        match result {
            Ok(outcome) => self.handle_outcome(outcome),
            Err(CoreError::Busy) => self.log("Another ekmp operation is already in progress"),
            Err(CoreError::Cancelled) => self.log("Operation cancelled"),
            Err(error) => self.log(format!("Operation failed: {error}")),
        }
        self.session_reports = self.core.session_reports();
        self.request_snapshot_reload();
    }

    fn handle_outcome(&mut self, outcome: OperationOutcome) {
        match outcome {
            OperationOutcome::Authenticated(character) => {
                self.log(format!("Character {} authenticated", character.name));
                self.refresh_after_snapshot = true;
            }
            OperationOutcome::RefreshTokensMigrated(result) => {
                self.log(format!(
                    "Secured {} refresh token{} in the system credential store",
                    result.migrated,
                    if result.migrated == 1 { "" } else { "s" }
                ));
                for warning in result.warnings {
                    self.log(warning);
                }
                self.refresh_after_snapshot = true;
            }
            OperationOutcome::CharacterRemoved(result) => {
                self.log(format!(
                    "Removed {}, its refresh token, and {} cached killmail(s)",
                    result.name, result.removed_killmails
                ));
                if let Some(warning) = result.credential_warning {
                    self.log(format!("Could not delete the system credential: {warning}"));
                }
            }
            OperationOutcome::Refreshed(result) => {
                for message in result.messages {
                    self.log(message);
                }
                if result.idle {
                    self.log("Refresh is idle because no characters are authenticated");
                } else if let Some(until) = result.deferred_until {
                    self.log(format!(
                        "Refresh deferred until {}",
                        super::relative_time_label(until, unix_time())
                    ));
                } else {
                    self.log(format!(
                        "Refresh complete: {} cached killmails, {} reported found",
                        result.fetched_killmails, result.reported_found
                    ));
                }
            }
            OperationOutcome::ProtectedVictimAdded { kind, victim } => {
                self.new_protected_victim_query.clear();
                let label = match kind {
                    ProtectedVictimKind::Character => "character",
                    ProtectedVictimKind::Corporation => "corporation",
                };
                self.log(format!(
                    "Added protected {label}: {} ({})",
                    victim.name, victim.id
                ));
            }
            OperationOutcome::ProtectedVictimRemoved { kind, id, removed } => {
                let label = match kind {
                    ProtectedVictimKind::Character => "character",
                    ProtectedVictimKind::Corporation => "corporation",
                };
                if removed {
                    self.log(format!("Removed protected {label} with EVE ID {id}"));
                } else {
                    self.log(format!("Protected {label} {id} was already absent"));
                }
            }
            OperationOutcome::ShowProtectedSet(show) => self.log(if show {
                "Protected killmails are visible"
            } else {
                "Protected killmails are hidden"
            }),
            OperationOutcome::KillmailProtectionSet { id, protected } => self.log(if protected {
                format!("Flagged killmail {id} for protection")
            } else {
                format!("Removed protection flag from killmail {id}")
            }),
            OperationOutcome::PostPrepared(prepared) => {
                if prepared.ids.is_empty() {
                    self.log("There are no eligible confirmed unreported killmails to submit");
                } else {
                    self.pending_post = Some(prepared);
                }
            }
            OperationOutcome::Posted(result) => {
                let submitted = result
                    .results
                    .iter()
                    .filter(|entry| {
                        matches!(
                            entry.status,
                            crate::core::PostResultStatus::Submitted
                                | crate::core::PostResultStatus::AlreadyPresent
                        )
                    })
                    .count();
                for entry in &result.results {
                    if let Some(message) = &entry.message {
                        self.log(format!("Killmail {}: {message}", entry.killmail_id));
                    }
                }
                self.session_reports = self.core.session_reports();
                self.log(format!(
                    "Submission complete: {submitted} requests completed, {} failed or skipped out of {}",
                    result.results.len().saturating_sub(submitted),
                    result.results.len()
                ));
            }
        }
    }

    fn request_snapshot_reload(&mut self) {
        if self.snapshot_result.is_some() {
            return;
        }
        self.last_snapshot_poll = Instant::now();
        let core = self.core.clone();
        let (tx, rx) = mpsc::channel();
        let job = move || {
            let _ = tx.send(core.snapshot());
        };
        if self.run_jobs_inline {
            job();
        } else {
            thread::spawn(job);
        }
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
