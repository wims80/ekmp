use super::{
    worker::{unix_time, IdentityImageKey},
    App, IdentityImageState, SessionReportStatus, SubmissionMode,
};
use crate::{
    killmail::{
        displayed_killmails, is_bulk_candidate, posting_summary, protected_victim_reasons,
        report_state, ProtectionReason, ReportState,
    },
    models::{Killmail, KillmailAttacker, KillmailItem, ProtectedVictimKind},
};
use eframe::egui;
use std::{collections::HashSet, time::Duration};

const ACCENT: egui::Color32 = egui::Color32::from_rgb(72, 181, 196);
const ACCENT_DARK: egui::Color32 = egui::Color32::from_rgb(35, 112, 124);
const SURFACE: egui::Color32 = egui::Color32::from_rgb(21, 27, 34);
const SURFACE_RAISED: egui::Color32 = egui::Color32::from_rgb(27, 35, 43);
const BORDER: egui::Color32 = egui::Color32::from_rgb(51, 64, 75);
const MUTED: egui::Color32 = egui::Color32::from_rgb(145, 158, 169);
const SUCCESS: egui::Color32 = egui::Color32::from_rgb(103, 194, 142);
const WARNING: egui::Color32 = egui::Color32::from_rgb(224, 177, 89);
const DANGER: egui::Color32 = egui::Color32::from_rgb(224, 112, 112);

