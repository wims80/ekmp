use super::components::accessible_button;
use eframe::egui;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ConfirmationAction {
    None,
    Confirmed,
    Cancelled,
}

pub(super) struct ConfirmationDialog<'a> {
    pub(super) window_title: &'a str,
    pub(super) heading: &'a str,
    pub(super) message: &'a str,
    pub(super) confirm_label: &'a str,
    pub(super) confirm_accessible_label: &'a str,
    pub(super) confirm_color: egui::Color32,
    pub(super) min_width: f32,
}

pub(super) fn confirmation_dialog(
    ctx: &egui::Context,
    options: ConfirmationDialog<'_>,
) -> ConfirmationAction {
    let mut action = ConfirmationAction::None;
    egui::Window::new(options.window_title)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            ui.set_min_width(options.min_width);
            ui.heading(options.heading);
            ui.label(options.message);
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    action = ConfirmationAction::Cancelled;
                }
                if accessible_button(
                    ui,
                    true,
                    egui::Button::new(options.confirm_label).fill(options.confirm_color),
                    options.confirm_accessible_label,
                )
                .clicked()
                {
                    action = ConfirmationAction::Confirmed;
                }
            });
        });
    action
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{kittest::Queryable, Harness};

    fn harness() -> Harness<'static, ConfirmationAction> {
        Harness::new_ui_state(
            |ui, action| {
                let next = confirmation_dialog(
                    ui.ctx(),
                    ConfirmationDialog {
                        window_title: "Test confirmation",
                        heading: "Proceed?",
                        message: "Confirm or cancel this action.",
                        confirm_label: "Proceed",
                        confirm_accessible_label: "Confirm test action",
                        confirm_color: egui::Color32::RED,
                        min_width: 320.0,
                    },
                );
                if next != ConfirmationAction::None {
                    *action = next;
                }
            },
            ConfirmationAction::None,
        )
    }

    #[test]
    fn confirmation_dialog_reports_confirmed_action_and_accessible_label() {
        let mut harness = harness();

        assert_eq!(*harness.state(), ConfirmationAction::None);
        harness
            .get_by_label("Confirm test action")
            .click_accesskit();
        harness.run();

        assert_eq!(*harness.state(), ConfirmationAction::Confirmed);
    }

    #[test]
    fn confirmation_dialog_reports_cancelled_action() {
        let mut harness = harness();

        harness.get_by_label("Cancel").click_accesskit();
        harness.run();

        assert_eq!(*harness.state(), ConfirmationAction::Cancelled);
    }
}
