use crate::{
    auth, esi,
    models::{Character, Killmail, ProtectedVictim, Store, ZkillCacheEntry},
    storage, zkill,
};
use eframe::egui;
use std::{
    collections::{HashSet, VecDeque},
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const NEGATIVE_CACHE_TTL_SECS: u64 = 15 * 60;
const REQUEST_SPACING: Duration = Duration::from_secs(1);
const STATUS_HISTORY_LIMIT: usize = 200;

enum WorkerEvent {
    Status(String),
    Character(Character),
    CharactersRefreshed(Vec<Character>),
    ProtectedVictimResolved {
        kind: ProtectedVictimKind,
        victim: ProtectedVictim,
    },
    KillmailsLoaded(Vec<Killmail>),
    LookupComplete {
        character_name: String,
        checked_ids: Vec<u64>,
        reported_ids: HashSet<u64>,
        checked_at: u64,
    },
    LookupFailed {
        character_name: String,
        error: String,
    },
    PostComplete {
        killmail_id: u64,
        result: Result<zkill::PostOutcome, String>,
    },
    Finished,
    Failed(String),
}

#[derive(Clone, Copy)]
enum Operation {
    Authenticate,
    Load,
    CheckCachedStatuses,
    AddProtectedVictim,
    Post { bulk: bool },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProtectedVictimKind {
    Character,
    Corporation,
}

#[derive(Default)]
struct PostStats {
    total: usize,
    new: usize,
    existing: usize,
    failed: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReportState {
    Reported,
    Unreported,
    Unknown,
}

pub struct App {
    store: Store,
    killmails: Vec<Killmail>,
    latest_status: String,
    status_history: VecDeque<String>,
    event_rx: Option<Receiver<WorkerEvent>>,
    operation: Option<Operation>,
    post_stats: PostStats,
    pending_bulk: Option<Vec<Killmail>>,
    new_protected_victim_id: String,
    new_protected_victim_kind: ProtectedVictimKind,
}

impl App {
    pub fn new() -> Self {
        let store = storage::load();
        let killmails = store.cached_killmails.clone();
        let mut app = Self {
            store,
            killmails,
            latest_status: "Ready".into(),
            status_history: VecDeque::from(["Ready".into()]),
            event_rx: None,
            operation: None,
            post_stats: PostStats::default(),
            pending_bulk: None,
            new_protected_victim_id: String::new(),
            new_protected_victim_kind: ProtectedVictimKind::Character,
        };
        app.check_cached_statuses_on_startup();
        app
    }

    fn is_busy(&self) -> bool {
        self.operation.is_some()
    }

    fn check_cached_statuses_on_startup(&mut self) {
        let now = unix_time();
        let unknown_count = self
            .killmails
            .iter()
            .filter(|mail| report_state(&self.store, mail.id, now) == ReportState::Unknown)
            .count();
        if unknown_count == 0 || self.store.characters.is_empty() {
            return;
        }

        let store = self.store.clone();
        let killmails = self.killmails.clone();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            check_zkill_statuses(&store, &killmails, &tx);
            let _ = tx.send(WorkerEvent::Finished);
        });
        self.log(format!(
            "Checking zKillboard status for {unknown_count} cached killmails..."
        ));
        self.event_rx = Some(rx);
        self.operation = Some(Operation::CheckCachedStatuses);
    }

    fn log(&mut self, message: impl Into<String>) {
        let message = message.into();
        self.latest_status.clone_from(&message);
        if self.status_history.len() == STATUS_HISTORY_LIMIT {
            self.status_history.pop_front();
        }
        self.status_history.push_back(message);
    }

    fn begin_auth(&mut self) {
        if self.is_busy() {
            self.log("Another operation is already in progress");
            return;
        }
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || match auth::authenticate() {
            Ok(mut character) => {
                if let Err(error) = esi::refresh_character_affiliation(&mut character) {
                    let _ = tx.send(WorkerEvent::Status(format!(
                        "Character authenticated, but corporation lookup failed: {error}"
                    )));
                }
                let _ = tx.send(WorkerEvent::Character(character));
                let _ = tx.send(WorkerEvent::Finished);
            }
            Err(error) => {
                let _ = tx.send(WorkerEvent::Failed(error));
            }
        });
        self.log("Authorize the character in your browser...");
        self.event_rx = Some(rx);
        self.operation = Some(Operation::Authenticate);
    }

    fn refresh_killmails(&mut self) {
        if self.is_busy() {
            self.log("Another operation is already in progress");
            return;
        }
        if self.store.characters.is_empty() {
            self.log("Authenticate at least one character first");
            return;
        }
        let store = self.store.clone();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || load_killmails_and_statuses(store, tx));
        self.log("Loading recent killmails from ESI...");
        self.event_rx = Some(rx);
        self.operation = Some(Operation::Load);
    }

    fn request_bulk_post(&mut self) {
        let mails = submission_candidates(&self.store, self.killmails.clone(), true, unix_time());
        if mails.is_empty() {
            self.log("There are no confirmed unreported killmails to submit");
        } else {
            self.pending_bulk = Some(mails);
        }
    }

    fn begin_add_protected_victim(&mut self) {
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

        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let result = match kind {
                ProtectedVictimKind::Character => esi::resolve_character_name(id),
                ProtectedVictimKind::Corporation => esi::resolve_corporation_name(id),
            };
            match result {
                Ok(name) => {
                    let _ = tx.send(WorkerEvent::ProtectedVictimResolved {
                        kind,
                        victim: ProtectedVictim { id, name },
                    });
                    let _ = tx.send(WorkerEvent::Finished);
                }
                Err(error) => {
                    let _ = tx.send(WorkerEvent::Failed(format!(
                        "Could not add protected victim: {error}"
                    )));
                }
            }
        });
        self.log(format!("Resolving EVE ID {id}..."));
        self.event_rx = Some(rx);
        self.operation = Some(Operation::AddProtectedVictim);
    }

    fn start_posts(&mut self, mails: Vec<Killmail>, bulk: bool) {
        if self.is_busy() {
            self.log("Another operation is already in progress");
            return;
        }
        let mails = submission_candidates(&self.store, mails, bulk, unix_time());
        if mails.is_empty() {
            self.log("There are no eligible unreported killmails to submit");
            return;
        }
        let total = mails.len();
        let first_id = mails[0].id;
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            for (index, mail) in mails.iter().enumerate() {
                if tx
                    .send(WorkerEvent::Status(format!(
                        "Submitting killmail {} to zKillboard ({}/{total})...",
                        mail.id,
                        index + 1
                    )))
                    .is_err()
                {
                    return;
                }
                let result = zkill::post(mail);
                if tx
                    .send(WorkerEvent::PostComplete {
                        killmail_id: mail.id,
                        result,
                    })
                    .is_err()
                {
                    return;
                }
                if index + 1 < total {
                    thread::sleep(REQUEST_SPACING);
                }
            }
            let _ = tx.send(WorkerEvent::Finished);
        });
        self.post_stats = PostStats {
            total,
            ..PostStats::default()
        };
        self.log(if bulk {
            format!("Starting submission of {total} unreported killmails...")
        } else {
            format!("Starting submission of killmail {first_id}...")
        });
        self.event_rx = Some(rx);
        self.operation = Some(Operation::Post { bulk });
    }

    fn poll_worker(&mut self) {
        let events = self
            .event_rx
            .as_ref()
            .map(|rx| rx.try_iter().collect::<Vec<_>>())
            .unwrap_or_default();
        for event in events {
            self.handle_event(event);
        }
    }

    fn handle_event(&mut self, event: WorkerEvent) {
        match event {
            WorkerEvent::Status(message) => self.log(message),
            WorkerEvent::Character(character) => {
                let name = character.name.clone();
                self.store.characters.retain(|old| old.id != character.id);
                self.store.characters.push(character);
                self.persist_or_log_error();
                self.log(format!("Character {name} authenticated"));
            }
            WorkerEvent::CharactersRefreshed(characters) => {
                self.store.characters = characters;
                self.persist_or_log_error();
            }
            WorkerEvent::ProtectedVictimResolved { kind, victim } => {
                let label = match kind {
                    ProtectedVictimKind::Character => {
                        self.store
                            .manually_protected_characters
                            .push(victim.clone());
                        "character"
                    }
                    ProtectedVictimKind::Corporation => {
                        self.store
                            .manually_protected_corporations
                            .push(victim.clone());
                        "corporation"
                    }
                };
                self.new_protected_victim_id.clear();
                self.persist_or_log_error();
                self.log(format!(
                    "Added protected {label}: {} ({})",
                    victim.name, victim.id
                ));
            }
            WorkerEvent::KillmailsLoaded(killmails) => {
                let count = killmails.len();
                self.store.cached_killmails.clone_from(&killmails);
                self.killmails = killmails;
                self.persist_or_log_error();
                self.log(format!(
                    "Loaded {count} unique recent killmails; checking zKillboard status..."
                ));
            }
            WorkerEvent::LookupComplete {
                character_name,
                checked_ids,
                reported_ids,
                checked_at,
            } => {
                let reported_count = checked_ids
                    .iter()
                    .filter(|id| reported_ids.contains(id))
                    .count();
                for id in checked_ids {
                    let already_reported = self
                        .store
                        .zkill_cache
                        .get(&id)
                        .is_some_and(|entry| entry.reported);
                    self.store.zkill_cache.insert(
                        id,
                        ZkillCacheEntry {
                            reported: already_reported || reported_ids.contains(&id),
                            checked_at,
                        },
                    );
                }
                self.persist_or_log_error();
                self.log(format!(
                    "Checked {character_name} on zKillboard: found {reported_count} reported killmails"
                ));
            }
            WorkerEvent::LookupFailed {
                character_name,
                error,
            } => self.log(format!(
                "Could not check zKillboard status for {character_name}: {error}"
            )),
            WorkerEvent::PostComplete {
                killmail_id,
                result,
            } => match result {
                Ok(outcome) => {
                    self.store.zkill_cache.insert(
                        killmail_id,
                        ZkillCacheEntry {
                            reported: true,
                            checked_at: unix_time(),
                        },
                    );
                    self.persist_or_log_error();
                    if outcome.new {
                        self.post_stats.new += 1;
                        self.log(format!(
                            "Killmail {killmail_id} was reported successfully: {}",
                            outcome.url
                        ));
                    } else {
                        self.post_stats.existing += 1;
                        self.log(format!(
                            "Killmail {killmail_id} was already on zKillboard: {}",
                            outcome.url
                        ));
                    }
                }
                Err(error) => {
                    if self
                        .store
                        .zkill_cache
                        .get(&killmail_id)
                        .is_some_and(|entry| !entry.reported)
                    {
                        self.store.zkill_cache.remove(&killmail_id);
                        self.persist_or_log_error();
                    }
                    self.post_stats.failed += 1;
                    self.log(format!("Killmail {killmail_id} submission failed: {error}"));
                }
            },
            WorkerEvent::Finished => self.finish_operation(),
            WorkerEvent::Failed(error) => {
                self.log(error);
                self.operation = None;
                self.event_rx = None;
            }
        }
    }

    fn finish_operation(&mut self) {
        let operation = self.operation.take();
        self.event_rx = None;
        match operation {
            Some(Operation::Authenticate) => {}
            Some(Operation::AddProtectedVictim) => {}
            Some(Operation::CheckCachedStatuses) => self.log_character_summaries(),
            Some(Operation::Load) => self.log_character_summaries(),
            Some(Operation::Post { bulk }) => {
                self.log_character_summaries();
                let label = if bulk {
                    "Bulk submission"
                } else {
                    "Submission"
                };
                self.log(format!(
                    "{label} complete: {} new, {} already present, {} failed out of {}",
                    self.post_stats.new,
                    self.post_stats.existing,
                    self.post_stats.failed,
                    self.post_stats.total
                ));
            }
            None => {}
        }
    }

    fn persist_or_log_error(&mut self) {
        if let Err(error) = storage::persist(&self.store) {
            self.log(format!("Could not save local state: {error}"));
        }
    }

    fn log_character_summaries(&mut self) {
        let messages = character_summaries(&self.store, &self.killmails, unix_time());
        for message in messages {
            self.log(message);
        }
    }

    fn show_protected_victims(&mut self, ui: &mut egui::Ui) {
        let mut remove = None;
        egui::CollapsingHeader::new("Protected victims")
            .default_open(false)
            .show(ui, |ui| {
                ui.label(
                    "Killmails involving these victims are excluded from bulk posting. You can still post them individually.",
                );
                ui.separator();
                ui.label("Automatically protected");
                for character in &self.store.characters {
                    ui.label(format!(
                        "Authenticated character: {} ({})",
                        character.name, character.id
                    ));
                }
                let mut corporation_ids = HashSet::new();
                for character in &self.store.characters {
                    if let (Some(id), Some(name)) =
                        (character.corporation_id, &character.corporation_name)
                    {
                        if corporation_ids.insert(id) {
                            ui.label(format!("Authenticated corporation: {name} ({id})"));
                        }
                    }
                }

                ui.separator();
                ui.label("Manually protected");
                for victim in self.store.manually_protected_characters.clone() {
                    ui.horizontal(|ui| {
                        ui.label(format!("Character: {} ({})", victim.name, victim.id));
                        if ui.button("Remove").clicked() {
                            remove = Some((ProtectedVictimKind::Character, victim.id));
                        }
                    });
                }
                for victim in self.store.manually_protected_corporations.clone() {
                    ui.horizontal(|ui| {
                        ui.label(format!("Corporation: {} ({})", victim.name, victim.id));
                        if ui.button("Remove").clicked() {
                            remove = Some((ProtectedVictimKind::Corporation, victim.id));
                        }
                    });
                }
                if self.store.manually_protected_characters.is_empty()
                    && self.store.manually_protected_corporations.is_empty()
                {
                    ui.label("No manually protected victims.");
                }

                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("protected_victim_kind")
                        .selected_text(match self.new_protected_victim_kind {
                            ProtectedVictimKind::Character => "Character",
                            ProtectedVictimKind::Corporation => "Corporation",
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut self.new_protected_victim_kind,
                                ProtectedVictimKind::Character,
                                "Character",
                            );
                            ui.selectable_value(
                                &mut self.new_protected_victim_kind,
                                ProtectedVictimKind::Corporation,
                                "Corporation",
                            );
                        });
                    ui.label("EVE ID");
                    ui.text_edit_singleline(&mut self.new_protected_victim_id);
                    if ui
                        .add_enabled(!self.is_busy(), egui::Button::new("Add"))
                        .clicked()
                    {
                        self.begin_add_protected_victim();
                    }
                });
            });

        if let Some((kind, id)) = remove {
            match kind {
                ProtectedVictimKind::Character => self
                    .store
                    .manually_protected_characters
                    .retain(|entry| entry.id != id),
                ProtectedVictimKind::Corporation => self
                    .store
                    .manually_protected_corporations
                    .retain(|entry| entry.id != id),
            }
            self.persist_or_log_error();
            self.log(format!("Removed protected victim with EVE ID {id}"));
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_worker();
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("akmp");
            ui.label("EVE Online killmail reporter");
            ui.separator();
            if ui
                .add_enabled(!self.is_busy(), egui::Button::new("Authenticate character"))
                .clicked()
            {
                self.begin_auth();
            }
            ui.separator();
            ui.heading("Authenticated characters");
            for character in &self.store.characters {
                let corporation = character
                    .corporation_name
                    .as_deref()
                    .unwrap_or("corporation unknown");
                ui.label(format!(
                    "{} ({}) — {}",
                    character.name, character.id, corporation
                ));
            }
            self.show_protected_victims(ui);
            if ui
                .add_enabled(!self.is_busy(), egui::Button::new("Load recent killmails"))
                .clicked()
            {
                self.refresh_killmails();
            }

            ui.separator();
            ui.heading("Status");
            ui.label(&self.latest_status);
            ui.label("Activity");
            egui::ScrollArea::vertical()
                .max_height(140.0)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    for message in &self.status_history {
                        ui.label(message);
                    }
                });

            ui.separator();
            ui.heading("Recent killmails");
            if self.killmails.is_empty() {
                ui.label("No killmails loaded.");
            }
            let now = unix_time();
            let unreported_count = self
                .killmails
                .iter()
                .filter(|mail| is_bulk_candidate(&self.store, mail, now))
                .count();
            if ui
                .add_enabled(
                    !self.is_busy() && unreported_count > 0,
                    egui::Button::new(format!(
                        "Post all unreported killmails ({unreported_count})"
                    )),
                )
                .clicked()
            {
                self.request_bulk_post();
            }
            if ui
                .checkbox(
                    &mut self.store.show_reported_killmails,
                    "Show reported killmails",
                )
                .changed()
            {
                self.persist_or_log_error();
            }
            if ui
                .checkbox(
                    &mut self.store.show_protected_killmails,
                    "Show protected killmails",
                )
                .changed()
            {
                self.persist_or_log_error();
            }

            let mut post_mail = None;
            let visible_killmail_count = self
                .killmails
                .iter()
                .filter(|mail| is_killmail_visible(&self.store, mail, now))
                .count();
            if !self.killmails.is_empty() && visible_killmail_count == 0 {
                ui.label("No killmails match the current display filters.");
            }
            let killmail_pane_height = ui.available_height().max(120.0);
            egui::ScrollArea::vertical()
                .id_salt("killmail_list")
                .max_height(killmail_pane_height)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for mail in self
                        .killmails
                        .iter()
                        .filter(|mail| is_killmail_visible(&self.store, mail, now))
                    {
                        ui.horizontal(|ui| {
                            let sources = mail
                                .sources
                                .iter()
                                .map(|source| source.name.as_str())
                                .collect::<Vec<_>>()
                                .join(", ");
                            ui.label(format!(
                                "{} | killed: {} | ship: {} | date: {} | source: {}",
                                mail.id, mail.victim, mail.ship, mail.time, sources
                            ));
                            let protection_reasons = protected_victim_reasons(&self.store, mail);
                            match report_state(&self.store, mail.id, now) {
                                ReportState::Reported => {
                                    ui.colored_label(egui::Color32::GREEN, "Reported");
                                }
                                ReportState::Unreported => {
                                    if protection_reasons.is_empty() {
                                        ui.colored_label(egui::Color32::YELLOW, "Not reported");
                                        if ui
                                            .add_enabled(
                                                !self.is_busy(),
                                                egui::Button::new("Post to zKillboard"),
                                            )
                                            .clicked()
                                        {
                                            post_mail = Some(mail.clone());
                                        }
                                    } else {
                                        ui.colored_label(
                                            egui::Color32::LIGHT_RED,
                                            format!(
                                                "Excluded from bulk posting: {}",
                                                protection_reasons.join(", ")
                                            ),
                                        );
                                        if ui
                                            .add_enabled(
                                                !self.is_busy(),
                                                egui::Button::new("Post anyway"),
                                            )
                                            .clicked()
                                        {
                                            post_mail = Some(mail.clone());
                                        }
                                    }
                                }
                                ReportState::Unknown => {
                                    ui.colored_label(egui::Color32::GRAY, "Status unknown");
                                    if !protection_reasons.is_empty() {
                                        ui.colored_label(
                                            egui::Color32::LIGHT_RED,
                                            format!(
                                                "Excluded from bulk posting: {}",
                                                protection_reasons.join(", ")
                                            ),
                                        );
                                    }
                                }
                            }
                        });
                    }
                });
            if let Some(mail) = post_mail {
                self.start_posts(vec![mail], false);
            }
        });

        if let Some(count) = self.pending_bulk.as_ref().map(Vec::len) {
            let mut confirm = false;
            let mut cancel = false;
            egui::Window::new("Confirm bulk submission")
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label(format!(
                        "Submit {count} unique unreported killmails to zKillboard?"
                    ));
                    ui.horizontal(|ui| {
                        confirm = ui.button("Submit all").clicked();
                        cancel = ui.button("Cancel").clicked();
                    });
                });
            if confirm {
                if let Some(mails) = self.pending_bulk.take() {
                    self.start_posts(mails, true);
                }
            } else if cancel {
                self.pending_bulk = None;
                self.log("Bulk submission cancelled");
            }
        }

        if self.is_busy() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }
}

