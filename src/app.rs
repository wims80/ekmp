use crate::{
    auth, esi,
    models::{Character, Killmail, Store},
    storage, zkill,
};
use eframe::egui;
use std::{
    sync::mpsc::{self, Receiver},
    thread,
    time::Duration,
};

enum AppEvent {
    Character(Character),
    Killmails(Vec<Killmail>),
}

pub struct App {
    store: Store,
    killmails: Vec<Killmail>,
    status: String,
    auth_rx: Option<Receiver<Result<AppEvent, String>>>,
    post_rx: Option<Receiver<Result<String, String>>>,
    loading: bool,
}

impl App {
    pub fn new() -> Self {
        Self {
            store: storage::load(),
            killmails: Vec::new(),
            status: "Ready".into(),
            auth_rx: None,
            post_rx: None,
            loading: false,
        }
    }
    fn begin_auth(&mut self) {
        if self.auth_rx.is_some() {
            self.status = "An authentication or load operation is already in progress".into();
            return;
        }
        if self.store.client_id.trim().is_empty() {
            self.status = "Enter the ESI client ID first".into();
            return;
        }
        let client_id = self.store.client_id.clone();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = tx.send(auth::authenticate(&client_id).map(AppEvent::Character));
        });
        self.status = "Authorize the character in your browser...".into();
        self.auth_rx = Some(rx);
        self.loading = true;
    }
    fn refresh_killmails(&mut self) {
        if self.auth_rx.is_some() {
            self.status = "An authentication or load operation is already in progress".into();
            return;
        }
        if self.store.characters.is_empty() {
            self.status = "Authenticate at least one character first".into();
            return;
        }
        let chars = self.store.characters.clone();
        let client_id = self.store.client_id.clone();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = tx.send(esi::load_killmails(&chars, &client_id).map(AppEvent::Killmails));
        });
        self.status = "Loading recent killmails...".into();
        self.loading = true;
        self.auth_rx = Some(rx);
    }
    fn post_async(&mut self, mail: &Killmail) {
        if self.post_rx.is_some() {
            self.status = "A zKillboard submission is already in progress".into();
            return;
        }
        let mail = mail.clone();
        let id = mail.id;
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = tx.send(zkill::post(&mail));
        });
        self.post_rx = Some(rx);
        self.status = format!("Submitting killmail {id} to zKillboard...");
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Some(rx) = &self.auth_rx {
            if let Ok(result) = rx.try_recv() {
                self.auth_rx = None;
                self.loading = false;
                match result {
                    Ok(AppEvent::Character(c)) => {
                        self.store.characters.retain(|old| old.id != c.id);
                        self.store.characters.push(c);
                        self.status = "Character authenticated".into();
                        let _ = storage::persist(&self.store);
                    }
                    Ok(AppEvent::Killmails(k)) => {
                        self.killmails = k;
                        self.status = "Killmails loaded".into();
                    }
                    Err(e) => self.status = e,
                }
            }
        }
        if let Some(rx) = &self.post_rx {
            if let Ok(result) = rx.try_recv() {
                self.post_rx = None;
                self.status = match result {
                    Ok(m) => m,
                    Err(e) => format!("zKillboard submission failed: {e}"),
                };
            }
        }
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("akmp");
            ui.label("EVE Online killmail reporter");
            ui.separator();
            ui.horizontal(|ui| {
                ui.label("ESI client ID");
                ui.text_edit_singleline(&mut self.store.client_id);
            });
            if ui.button("Authenticate character").clicked() {
                let _ = storage::persist(&self.store);
                self.begin_auth();
            }
            ui.label("Register this callback URL in your EVE developer application:");
            ui.monospace(auth::callback_url());
            ui.separator();
            ui.heading("Authenticated characters");
            for c in &self.store.characters {
                ui.label(format!("{} ({})", c.name, c.id));
            }
            if ui.button("Load recent killmails").clicked() {
                self.refresh_killmails();
            }
            ui.separator();
            ui.heading("Recent killmails");
            if self.killmails.is_empty() {
                ui.label("No killmails loaded.");
            }
            let ids: Vec<u64> = self.store.characters.iter().map(|c| c.id).collect();
            let mut post_id = None;
            for mail in &self.killmails {
                ui.horizontal(|ui| {
                    ui.label(format!(
                        "{} | killed: {} | ship: {} | date: {} | source: {}",
                        mail.id, mail.victim, mail.ship, mail.time, mail.character
                    ));
                    let own = mail.victim_id.is_some_and(|id| ids.contains(&id));
                    if !own
                        && ui
                            .add_enabled(
                                self.post_rx.is_none(),
                                egui::Button::new("Post to zKillboard"),
                            )
                            .clicked()
                    {
                        post_id = Some(mail.id);
                    }
                    if own {
                        ui.label("Not postable: authenticated character");
                    }
                });
            }
            if let Some(id) = post_id {
                if let Some(mail) = self.killmails.iter().find(|m| m.id == id).cloned() {
                    self.post_async(&mail);
                }
            }
            ui.separator();
            ui.label(format!("Status: {}", self.status));
        });
        if self.auth_rx.is_some() || self.post_rx.is_some() || self.loading {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }
}
