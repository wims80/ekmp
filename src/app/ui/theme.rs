use eframe::egui;

pub(super) const ACCENT: egui::Color32 = egui::Color32::from_rgb(72, 181, 196);
pub(super) const ACCENT_DARK: egui::Color32 = egui::Color32::from_rgb(35, 112, 124);
pub(super) const SURFACE: egui::Color32 = egui::Color32::from_rgb(21, 27, 34);
pub(super) const SURFACE_RAISED: egui::Color32 = egui::Color32::from_rgb(27, 35, 43);
pub(super) const BORDER: egui::Color32 = egui::Color32::from_rgb(51, 64, 75);
pub(super) const MUTED: egui::Color32 = egui::Color32::from_rgb(145, 158, 169);
pub(super) const SUCCESS: egui::Color32 = egui::Color32::from_rgb(103, 194, 142);
pub(super) const WARNING: egui::Color32 = egui::Color32::from_rgb(224, 177, 89);
pub(super) const DANGER: egui::Color32 = egui::Color32::from_rgb(224, 112, 112);

pub(super) fn apply_theme(ctx: &egui::Context) {
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