impl App {
    fn apply_theme(ctx: &egui::Context) {
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = egui::Color32::from_rgb(14, 19, 24);
        visuals.window_fill = SURFACE;
        visuals.faint_bg_color = SURFACE_RAISED;
        visuals.extreme_bg_color = egui::Color32::from_rgb(10, 14, 18);
        visuals.selection.bg_fill = ACCENT_DARK;
        visuals.hyperlink_color = ACCENT;
        visuals.widgets.noninteractive.bg_fill = SURFACE;
        visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, BORDER);
        visuals.widgets.inactive.bg_fill = SURFACE_RAISED;
        visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, BORDER);
        visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(38, 49, 59);
        visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, ACCENT);
        visuals.widgets.active.bg_fill = ACCENT_DARK;
        visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0, ACCENT);
        ctx.set_visuals(visuals);

        let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
        style.spacing.item_spacing = egui::vec2(10.0, 8.0);
        style.spacing.button_padding = egui::vec2(14.0, 8.0);
        style.spacing.interact_size.y = 34.0;
        ctx.set_style_of(egui::Theme::Dark, style);
    }

    fn show_top_bar(&mut self, ui: &mut egui::Ui) {
        let mut cancel_authentication = false;
        let pill_status = self.status_pill_text();
        egui::Frame::new()
            .fill(SURFACE)
            .inner_margin(egui::Margin::symmetric(20, 16))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new("EVE KILLMAIL PUBLISHER")
                                .size(11.0)
                                .strong()
                                .color(ACCENT),
                        );
                        ui.label(
                            egui::RichText::new("Review before you report")
                                .size(22.0)
                                .strong(),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        status_badge(ui, self.is_busy(), &pill_status);
                        if self.is_authenticating() && ui.button("Cancel connection").clicked() {
                            cancel_authentication = true;
                        }
                    });
                });
            });
        if cancel_authentication {
            self.cancel_authentication();
        }
    }

    fn show_warnings(&self, ui: &mut egui::Ui) {
        if let Some(name) = &self.simulation_name {
            notice(
                ui,
                ACCENT,
                "SIMULATION",
                &format!(
                    "Offline scenario {name:?}. No EVE, zKillboard, credential-store, or image-service requests can be made."
                ),
            );
        }
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

        ui.style_mut()
            .text_styles
            .insert(egui::TextStyle::Body, egui::FontId::proportional(15.5));
        ui.style_mut()
            .text_styles
            .insert(egui::TextStyle::Button, egui::FontId::proportional(15.0));
        ui.style_mut()
            .text_styles
            .insert(egui::TextStyle::Small, egui::FontId::proportional(13.5));

        let pane_height = ui.available_height().max(120.0);
        egui::ScrollArea::vertical()
            .id_salt("workspace_sidebar")
            .max_height(pane_height)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_min_width(250.0);
                section_label(ui, "CONNECTED CHARACTERS");
                ui.label(
                    egui::RichText::new("Choose which pilots supply recent killmails.")
                        .small()
                        .color(MUTED),
                );
                ui.add_space(6.0);

                if self.store.characters.is_empty() {
                    empty_sidebar_card(
                        ui,
                        "No characters connected",
                        "Authenticate with EVE SSO to begin reviewing killmails.",
                    );
                }

                let mut remove_character = None;
                for character in &self.store.characters {
                    egui::Frame::new()
                        .fill(SURFACE_RAISED)
                        .stroke(egui::Stroke::new(1.0, BORDER))
                        .corner_radius(8)
                        .inner_margin(12)
                        .show(ui, |ui| {
                            ui.set_min_width(ui.available_width().max(200.0));
                            ui.horizontal_top(|ui| {
                                ui.vertical(|ui| {
                                    identity_image(
                                        ui,
                                        self.identity_images
                                            .get(&IdentityImageKey::Character(character.id)),
                                        64.0,
                                        character.name.chars().next().unwrap_or('?'),
                                        "Character portrait",
                                    );
                                    if let Some(corporation_id) = character.corporation_id {
                                        ui.horizontal(|ui| {
                                            ui.add_space(16.0);
                                            identity_image(
                                                ui,
                                                self.identity_images.get(
                                                    &IdentityImageKey::Corporation(corporation_id),
                                                ),
                                                32.0,
                                                'C',
                                                "Corporation logo",
                                            );
                                        });
                                    }
                                });
                                ui.vertical(|ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            egui::RichText::new("●").color(SUCCESS).size(10.0),
                                        );
                                        ui.add(
                                            egui::Label::new(
                                                egui::RichText::new(&character.name).strong(),
                                            )
                                            .truncate(),
                                        );
                                    });
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(format!(
                                                "Character {}",
                                                character.id
                                            ))
                                            .small()
                                            .color(MUTED),
                                        )
                                        .truncate(),
                                    );
                                    if let (Some(corporation_id), Some(corporation)) =
                                        (character.corporation_id, &character.corporation_name)
                                    {
                                        ui.add_space(12.0);
                                        ui.add(egui::Label::new(corporation).truncate());
                                        ui.label(
                                            egui::RichText::new(format!(
                                                "Corporation {corporation_id}"
                                            ))
                                            .small()
                                            .color(MUTED),
                                        );
                                    }
                                });
                            });
                            let enabled = self.persisted_controls_enabled();
                            let response = ui.add_enabled(enabled, egui::Button::new("Disconnect"));
                            response.widget_info(|| {
                                egui::WidgetInfo::labeled(
                                    egui::WidgetType::Button,
                                    enabled,
                                    format!("Disconnect {}", character.name),
                                )
                            });
                            if response.clicked() {
                                remove_character = Some(character.clone());
                            }
                        });
                }
                if let Some(character) = remove_character {
                    self.pending_character_removal = Some(character);
                }

                if ui
                    .add_enabled(
                        self.persisted_controls_enabled(),
                        egui::Button::new("+  Connect another character")
                            .fill(ACCENT_DARK)
                            .min_size(egui::vec2(ui.available_width(), 38.0)),
                    )
                    .clicked()
                {
                    self.begin_auth();
                }

                ui.add_space(22.0);
                self.show_protected_victims(ui);

                ui.add_space(22.0);
                section_label(ui, "ACTIVITY");
                egui::Frame::new()
                    .fill(SURFACE_RAISED)
                    .corner_radius(8)
                    .inner_margin(12)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.label(egui::RichText::new("Current status").strong().color(ACCENT));
                        ui.label(&self.latest_status);
                        egui::CollapsingHeader::new(format!(
                            "Activity log · {} entries",
                            self.status_history.len()
                        ))
                        .default_open(false)
                        .show(ui, |ui| {
                            egui::ScrollArea::vertical()
                                .id_salt("activity_log")
                                .min_scrolled_height(300.0)
                                .max_height(420.0)
                                .stick_to_bottom(true)
                                .show(ui, |ui| {
                                    ui.vertical(|ui| {
                                        ui.set_width(ui.available_width());
                                        for (index, message) in
                                            self.status_history.iter().enumerate()
                                        {
                                            egui::Frame::new()
                                                .fill(if index % 2 == 0 {
                                                    SURFACE
                                                } else {
                                                    SURFACE_RAISED
                                                })
                                                .corner_radius(4)
                                                .inner_margin(egui::Margin::symmetric(8, 7))
                                                .show(ui, |ui| {
                                                    ui.set_width(ui.available_width());
                                                    ui.add(
                                                        egui::Label::new(
                                                            egui::RichText::new(message)
                                                                .color(MUTED),
                                                        )
                                                        .wrap(),
                                                    );
                                                });
                                            ui.add_space(3.0);
                                        }
                                    });
                                });
                        });
                    });
            });
    }

    fn show_protected_victims(&mut self, ui: &mut egui::Ui) {
        let automatic_count = self.store.characters.len()
            + self
                .store
                .characters
                .iter()
                .filter_map(|character| {
                    character
                        .corporation_name
                        .as_ref()
                        .and(character.corporation_id)
                })
                .collect::<HashSet<_>>()
                .len();
        let manual_count = self.store.manually_protected_characters.len()
            + self.store.manually_protected_corporations.len();

        section_label(ui, "PROTECTED VICTIMS");
        ui.label(
            egui::RichText::new("Excluded from bulk posting. Individual posting remains explicit.")
                .small()
                .color(MUTED),
        );
        ui.add_space(5.0);
        egui::CollapsingHeader::new(format!(
            "{} automatic  ·  {} manual",
            automatic_count, manual_count
        ))
        .default_open(false)
        .show(ui, |ui| {
            ui.label(egui::RichText::new("Automatic protection").strong());
            for character in &self.store.characters {
                ui.label(format!("Character · {}", character.name));
            }
            let mut corporation_ids = HashSet::new();
            for character in &self.store.characters {
                if let (Some(id), Some(name)) =
                    (character.corporation_id, &character.corporation_name)
                {
                    if corporation_ids.insert(id) {
                        ui.label(format!("Corporation · {name}"));
                    }
                }
            }
            if automatic_count == 0 {
                ui.label(egui::RichText::new("None yet").color(MUTED));
            }

            ui.add_space(10.0);
            ui.label(egui::RichText::new("Manual protection").strong());
            let mut remove = None;
            for victim in &self.store.manually_protected_characters {
                protected_victim_row(
                    ui,
                    "Character",
                    &victim.name,
                    victim.id,
                    self.persistence_blocked.is_none(),
                    &mut remove,
                    ProtectedVictimKind::Character,
                );
            }
            for victim in &self.store.manually_protected_corporations {
                protected_victim_row(
                    ui,
                    "Corporation",
                    &victim.name,
                    victim.id,
                    self.persistence_blocked.is_none(),
                    &mut remove,
                    ProtectedVictimKind::Corporation,
                );
            }
            if manual_count == 0 {
                ui.label(egui::RichText::new("No manually protected victims").color(MUTED));
            }

            ui.add_space(8.0);
            egui::ComboBox::from_id_salt("protected_victim_kind")
                .selected_text(match self.new_protected_victim_kind {
                    ProtectedVictimKind::Character => "Character",
                    ProtectedVictimKind::Corporation => "Corporation",
                })
                .width(ui.available_width())
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
            let input_label = ui.label("Protected victim name or ID");
            ui.add(
                egui::TextEdit::singleline(&mut self.new_protected_victim_query)
                    .hint_text("Exact name or EVE ID")
                    .desired_width(ui.available_width()),
            )
            .labelled_by(input_label.id);
            if ui
                .add_enabled(
                    self.persisted_controls_enabled(),
                    egui::Button::new("Add protected victim"),
                )
                .clicked()
            {
                self.begin_add_protected_victim();
            }

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
        });
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

        let pane_height = ui.available_height().max(120.0);
        egui::ScrollArea::vertical()
            .id_salt("review_workspace")
            .max_height(pane_height)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.heading("Review queue");
                        ui.label(
                            egui::RichText::new(
                                "Only confirmed, unreported killmails can be submitted.",
                            )
                            .color(MUTED),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let enabled =
                            self.persisted_controls_enabled() && !self.store.characters.is_empty();
                        let response = ui.add_enabled(
                            enabled,
                            egui::Button::new("↻  Refresh killmails").fill(SURFACE_RAISED),
                        );
                        response.widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::WidgetType::Button,
                                enabled,
                                "Refresh killmails",
                            )
                        });
                        if response.clicked() {
                            self.refresh_killmails();
                        }
                    });
                });
                ui.add_space(14.0);
                self.show_posting_summary(ui);
                ui.add_space(18.0);
                self.show_queue_toolbar(ui);
                ui.add_space(8.0);
                self.show_recent_killmails(ui);
                self.show_session_reports(ui);
            });
    }

    fn show_posting_summary(&self, ui: &mut egui::Ui) {
        let summary = posting_summary(&self.store, &self.store.cached_killmails, unix_time());
        ui.columns(3, |columns| {
            metric_card(
                &mut columns[0],
                summary.eligible_for_bulk_posting,
                "ELIGIBLE",
                "Ready for bulk posting",
                SUCCESS,
            );
            metric_card(
                &mut columns[1],
                summary.protected,
                "PROTECTED",
                "Excluded from bulk posting",
                WARNING,
            );
            metric_card(
                &mut columns[2],
                summary.awaiting_status,
                "CHECKING",
                "Awaiting zKillboard status",
                MUTED,
            );
        });
    }

    fn show_queue_toolbar(&mut self, ui: &mut egui::Ui) {
        let now = unix_time();
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
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
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
        let card_context = KillmailCardContext {
            store: &self.store,
            now,
            busy: self.is_busy(),
            images: &self.identity_images,
        };
        for mail in visible_killmails {
            let expanded = self.expanded_killmail_ids.contains(&mail.id);
            if killmail_card(ui, &card_context, mail, expanded, &mut post_mail) {
                self.expanded_killmail_ids.insert(mail.id);
            } else {
                self.expanded_killmail_ids.remove(&mail.id);
            }
            ui.add_space(8.0);
        }
        if let Some(mail) = post_mail {
            self.start_posts(vec![mail], SubmissionMode::Individual);
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
                        ui.label(egui::RichText::new("✓").color(SUCCESS));
                        ui.label(format!("Killmail {} · {status}", report.killmail_id));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if self.simulation_name.is_some() {
                                ui.label("Simulated result");
                            } else {
                                ui.hyperlink_to("Open ↗", &report.url);
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
        let mut confirm = false;
        let mut cancel = false;
        egui::Window::new("Confirm bulk posting")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.set_min_width(390.0);
                ui.heading(format!("Post {count} eligible killmails?"));
                ui.label(
                    "Each killmail was confirmed as unreported. Protected victims are excluded and eligibility is checked again before posting.",
                );
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    cancel = ui.button("Cancel").clicked();
                    let response = ui.add(
                        egui::Button::new("Post to zKillboard").fill(ACCENT_DARK),
                    );
                    response.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::WidgetType::Button,
                            true,
                            "Confirm bulk post",
                        )
                    });
                    confirm = response.clicked();
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
        egui::Window::new("Disconnect character")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.set_min_width(380.0);
                ui.heading(format!("Disconnect {name}?"));
                ui.label(
                    "The character, its refresh token, and killmails available only through this character will be removed.",
                );
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    cancel = ui.button("Cancel").clicked();
                    let response = ui.add(
                        egui::Button::new("Disconnect character").fill(DANGER),
                    );
                    response.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::WidgetType::Button,
                            true,
                            format!("Confirm disconnect {name}"),
                        )
                    });
                    confirm = response.clicked();
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
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_worker();
        self.poll_identity_images(ctx);
        if self.is_busy() || self.identity_images_loading() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        Self::apply_theme(&ctx);

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
                            .inner_margin(egui::Margin::symmetric(18, 0)),
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

