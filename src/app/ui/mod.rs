use super::{
    worker::{unix_time, IdentityImageKey},
    App, IdentityImageState, SessionReportStatus, SubmissionMode,
};
use crate::{
    killmail::{displayed_killmails, is_bulk_candidate, posting_summary, ReportState},
    models::{Killmail, KillmailAttacker, KillmailItem, ProtectedVictimKind},
};
use eframe::egui;
use std::time::Duration;

mod components;
mod dialogs;
mod killmail;
mod sidebar;
mod theme;

use components::*;
use dialogs::{confirmation_dialog, ConfirmationAction, ConfirmationDialog};
#[cfg(test)]
use killmail::{fitting_rows, format_number, ordered_attackers};
use killmail::{killmail_card, killmail_image_keys, KillmailCardContext};
use sidebar::{sidebar, ProtectedVictimDraft, SidebarAction, SidebarProps};
use theme::*;

impl App {
    fn show_top_bar(&mut self, ui: &mut egui::Ui) {
        let mut cancel_authentication = false;
        let pill_status = self.status_pill_text();
        let simulation_notice = self.simulation_name.as_ref().map(|name| {
            (
                format!("SIMULATION - Offline scenario {name:?}"),
                format!(
                    "SIMULATION - Offline scenario {name:?}. No EVE, zKillboard, credential-store, or image-service requests can be made."
                ),
            )
        });
        egui::Frame::new()
            .fill(SURFACE)
            .inner_margin(egui::Margin::symmetric(20, 16))
            .show(ui, |ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), 48.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.vertical(|ui| {
                            ui.label(
                                egui::RichText::new("EVE KILLMAIL PUBLISHER")
                                    .size(28.0)
                                    .strong()
                                    .color(ACCENT),
                            );
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            status_badge(ui, self.is_busy(), &pill_status);
                            if self.is_authenticating() && ui.button("Cancel connection").clicked()
                            {
                                cancel_authentication = true;
                            }
                            if let Some((label, message)) = &simulation_notice {
                                let response = egui::Frame::new()
                                    .fill(ACCENT.gamma_multiply(0.14))
                                    .stroke(egui::Stroke::new(1.0, ACCENT.gamma_multiply(0.7)))
                                    .corner_radius(12)
                                    .inner_margin(egui::Margin::symmetric(10, 5))
                                    .show(ui, |ui| {
                                        ui.label(
                                            egui::RichText::new(label)
                                                .size(13.0)
                                                .strong()
                                                .color(ACCENT),
                                        );
                                    })
                                    .response;
                                response.on_hover_ui(|ui| {
                                    ui.set_max_width(460.0);
                                    ui.label(egui::RichText::new(message).color(ACCENT));
                                });
                            }
                        });
                    },
                );
            });
        if cancel_authentication {
            self.cancel_authentication();
        }
    }

    fn show_warnings(&self, ui: &mut egui::Ui) {
        if let Some(error) = &self.persistence_blocked {
            notice(
                ui,
                DANGER,
                "LOCAL STATE UNAVAILABLE",
                &format!("Saving is disabled to avoid overwriting unreadable data. {error}"),
            );
        }
        if self.has_json_refresh_token_fallback() {
            notice(
                ui,
                WARNING,
                "CREDENTIAL STORAGE WARNING",
                "A refresh token is stored in ekmp.json because the system credential store was unavailable.",
            );
        }
    }

    fn show_sidebar(&mut self, ui: &mut egui::Ui) {
        let image_keys = self
            .store
            .characters
            .iter()
            .flat_map(|character| {
                [
                    Some(IdentityImageKey::Character(character.id)),
                    character.corporation_id.map(IdentityImageKey::Corporation),
                ]
            })
            .flatten()
            .collect::<Vec<_>>();
        for key in image_keys {
            self.queue_identity_image(key);
        }

        let controls_enabled = self.persisted_controls_enabled();
        let action = sidebar(
            ui,
            SidebarProps {
                characters: &self.store.characters,
                manually_protected_characters: &self.store.manually_protected_characters,
                manually_protected_corporations: &self.store.manually_protected_corporations,
                images: &self.identity_images,
                latest_status: &self.latest_status,
                status_history: &self.status_history,
                controls_enabled,
                persistence_enabled: self.persistence_blocked.is_none(),
            },
            ProtectedVictimDraft {
                kind: &mut self.new_protected_victim_kind,
                query: &mut self.new_protected_victim_query,
            },
        );

        match action {
            Some(SidebarAction::ConnectCharacter) => self.begin_auth(),
            Some(SidebarAction::DisconnectCharacter(id)) => {
                self.pending_character_removal = self
                    .store
                    .characters
                    .iter()
                    .find(|character| character.id == id)
                    .cloned();
            }
            Some(SidebarAction::AddProtectedVictim) => self.begin_add_protected_victim(),
            Some(SidebarAction::RemoveProtectedVictim(kind, id)) => {
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
            None => {}
        }
    }

    fn show_review_workspace(&mut self, ui: &mut egui::Ui) {
        ui.style_mut()
            .text_styles
            .insert(egui::TextStyle::Body, egui::FontId::proportional(16.0));
        ui.style_mut()
            .text_styles
            .insert(egui::TextStyle::Button, egui::FontId::proportional(15.5));
        ui.style_mut()
            .text_styles
            .insert(egui::TextStyle::Small, egui::FontId::proportional(13.5));
        ui.style_mut()
            .text_styles
            .insert(egui::TextStyle::Heading, egui::FontId::proportional(23.0));

        self.show_queue_toolbar(ui);
        ui.add_space(8.0);

        let pane_height = ui.available_height().max(120.0);
        egui::ScrollArea::vertical()
            .id_salt("review_workspace")
            .max_height(pane_height)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                self.show_recent_killmails(ui);
                self.show_session_reports(ui);
            });
    }

    fn show_queue_toolbar(&mut self, ui: &mut egui::Ui) {
        let now = unix_time();
        let summary = posting_summary(&self.store, &self.store.cached_killmails, now);
        let eligible_count = self
            .store
            .cached_killmails
            .iter()
            .filter(|mail| is_bulk_candidate(&self.store, mail, now))
            .count();

        egui::Frame::new()
            .fill(SURFACE)
            .stroke(egui::Stroke::new(1.0, BORDER))
            .corner_radius(8)
            .inner_margin(12)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.heading("Killmails");
                        ui.label(
                            egui::RichText::new(format!(
                                "{} eligible · {} protected · {} checking",
                                summary.eligible_for_bulk_posting,
                                summary.protected,
                                summary.awaiting_status,
                            ))
                            .small()
                            .color(MUTED),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let refresh_enabled =
                            self.persisted_controls_enabled() && !self.store.characters.is_empty();
                        let refresh = accessible_button(
                            ui,
                            refresh_enabled,
                            egui::Button::new("Refresh killmails").fill(SURFACE_RAISED),
                            "Refresh killmails",
                        );
                        if refresh.clicked() {
                            self.refresh_killmails();
                        }

                        if ui
                            .add_enabled(
                                !self.is_busy() && eligible_count > 0,
                                egui::Button::new(format!("Post eligible ({eligible_count})"))
                                    .fill(ACCENT_DARK),
                            )
                            .on_hover_text(
                                "Opens a confirmation. Protected victims are always excluded.",
                            )
                            .clicked()
                        {
                            self.request_bulk_post();
                        }

                        let changed = ui
                            .add_enabled_ui(self.persistence_blocked.is_none(), |ui| {
                                ui.checkbox(
                                    &mut self.store.show_protected_killmails,
                                    "Show protected killmails",
                                )
                                .changed()
                            })
                            .inner;
                        if changed {
                            self.persist_or_log_error();
                        }
                    });
                });
            });
    }

    fn show_recent_killmails(&mut self, ui: &mut egui::Ui) {
        let now = unix_time();
        let image_keys = self
            .store
            .cached_killmails
            .iter()
            .filter(|mail| self.expanded_killmail_ids.contains(&mail.id))
            .flat_map(killmail_image_keys)
            .collect::<Vec<_>>();
        for key in image_keys {
            self.queue_identity_image(key);
        }
        let visible_killmails = displayed_killmails(&self.store, &self.store.cached_killmails, now);
        if self.store.cached_killmails.is_empty() {
            empty_queue(
                ui,
                if self.store.characters.is_empty() {
                    "Connect a character to get started"
                } else {
                    "Your review queue is empty"
                },
                if self.store.characters.is_empty() {
                    "Use the connection panel to authenticate with EVE SSO."
                } else {
                    "Refresh to load recent killmails from your connected characters."
                },
            );
            return;
        }
        if visible_killmails.is_empty() {
            empty_queue(
                ui,
                "Nothing matches this view",
                "Protected killmails are hidden. Enable the filter above to review them.",
            );
            return;
        }

        let mut post_mail = None;
        let mut toggle_protection = None;
        let card_context = KillmailCardContext {
            store: &self.store,
            now,
            busy: self.is_busy(),
            protection_controls_enabled: self.persisted_controls_enabled(),
            images: &self.identity_images,
        };
        for mail in visible_killmails {
            let expanded = self.expanded_killmail_ids.contains(&mail.id);
            if killmail_card(
                ui,
                &card_context,
                mail,
                expanded,
                &mut post_mail,
                &mut toggle_protection,
            ) {
                self.expanded_killmail_ids.insert(mail.id);
            } else {
                self.expanded_killmail_ids.remove(&mail.id);
            }
            ui.add_space(6.0);
        }
        if let Some(mail) = post_mail {
            self.start_posts(vec![mail], SubmissionMode::Individual);
        }
        if let Some(killmail_id) = toggle_protection {
            if let Some(index) = self
                .store
                .manually_protected_killmail_ids
                .iter()
                .position(|id| *id == killmail_id)
            {
                self.store.manually_protected_killmail_ids.remove(index);
                self.persist_or_log_error();
                self.log(format!(
                    "Removed protection flag from killmail {killmail_id}"
                ));
            } else {
                self.store.manually_protected_killmail_ids.push(killmail_id);
                self.persist_or_log_error();
                self.log(format!("Flagged killmail {killmail_id} for protection"));
            }
        }
    }

    fn show_session_reports(&self, ui: &mut egui::Ui) {
        if self.session_reports.is_empty() {
            return;
        }
        ui.add_space(18.0);
        section_label(ui, "REPORTED THIS SESSION");
        egui::Frame::new()
            .fill(SURFACE)
            .stroke(egui::Stroke::new(1.0, BORDER))
            .corner_radius(8)
            .inner_margin(12)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                for report in &self.session_reports {
                    ui.horizontal(|ui| {
                        let status = match report.status {
                            SessionReportStatus::Submitted => "Submitted",
                            SessionReportStatus::AlreadyPresent => "Already on zKillboard",
                        };
                        ui.label(egui::RichText::new("OK").small().color(SUCCESS));
                        ui.label(format!("Killmail {} - {status}", report.killmail_id));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if self.simulation_name.is_some() {
                                ui.label("Simulated result");
                            } else {
                                ui.hyperlink_to("Open", &report.url);
                            }
                        });
                    });
                }
            });
    }

    fn show_bulk_confirmation(&mut self, ctx: &egui::Context) {
        let Some(count) = self.pending_bulk.as_ref().map(Vec::len) else {
            return;
        };
        let heading = format!("Post {count} eligible killmails?");
        match confirmation_dialog(
            ctx,
            ConfirmationDialog {
                window_title: "Confirm bulk posting",
                heading: &heading,
                message: "Each killmail was confirmed as unreported. Protected victims are excluded and eligibility is checked again before posting.",
                confirm_label: "Post to zKillboard",
                confirm_accessible_label: "Confirm bulk post",
                confirm_color: ACCENT_DARK,
                min_width: 390.0,
            },
        ) {
            ConfirmationAction::Confirmed => {
                if let Some(mails) = self.pending_bulk.take() {
                    self.start_posts(mails, SubmissionMode::Bulk);
                }
            }
            ConfirmationAction::Cancelled => {
                self.pending_bulk = None;
                self.log("Bulk submission cancelled");
            }
            ConfirmationAction::None => {}
        }
    }

    fn show_character_removal_confirmation(&mut self, ctx: &egui::Context) {
        let Some(character) = self.pending_character_removal.as_ref() else {
            return;
        };
        let name = character.name.clone();
        let heading = format!("Disconnect {name}?");
        let accessible_label = format!("Confirm disconnect {name}");
        match confirmation_dialog(
            ctx,
            ConfirmationDialog {
                window_title: "Disconnect character",
                heading: &heading,
                message: "The character, its refresh token, and killmails available only through this character will be removed.",
                confirm_label: "Disconnect character",
                confirm_accessible_label: &accessible_label,
                confirm_color: DANGER,
                min_width: 380.0,
            },
        ) {
            ConfirmationAction::Confirmed => {
                if let Some(character) = self.pending_character_removal.take() {
                    self.remove_character(character);
                }
            }
            ConfirmationAction::Cancelled => {
                self.pending_character_removal = None;
                self.log("Character removal cancelled");
            }
            ConfirmationAction::None => {}
        }
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_worker();
        self.poll_identity_images(ctx);
        if self.is_busy() || self.identity_images_loading() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        apply_theme(&ctx);

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(egui::Color32::from_rgb(14, 19, 24)))
            .show(ui, |ui| {
                self.show_top_bar(ui);
                self.show_warnings(ui);
                ui.add_space(10.0);
                egui::Panel::left("workspace_navigation")
                    .default_size(300.0)
                    .min_size(270.0)
                    .max_size(460.0)
                    .resizable(true)
                    .show_separator_line(true)
                    .frame(
                        egui::Frame::new()
                            .fill(SURFACE)
                            .stroke(egui::Stroke::new(1.0, BORDER))
                            .inner_margin(16),
                    )
                    .show(ui, |ui| self.show_sidebar(ui));
                egui::CentralPanel::default()
                    .frame(
                        egui::Frame::new()
                            .fill(egui::Color32::from_rgb(14, 19, 24))
                            .inner_margin(egui::Margin {
                                left: 18,
                                right: 2,
                                top: 0,
                                bottom: 0,
                            }),
                    )
                    .show(ui, |ui| {
                        self.show_review_workspace(ui);
                    });
            });
        self.show_bulk_confirmation(&ctx);
        self.show_character_removal_confirmation(&ctx);
        if self.identity_images_loading() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integrations::simulation;
    use egui_kittest::{kittest::Queryable, Harness};
    use std::sync::Arc;

    fn attacker(name: &str, damage_done: u64, final_blow: bool) -> KillmailAttacker {
        KillmailAttacker {
            character_id: None,
            character_name: Some(name.into()),
            corporation_id: None,
            corporation_name: None,
            alliance_id: None,
            alliance_name: None,
            faction_id: None,
            faction_name: None,
            ship_type_id: None,
            ship_name: None,
            weapon_type_id: None,
            weapon_name: None,
            damage_done,
            final_blow,
            security_status: None,
        }
    }

    fn item(type_id: u64, name: &str, flag: u32, destroyed: u64, dropped: u64) -> KillmailItem {
        KillmailItem {
            item_type_id: type_id,
            name: name.into(),
            flag,
            quantity_destroyed: destroyed,
            quantity_dropped: dropped,
            singleton: 0,
            items: Vec::new(),
        }
    }

    #[test]
    fn attackers_put_final_blow_before_top_damage_and_remaining_damage() {
        let attackers = [
            attacker("Other", 200, false),
            attacker("Top", 900, false),
            attacker("Final", 100, true),
        ];

        let ordered = ordered_attackers(&attackers)
            .into_iter()
            .map(|attacker| attacker.character_name.as_deref().unwrap())
            .collect::<Vec<_>>();

        assert_eq!(ordered, ["Final", "Top", "Other"]);
    }

    #[test]
    fn fitting_rows_group_slots_aggregate_quantities_and_keep_unknown_flags() {
        let mut container = item(3, "Container", 5, 1, 0);
        container.items.push(item(4, "Nested Cargo", 5, 2, 3));
        let rows = fitting_rows(&[
            item(1, "Gun", 27, 1, 0),
            item(1, "Gun", 27, 0, 2),
            item(2, "Future Item", 222, 1, 0),
            container,
        ]);

        assert_eq!(rows[0].section, "High Power Slots");
        assert_eq!(rows[0].destroyed, 1);
        assert_eq!(rows[0].dropped, 2);
        assert!(rows.iter().any(|row| row.name == "Nested Cargo"));
        assert!(rows.iter().any(|row| row.section == "Other (flag 222)"));
    }

    #[test]
    fn damage_and_quantity_formatting_is_stable() {
        assert_eq!(format_number(0), "0");
        assert_eq!(format_number(12_345_678), "12,345,678");
    }

    fn mixed_harness() -> (Harness<'static, App>, Arc<simulation::SimulatorBackend>) {
        let loaded = simulation::load("mixed").unwrap();
        let backend = Arc::new(loaded.backend);
        let app = App::simulated(loaded.store, backend.clone(), loaded.name, None, true);
        let harness = Harness::builder()
            .with_size(egui::vec2(1180.0, 760.0))
            .build_eframe(move |_| app);
        (harness, backend)
    }

    #[test]
    fn simulator_ui_refreshes_on_startup_and_posts_only_after_a_button_click() {
        let (mut harness, backend) = mixed_harness();

        assert!(backend.posted_ids().is_empty());
        harness.run_steps(4);

        assert!(harness.query_by_label("Eligible Example").is_some());
        assert!(harness
            .query_by_label_contains("Est. cost 2.9M ISK")
            .is_some());
        assert!(harness.query_by_label("Post killmail 9001").is_none());
        assert!(backend.posted_ids().is_empty());

        harness
            .get_by_label("Expand killmail 9001")
            .click_accesskit();
        harness.run_steps(2);
        assert!(harness.query_by_label("Post killmail 9001").is_some());
        assert!(harness.query_by_label("INVOLVED PARTIES").is_some());
        assert!(harness.query_by_label("FITTING AND CONTENT").is_some());
        assert!(harness.query_by_label("Final Blow Example").is_some());
        assert!(harness.query_by_label("High Power Slots").is_some());

        harness
            .get_by_label("Collapse killmail 9001")
            .click_accesskit();
        harness.run_steps(2);
        assert!(harness.query_by_label("Post killmail 9001").is_none());

        harness
            .get_by_label("Expand killmail 9001")
            .click_accesskit();
        harness.run_steps(2);
        harness.get_by_label("Post killmail 9001").click_accesskit();
        harness.run_steps(4);

        assert_eq!(backend.posted_ids(), vec![9001]);
        assert!(harness
            .query_by_label_contains("Killmail 9001 - Submitted")
            .is_some());
    }

    #[test]
    fn killmail_protection_can_be_toggled_from_the_expanded_card() {
        let (mut harness, _backend) = mixed_harness();
        harness.run_steps(4);
        harness
            .get_by_label("Expand killmail 9001")
            .click_accesskit();
        harness.run_steps(2);

        harness
            .get_by_label("Protect killmail 9001")
            .click_accesskit();
        harness.run_steps(2);

        assert_eq!(
            harness.state().store.manually_protected_killmail_ids,
            vec![9001]
        );
        assert!(harness.query_by_label("Eligible Example").is_none());
        assert!(harness.query_by_label("Post eligible (1)").is_some());

        harness.state_mut().store.show_protected_killmails = true;
        harness.run_steps(2);
        assert!(harness
            .query_by_label_contains("killmail flagged for protection")
            .is_some());
        harness
            .get_by_label("Remove protection flag from killmail 9001")
            .click_accesskit();
        harness.run_steps(2);

        assert!(harness
            .state()
            .store
            .manually_protected_killmail_ids
            .is_empty());
        assert!(harness.query_by_label("Post eligible (2)").is_some());
    }

    #[test]
    fn bulk_confirmation_revalidates_protection_in_the_ui_workflow() {
        let (mut harness, backend) = mixed_harness();
        harness.run_steps(4);
        harness.get_by_label("Post eligible (2)").click_accesskit();
        harness.run_steps(2);

        harness
            .state_mut()
            .store
            .manually_protected_killmail_ids
            .push(9006);
        harness.get_by_label("Confirm bulk post").click_accesskit();
        harness.run_steps(4);

        assert_eq!(backend.posted_ids(), vec![9001]);
    }
}
