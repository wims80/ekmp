use super::{
    theme::{ACCENT, BORDER, MUTED, SUCCESS, SURFACE, SURFACE_RAISED},
    IdentityImageState, ProtectedVictimKind,
};
use crate::killmail::ProtectionReason;
use eframe::egui;

pub(super) fn accessible_button(
    ui: &mut egui::Ui,
    enabled: bool,
    button: egui::Button<'_>,
    accessible_label: impl Into<String>,
) -> egui::Response {
    let accessible_label = accessible_label.into();
    let response = ui.add_enabled(enabled, button);
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, accessible_label.clone())
    });
    response
}

pub(super) fn protected_victim_row(
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
            let response = accessible_button(
                ui,
                enabled,
                egui::Button::new("Remove").small(),
                format!("Remove protected {kind_label} {name} {id}"),
            );
            if response.clicked() {
                *remove = Some((kind, id));
            }
        });
    });
}

pub(super) fn identity_image(
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

pub(super) fn status_badge(ui: &mut egui::Ui, busy: bool, status: &str) {
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
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                    ui.painter().circle_filled(rect.center(), 4.0, SUCCESS);
                }
                ui.add(
                    egui::Label::new(egui::RichText::new(status).size(15.5).color(MUTED))
                        .truncate(),
                )
                .on_hover_text(status);
            });
        });
}

pub(super) fn chip(ui: &mut egui::Ui, text: &str, color: egui::Color32) {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.16))
        .corner_radius(10)
        .inner_margin(egui::Margin::symmetric(9, 4))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(text).size(12.0).strong().color(color));
        });
}

pub(super) fn section_label(ui: &mut egui::Ui, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .text_style(egui::TextStyle::Small)
            .strong()
            .color(ACCENT),
    );
}

pub(super) fn notice(ui: &mut egui::Ui, color: egui::Color32, title: &str, message: &str) {
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

pub(super) fn empty_sidebar_card(ui: &mut egui::Ui, title: &str, description: &str) {
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

pub(super) fn empty_queue(ui: &mut egui::Ui, title: &str, description: &str) {
    egui::Frame::new()
        .fill(SURFACE)
        .stroke(egui::Stroke::new(1.0, BORDER))
        .corner_radius(8)
        .inner_margin(egui::Margin::symmetric(24, 36))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new("o").size(28.0).color(ACCENT));
                ui.label(egui::RichText::new(title).size(17.0).strong());
                ui.label(egui::RichText::new(description).color(MUTED));
            });
        });
}

pub(super) fn protection_reason_label(reason: &ProtectionReason) -> String {
    match reason {
        ProtectionReason::AuthenticatedCharacter(name) => {
            format!("authenticated character {name}")
        }
        ProtectionReason::AuthenticatedCorporation(name) => {
            format!("authenticated corporation {name}")
        }
        ProtectionReason::ManuallyProtectedCharacter(name) => format!("character {name}"),
        ProtectionReason::ManuallyProtectedCorporation(name) => format!("corporation {name}"),
        ProtectionReason::ManuallyProtectedKillmail => "killmail flagged for protection".into(),
    }
}