struct KillmailCardContext<'a> {
    store: &'a crate::models::Store,
    now: u64,
    busy: bool,
    images: &'a std::collections::HashMap<IdentityImageKey, IdentityImageState>,
}

fn killmail_card(
    ui: &mut egui::Ui,
    context: &KillmailCardContext<'_>,
    mail: &Killmail,
    mut expanded: bool,
    post_mail: &mut Option<Killmail>,
) -> bool {
    let protection_reasons = protected_victim_reasons(context.store, mail);
    let protected = !protection_reasons.is_empty();
    let state = report_state(context.store, mail.id, context.now);
    let edge_color = if protected { WARNING } else { ACCENT_DARK };

    egui::Frame::new()
        .fill(SURFACE_RAISED)
        .stroke(egui::Stroke::new(1.0, edge_color))
        .corner_radius(8)
        .inner_margin(14)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new(&mail.victim).size(17.0).strong());
                    ui.label(
                        egui::RichText::new(format!(
                            "{}  ·  {}  ·  {}",
                            mail.ship,
                            estimated_value_label(mail.estimated_value_isk),
                            mail.time
                        ))
                        .color(MUTED),
                    );
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                    let arrow = if expanded { "⌄" } else { "›" };
                    let action = if expanded { "Collapse" } else { "Expand" };
                    let response = ui
                        .add(egui::Button::new(arrow).min_size(egui::vec2(28.0, 28.0)))
                        .on_hover_text(format!("{action} killmail details"));
                    response.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::WidgetType::Button,
                            true,
                            format!("{action} killmail {}", mail.id),
                        )
                    });
                    if response.clicked() {
                        expanded = !expanded;
                    }
                    match state {
                        ReportState::Reported => chip(ui, "REPORTED", SUCCESS),
                        ReportState::Unreported if protected => chip(ui, "PROTECTED", WARNING),
                        ReportState::Unreported => chip(ui, "READY", SUCCESS),
                        ReportState::Unknown => chip(ui, "CHECKING", MUTED),
                    }
                });
            });
            if expanded {
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(5.0);
                expanded_killmail(ui, mail, context.images);

                if protected {
                    ui.add_space(5.0);
                    let reasons = protection_reasons
                        .iter()
                        .map(protection_reason_label)
                        .collect::<Vec<_>>()
                        .join(", ");
                    ui.label(
                        egui::RichText::new(format!("Excluded from bulk posting · {reasons}"))
                            .small()
                            .color(WARNING),
                    );
                }

                if state == ReportState::Unreported {
                    ui.add_space(8.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let label = if protected {
                            "Post anyway"
                        } else {
                            "Post to zKillboard"
                        };
                        let response = ui.add_enabled(!context.busy, egui::Button::new(label));
                        response.widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::WidgetType::Button,
                                !context.busy,
                                if protected {
                                    format!("Post protected killmail {} anyway", mail.id)
                                } else {
                                    format!("Post killmail {}", mail.id)
                                },
                            )
                        });
                        if response.clicked() {
                            *post_mail = Some(mail.clone());
                        }
                    });
                }
            }
        });
    expanded
}