fn load_killmails_and_statuses(mut store: Store, tx: Sender<WorkerEvent>) {
    for character in &mut store.characters {
        if let Err(error) = esi::refresh_character_affiliation(character) {
            let _ = tx.send(WorkerEvent::Status(format!(
                "Could not refresh corporation for {}: {error}",
                character.name
            )));
        }
    }
    if tx
        .send(WorkerEvent::CharactersRefreshed(store.characters.clone()))
        .is_err()
    {
        return;
    }

    let killmails = match esi::load_killmails(&store.characters) {
        Ok(killmails) => killmails,
        Err(error) => {
            let _ = tx.send(WorkerEvent::Failed(format!(
                "Could not load recent killmails: {error}"
            )));
            return;
        }
    };
    if tx
        .send(WorkerEvent::KillmailsLoaded(killmails.clone()))
        .is_err()
    {
        return;
    }

    check_zkill_statuses(&store, &killmails, &tx);
    let _ = tx.send(WorkerEvent::Finished);
}

fn check_zkill_statuses(store: &Store, killmails: &[Killmail], tx: &Sender<WorkerEvent>) {
    let checks = zkill_status_checks(store, killmails, unix_time());

    for (index, (name, id, candidate_ids, is_loss)) in checks.iter().enumerate() {
        let mail_type = if *is_loss { "losses" } else { "kills" };
        if tx
            .send(WorkerEvent::Status(format!(
                "Checking recent {mail_type} for {name} on zKillboard ({}/{})...",
                index + 1,
                checks.len()
            )))
            .is_err()
        {
            return;
        }
        let result = if *is_loss {
            zkill::character_loss_ids(*id)
        } else {
            zkill::character_kill_ids(*id)
        };
        match result {
            Ok(reported_ids) => {
                if tx
                    .send(WorkerEvent::LookupComplete {
                        character_name: name.clone(),
                        checked_ids: candidate_ids.clone(),
                        reported_ids,
                        checked_at: unix_time(),
                    })
                    .is_err()
                {
                    return;
                }
            }
            Err(error) => {
                if tx
                    .send(WorkerEvent::LookupFailed {
                        character_name: name.clone(),
                        error,
                    })
                    .is_err()
                {
                    return;
                }
            }
        }
        if index + 1 < checks.len() {
            thread::sleep(REQUEST_SPACING);
        }
    }
}

