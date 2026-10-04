use super::{
    accessible_button, chip, identity_image, protection_reason_label,
    theme::{
        ACCENT, ACCENT_DARK, BORDER, DANGER, MUTED, SUCCESS, SURFACE, SURFACE_RAISED, WARNING,
    },
    IdentityImageKey, Images, Killmail, KillmailAttacker, KillmailItem, ReportState,
};
use crate::killmail::{protection_reasons, report_state};
use eframe::egui;

pub(super) struct KillmailCardContext<'a> {
    pub(super) store: &'a crate::models::Store,
    pub(super) now: u64,
    pub(super) busy: bool,
    pub(super) protection_controls_enabled: bool,
    pub(super) images: &'a Images,
}

/// A user action on a killmail card.
pub(super) enum CardAction {
    ToggleExpanded,
    /// Request posting. `post_anyway` is only set by a protected card's own button.
    Post {
        post_anyway: bool,
    },
    ToggleProtection,
}

pub(super) fn killmail_card(
    ui: &mut egui::Ui,
    context: &KillmailCardContext<'_>,
    mail: &Killmail,
    expanded: bool,
) -> Option<CardAction> {
    let mut action = None;
    let protection_reasons = protection_reasons(context.store, mail);
    let protected = !protection_reasons.is_empty();
    let manually_protected = context
        .store
        .manually_protected_killmail_ids
        .contains(&mail.id);
    let state = report_state(context.store, mail.id, context.now);
    let edge_color = if protected { WARNING } else { ACCENT_DARK };

    egui::Frame::new()
        .fill(SURFACE_RAISED)
        .stroke(egui::Stroke::new(1.0, edge_color))
        .corner_radius(8)
        .inner_margin(10)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let verb = if expanded { "Collapse" } else { "Expand" };
                let (_rect, response) =
                    ui.allocate_exact_size(egui::vec2(26.0, 26.0), egui::Sense::click());
                egui::collapsing_header::paint_default_icon(
                    ui,
                    if expanded { 1.0 } else { 0.0 },
                    &response,
                );
                let response = response.on_hover_text(format!("{verb} killmail details"));
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::WidgetType::Button,
                        true,
                        format!("{verb} killmail {}", mail.id),
                    )
                });
                if response.clicked() {
                    action = Some(CardAction::ToggleExpanded);
                }

                ui.vertical(|ui| {
                    ui.label(egui::RichText::new(&mail.victim).size(16.0).strong());
                    ui.label(
                        egui::RichText::new(format!(
                            "{}  -  {}  -  {}",
                            mail.ship,
                            estimated_value_label(mail.estimated_value_isk),
                            mail.time
                        ))
                        .small()
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
                        egui::RichText::new(format!("Excluded from bulk posting - {reasons}"))
                            .small()
                            .color(WARNING),
                    );
                }

                ui.add_space(8.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if state == ReportState::Unreported {
                        let label = if protected {
                            "Post anyway"
                        } else {
                            "Post to zKillboard"
                        };
                        let accessible_label = if protected {
                            format!("Post protected killmail {} anyway", mail.id)
                        } else {
                            format!("Post killmail {}", mail.id)
                        };
                        let response = accessible_button(
                            ui,
                            !context.busy,
                            egui::Button::new(label),
                            accessible_label,
                        );
                        if response.clicked() {
                            action = Some(CardAction::Post {
                                post_anyway: protected,
                            });
                        }
                    }

                    let (label, accessible_label) = if manually_protected {
                        (
                            "Remove protection flag",
                            format!("Remove protection flag from killmail {}", mail.id),
                        )
                    } else {
                        ("Protect killmail", format!("Protect killmail {}", mail.id))
                    };
                    let response = accessible_button(
                        ui,
                        context.protection_controls_enabled,
                        egui::Button::new(label).fill(SURFACE),
                        accessible_label,
                    );
                    if response.clicked() {
                        action = Some(CardAction::ToggleProtection);
                    }
                });
            }
        });
    action
}