fn expanded_killmail(
    ui: &mut egui::Ui,
    mail: &Killmail,
    images: &std::collections::HashMap<IdentityImageKey, IdentityImageState>,
) {
    let sources = mail
        .sources
        .iter()
        .map(|source| source.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    ui.label(
        egui::RichText::new(format!("KILLMAIL {} · FROM {sources}", mail.id))
            .small()
            .color(MUTED),
    );
    ui.add_space(8.0);

    let Some(detail) = &mail.detail else {
        ui.label(
            egui::RichText::new("Detailed ESI data is unavailable; refresh killmails to load it.")
                .color(MUTED),
        );
        return;
    };

    egui::Frame::new()
        .fill(SURFACE)
        .stroke(egui::Stroke::new(1.0, BORDER))
        .corner_radius(6)
        .inner_margin(10)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                if let Some(character_id) = mail.victim_id {
                    identity_image(
                        ui,
                        images.get(&IdentityImageKey::Character(character_id)),
                        88.0,
                        mail.victim.chars().next().unwrap_or('?'),
                        "Victim portrait",
                    );
                }
                if let Some(ship_type_id) = detail.victim.ship_type_id {
                    identity_image(
                        ui,
                        images.get(&IdentityImageKey::TypeRender(ship_type_id)),
                        112.0,
                        '◇',
                        "Victim ship render",
                    );
                }
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new(&mail.victim).size(19.0).strong());
                    ui.label(egui::RichText::new(&mail.ship).strong().color(ACCENT));
                    let organizations = [
                        detail.victim.corporation_name.as_deref(),
                        detail.victim.alliance_name.as_deref(),
                    ]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" · ");
                    if !organizations.is_empty() {
                        ui.label(egui::RichText::new(organizations).color(MUTED));
                    }
                    ui.horizontal(|ui| {
                        if let Some(corporation_id) = mail.victim_corporation_id {
                            identity_image(
                                ui,
                                images.get(&IdentityImageKey::Corporation(corporation_id)),
                                24.0,
                                'C',
                                "Victim corporation logo",
                            );
                        }
                        if let Some(alliance_id) = detail.victim.alliance_id {
                            identity_image(
                                ui,
                                images.get(&IdentityImageKey::Alliance(alliance_id)),
                                24.0,
                                'A',
                                "Victim alliance logo",
                            );
                        }
                    });
                    let location = match &detail.location.region_name {
                        Some(region) => {
                            format!("{} · {region}", detail.location.solar_system_name)
                        }
                        None => detail.location.solar_system_name.clone(),
                    };
                    ui.label(format!("{} · {location}", mail.time));
                    ui.label(
                        egui::RichText::new(format!(
                            "{} damage taken · {}",
                            format_number(detail.victim.damage_taken),
                            estimated_value_label(mail.estimated_value_isk)
                        ))
                        .color(DANGER),
                    );
                });
            });
        });
    ui.add_space(8.0);

    if ui.available_width() >= 720.0 {
        ui.columns(2, |columns| {
            aggressor_pane(
                &mut columns[0],
                mail.id,
                detail.victim.damage_taken,
                &detail.attackers,
                images,
            );
            fitting_pane(&mut columns[1], mail.id, &detail.victim.items, images);
        });
    } else {
        aggressor_pane(
            ui,
            mail.id,
            detail.victim.damage_taken,
            &detail.attackers,
            images,
        );
        ui.add_space(8.0);
        fitting_pane(ui, mail.id, &detail.victim.items, images);
    }
}

