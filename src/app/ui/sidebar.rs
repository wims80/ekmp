use super::{
    components::{
        accessible_button, empty_sidebar_card, identity_image, protected_victim_row, section_label,
    },
    theme::{ACCENT, ACCENT_DARK, BORDER, MUTED, SUCCESS, SURFACE, SURFACE_RAISED},
    IdentityImageKey, IdentityImageState,
};
use crate::models::{Character, ProtectedVictim, ProtectedVictimKind};
use eframe::egui;
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SidebarAction {
    ConnectCharacter,
    DisconnectCharacter(u64),
    AddProtectedVictim,
    RemoveProtectedVictim(ProtectedVictimKind, u64),
}

pub(super) struct SidebarProps<'a> {
    pub(super) characters: &'a [Character],
    pub(super) manually_protected_characters: &'a [ProtectedVictim],
    pub(super) manually_protected_corporations: &'a [ProtectedVictim],
    pub(super) images: &'a HashMap<IdentityImageKey, IdentityImageState>,
    pub(super) latest_status: &'a str,
    pub(super) status_history: &'a VecDeque<String>,
    pub(super) controls_enabled: bool,
    pub(super) persistence_enabled: bool,
}

pub(super) struct ProtectedVictimDraft<'a> {
    pub(super) kind: &'a mut ProtectedVictimKind,
    pub(super) query: &'a mut String,
}

pub(super) fn sidebar(
    ui: &mut egui::Ui,
    props: SidebarProps<'_>,
    draft: ProtectedVictimDraft<'_>,
) -> Option<SidebarAction> {
    let mut action = None;
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
            ui.add_space(8.0);
            if ui
                .add_enabled(
                    props.controls_enabled,
                    egui::Button::new("+  Connect another character")
                        .fill(ACCENT_DARK)
                        .min_size(egui::vec2(ui.available_width(), 38.0)),
                )
                .clicked()
            {
                action = Some(SidebarAction::ConnectCharacter);
            }
            ui.add_space(8.0);

            if props.characters.is_empty() {
                empty_sidebar_card(
                    ui,
                    "No characters connected",
                    "Authenticate with EVE SSO to begin reviewing killmails.",
                );
            }

            for character in props.characters {
                connected_character_card(ui, &props, character, &mut action);
            }

            ui.add_space(22.0);
            protected_victims(ui, &props, draft, &mut action);

            ui.add_space(22.0);
            activity(ui, &props);
        });
    action
}

fn connected_character_card(
    ui: &mut egui::Ui,
    props: &SidebarProps<'_>,
    character: &Character,
    action: &mut Option<SidebarAction>,
) {
    egui::Frame::new()
        .fill(SURFACE_RAISED)
        .stroke(egui::Stroke::new(1.0, BORDER))
        .corner_radius(8)
        .inner_margin(12)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width().max(200.0));
            let collapsing = egui::collapsing_header::CollapsingState::load_with_default_open(
                ui.ctx(),
                ui.make_persistent_id(("connected_character", character.id)),
                false,
            )
            .show_header(ui, |ui| {
                ui.horizontal(|ui| {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                    ui.painter().circle_filled(rect.center(), 4.0, SUCCESS);
                    ui.label(egui::RichText::new(&character.name).strong());
                });
            })
            .body_unindented(|ui| {
                ui.add_space(8.0);
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        identity_image(
                            ui,
                            props.images.get(&IdentityImageKey::Character(character.id)),
                            64.0,
                            character.name.chars().next().unwrap_or('?'),
                            "Character portrait",
                        );
                        if let Some(corporation_id) = character.corporation_id {
                            ui.horizontal(|ui| {
                                ui.add_space(16.0);
                                identity_image(
                                    ui,
                                    props
                                        .images
                                        .get(&IdentityImageKey::Corporation(corporation_id)),
                                    32.0,
                                    'C',
                                    "Corporation logo",
                                );
                            });
                        }
                    });
                    ui.vertical(|ui| {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(format!("Character {}", character.id))
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
                                egui::RichText::new(format!("Corporation {corporation_id}"))
                                    .small()
                                    .color(MUTED),
                            );
                        }
                    });
                });
                if accessible_button(
                    ui,
                    props.controls_enabled,
                    egui::Button::new("Disconnect"),
                    format!("Disconnect {}", character.name),
                )
                .clicked()
                {
                    *action = Some(SidebarAction::DisconnectCharacter(character.id));
                }
            });
            collapsing.0.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::WidgetType::Button,
                    true,
                    format!("Toggle connected character {} details", character.name),
                )
            });
        });
}

