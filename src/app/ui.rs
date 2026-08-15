use super::{worker::unix_time, App, ProtectedVictimKind, SessionReportStatus};
use crate::killmail::{
    displayed_killmails, is_bulk_candidate, protected_victim_reasons, report_state, ReportState,
};
use eframe::egui;
use std::{collections::HashSet, time::Duration};

impl App {
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
            ui.heading("EVE Killmail Publisher");
            ui.label("EVE Online killmail publisher");
            if self.has_json_refresh_token_fallback() {
                ui.colored_label(
                    egui::Color32::YELLOW,
                    "Security warning: one or more refresh tokens are stored in akmp.json because the system credential store was unavailable.",
                );
            }
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

            if !self.session_reports.is_empty() {
                ui.separator();
                ui.heading("Reported this session");
                egui::ScrollArea::vertical()
                    .id_salt("session_reports")
                    .max_height(120.0)
                    .show(ui, |ui| {
                        for report in &self.session_reports {
                            ui.horizontal(|ui| {
                                let status = match report.status {
                                    SessionReportStatus::Submitted => "Submitted",
                                    SessionReportStatus::AlreadyPresent => "Already on zKillboard",
                                };
                                ui.label(format!("Killmail {} — {status}", report.killmail_id));
                                ui.hyperlink_to("Open on zKillboard", &report.url);
                            });
                        }
                    });
            }

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
                    &mut self.store.show_protected_killmails,
                    "Show protected killmails",
                )
                .changed()
            {
                self.persist_or_log_error();
            }

            let mut post_mail = None;
            let visible_killmails = displayed_killmails(&self.store, &self.killmails, now);
            if !self.killmails.is_empty() && visible_killmails.is_empty() {
                ui.label("No killmails match the current display filters.");
            }
            let killmail_pane_height = ui.available_height().max(120.0);
            egui::ScrollArea::vertical()
                .id_salt("killmail_list")
                .max_height(killmail_pane_height)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for mail in visible_killmails {
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