fn aggressor_pane(
    ui: &mut egui::Ui,
    killmail_id: u64,
    damage_taken: u64,
    attackers: &[KillmailAttacker],
    images: &std::collections::HashMap<IdentityImageKey, IdentityImageState>,
) {
    let top_damage = attackers.iter().map(|attacker| attacker.damage_done).max();
    let ordered = ordered_attackers(attackers);

    detail_pane(ui, "INVOLVED PARTIES", |ui| {
        ui.label(
            egui::RichText::new(format!("{} attackers", attackers.len()))
                .small()
                .color(MUTED),
        );
        egui::ScrollArea::vertical()
            .id_salt(("killmail_attackers", killmail_id))
            .max_height(390.0)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if ordered.is_empty() {
                    ui.label(egui::RichText::new("No attacker data").color(MUTED));
                }
                for attacker in ordered {
                    attacker_row(ui, attacker, damage_taken, top_damage, images);
                    ui.separator();
                }
            });
    });
}

fn ordered_attackers(attackers: &[KillmailAttacker]) -> Vec<&KillmailAttacker> {
    let mut ordered = attackers.iter().collect::<Vec<_>>();
    ordered.sort_by(|left, right| {
        right
            .final_blow
            .cmp(&left.final_blow)
            .then_with(|| right.damage_done.cmp(&left.damage_done))
    });
    ordered
}