fn expanded_killmail(ui: &mut egui::Ui, mail: &Killmail, images: &Images) {
    let sources = mail
        .sources
        .iter()
        .map(|source| source.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    ui.label(
        egui::RichText::new(format!("KILLMAIL {} - FROM {sources}", mail.id))
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
                        '?',
                        "Victim ship render",
                    );
                }
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new(&mail.victim).size(19.0).strong());
                    ui.label(egui::RichText::new(&mail.ship).strong().color(ACCENT));
                    let organizations = join_present([
                        detail.victim.corporation_name.as_deref(),
                        detail.victim.alliance_name.as_deref(),
                    ]);
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
                            format!("{} - {region}", detail.location.solar_system_name)
                        }
                        None => detail.location.solar_system_name.clone(),
                    };
                    ui.label(format!("{} - {location}", mail.time));
                    ui.label(
                        egui::RichText::new(format!(
                            "{} damage taken - {}",
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
                detail.victim.damage_taken,
                &detail.attackers,
                images,
            );
            fitting_pane(&mut columns[1], &detail.victim.items, images);
        });
    } else {
        aggressor_pane(ui, detail.victim.damage_taken, &detail.attackers, images);
        ui.add_space(8.0);
        fitting_pane(ui, &detail.victim.items, images);
    }
}

fn aggressor_pane(
    ui: &mut egui::Ui,
    damage_taken: u64,
    attackers: &[KillmailAttacker],
    images: &Images,
) {
    let top_damage = attackers.iter().map(|attacker| attacker.damage_done).max();
    let ordered = ordered_attackers(attackers);

    detail_pane(ui, "INVOLVED PARTIES", |ui| {
        ui.label(
            egui::RichText::new(format!("{} attackers", attackers.len()))
                .small()
                .color(MUTED),
        );
        if ordered.is_empty() {
            ui.label(egui::RichText::new("No attacker data").color(MUTED));
        }
        for attacker in ordered {
            attacker_row(ui, attacker, damage_taken, top_damage, images);
            ui.separator();
        }
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
    images: &Images,
) {
    ui.horizontal_top(|ui| {
        identity_image(
            ui,
            attacker_portrait_key(attacker).and_then(|key| images.get(&key)),
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
                    '?',
                    "Attacker ship",
                );
            }
            if let Some(weapon_type_id) = attacker.weapon_type_id {
                identity_image(
                    ui,
                    images.get(&IdentityImageKey::TypeIcon(weapon_type_id)),
                    28.0,
                    '~',
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
            let organization = join_present([
                attacker.corporation_name.as_deref(),
                attacker.alliance_name.as_deref(),
            ]);
            if !organization.is_empty() {
                ui.label(egui::RichText::new(organization).small().color(MUTED));
            }
            ui.label(
                egui::RichText::new(format!(
                    "{} - {}",
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

fn fitting_pane(ui: &mut egui::Ui, items: &[KillmailItem], images: &Images) {
    let rows = fitting_rows(items);
    detail_pane(ui, "FITTING AND CONTENT", |ui| {
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
                    '?',
                    "Fitting item",
                );
                ui.vertical(|ui| {
                    ui.label(&row.name);
                    let outcome = match (row.destroyed, row.dropped) {
                        (0, dropped) => format!("Dropped {dropped}"),
                        (destroyed, 0) => format!("Destroyed {destroyed}"),
                        (destroyed, dropped) => {
                            format!("Destroyed {destroyed} - Dropped {dropped}")
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

pub(super) fn killmail_image_keys(mail: &Killmail) -> Vec<IdentityImageKey> {
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
            keys.extend(attacker_portrait_key(attacker));
            if let Some(id) = attacker.ship_type_id {
                keys.push(IdentityImageKey::TypeIcon(id));
            }
            if let Some(id) = attacker.weapon_type_id {
                keys.push(IdentityImageKey::TypeIcon(id));
            }
        }
        collect_item_image_keys(&detail.victim.items, &mut keys);
    }
    keys.sort_unstable();
    keys.dedup();
    keys
}

/// The attacker's portrait, or the logo of their faction or corporation.
fn attacker_portrait_key(attacker: &KillmailAttacker) -> Option<IdentityImageKey> {
    attacker
        .character_id
        .map(IdentityImageKey::Character)
        .or_else(|| {
            attacker
                .faction_id
                .or(attacker.corporation_id)
                .map(IdentityImageKey::Corporation)
        })
}

/// Joins the present names with a separator.
fn join_present<'a>(names: impl IntoIterator<Item = Option<&'a str>>) -> String {
    names.into_iter().flatten().collect::<Vec<_>>().join(" - ")
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