fn zkill_status_checks(
    store: &Store,
    killmails: &[Killmail],
    now: u64,
) -> Vec<(String, u64, Vec<u64>, bool)> {
    let mut checks = Vec::new();
    for character in &store.characters {
        let character_mails = killmails
            .iter()
            .filter(|mail| mail.sources.iter().any(|source| source.id == character.id));
        let (losses, kills): (Vec<_>, Vec<_>) =
            character_mails.partition(|mail| mail.victim_id == Some(character.id));
        for (is_loss, candidates) in [(false, kills), (true, losses)] {
            let candidate_ids = candidates
                .into_iter()
                .filter(|mail| report_state(store, mail.id, now) == ReportState::Unknown)
                .map(|mail| mail.id)
                .collect::<Vec<_>>();
            if !candidate_ids.is_empty() {
                checks.push((character.name.clone(), character.id, candidate_ids, is_loss));
            }
        }
    }
    checks
}

fn unix_time() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn protected_victim_reasons(store: &Store, mail: &Killmail) -> Vec<String> {
    let mut reasons = Vec::new();
    if let Some(victim_id) = mail.victim_id {
        if let Some(character) = store
            .characters
            .iter()
            .find(|character| character.id == victim_id)
        {
            reasons.push(format!("authenticated character {}", character.name));
        }
        if let Some(character) = store
            .manually_protected_characters
            .iter()
            .find(|character| character.id == victim_id)
        {
            reasons.push(format!("character {}", character.name));
        }
    }
    if let Some(corporation_id) = mail.victim_corporation_id {
        if let Some(character) = store.characters.iter().find(|character| {
            character.corporation_id == Some(corporation_id) && character.corporation_name.is_some()
        }) {
            reasons.push(format!(
                "authenticated corporation {}",
                character.corporation_name.as_deref().unwrap_or_default()
            ));
        }
        if let Some(corporation) = store
            .manually_protected_corporations
            .iter()
            .find(|corporation| corporation.id == corporation_id)
        {
            reasons.push(format!("corporation {}", corporation.name));
        }
    }
    reasons
}