fn attacker_row(
    ui: &mut egui::Ui,
    attacker: &KillmailAttacker,
    damage_taken: u64,
    top_damage: Option<u64>,
    images: &std::collections::HashMap<IdentityImageKey, IdentityImageState>,
) {
    ui.horizontal_top(|ui| {
        let portrait_key = attacker
            .character_id
            .map(IdentityImageKey::Character)
            .or_else(|| attacker.faction_id.map(IdentityImageKey::Corporation))
            .or_else(|| attacker.corporation_id.map(IdentityImageKey::Corporation));
        identity_image(
            ui,
            portrait_key.and_then(|key| images.get(&key)),
            58.0,
            '?',
            "Attacker portrait or logo",
        );
        ui.vertical(|ui| {
            if let Some(ship_type_id) = attacker.ship_type_id {
                identity_image(
                    ui,
                    images.get(&IdentityImageKey::TypeIcon(ship_type_id)),
                    28.0,
                    '◇',
                    "Attacker ship",
                );
            }
            if let Some(weapon_type_id) = attacker.weapon_type_id {
                identity_image(
                    ui,
                    images.get(&IdentityImageKey::TypeIcon(weapon_type_id)),
                    28.0,
                    '⌁',
                    "Attacker weapon",
                );
            }
        });
        ui.vertical(|ui| {
            ui.horizontal_wrapped(|ui| {
                let name = attacker
                    .character_name
                    .as_deref()
                    .or(attacker.faction_name.as_deref())
                    .or(attacker.corporation_name.as_deref())
                    .unwrap_or("Unknown attacker");
                ui.label(egui::RichText::new(name).strong());
                if attacker.final_blow {
                    chip(ui, "FINAL BLOW", DANGER);
                }
                if top_damage == Some(attacker.damage_done) {
                    chip(ui, "TOP DAMAGE", WARNING);
                }
            });
            let organization = [
                attacker.corporation_name.as_deref(),
                attacker.alliance_name.as_deref(),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ");
            if !organization.is_empty() {
                ui.label(egui::RichText::new(organization).small().color(MUTED));
            }
            ui.label(
                egui::RichText::new(format!(
                    "{} · {}",
                    attacker.ship_name.as_deref().unwrap_or("Unknown ship"),
                    attacker.weapon_name.as_deref().unwrap_or("Unknown weapon")
                ))
                .small()
                .color(MUTED),
            );
            let percentage = if damage_taken == 0 {
                0.0
            } else {
                attacker.damage_done as f64 * 100.0 / damage_taken as f64
            };
            ui.label(format!(
                "{} damage ({percentage:.1}%)",
                format_number(attacker.damage_done)
            ));
        });
    });
}

fn fitting_pane(
    ui: &mut egui::Ui,
    killmail_id: u64,
    items: &[KillmailItem],
    images: &std::collections::HashMap<IdentityImageKey, IdentityImageState>,
) {
    let rows = fitting_rows(items);
    detail_pane(ui, "FITTING AND CONTENT", |ui| {
        egui::ScrollArea::vertical()
            .id_salt(("killmail_fitting", killmail_id))
            .max_height(414.0)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if rows.is_empty() {
                    ui.label(egui::RichText::new("No fitting or cargo data").color(MUTED));
                }
                let mut last_section = None;
                for row in &rows {
                    if last_section != Some(row.section.as_str()) {
                        if last_section.is_some() {
                            ui.add_space(4.0);
                        }
                        ui.label(egui::RichText::new(&row.section).strong().color(ACCENT));
                        last_section = Some(row.section.as_str());
                    }
                    ui.horizontal(|ui| {
                        identity_image(
                            ui,
                            images.get(&IdentityImageKey::TypeIcon(row.item_type_id)),
                            28.0,
                            '□',
                            "Fitting item",
                        );
                        ui.vertical(|ui| {
                            ui.label(&row.name);
                            let outcome = match (row.destroyed, row.dropped) {
                                (0, dropped) => format!("Dropped {dropped}"),
                                (destroyed, 0) => format!("Destroyed {destroyed}"),
                                (destroyed, dropped) => {
                                    format!("Destroyed {destroyed} · Dropped {dropped}")
                                }
                            };
                            ui.label(
                                egui::RichText::new(outcome)
                                    .small()
                                    .color(if row.dropped > 0 { SUCCESS } else { MUTED }),
                            );
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(format_number(row.destroyed + row.dropped));
                        });
                    });
                }
            });
    });
}

fn detail_pane(ui: &mut egui::Ui, title: &str, contents: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(SURFACE)
        .stroke(egui::Stroke::new(1.0, BORDER))
        .corner_radius(6)
        .inner_margin(10)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(egui::RichText::new(title).small().strong().color(MUTED));
            ui.separator();
            contents(ui);
        });
}

#[derive(Clone)]
struct FittingRow {
    section: String,
    rank: u8,
    slot: u32,
    item_type_id: u64,
    name: String,
    destroyed: u64,
    dropped: u64,
}

fn fitting_rows(items: &[KillmailItem]) -> Vec<FittingRow> {
    let mut rows = Vec::new();
    collect_fitting_rows(items, &mut rows);
    rows.sort_by(|left, right| {
        left.rank
            .cmp(&right.rank)
            .then_with(|| left.slot.cmp(&right.slot))
            .then_with(|| left.name.cmp(&right.name))
    });
    let mut aggregated: Vec<FittingRow> = Vec::new();
    for row in rows {
        if let Some(existing) = aggregated.iter_mut().find(|existing| {
            existing.section == row.section
                && existing.slot == row.slot
                && existing.item_type_id == row.item_type_id
        }) {
            existing.destroyed += row.destroyed;
            existing.dropped += row.dropped;
        } else {
            aggregated.push(row);
        }
    }
    aggregated
}

