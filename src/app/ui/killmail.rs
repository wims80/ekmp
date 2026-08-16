use super::{
    chip, identity_image, protection_reason_label, IdentityImageKey, IdentityImageState, Killmail,
    KillmailAttacker, KillmailItem, ReportState, ACCENT, ACCENT_DARK, BORDER, DANGER, MUTED,
    SUCCESS, SURFACE, SURFACE_RAISED, WARNING,
};
use crate::killmail::{protected_victim_reasons, report_state};
use eframe::egui;

pub(super) struct KillmailCardContext<'a> {
    pub(super) store: &'a crate::models::Store,
    pub(super) now: u64,
    pub(super) busy: bool,
    pub(super) images: &'a std::collections::HashMap<IdentityImageKey, IdentityImageState>,
}

pub(super) fn killmail_card(
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

pub(super) fn ordered_attackers(attackers: &[KillmailAttacker]) -> Vec<&KillmailAttacker> {
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
pub(super) struct FittingRow {
    pub(super) section: String,
    rank: u8,
    slot: u32,
    item_type_id: u64,
    pub(super) name: String,
    pub(super) destroyed: u64,
    pub(super) dropped: u64,
}

pub(super) fn fitting_rows(items: &[KillmailItem]) -> Vec<FittingRow> {
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

pub(super) fn format_number(value: u64) -> String {
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
