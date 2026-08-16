use super::{worker::unix_time, App, SessionReportStatus, SubmissionMode};
use crate::{
    killmail::{
        displayed_killmails, is_bulk_candidate, posting_summary, protected_victim_reasons,
        report_state, ProtectionReason, ReportState,
    },
    models::ProtectedVictimKind,
};
use eframe::egui;
use std::{collections::HashSet, time::Duration};

impl App {
    fn show_header(&self, ui: &mut egui::Ui) {
        ui.heading("EVE Killmail Publisher");
        ui.label("EVE Online killmail publisher");
        if let Some(error) = &self.persistence_blocked {
            ui.colored_label(
                egui::Color32::RED,
                format!(
                    "Local state could not be loaded, so saving is disabled to avoid overwriting it: {error}"
                ),
            );
        }
        if self.has_json_refresh_token_fallback() {
            ui.colored_label(
                egui::Color32::YELLOW,
                "Security warning: one or more refresh tokens are stored in ekmp.json because the system credential store was unavailable.",
            );
        }
    }

    fn show_authenticated_characters(&mut self, ui: &mut egui::Ui) {
        if ui
            .add_enabled(
                self.persisted_controls_enabled(),
                egui::Button::new("Authenticate character"),
            )
            .clicked()
        {
            self.begin_auth();
        }
        ui.separator();
        ui.heading("Authenticated characters");
        let mut remove_character = None;
        for character in &self.store.characters {
            let corporation = character
                .corporation_name
                .as_deref()
                .unwrap_or("corporation unknown");
            ui.horizontal(|ui| {
                ui.label(format!(
                    "{} ({}) — {}",
                    character.name, character.id, corporation
                ));
                if ui
                    .add_enabled(
                        self.persisted_controls_enabled(),
                        egui::Button::new("Remove character"),
                    )
                    .clicked()
                {
                    remove_character = Some(character.clone());
                }
            });
        }
        if let Some(character) = remove_character {
            self.pending_character_removal = Some(character);
        }
        self.show_protected_victims(ui);
        if ui
            .add_enabled(
                self.persisted_controls_enabled(),
                egui::Button::new("Load recent killmails"),
            )
            .clicked()
        {
            self.refresh_killmails();
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
                for victim in &self.store.manually_protected_characters {
                    ui.horizontal(|ui| {
                        ui.label(format!("Character: {} ({})", victim.name, victim.id));
                        if ui
                            .add_enabled(
                                self.persistence_blocked.is_none(),
                                egui::Button::new("Remove"),
                            )
                            .clicked()
                        {
                            remove = Some((ProtectedVictimKind::Character, victim.id));
                        }
                    });
                }
                for victim in &self.store.manually_protected_corporations {
                    ui.horizontal(|ui| {
                        ui.label(format!("Corporation: {} ({})", victim.name, victim.id));
                        if ui
                            .add_enabled(
                                self.persistence_blocked.is_none(),
                                egui::Button::new("Remove"),
                            )
                            .clicked()
                        {
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
                        .add_enabled(
                            self.persisted_controls_enabled(),
                            egui::Button::new("Add"),
                        )
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

    fn show_posting_status(&self, ui: &mut egui::Ui) {
        ui.heading("Posting status");
        if self.store.cached_killmails.is_empty() {
            ui.label("Load recent killmails to see what can be posted.");
        } else {
            let summary = posting_summary(&self.store, &self.store.cached_killmails, unix_time());
            ui.label(format!(
                "{} killmail{} {} eligible for bulk posting.",
                summary.eligible_for_bulk_posting,
                if summary.eligible_for_bulk_posting == 1 {
                    ""
                } else {
                    "s"
                },
                if summary.eligible_for_bulk_posting == 1 {
                    "is"
                } else {
                    "are"
                }
            ));
            ui.label(format!(
                "{} protected killmail{} {} excluded from bulk posting.",
                summary.protected,
                if summary.protected == 1 { "" } else { "s" },
                if summary.protected == 1 { "is" } else { "are" }
            ));
            for (reason, count) in summary.protection_reasons {
                ui.label(format!(
                    "  {count} protected by {}",
                    protection_reason_label(&reason)
                ));
            }
            if summary.awaiting_status > 0 {
                ui.label(format!(
                    "{} unprotected killmail{} still need{} a status check before posting.",
                    summary.awaiting_status,
                    if summary.awaiting_status == 1 {
                        ""
                    } else {
                        "s"
                    },
                    if summary.awaiting_status == 1 {
                        "s"
                    } else {
                        ""
                    }
                ));
            } else if summary.eligible_for_bulk_posting == 0 && summary.protected == 0 {
                ui.label("No unreported killmails need action.");
            }
        }
    }

    fn show_activity(&self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("Activity details")
            .default_open(false)
            .show(ui, |ui| {
                ui.label(&self.latest_status);
                egui::ScrollArea::vertical()
                    .max_height(140.0)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for message in &self.status_history {
                            ui.label(message);
                        }
                    });
            });
    }

    fn show_session_reports(&self, ui: &mut egui::Ui) {
        if self.session_reports.is_empty() {
            return;
        }
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

    fn show_recent_killmails(&mut self, ui: &mut egui::Ui) {
        ui.heading("Recent killmails");
        if self.store.cached_killmails.is_empty() {
            ui.label("No killmails loaded.");
        }
        let now = unix_time();
        let unreported_count = self
            .store
            .cached_killmails
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
        let show_protected_changed = ui
            .add_enabled_ui(self.persistence_blocked.is_none(), |ui| {
                ui.checkbox(
                    &mut self.store.show_protected_killmails,
                    "Show protected killmails",
                )
                .changed()
            })
            .inner;
        if show_protected_changed {
            self.persist_or_log_error();
        }

        let mut post_mail = None;
        let visible_killmails = displayed_killmails(&self.store, &self.store.cached_killmails, now);
        if !self.store.cached_killmails.is_empty() && visible_killmails.is_empty() {
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
                        let protection_labels = protection_reasons
                            .iter()
                            .map(protection_reason_label)
                            .collect::<Vec<_>>();
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
                                            protection_labels.join(", ")
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
                                            protection_labels.join(", ")
                                        ),
                                    );
                                }
                            }
                        }
                    });
                }
            });
        if let Some(mail) = post_mail {
            self.start_posts(vec![mail], SubmissionMode::Individual);
        }
    }

    fn show_bulk_confirmation(&mut self, ctx: &egui::Context) {
        let Some(count) = self.pending_bulk.as_ref().map(Vec::len) else {
            return;
        };
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
                self.start_posts(mails, SubmissionMode::Bulk);
            }
        } else if cancel {
            self.pending_bulk = None;
            self.log("Bulk submission cancelled");
        }
    }

    fn show_character_removal_confirmation(&mut self, ctx: &egui::Context) {
        let Some(character) = self.pending_character_removal.as_ref() else {
            return;
        };
        let name = character.name.clone();
        let mut confirm = false;
        let mut cancel = false;
        egui::Window::new("Remove authenticated character")
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(format!(
                    "Remove {name} and delete its refresh token from this application?"
                ));
                ui.horizontal(|ui| {
                    confirm = ui.button("Remove character").clicked();
                    cancel = ui.button("Cancel").clicked();
                });
            });
        if confirm {
            if let Some(character) = self.pending_character_removal.take() {
                self.remove_character(character);
            }
        } else if cancel {
            self.pending_character_removal = None;
            self.log("Character removal cancelled");
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_worker();
        egui::CentralPanel::default().show(ctx, |ui| {
            self.show_header(ui);
            ui.separator();
            self.show_authenticated_characters(ui);
            ui.separator();
            self.show_posting_status(ui);
            self.show_activity(ui);
            self.show_session_reports(ui);
            ui.separator();
            self.show_recent_killmails(ui);
        });
        self.show_bulk_confirmation(ctx);
        self.show_character_removal_confirmation(ctx);

        if self.is_busy() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }
}

fn protection_reason_label(reason: &ProtectionReason) -> String {
    match reason {
        ProtectionReason::AuthenticatedCharacter(name) => {
            format!("authenticated character {name}")
        }
        ProtectionReason::AuthenticatedCorporation(name) => {
            format!("authenticated corporation {name}")
        }
        ProtectionReason::ManuallyProtectedCharacter(name) => format!("character {name}"),
        ProtectionReason::ManuallyProtectedCorporation(name) => format!("corporation {name}"),
    }
}
