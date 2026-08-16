use super::{
    worker::{unix_time, IdentityImageKey},
    App, IdentityImageState, SessionReportStatus, SubmissionMode,
};
use crate::{
    killmail::{
        displayed_killmails, is_bulk_candidate, posting_summary, protected_victim_reasons,
        report_state, ProtectionReason, ReportState,
    },
    models::{Killmail, ProtectedVictimKind},
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
        for mail in visible_killmails {
            killmail_card(ui, &self.store, mail, now, self.is_busy(), &mut post_mail);
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

fn killmail_card(
    ui: &mut egui::Ui,
    store: &crate::models::Store,
    mail: &Killmail,
    now: u64,
    busy: bool,
    post_mail: &mut Option<Killmail>,
) {
    let protection_reasons = protected_victim_reasons(store, mail);
    let protected = !protection_reasons.is_empty();
    let state = report_state(store, mail.id, now);
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
                        egui::RichText::new(format!("{}  ·  {}", mail.ship, mail.time))
                            .color(MUTED),
                    );
                });
                ui.with_layout(
                    egui::Layout::right_to_left(egui::Align::TOP),
                    |ui| match state {
                        ReportState::Reported => chip(ui, "REPORTED", SUCCESS),
                        ReportState::Unreported if protected => chip(ui, "PROTECTED", WARNING),
                        ReportState::Unreported => chip(ui, "READY", SUCCESS),
                        ReportState::Unknown => chip(ui, "CHECKING", MUTED),
                    },
                );
            });
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(5.0);
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!("KILLMAIL {}", mail.id))
                        .small()
                        .color(MUTED),
                );
                let sources = mail
                    .sources
                    .iter()
                    .map(|source| source.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                ui.label(
                    egui::RichText::new(format!("FROM {sources}"))
                        .small()
                        .color(MUTED),
                );
            });

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
                    let response = ui.add_enabled(!busy, egui::Button::new(label));
                    response.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::WidgetType::Button,
                            !busy,
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
        });
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
        assert!(backend.posted_ids().is_empty());

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