fn collect_fitting_rows(items: &[KillmailItem], rows: &mut Vec<FittingRow>) {
    for item in items {
        let (section, rank, slot) = fitting_section(item.flag);
        rows.push(FittingRow {
            section,
            rank,
            slot,
            item_type_id: item.item_type_id,
            name: item.name.clone(),
            destroyed: item.quantity_destroyed,
            dropped: item.quantity_dropped,
        });
        collect_fitting_rows(&item.items, rows);
    }
}

fn fitting_section(flag: u32) -> (String, u8, u32) {
    match flag {
        27..=34 => ("High Power Slots".into(), 0, flag - 27),
        19..=26 => ("Medium Power Slots".into(), 1, flag - 19),
        11..=18 => ("Low Power Slots".into(), 2, flag - 11),
        92..=99 => ("Rig Slots".into(), 3, flag - 92),
        125..=132 => ("Subsystem Slots".into(), 4, flag - 125),
        164..=171 => ("Service Slots".into(), 5, flag - 164),
        87 => ("Drone Bay".into(), 10, 0),
        5 => ("Cargo Bay".into(), 11, 0),
        158 => ("Fighter Bay".into(), 12, 0),
        90 => ("Ship Maintenance Bay".into(), 13, 0),
        155 => ("Fleet Hangar".into(), 14, 0),
        133..=154 | 156..=157 | 159..=163 => ("Specialized Hold".into(), 15, flag),
        _ => (format!("Other (flag {flag})"), 20, flag),
    }
}

fn killmail_image_keys(mail: &Killmail) -> Vec<IdentityImageKey> {
    let mut keys = Vec::new();
    if let Some(id) = mail.victim_id {
        keys.push(IdentityImageKey::Character(id));
    }
    if let Some(id) = mail.victim_corporation_id {
        keys.push(IdentityImageKey::Corporation(id));
    }
    if let Some(detail) = &mail.detail {
        if let Some(id) = detail.victim.alliance_id {
            keys.push(IdentityImageKey::Alliance(id));
        }
        if let Some(id) = detail.victim.ship_type_id {
            keys.push(IdentityImageKey::TypeRender(id));
        }
        for attacker in &detail.attackers {
            if let Some(id) = attacker.character_id {
                keys.push(IdentityImageKey::Character(id));
            } else if let Some(id) = attacker.faction_id.or(attacker.corporation_id) {
                keys.push(IdentityImageKey::Corporation(id));
            }
            if let Some(id) = attacker.ship_type_id {
                keys.push(IdentityImageKey::TypeIcon(id));
            }
            if let Some(id) = attacker.weapon_type_id {
                keys.push(IdentityImageKey::TypeIcon(id));
            }
        }
        collect_item_image_keys(&detail.victim.items, &mut keys);
    }
    keys.sort_by_key(|key| key.texture_name());
    keys.dedup();
    keys
}

fn collect_item_image_keys(items: &[KillmailItem], keys: &mut Vec<IdentityImageKey>) {
    for item in items {
        keys.push(IdentityImageKey::TypeIcon(item.item_type_id));
        collect_item_image_keys(&item.items, keys);
    }
}

fn format_number(value: u64) -> String {
    let digits = value.to_string();
    let mut formatted = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, character) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            formatted.push(',');
        }
        formatted.push(character);
    }
    formatted
}

fn estimated_value_label(value: Option<f64>) -> String {
    let Some(value) = value else {
        return "Est. cost unavailable".into();
    };
    if value >= 1_000_000_000.0 {
        format!("Est. cost {:.1}B ISK", value / 1_000_000_000.0)
    } else if value >= 1_000_000.0 {
        format!("Est. cost {:.1}M ISK", value / 1_000_000.0)
    } else if value >= 1_000.0 {
        format!("Est. cost {:.1}K ISK", value / 1_000.0)
    } else {
        format!("Est. cost {:.0} ISK", value)
    }
}

fn metric_card(
    ui: &mut egui::Ui,
    value: usize,
    label: &str,
    description: &str,
    color: egui::Color32,
) {
    egui::Frame::new()
        .fill(SURFACE)
        .stroke(egui::Stroke::new(1.0, BORDER))
        .corner_radius(8)
        .inner_margin(14)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                egui::RichText::new(value.to_string())
                    .size(26.0)
                    .strong()
                    .color(color),
            );
            ui.label(egui::RichText::new(label).size(12.0).strong().color(color));
            ui.label(egui::RichText::new(description).small().color(MUTED));
        });
}

fn protected_victim_row(
    ui: &mut egui::Ui,
    kind_label: &str,
    name: &str,
    id: u64,
    enabled: bool,
    remove: &mut Option<(ProtectedVictimKind, u64)>,
    kind: ProtectedVictimKind,
) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(name);
            ui.label(
                egui::RichText::new(format!("{kind_label} {id}"))
                    .small()
                    .color(MUTED),
            );
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let response = ui.add_enabled(enabled, egui::Button::new("Remove").small());
            response.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::WidgetType::Button,
                    enabled,
                    format!("Remove protected {kind_label} {name} {id}"),
                )
            });
            if response.clicked() {
                *remove = Some((kind, id));
            }
        });
    });
}