fn protected_victims(
    ui: &mut egui::Ui,
    props: &SidebarProps<'_>,
    draft: ProtectedVictimDraft<'_>,
    action: &mut Option<SidebarAction>,
) {
    let automatic_count = props.characters.len()
        + props
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
    let manual_count =
        props.manually_protected_characters.len() + props.manually_protected_corporations.len();

    section_label(ui, "PROTECTED VICTIMS");
    egui::Frame::new()
        .fill(SURFACE_RAISED)
        .corner_radius(8)
        .inner_margin(12)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                egui::RichText::new(
                    "Excluded from bulk posting. Individual posting remains explicit.",
                )
                .small()
                .color(MUTED),
            );
            ui.add_space(5.0);
            egui::CollapsingHeader::new(format!(
                "{} automatic - {} manual",
                automatic_count, manual_count
            ))
            .default_open(false)
            .show_unindented(ui, |ui| {
                ui.add_space(6.0);
                automatic_protection(ui, props.characters, automatic_count);
                ui.add_space(10.0);
                manual_protection(ui, props, draft, manual_count, action);
            });
        });
}

fn automatic_protection(ui: &mut egui::Ui, characters: &[Character], automatic_count: usize) {
    egui::Frame::new()
        .fill(SURFACE)
        .corner_radius(4)
        .inner_margin(egui::Margin::symmetric(8, 7))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(egui::RichText::new("Automatic protection").strong());
            for character in characters {
                ui.label(format!("Character - {}", character.name));
            }
            let mut corporation_ids = HashSet::new();
            for character in characters {
                if let (Some(id), Some(name)) =
                    (character.corporation_id, &character.corporation_name)
                {
                    if corporation_ids.insert(id) {
                        ui.label(format!("Corporation - {name}"));
                    }
                }
            }
            if automatic_count == 0 {
                ui.label(egui::RichText::new("None yet").color(MUTED));
            }
        });
}

fn manual_protection(
    ui: &mut egui::Ui,
    props: &SidebarProps<'_>,
    draft: ProtectedVictimDraft<'_>,
    manual_count: usize,
    action: &mut Option<SidebarAction>,
) {
    egui::Frame::new()
        .fill(SURFACE)
        .corner_radius(4)
        .inner_margin(egui::Margin::symmetric(8, 7))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(egui::RichText::new("Manual protection").strong());
            let mut remove = None;
            for victim in props.manually_protected_characters {
                protected_victim_row(
                    ui,
                    "Character",
                    &victim.name,
                    victim.id,
                    props.persistence_enabled,
                    &mut remove,
                    ProtectedVictimKind::Character,
                );
            }
            for victim in props.manually_protected_corporations {
                protected_victim_row(
                    ui,
                    "Corporation",
                    &victim.name,
                    victim.id,
                    props.persistence_enabled,
                    &mut remove,
                    ProtectedVictimKind::Corporation,
                );
            }
            if manual_count == 0 {
                ui.label(egui::RichText::new("No manually protected victims").color(MUTED));
            }

            ui.add_space(8.0);
            egui::ComboBox::from_id_salt("protected_victim_kind")
                .selected_text(match draft.kind {
                    ProtectedVictimKind::Character => "Character",
                    ProtectedVictimKind::Corporation => "Corporation",
                })
                .width(ui.available_width())
                .show_ui(ui, |ui| {
                    ui.selectable_value(draft.kind, ProtectedVictimKind::Character, "Character");
                    ui.selectable_value(
                        draft.kind,
                        ProtectedVictimKind::Corporation,
                        "Corporation",
                    );
                });
            let input_label = ui.label("Protected victim name or ID");
            ui.add(
                egui::TextEdit::singleline(draft.query)
                    .hint_text("Exact name or EVE ID")
                    .desired_width(ui.available_width()),
            )
            .labelled_by(input_label.id);
            if ui
                .add_enabled(
                    props.controls_enabled,
                    egui::Button::new("Add protected victim"),
                )
                .clicked()
            {
                *action = Some(SidebarAction::AddProtectedVictim);
            }
            if let Some((kind, id)) = remove {
                *action = Some(SidebarAction::RemoveProtectedVictim(kind, id));
            }
        });
}

