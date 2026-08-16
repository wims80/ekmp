use super::{
    cached_get_json, enrich_locations, esi_limit_error, estimate_killmail_value,
    estimate_stored_killmail_value, market_prices, Character, Client, Detail, EsiCache, HashMap,
    HashSet, Item, Killmail, KillmailAttacker, KillmailDetail, KillmailItem, KillmailLocation,
    KillmailVictimDetail, Recent, UniverseName, USER_AGENT, USER_AGENT_VALUE,
};

pub(super) fn load_killmails_at(
    esi: &str,
    chars: &[Character],
    cached_killmails: &[Killmail],
    reported_ids: &HashSet<u64>,
    cache: &mut Option<EsiCache>,
    mut access_token: impl FnMut(&Character) -> Result<String, String>,
) -> Result<Vec<Killmail>, String> {
    let client = Client::new();
    let mut pending = Vec::new();
    let mut positions = HashMap::new();
    for c in chars {
        let response: Vec<Recent> = cached_get_json(
            &client,
            cache,
            format!("{esi}/characters/{}/killmails/recent/", c.id),
            true,
            || access_token(c).map(Some),
            "Recent killmail request",
        )?;
        for recent in response {
            if !reported_ids.contains(&recent.killmail_id) {
                add_pending(&mut pending, &mut positions, recent, c);
            }
        }
    }
    if pending.is_empty() {
        return Ok(Vec::new());
    }
    let market_prices = market_prices(&client, esi, cache).unwrap_or_default();
    let cached_by_id = cached_killmails
        .iter()
        .map(|mail| (mail.id, mail))
        .collect::<HashMap<_, _>>();
    let mut mails = pending
        .into_iter()
        .map(|pending| -> Result<Killmail, String> {
            let recent = &pending.recent;
            if let Some(cached) = cached_by_id
                .get(&recent.killmail_id)
                .filter(|cached| cached.hash == recent.killmail_hash && cached.detail.is_some())
            {
                let mut mail = (*cached).clone();
                mail.sources = pending.sources;
                mail.estimated_value_isk = estimate_stored_killmail_value(&mail, &market_prices);
                return Ok(mail);
            }
            let detail: Detail = cached_get_json(
                &client,
                cache,
                format!(
                    "{esi}/killmails/{}/{}",
                    recent.killmail_id, recent.killmail_hash
                ),
                false,
                || Ok(None),
                &format!("Killmail {} request", recent.killmail_id),
            )?;
            let estimated_value_isk = estimate_killmail_value(&detail.victim, &market_prices);
            let time = detail.killmail_time.clone();
            Ok(Killmail {
                id: recent.killmail_id,
                hash: recent.killmail_hash.clone(),
                sources: pending.sources,
                victim_id: detail.victim.character_id,
                victim_corporation_id: detail.victim.corporation_id,
                victim: detail
                    .victim
                    .character_id
                    .map(|id| format!("Character {id}"))
                    .unwrap_or_else(|| "Unknown character".into()),
                ship: detail
                    .victim
                    .ship_type_id
                    .map(|id| format!("Type {id}"))
                    .unwrap_or_else(|| "Unknown ship".into()),
                time,
                estimated_value_isk,
                detail: Some(convert_detail(detail)),
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    enrich_locations(&client, esi, cache, &mut mails)?;
    if let Ok(names) = resolve_names(&client, esi, &mails) {
        apply_names(&mut mails, &names);
    }
    Ok(mails)
}

pub(super) fn add_pending(
    pending: &mut Vec<PendingKillmail>,
    positions: &mut HashMap<u64, usize>,
    recent: Recent,
    character: &Character,
) {
    let source = crate::models::CharacterSource {
        id: character.id,
        name: character.name.clone(),
    };
    if let Some(&index) = positions.get(&recent.killmail_id) {
        if !pending[index].sources.iter().any(|old| old.id == source.id) {
            pending[index].sources.push(source);
        }
    } else {
        positions.insert(recent.killmail_id, pending.len());
        pending.push(PendingKillmail {
            recent,
            sources: vec![source],
        });
    }
}

pub(super) struct PendingKillmail {
    recent: Recent,
    pub(super) sources: Vec<crate::models::CharacterSource>,
}

fn convert_detail(detail: Detail) -> KillmailDetail {
    KillmailDetail {
        victim: KillmailVictimDetail {
            corporation_name: None,
            alliance_id: detail.victim.alliance_id,
            alliance_name: None,
            ship_type_id: detail.victim.ship_type_id,
            damage_taken: detail.victim.damage_taken,
            items: detail.victim.items.into_iter().map(convert_item).collect(),
        },
        attackers: detail
            .attackers
            .into_iter()
            .map(|attacker| KillmailAttacker {
                character_id: attacker.character_id,
                character_name: None,
                corporation_id: attacker.corporation_id,
                corporation_name: None,
                alliance_id: attacker.alliance_id,
                alliance_name: None,
                faction_id: attacker.faction_id,
                faction_name: None,
                ship_type_id: attacker.ship_type_id,
                ship_name: None,
                weapon_type_id: attacker.weapon_type_id,
                weapon_name: None,
                damage_done: attacker.damage_done,
                final_blow: attacker.final_blow,
                security_status: attacker.security_status,
            })
            .collect(),
        location: KillmailLocation {
            solar_system_id: detail.solar_system_id,
            solar_system_name: format!("System {}", detail.solar_system_id),
            region_id: None,
            region_name: None,
        },
    }
}

fn convert_item(item: Item) -> KillmailItem {
    KillmailItem {
        item_type_id: item.item_type_id,
        name: format!("Type {}", item.item_type_id),
        flag: item.flag,
        quantity_destroyed: item.quantity_destroyed.unwrap_or(0),
        quantity_dropped: item.quantity_dropped.unwrap_or(0),
        singleton: item.singleton,
        items: item.items.into_iter().map(convert_item).collect(),
    }
}

fn resolve_names(
    client: &Client,
    esi: &str,
    mails: &[Killmail],
) -> Result<HashMap<u64, String>, String> {
    let mut ids = HashSet::new();
    for mail in mails {
        if let Some(id) = mail.victim_id {
            ids.insert(id);
        }
        if let Some(id) = mail.victim_corporation_id {
            ids.insert(id);
        }
        if let Some(detail) = &mail.detail {
            ids.extend(detail.victim.alliance_id);
            ids.extend(detail.victim.ship_type_id);
            for attacker in &detail.attackers {
                ids.extend(attacker.character_id);
                ids.extend(attacker.corporation_id);
                ids.extend(attacker.alliance_id);
                ids.extend(attacker.faction_id);
                ids.extend(attacker.ship_type_id);
                ids.extend(attacker.weapon_type_id);
            }
            collect_item_type_ids(&detail.victim.items, &mut ids);
        }
    }
    let mut ids = ids.into_iter().collect::<Vec<_>>();
    ids.sort_unstable();
    let mut names = HashMap::new();
    for chunk in ids.chunks(1_000) {
        let response = client
            .post(format!("{esi}/universe/names/"))
            .header(USER_AGENT, USER_AGENT_VALUE)
            .json(chunk)
            .send()
            .map_err(|error| format!("EVE bulk name lookup failed: {error}"))?;
        if let Some(error) = esi_limit_error(&response, "EVE bulk name lookup") {
            return Err(error);
        }
        let response: Vec<UniverseName> = response
            .error_for_status()
            .map_err(|error| format!("EVE bulk name lookup failed: {error}"))?
            .json()
            .map_err(|error| format!("EVE bulk name response invalid: {error}"))?;
        names.extend(response.into_iter().map(|entry| (entry.id, entry.name)));
    }
    Ok(names)
}

fn collect_item_type_ids(items: &[KillmailItem], ids: &mut HashSet<u64>) {
    for item in items {
        ids.insert(item.item_type_id);
        collect_item_type_ids(&item.items, ids);
    }
}

fn apply_names(mails: &mut [Killmail], names: &HashMap<u64, String>) {
    for mail in mails {
        if let Some(name) = mail.victim_id.and_then(|id| names.get(&id)) {
            mail.victim.clone_from(name);
        }
        if let Some(detail) = &mut mail.detail {
            if let Some(name) = detail.victim.ship_type_id.and_then(|id| names.get(&id)) {
                mail.ship.clone_from(name);
            }
            detail.victim.corporation_name = mail
                .victim_corporation_id
                .and_then(|id| names.get(&id).cloned());
            detail.victim.alliance_name = detail
                .victim
                .alliance_id
                .and_then(|id| names.get(&id).cloned());
            for attacker in &mut detail.attackers {
                attacker.character_name =
                    attacker.character_id.and_then(|id| names.get(&id).cloned());
                attacker.corporation_name = attacker
                    .corporation_id
                    .and_then(|id| names.get(&id).cloned());
                attacker.alliance_name =
                    attacker.alliance_id.and_then(|id| names.get(&id).cloned());
                attacker.faction_name = attacker.faction_id.and_then(|id| names.get(&id).cloned());
                attacker.ship_name = attacker.ship_type_id.and_then(|id| names.get(&id).cloned());
                attacker.weapon_name = attacker
                    .weapon_type_id
                    .and_then(|id| names.get(&id).cloned());
            }
            apply_item_names(&mut detail.victim.items, names);
        }
    }
}

fn apply_item_names(items: &mut [KillmailItem], names: &HashMap<u64, String>) {
    for item in items {
        if let Some(name) = names.get(&item.item_type_id) {
            item.name.clone_from(name);
        }
        apply_item_names(&mut item.items, names);
    }
}