fn identity_image(
    ui: &mut egui::Ui,
    state: Option<&IdentityImageState>,
    size: f32,
    fallback: char,
    description: &str,
) {
    match state {
        Some(IdentityImageState::Ready(texture)) => {
            ui.add(
                egui::Image::new(texture)
                    .fit_to_exact_size(egui::Vec2::splat(size))
                    .corner_radius(6)
                    .alt_text(description),
            );
        }
        state => {
            let response = egui::Frame::new()
                .fill(egui::Color32::from_rgb(12, 17, 21))
                .stroke(egui::Stroke::new(1.0, BORDER))
                .corner_radius(6)
                .show(ui, |ui| {
                    ui.allocate_ui_with_layout(
                        egui::Vec2::splat(size),
                        egui::Layout::centered_and_justified(egui::Direction::TopDown),
                        |ui| {
                            if matches!(state, Some(IdentityImageState::Loading)) {
                                ui.spinner();
                            } else {
                                ui.label(
                                    egui::RichText::new(fallback.to_string())
                                        .size(size * 0.38)
                                        .strong()
                                        .color(MUTED),
                                );
                            }
                        },
                    );
                })
                .response;
            if matches!(state, Some(IdentityImageState::Failed)) {
                response.on_hover_text(format!("{description} unavailable"));
            }
        }
    }
}

fn status_badge(ui: &mut egui::Ui, busy: bool, status: &str) {
    egui::Frame::new()
        .fill(SURFACE_RAISED)
        .stroke(egui::Stroke::new(1.0, BORDER))
        .corner_radius(20)
        .inner_margin(egui::Margin::symmetric(12, 7))
        .show(ui, |ui| {
            ui.set_max_width(340.0);
            ui.horizontal(|ui| {
                if busy {
                    ui.spinner();
                } else {
                    ui.label(egui::RichText::new("●").color(SUCCESS).size(10.0));
                }
                ui.add(
                    egui::Label::new(egui::RichText::new(status).size(15.5).color(MUTED))
                        .truncate(),
                )
                .on_hover_text(status);
            });
        });
}

fn chip(ui: &mut egui::Ui, text: &str, color: egui::Color32) {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.16))
        .corner_radius(10)
        .inner_margin(egui::Margin::symmetric(9, 4))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(text).size(12.0).strong().color(color));
        });
}

fn section_label(ui: &mut egui::Ui, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .text_style(egui::TextStyle::Small)
            .strong()
            .color(ACCENT),
    );
}

fn notice(ui: &mut egui::Ui, color: egui::Color32, title: &str, message: &str) {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.12))
        .stroke(egui::Stroke::new(1.0, color.gamma_multiply(0.75)))
        .inner_margin(egui::Margin::symmetric(16, 10))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(title).small().strong().color(color));
                ui.label(message);
            });
        });
}

fn empty_sidebar_card(ui: &mut egui::Ui, title: &str, description: &str) {
    egui::Frame::new()
        .fill(SURFACE_RAISED)
        .corner_radius(8)
        .inner_margin(12)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(egui::RichText::new(title).strong());
            ui.label(egui::RichText::new(description).small().color(MUTED));
        });
}

fn empty_queue(ui: &mut egui::Ui, title: &str, description: &str) {
    egui::Frame::new()
        .fill(SURFACE)
        .stroke(egui::Stroke::new(1.0, BORDER))
        .corner_radius(8)
        .inner_margin(egui::Margin::symmetric(24, 36))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new("◎").size(28.0).color(ACCENT));
                ui.label(egui::RichText::new(title).size(17.0).strong());
                ui.label(egui::RichText::new(description).color(MUTED));
            });
        });
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{integrations::simulation, models::ProtectedVictim};
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
    fn simulator_ui_refreshes_and_posts_only_after_a_button_click() {
        let (mut harness, backend) = mixed_harness();

        assert!(backend.posted_ids().is_empty());
        harness.get_by_label("Refresh killmails").click_accesskit();
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
            .query_by_label_contains("Killmail 9001 · Submitted")
            .is_some());
    }

    #[test]
    fn bulk_confirmation_revalidates_protection_in_the_ui_workflow() {
        let (mut harness, backend) = mixed_harness();
        harness.get_by_label("Refresh killmails").click_accesskit();
        harness.run_steps(4);
        harness.get_by_label("Post eligible (2)").click_accesskit();
        harness.run_steps(2);

        harness
            .state_mut()
            .store
            .manually_protected_characters
            .push(ProtectedVictim {
                id: 3006,
                name: "Newly Protected".into(),
            });
        harness.get_by_label("Confirm bulk post").click_accesskit();
        harness.run_steps(4);

        assert_eq!(backend.posted_ids(), vec![9001]);
    }
}
