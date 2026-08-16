use super::{worker, ActiveOperation, App, Operation, PostStats, SubmissionMode, WorkerEvent};
use crate::{
    killmail::{
        bulk_submission_candidates, individual_submission_candidate, report_state, ReportState,
    },
    models::{Character, Killmail, ProtectedVictimKind},
};
use std::{sync::mpsc, thread};

impl App {
    pub(super) fn check_cached_statuses_on_startup(&mut self) {
        let now = worker::unix_time();
        let unknown_count = self
            .store
            .cached_killmails
            .iter()
            .filter(|mail| report_state(&self.store, mail.id, now) == ReportState::Unknown)
            .count();
        if unknown_count == 0 || self.store.characters.is_empty() {
            return;
        }

        let store = self.store.clone();
        let killmails = self.store.cached_killmails.clone();
        self.start_operation(
            Operation::CheckCachedStatuses,
            format!("Checking zKillboard status for {unknown_count} cached killmails..."),
            move |tx| {
                worker::check_zkill_statuses(&store, &killmails, &tx);
                let _ = tx.send(WorkerEvent::Finished);
            },
        );
    }

    pub(super) fn migrate_refresh_tokens(&mut self) {
        let characters = self.store.characters.clone();
        self.start_operation(
            Operation::MigrateRefreshTokens,
            "Moving refresh tokens to the system credential store...",
            move |tx| worker::migrate_refresh_tokens(characters, tx),
        );
    }

    pub(super) fn begin_auth(&mut self) {
        self.start_operation(
            Operation::Authenticate,
            "Authorize the character in your browser...",
            worker::authenticate,
        );
    }

    pub(super) fn remove_character(&mut self, character: Character) {
        let name = character.name.clone();
        self.start_operation(
            Operation::RemoveCharacter,
            format!("Removing {name}..."),
            move |tx| worker::remove_character(character, tx),
        );
    }

    pub(super) fn refresh_killmails(&mut self) {
        if self.is_busy() {
            self.log("Another operation is already in progress");
            return;
        }
        if self.store.characters.is_empty() {
            self.log("Authenticate at least one character first");
            return;
        }
        let store = self.store.clone();
        self.start_operation(Operation::Load, "Loading recent killmails...", move |tx| {
            worker::load_killmails_and_statuses(store, tx);
        });
    }

    pub(super) fn request_bulk_post(&mut self) {
        let mails = bulk_submission_candidates(
            &self.store,
            self.store.cached_killmails.clone(),
            worker::unix_time(),
        );
        if mails.is_empty() {
            self.log("There are no confirmed unreported killmails to submit");
        } else {
            self.pending_bulk = Some(mails);
        }
    }

    pub(super) fn begin_add_protected_victim(&mut self) {
        if self.is_busy() {
            self.log("Another operation is already in progress");
            return;
        }
        let id = match self.new_protected_victim_id.trim().parse::<u64>() {
            Ok(id) if id > 0 => id,
            _ => {
                self.log("Enter a valid numeric EVE character or corporation ID");
                return;
            }
        };
        let kind = self.new_protected_victim_kind;
        let automatically_present = match kind {
            ProtectedVictimKind::Character => {
                self.store.characters.iter().any(|entry| entry.id == id)
            }
            ProtectedVictimKind::Corporation => self
                .store
                .characters
                .iter()
                .any(|entry| entry.corporation_id == Some(id)),
        };
        let manually_present = match kind {
            ProtectedVictimKind::Character => self
                .store
                .manually_protected_characters
                .iter()
                .any(|entry| entry.id == id),
            ProtectedVictimKind::Corporation => self
                .store
                .manually_protected_corporations
                .iter()
                .any(|entry| entry.id == id),
        };
        if automatically_present || manually_present {
            self.log("That protected victim is already in the list");
            return;
        }

        self.start_operation(
            Operation::AddProtectedVictim,
            format!("Resolving EVE ID {id}..."),
            move |tx| worker::resolve_protected_victim(kind, id, tx),
        );
    }

    pub(super) fn start_posts(&mut self, mails: Vec<Killmail>, mode: SubmissionMode) {
        if self.is_busy() {
            self.log("Another operation is already in progress");
            return;
        }
        let now = worker::unix_time();
        let mails = match mode {
            SubmissionMode::Bulk => bulk_submission_candidates(&self.store, mails, now),
            SubmissionMode::Individual => mails
                .into_iter()
                .filter_map(|mail| individual_submission_candidate(&self.store, mail, now))
                .collect(),
        };
        if mails.is_empty() {
            self.log(match mode {
                SubmissionMode::Bulk => {
                    "There are no eligible confirmed unreported killmails to submit"
                }
                SubmissionMode::Individual => {
                    "The killmail is no longer confirmed unreported; refresh its status before submitting"
                }
            });
            return;
        }
        let total = mails.len();
        let first_id = mails[0].id;
        self.post_stats = PostStats {
            total,
            ..PostStats::default()
        };
        self.start_operation(
            Operation::Post(mode),
            match mode {
                SubmissionMode::Bulk => {
                    format!("Starting submission of {total} unreported killmails...")
                }
                SubmissionMode::Individual => {
                    format!("Starting submission of killmail {first_id}...")
                }
            },
            move |tx| worker::post_killmails(mails, tx),
        );
    }

    fn start_operation(
        &mut self,
        kind: Operation,
        status: impl Into<String>,
        work: impl FnOnce(mpsc::Sender<WorkerEvent>) + Send + 'static,
    ) {
        if self.is_busy() {
            self.log("Another operation is already in progress");
            return;
        }
        if self.persistence_blocked.is_some() {
            self.log("The operation cannot start because local state could not be loaded safely");
            return;
        }
        let (tx, events) = mpsc::channel();
        thread::spawn(move || work(tx));
        self.log(status);
        self.active_operation = Some(ActiveOperation { kind, events });
    }
}