fn activity(ui: &mut egui::Ui, props: &SidebarProps<'_>) {
    section_label(ui, "ACTIVITY");
    egui::Frame::new()
        .fill(SURFACE_RAISED)
        .corner_radius(8)
        .inner_margin(12)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(egui::RichText::new("Current status").strong().color(ACCENT));
            ui.label(props.latest_status);
            egui::CollapsingHeader::new(format!(
                "Activity log - {} entries",
                props.status_history.len()
            ))
            .default_open(false)
            .show_unindented(ui, |ui| {
                ui.add_space(6.0);
                for message in props.status_history {
                    egui::Frame::new()
                        .fill(SURFACE)
                        .corner_radius(4)
                        .inner_margin(egui::Margin::symmetric(8, 7))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.add(
                                egui::Label::new(egui::RichText::new(message).color(MUTED)).wrap(),
                            );
                        });
                    ui.add_space(6.0);
                }
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{kittest::Queryable, Harness};

    struct TestState {
        characters: Vec<Character>,
        protected_characters: Vec<ProtectedVictim>,
        protected_corporations: Vec<ProtectedVictim>,
        images: HashMap<IdentityImageKey, IdentityImageState>,
        status_history: VecDeque<String>,
        draft_kind: ProtectedVictimKind,
        draft_query: String,
        action: Option<SidebarAction>,
    }

    fn harness() -> Harness<'static, TestState> {
        Harness::new_ui_state(
            |ui, state| {
                if let Some(action) = sidebar(
                    ui,
                    SidebarProps {
                        characters: &state.characters,
                        manually_protected_characters: &state.protected_characters,
                        manually_protected_corporations: &state.protected_corporations,
                        images: &state.images,
                        latest_status: "Ready",
                        status_history: &state.status_history,
                        controls_enabled: true,
                        persistence_enabled: true,
                    },
                    ProtectedVictimDraft {
                        kind: &mut state.draft_kind,
                        query: &mut state.draft_query,
                    },
                ) {
                    state.action = Some(action);
                }
            },
            TestState {
                characters: vec![Character {
                    id: 7,
                    name: "Test Pilot".into(),
                    refresh_token: None,
                    corporation_id: None,
                    corporation_name: None,
                }],
                protected_characters: vec![ProtectedVictim {
                    id: 11,
                    name: "Protected Pilot".into(),
                }],
                protected_corporations: Vec::new(),
                images: HashMap::new(),
                status_history: VecDeque::from(["Ready".into()]),
                draft_kind: ProtectedVictimKind::Character,
                draft_query: String::new(),
                action: None,
            },
        )
    }

    #[test]
    fn disconnect_returns_an_action_without_mutating_characters() {
        let mut harness = harness();
        harness
            .get_by_label("Toggle connected character Test Pilot details")
            .click_accesskit();
        harness.run();
        harness
            .get_by_label("Disconnect Test Pilot")
            .click_accesskit();
        harness.run();

        assert_eq!(
            harness.state().action,
            Some(SidebarAction::DisconnectCharacter(7))
        );
        assert_eq!(harness.state().characters.len(), 1);
    }

    #[test]
    fn protected_victim_removal_returns_an_action_without_mutating_data() {
        let mut harness = harness();
        harness
            .get_by_label("1 automatic - 1 manual")
            .click_accesskit();
        harness.run();
        harness
            .get_by_label("Remove protected Character Protected Pilot 11")
            .click_accesskit();
        harness.run();

        assert_eq!(
            harness.state().action,
            Some(SidebarAction::RemoveProtectedVictim(
                ProtectedVictimKind::Character,
                11
            ))
        );
        assert_eq!(harness.state().protected_characters.len(), 1);
    }
}