fn is_eligible_for_bulk_posting(store: &Store, mail: &Killmail) -> bool {
    protected_victim_reasons(store, mail).is_empty()
}

fn is_killmail_visible(store: &Store, mail: &Killmail, now: u64) -> bool {
    (store.show_reported_killmails || report_state(store, mail.id, now) != ReportState::Reported)
        && (store.show_protected_killmails || is_eligible_for_bulk_posting(store, mail))
}

fn report_state(store: &Store, killmail_id: u64, now: u64) -> ReportState {
    match store.zkill_cache.get(&killmail_id).copied() {
        Some(entry) if entry.reported => ReportState::Reported,
        Some(entry) if entry.is_fresh(now, NEGATIVE_CACHE_TTL_SECS) => ReportState::Unreported,
        _ => ReportState::Unknown,
    }
}

fn is_bulk_candidate(store: &Store, mail: &Killmail, now: u64) -> bool {
    is_eligible_for_bulk_posting(store, mail)
        && report_state(store, mail.id, now) == ReportState::Unreported
}

fn submission_candidates(
    store: &Store,
    mut mails: Vec<Killmail>,
    bulk: bool,
    now: u64,
) -> Vec<Killmail> {
    if bulk {
        mails.retain(|mail| is_bulk_candidate(store, mail, now));
    }
    mails
}

