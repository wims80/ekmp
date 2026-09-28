use super::{
    active_cooldown_error, cached_get_json, client::observe_rate_limit, enrich_locations,
    esi_limit_error, estimate_killmail_value, estimate_stored_killmail_value, http_client,
    market_prices, Character, Client, Detail, EsiCache, HashMap, HashSet, Item, Killmail,
    KillmailAttacker, KillmailDetail, KillmailItem, KillmailLocation, KillmailVictimDetail, Recent,
    UniverseName, USER_AGENT, USER_AGENT_VALUE,
};
use crate::integrations::backend::{CharacterRefreshFailure, LoadKillmailsOutcome};
use std::sync::atomic::AtomicBool;

pub(super) fn load_killmails_at(
    esi: &str,
    chars: &[Character],
    cached_killmails: &[Killmail],
    reported_ids: &HashSet<u64>,
    cancelled: &AtomicBool,
    cache: &mut Option<EsiCache>,
    mut access_token: impl FnMut(&mut Character) -> Result<String, String>,
) -> Result<LoadKillmailsOutcome, String> {
    let client = http_client()?;
    let mut pending = Vec::new();
    let mut positions = HashMap::new();
    let mut character_failures = Vec::new();
    for c in chars {
        super::check_cancelled(cancelled)?;
        let mut current_character = c.clone();
        let response: Vec<Recent> =
            match super::with_rate_limit_scope(format!("character:{}", c.id), || {
                cached_get_json(
                    &client,
                    cache,
                    format!("{esi}/characters/{}/killmails/recent/", c.id),
                    true,
                    || {
                        let token = access_token(&mut current_character)?;
                        super::check_cancelled(cancelled)?;
                        Ok(Some(token))
                    },
                    "Recent killmail request",
                )
            }) {
                Ok(response) => response,
                Err(error)
                    if !error.contains("error limit")
                        && !error.contains("rate limited")
                        && !error.contains("rate budget")
                        && !error.contains("Persistence failure")
                        && error != "Operation cancelled" =>
                {
                    character_failures.push(CharacterRefreshFailure {
                        character_id: c.id,
                        character_name: c.name.clone(),
                        error,
                    });
                    continue;
                }
                Err(error) => return Err(error),
            };
        for recent in response {
            if !reported_ids.contains(&recent.killmail_id) {
                add_pending(&mut pending, &mut positions, recent, c);
            }
        }
    }
    if pending.is_empty() {
        let mut killmails = Vec::new();
        retain_failed_character_cache(
            &mut killmails,
            cached_killmails,
            reported_ids,
            &character_failures,
        );
        return Ok(LoadKillmailsOutcome {
            killmails,
            character_failures,
        });
    }
    super::check_cancelled(cancelled)?;
    let market_prices = match market_prices(&client, esi, cache) {
        Ok(prices) => prices,
        Err(error)
            if error.contains("error limit")
                || error.contains("rate limited")
                || error.contains("rate budget") =>
        {
            return Err(error);
        }
        Err(_) => HashMap::new(),
    };
    let cached_by_id = cached_killmails
        .iter()
        .map(|mail| (mail.id, mail))
        .collect::<HashMap<_, _>>();
    let mut mails = pending
        .into_iter()
        .map(|pending| -> Result<Killmail, String> {
            super::check_cancelled(cancelled)?;
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

    enrich_locations(&client, esi, cache, &mut mails, cancelled)?;
    match resolve_names(&client, esi, &mails, cancelled) {
        Ok(names) => apply_names(&mut mails, &names),
        Err(error)
            if error == "Operation cancelled"
                || error.contains("error limit")
                || error.contains("rate limited")
                || error.contains("rate budget") =>
        {
            return Err(error);
        }
        Err(_) => {}
    }
    retain_failed_character_cache(
        &mut mails,
        cached_killmails,
        reported_ids,
        &character_failures,
    );
    Ok(LoadKillmailsOutcome {
        killmails: mails,
        character_failures,
    })
}

fn retain_failed_character_cache(
    refreshed: &mut Vec<Killmail>,
    cached: &[Killmail],
    reported_ids: &HashSet<u64>,
    failures: &[CharacterRefreshFailure],
) {
    let failed_ids = failures
        .iter()
        .map(|failure| failure.character_id)
        .collect::<HashSet<_>>();
    if failed_ids.is_empty() {
        return;
    }
    for old in cached
        .iter()
        .filter(|mail| !reported_ids.contains(&mail.id))
    {
        let failed_sources = old
            .sources
            .iter()
            .filter(|source| failed_ids.contains(&source.id))
            .cloned()
            .collect::<Vec<_>>();
        if failed_sources.is_empty() {
            continue;
        }
        if let Some(current) = refreshed.iter_mut().find(|mail| mail.id == old.id) {
            for source in failed_sources {
                if !current.sources.iter().any(|known| known.id == source.id) {
                    current.sources.push(source);
                }
            }
        } else {
            let mut retained = old.clone();
            retained.sources = failed_sources;
            refreshed.push(retained);
        }
    }
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
    cancelled: &AtomicBool,
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
        super::check_cancelled(cancelled)?;
        if let Some(error) = active_cooldown_error() {
            return Err(error);
        }
        let response = client
            .post(format!("{esi}/universe/names/"))
            .header(USER_AGENT, USER_AGENT_VALUE)
            .json(chunk)
            .send()
            .map_err(|error| {
                if error.is_timeout() {
                    "EVE bulk name lookup timed out".to_string()
                } else if error.is_connect() {
                    "EVE bulk name lookup could not connect".to_string()
                } else {
                    "EVE bulk name lookup transport failed".to_string()
                }
            })?;
        observe_rate_limit(&response);
        if let Some(error) = esi_limit_error(&response, "EVE bulk name lookup") {
            return Err(error);
        }
        if !response.status().is_success() {
            return Err(format!(
                "EVE bulk name lookup failed (HTTP {})",
                response.status().as_u16()
            ));
        }
        let response: Vec<UniverseName> = response
            .json()
            .map_err(|_| "EVE bulk name response was invalid".to_string())?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::CharacterSource;

    fn mail(source_ids: &[u64]) -> Killmail {
        Killmail {
            id: 42,
            hash: "hash".into(),
            sources: source_ids
                .iter()
                .map(|id| CharacterSource {
                    id: *id,
                    name: format!("Pilot {id}"),
                })
                .collect(),
            victim_id: None,
            victim_corporation_id: None,
            victim: "Victim".into(),
            ship: "Ship".into(),
            time: "2026-01-01T00:00:00Z".into(),
            estimated_value_isk: None,
            detail: None,
        }
    }

    #[test]
    fn failed_character_keeps_only_its_cached_source_membership() {
        let mut refreshed = Vec::new();
        retain_failed_character_cache(
            &mut refreshed,
            &[mail(&[1, 2])],
            &HashSet::new(),
            &[CharacterRefreshFailure {
                character_id: 2,
                character_name: "Pilot 2".into(),
                error: "credential unavailable".into(),
            }],
        );

        assert_eq!(refreshed.len(), 1);
        assert_eq!(refreshed[0].sources.len(), 1);
        assert_eq!(refreshed[0].sources[0].id, 2);
    }

    #[test]
    fn failed_character_source_merges_into_fresh_shared_mail() {
        let mut refreshed = vec![mail(&[1])];
        retain_failed_character_cache(
            &mut refreshed,
            &[mail(&[1, 2])],
            &HashSet::new(),
            &[CharacterRefreshFailure {
                character_id: 2,
                character_name: "Pilot 2".into(),
                error: "credential unavailable".into(),
            }],
        );

        assert_eq!(refreshed.len(), 1);
        assert_eq!(refreshed[0].sources.len(), 2);
    }
}