fn character_summaries(store: &Store, killmails: &[Killmail], now: u64) -> Vec<String> {
    store
        .characters
        .iter()
        .map(|character| {
            let eligible = killmails
                .iter()
                .filter(|mail| {
                    is_eligible_for_bulk_posting(store, mail)
                        && mail
                            .sources
                            .iter()
                            .any(|source| source.id == character.id)
                })
                .collect::<Vec<_>>();
            if eligible.is_empty() {
                return format!(
                    "{} has no recent killmails eligible for bulk posting",
                    character.name
                );
            }
            let unknown = eligible
                .iter()
                .filter(|mail| report_state(store, mail.id, now) == ReportState::Unknown)
                .count();
            if unknown > 0 {
                return format!(
                    "Could not determine zKillboard status for {unknown} of {} eligible recent killmails for {}",
                    eligible.len(),
                    character.name
                );
            }
            let unreported = eligible
                .iter()
                .filter(|mail| report_state(store, mail.id, now) == ReportState::Unreported)
                .count();
            if unreported == 0 {
                format!(
                    "All {} eligible recent killmails for {} are reported to zKillboard",
                    eligible.len(),
                    character.name
                )
            } else {
                format!(
                    "{} has {unreported} of {} eligible recent killmails still unreported",
                    character.name,
                    eligible.len()
                )
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::CharacterSource;

    fn mail(id: u64, source_ids: &[u64], victim_id: Option<u64>) -> Killmail {
        Killmail {
            id,
            hash: "hash".into(),
            sources: source_ids
                .iter()
                .map(|id| CharacterSource {
                    id: *id,
                    name: format!("Pilot {id}"),
                })
                .collect(),
            victim_id,
            victim_corporation_id: None,
            victim: "Victim".into(),
            ship: "Ship".into(),
            time: "Time".into(),
        }
    }

    fn store() -> Store {
        Store {
            characters: vec![Character {
                id: 1,
                name: "Pilot 1".into(),
                refresh_token: String::new(),
                corporation_id: Some(100),
                corporation_name: Some("Pilot Corp".into()),
            }],
            ..Store::default()
        }
    }

    #[test]
    fn report_state_distinguishes_fresh_stale_and_reported_entries() {
        let mut store = store();
        store.zkill_cache.insert(
            1,
            ZkillCacheEntry {
                reported: false,
                checked_at: 100,
            },
        );
        store.zkill_cache.insert(
            2,
            ZkillCacheEntry {
                reported: true,
                checked_at: 0,
            },
        );

        assert_eq!(report_state(&store, 1, 999), ReportState::Unreported);
        assert_eq!(report_state(&store, 1, 1_000), ReportState::Unknown);
        assert_eq!(report_state(&store, 2, u64::MAX), ReportState::Reported);
        assert_eq!(report_state(&store, 3, 100), ReportState::Unknown);
    }

    #[test]
    fn zkill_status_checks_include_only_unknown_cached_killmails() {
        let mut store = store();
        store.zkill_cache.insert(
            10,
            ZkillCacheEntry {
                reported: false,
                checked_at: 200,
            },
        );
        store.zkill_cache.insert(
            11,
            ZkillCacheEntry {
                reported: true,
                checked_at: 0,
            },
        );
        store.zkill_cache.insert(
            13,
            ZkillCacheEntry {
                reported: false,
                checked_at: 0,
            },
        );
        let killmails = vec![
            mail(10, &[1], None),
            mail(11, &[1], None),
            mail(12, &[1], None),
            mail(13, &[1], Some(1)),
            mail(14, &[2], None),
        ];

        assert_eq!(
            zkill_status_checks(&store, &killmails, 1_000),
            vec![
                ("Pilot 1".into(), 1, vec![12], false),
                ("Pilot 1".into(), 1, vec![13], true),
            ]
        );
    }

    #[test]
    fn summaries_exclude_authenticated_character_losses() {
        let mut store = store();
        store.zkill_cache.insert(
            10,
            ZkillCacheEntry {
                reported: true,
                checked_at: 0,
            },
        );
        let killmails = vec![mail(10, &[1], None), mail(11, &[1], Some(1))];

        assert_eq!(
            character_summaries(&store, &killmails, 1),
            vec!["All 1 eligible recent killmails for Pilot 1 are reported to zKillboard"]
        );
    }

    #[test]
    fn summaries_report_partial_and_unknown_states() {
        let mut store = store();
        store.zkill_cache.insert(
            10,
            ZkillCacheEntry {
                reported: false,
                checked_at: 100,
            },
        );
        let killmails = vec![mail(10, &[1], None), mail(11, &[1], None)];

        assert!(character_summaries(&store, &killmails, 100)[0]
            .starts_with("Could not determine zKillboard status for 1 of 2"));
        store.zkill_cache.insert(
            11,
            ZkillCacheEntry {
                reported: true,
                checked_at: 100,
            },
        );
        assert_eq!(
            character_summaries(&store, &killmails, 100),
            vec!["Pilot 1 has 1 of 2 eligible recent killmails still unreported"]
        );
    }

    #[test]
    fn bulk_candidates_exclude_reported_unknown_and_authenticated_losses() {
        let mut store = store();
        store.zkill_cache.insert(
            10,
            ZkillCacheEntry {
                reported: false,
                checked_at: 100,
            },
        );
        store.zkill_cache.insert(
            11,
            ZkillCacheEntry {
                reported: true,
                checked_at: 100,
            },
        );
        store.zkill_cache.insert(
            13,
            ZkillCacheEntry {
                reported: false,
                checked_at: 100,
            },
        );
        let killmails = [
            mail(10, &[1], None),
            mail(11, &[1], None),
            mail(12, &[1], None),
            mail(13, &[1], Some(1)),
        ];

        let candidates = killmails
            .iter()
            .filter(|mail| is_bulk_candidate(&store, mail, 100))
            .map(|mail| mail.id)
            .collect::<Vec<_>>();

        assert_eq!(candidates, vec![10]);
    }

    #[test]
    fn protected_killmails_require_individual_submission() {
        let mut store = store();
        store.manually_protected_characters.push(ProtectedVictim {
            id: 2,
            name: "Protected Pilot".into(),
        });
        store.zkill_cache.insert(
            10,
            ZkillCacheEntry {
                reported: false,
                checked_at: 100,
            },
        );
        let protected = mail(10, &[1], Some(2));

        assert!(submission_candidates(&store, vec![protected.clone()], true, 100).is_empty());
        assert_eq!(
            submission_candidates(&store, vec![protected], false, 100)[0].id,
            10
        );
    }

    #[test]
    fn killmail_visibility_respects_reported_and_protected_preferences() {
        let mut store = store();
        store.manually_protected_characters.push(ProtectedVictim {
            id: 2,
            name: "Protected Pilot".into(),
        });
        for id in [10, 11] {
            store.zkill_cache.insert(
                id,
                ZkillCacheEntry {
                    reported: false,
                    checked_at: 100,
                },
            );
        }
        store.zkill_cache.insert(
            12,
            ZkillCacheEntry {
                reported: true,
                checked_at: 100,
            },
        );
        let visible = mail(10, &[1], None);
        let protected = mail(11, &[1], Some(2));
        let reported = mail(12, &[1], None);

        assert!(is_killmail_visible(&store, &visible, 100));
        assert!(!is_killmail_visible(&store, &protected, 100));
        assert!(!is_killmail_visible(&store, &reported, 100));

        store.show_protected_killmails = true;
        assert!(is_killmail_visible(&store, &protected, 100));
        assert!(!is_killmail_visible(&store, &reported, 100));

        store.show_reported_killmails = true;
        assert!(is_killmail_visible(&store, &reported, 100));
    }

    #[test]
    fn automatically_and_manually_protected_victims_match_characters_and_corporations() {
        let mut store = store();
        store.manually_protected_characters.push(ProtectedVictim {
            id: 2,
            name: "Protected Pilot".into(),
        });
        store.manually_protected_corporations.push(ProtectedVictim {
            id: 200,
            name: "Protected Corp".into(),
        });
        let authenticated_character = mail(1, &[1], Some(1));
        let mut authenticated_corporation = mail(2, &[1], Some(9));
        authenticated_corporation.victim_corporation_id = Some(100);
        let manually_protected_character = mail(3, &[1], Some(2));
        let mut manually_protected_corporation = mail(4, &[1], Some(9));
        manually_protected_corporation.victim_corporation_id = Some(200);
        let unrelated = mail(5, &[1], Some(9));

        assert!(!is_eligible_for_bulk_posting(
            &store,
            &authenticated_character
        ));
        assert!(!is_eligible_for_bulk_posting(
            &store,
            &authenticated_corporation
        ));
        assert!(!is_eligible_for_bulk_posting(
            &store,
            &manually_protected_character
        ));
        assert!(!is_eligible_for_bulk_posting(
            &store,
            &manually_protected_corporation
        ));
        assert!(is_eligible_for_bulk_posting(&store, &unrelated));
    }
}
