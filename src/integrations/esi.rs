use crate::{
    integrations::auth,
    models::{
        Character, Killmail, KillmailAttacker, KillmailDetail, KillmailItem, KillmailLocation,
        KillmailVictimDetail, ProtectedVictimKind,
    },
    persistence::esi_cache::{CachedResponse, EsiCache},
};
use reqwest::{
    blocking::Client,
    header::{
        HeaderMap, CACHE_CONTROL, ETAG, EXPIRES, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED,
        RETRY_AFTER, USER_AGENT,
    },
    StatusCode,
};
use serde::{de::DeserializeOwned, Deserialize};
use std::collections::{HashMap, HashSet};

const ESI: &str = "https://esi.evetech.net/latest";
const USER_AGENT_VALUE: &str = concat!(
    "ekmp/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/wims80/ekmp)"
);

pub fn load_killmails(
    chars: &[Character],
    cached_killmails: &[Killmail],
    reported_ids: &HashSet<u64>,
) -> Result<Vec<Killmail>, String> {
    let mut cache = EsiCache::open().ok();
    load_killmails_at(
        ESI,
        chars,
        cached_killmails,
        reported_ids,
        &mut cache,
        auth::access_token,
    )
}

fn load_killmails_at(
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

pub fn refresh_character_affiliation(character: &mut Character) -> Result<(), String> {
    let mut cache = EsiCache::open().ok();
    refresh_character_affiliation_at(ESI, character, &mut cache)
}

fn refresh_character_affiliation_at(
    esi: &str,
    character: &mut Character,
    cache: &mut Option<EsiCache>,
) -> Result<(), String> {
    let client = Client::new();
    let info = character_info(&client, esi, character.id, cache)?;
    let corporation_name = corporation_name(&client, esi, info.corporation_id, cache)?;
    character.name = info.name;
    character.corporation_id = Some(info.corporation_id);
    character.corporation_name = Some(corporation_name);
    Ok(())
}

pub fn resolve_character_name(id: u64) -> Result<String, String> {
    let mut cache = EsiCache::open().ok();
    character_info(&Client::new(), ESI, id, &mut cache).map(|info| info.name)
}

pub fn resolve_corporation_name(id: u64) -> Result<String, String> {
    let mut cache = EsiCache::open().ok();
    corporation_name(&Client::new(), ESI, id, &mut cache)
}

pub fn resolve_protected_victim_name(
    kind: ProtectedVictimKind,
    name: &str,
) -> Result<(u64, String), String> {
    resolve_protected_victim_name_at(ESI, kind, name)
}

fn resolve_protected_victim_name_at(
    esi: &str,
    kind: ProtectedVictimKind,
    name: &str,
) -> Result<(u64, String), String> {
    let response: UniverseIds = Client::new()
        .post(format!("{esi}/universe/ids/"))
        .header(USER_AGENT, USER_AGENT_VALUE)
        .json(&[name])
        .send()
        .map_err(|error| format!("EVE name lookup failed for {name}: {error}"))?
        .error_for_status()
        .map_err(|error| format!("EVE name lookup failed for {name}: {error}"))?
        .json()
        .map_err(|error| format!("EVE name response invalid for {name}: {error}"))?;
    matching_protected_victim(response, kind, name)
        .map(|entity| (entity.id, entity.name))
        .ok_or_else(|| {
            format!(
                "no exact {} named {name} was found",
                match kind {
                    ProtectedVictimKind::Character => "character",
                    ProtectedVictimKind::Corporation => "corporation",
                }
            )
        })
}

fn matching_protected_victim(
    response: UniverseIds,
    kind: ProtectedVictimKind,
    requested_name: &str,
) -> Option<UniverseEntity> {
    let entities = match kind {
        ProtectedVictimKind::Character => response.characters,
        ProtectedVictimKind::Corporation => response.corporations,
    };
    entities
        .into_iter()
        .find(|entity| entity.name.eq_ignore_ascii_case(requested_name))
}

fn add_pending(
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

struct PendingKillmail {
    recent: Recent,
    sources: Vec<crate::models::CharacterSource>,
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

fn enrich_locations(
    client: &Client,
    esi: &str,
    cache: &mut Option<EsiCache>,
    mails: &mut [Killmail],
) -> Result<(), String> {
    let mut locations = HashMap::new();
    for system_id in mails.iter().filter_map(|mail| {
        mail.detail.as_ref().and_then(|detail| {
            detail
                .location
                .region_id
                .is_none()
                .then_some(detail.location.solar_system_id)
        })
    }) {
        if locations.contains_key(&system_id) {
            continue;
        }
        let location = (|| -> Result<KillmailLocation, String> {
            let system: SolarSystemInfo = cached_get_json(
                client,
                cache,
                format!("{esi}/universe/systems/{system_id}/"),
                true,
                || Ok(None),
                &format!("Solar system lookup for {system_id}"),
            )?;
            let constellation: ConstellationInfo = cached_get_json(
                client,
                cache,
                format!("{esi}/universe/constellations/{}/", system.constellation_id),
                true,
                || Ok(None),
                &format!("Constellation lookup for {}", system.constellation_id),
            )?;
            let region: Name = cached_get_json(
                client,
                cache,
                format!("{esi}/universe/regions/{}/", constellation.region_id),
                true,
                || Ok(None),
                &format!("Region lookup for {}", constellation.region_id),
            )?;
            Ok(KillmailLocation {
                solar_system_id: system_id,
                solar_system_name: system.name,
                region_id: Some(constellation.region_id),
                region_name: Some(region.name),
            })
        })();
        match location {
            Ok(location) => {
                locations.insert(system_id, location);
            }
            Err(error)
                if error.contains("error limit; retry")
                    || error.contains("rate limited; retry") =>
            {
                return Err(error);
            }
            Err(_) => {}
        }
    }
    for mail in mails {
        if let Some(detail) = &mut mail.detail {
            if let Some(location) = locations.get(&detail.location.solar_system_id) {
                detail.location.clone_from(location);
            }
        }
    }
    Ok(())
}

fn character_info(
    client: &Client,
    esi: &str,
    id: u64,
    cache: &mut Option<EsiCache>,
) -> Result<CharacterInfo, String> {
    cached_get_json(
        client,
        cache,
        format!("{esi}/characters/{id}/"),
        true,
        || Ok(None),
        &format!("Character name lookup for {id}"),
    )
}

fn corporation_name(
    client: &Client,
    esi: &str,
    id: u64,
    cache: &mut Option<EsiCache>,
) -> Result<String, String> {
    let info: Name = cached_get_json(
        client,
        cache,
        format!("{esi}/corporations/{id}/"),
        true,
        || Ok(None),
        &format!("Corporation name lookup for {id}"),
    )?;
    Ok(info.name)
}
fn market_prices(
    client: &Client,
    esi: &str,
    cache: &mut Option<EsiCache>,
) -> Result<HashMap<u64, f64>, String> {
    let prices: Vec<MarketPrice> = cached_get_json(
        client,
        cache,
        format!("{esi}/markets/prices/"),
        true,
        || Ok(None),
        "Market price request",
    )?;
    Ok(prices
        .into_iter()
        .filter_map(|price| {
            price
                .average_price
                .or(price.adjusted_price)
                .map(|value| (price.type_id, value))
        })
        .collect())
}

fn cached_get_json<T: DeserializeOwned>(
    client: &Client,
    cache: &mut Option<EsiCache>,
    url: String,
    cacheable: bool,
    bearer_token: impl FnOnce() -> Result<Option<String>, String>,
    request_description: &str,
) -> Result<T, String> {
    let cached = if cacheable {
        cache
            .as_ref()
            .and_then(|cache| cache.load(&url).ok())
            .flatten()
    } else {
        None
    };
    if let Some(entry) = cached.as_ref().filter(|entry| entry.fresh) {
        return deserialize_cached_response(entry, request_description);
    }

    let mut request = client.get(&url).header(USER_AGENT, USER_AGENT_VALUE);
    if let Some(token) = bearer_token()? {
        request = request.bearer_auth(token);
    }
    if let Some(etag) = cached.as_ref().and_then(|entry| entry.etag.as_deref()) {
        request = request.header(IF_NONE_MATCH, etag);
    } else if let Some(last_modified) = cached
        .as_ref()
        .and_then(|entry| entry.last_modified.as_deref())
    {
        request = request.header(IF_MODIFIED_SINCE, last_modified);
    }

    let response = match request.send() {
        Ok(response) => response,
        Err(error) => {
            if let Some(entry) = cached.as_ref() {
                return deserialize_cached_response(entry, request_description);
            }
            return Err(format!("{request_description} failed: {error}"));
        }
    };
    if response.status() == StatusCode::NOT_MODIFIED {
        let expires = header_value(response.headers(), EXPIRES);
        let etag = header_value(response.headers(), ETAG);
        let last_modified = header_value(response.headers(), LAST_MODIFIED);
        let Some(entry) = cached.as_ref() else {
            return Err(format!(
                "{request_description} returned 304 without a cached response"
            ));
        };
        if cacheable {
            if let Some(cache) = cache.as_ref() {
                let _ = cache.revalidate(
                    &url,
                    expires.as_deref(),
                    etag.as_deref(),
                    last_modified.as_deref(),
                );
            }
        }
        return deserialize_cached_response(entry, request_description);
    }
    if let Some(error) = esi_limit_error(&response, request_description) {
        return Err(error);
    }
    if response.status().is_server_error() {
        if let Some(entry) = cached.as_ref() {
            return deserialize_cached_response(entry, request_description);
        }
    }
    let response = response
        .error_for_status()
        .map_err(|error| format!("{request_description} failed: {error}"))?;
    let expires = header_value(response.headers(), EXPIRES);
    let etag = header_value(response.headers(), ETAG);
    let last_modified = header_value(response.headers(), LAST_MODIFIED);
    let allows_storage = !header_value(response.headers(), CACHE_CONTROL)
        .is_some_and(|value| value.to_ascii_lowercase().contains("no-store"));
    let body = response
        .bytes()
        .map_err(|error| format!("{request_description} response body failed: {error}"))?;
    if cacheable && allows_storage {
        if let Some(cache) = cache.as_ref() {
            let _ = cache.store(
                &url,
                &body,
                expires.as_deref(),
                etag.as_deref(),
                last_modified.as_deref(),
            );
        }
    }
    serde_json::from_slice(&body)
        .map_err(|error| format!("{request_description} response invalid: {error}"))
}

fn esi_limit_error(response: &reqwest::blocking::Response, description: &str) -> Option<String> {
    match response.status().as_u16() {
        420 => {
            let reset = response
                .headers()
                .get("x-esi-error-limit-reset")
                .and_then(|value| value.to_str().ok())
                .unwrap_or("an unspecified interval");
            Some(format!(
                "{description} reached ESI's error limit; retry after {reset} seconds"
            ))
        }
        429 => {
            let retry_after = header_value(response.headers(), RETRY_AFTER)
                .unwrap_or_else(|| "an unspecified delay".into());
            Some(format!(
                "{description} was rate limited; retry after {retry_after} seconds"
            ))
        }
        _ => None,
    }
}

fn deserialize_cached_response<T: DeserializeOwned>(
    response: &CachedResponse,
    request_description: &str,
) -> Result<T, String> {
    serde_json::from_slice(&response.body)
        .map_err(|error| format!("cached {request_description} response invalid: {error}"))
}

fn header_value(headers: &HeaderMap, name: reqwest::header::HeaderName) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

fn estimate_killmail_value(victim: &Victim, market_prices: &HashMap<u64, f64>) -> Option<f64> {
    let mut found_price = false;
    let mut value = victim
        .ship_type_id
        .and_then(|type_id| market_prices.get(&type_id))
        .map(|price| {
            found_price = true;
            *price
        })
        .unwrap_or_default();
    value += estimate_raw_items(&victim.items, market_prices, &mut found_price);
    found_price.then_some(value)
}

fn estimate_raw_items(
    items: &[Item],
    market_prices: &HashMap<u64, f64>,
    found_price: &mut bool,
) -> f64 {
    items
        .iter()
        .map(|item| {
            let quantity =
                item.quantity_destroyed.unwrap_or(0) + item.quantity_dropped.unwrap_or(0);
            let own_value = market_prices
                .get(&item.item_type_id)
                .map(|price| {
                    *found_price = true;
                    *price * quantity as f64
                })
                .unwrap_or_default();
            own_value + estimate_raw_items(&item.items, market_prices, found_price)
        })
        .sum()
}

fn estimate_stored_killmail_value(
    mail: &Killmail,
    market_prices: &HashMap<u64, f64>,
) -> Option<f64> {
    let detail = mail.detail.as_ref()?;
    let mut found_price = false;
    let mut value = detail
        .victim
        .ship_type_id
        .and_then(|type_id| market_prices.get(&type_id))
        .map(|price| {
            found_price = true;
            *price
        })
        .unwrap_or_default();
    value += estimate_stored_items(&detail.victim.items, market_prices, &mut found_price);
    found_price.then_some(value)
}

fn estimate_stored_items(
    items: &[KillmailItem],
    market_prices: &HashMap<u64, f64>,
    found_price: &mut bool,
) -> f64 {
    items
        .iter()
        .map(|item| {
            let quantity = item.quantity_destroyed + item.quantity_dropped;
            let own_value = market_prices
                .get(&item.item_type_id)
                .map(|price| {
                    *found_price = true;
                    *price * quantity as f64
                })
                .unwrap_or_default();
            own_value + estimate_stored_items(&item.items, market_prices, found_price)
        })
        .sum()
}

#[derive(Deserialize)]
struct Recent {
    killmail_id: u64,
    killmail_hash: String,
}
#[derive(Deserialize)]
struct Detail {
    killmail_time: String,
    solar_system_id: u64,
    victim: Victim,
    #[serde(default)]
    attackers: Vec<Attacker>,
}
#[derive(Deserialize)]
struct Victim {
    character_id: Option<u64>,
    corporation_id: Option<u64>,
    alliance_id: Option<u64>,
    ship_type_id: Option<u64>,
    #[serde(default)]
    damage_taken: u64,
    #[serde(default)]
    items: Vec<Item>,
}
#[derive(Deserialize)]
struct Item {
    item_type_id: u64,
    #[serde(default)]
    flag: u32,
    quantity_destroyed: Option<u64>,
    quantity_dropped: Option<u64>,
    #[serde(default)]
    singleton: u32,
    #[serde(default)]
    items: Vec<Item>,
}
#[derive(Deserialize)]
struct Attacker {
    character_id: Option<u64>,
    corporation_id: Option<u64>,
    alliance_id: Option<u64>,
    faction_id: Option<u64>,
    ship_type_id: Option<u64>,
    weapon_type_id: Option<u64>,
    #[serde(default)]
    damage_done: u64,
    #[serde(default)]
    final_blow: bool,
    security_status: Option<f32>,
}
#[derive(Deserialize)]
struct MarketPrice {
    type_id: u64,
    adjusted_price: Option<f64>,
    average_price: Option<f64>,
}
#[derive(Deserialize)]
struct CharacterInfo {
    name: String,
    corporation_id: u64,
}
#[derive(Deserialize)]
struct Name {
    name: String,
}

#[derive(Deserialize)]
struct SolarSystemInfo {
    name: String,
    constellation_id: u64,
}

#[derive(Deserialize)]
struct ConstellationInfo {
    region_id: u64,
}

#[derive(Deserialize)]
struct UniverseName {
    id: u64,
    name: String,
}

#[derive(Deserialize)]
struct UniverseIds {
    #[serde(default)]
    characters: Vec<UniverseEntity>,
    #[serde(default)]
    corporations: Vec<UniverseEntity>,
}

#[derive(Deserialize)]
struct UniverseEntity {
    id: u64,
    name: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT_CACHE_PATH: AtomicU64 = AtomicU64::new(0);

    fn temporary_cache_path() -> PathBuf {
        let sequence = NEXT_CACHE_PATH.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir()
            .join(format!(
                "ekmp-esi-client-test-{}-{sequence}",
                std::process::id()
            ))
            .join("cache.sqlite3")
    }

    #[test]
    fn user_agent_identifies_the_application_and_source() {
        assert!(USER_AGENT_VALUE.starts_with("ekmp/"));
        assert!(USER_AGENT_VALUE.contains("https://github.com/wims80/ekmp"));
    }

    #[test]
    fn protected_victim_name_resolution_uses_the_selected_category() {
        let response = UniverseIds {
            characters: vec![UniverseEntity {
                id: 42,
                name: "Shared Name".into(),
            }],
            corporations: vec![UniverseEntity {
                id: 84,
                name: "Shared Name".into(),
            }],
        };

        let corporation =
            matching_protected_victim(response, ProtectedVictimKind::Corporation, "shared name")
                .unwrap();

        assert_eq!(corporation.id, 84);
    }

    fn character(id: u64, name: &str) -> Character {
        Character {
            id,
            name: name.into(),
            refresh_token: None,
            corporation_id: None,
            corporation_name: None,
        }
    }

    #[test]
    fn duplicate_killmails_retain_all_source_characters() {
        let mut pending = Vec::new();
        let mut positions = HashMap::new();
        add_pending(
            &mut pending,
            &mut positions,
            Recent {
                killmail_id: 42,
                killmail_hash: "hash".into(),
            },
            &character(1, "One"),
        );
        add_pending(
            &mut pending,
            &mut positions,
            Recent {
                killmail_id: 42,
                killmail_hash: "hash".into(),
            },
            &character(2, "Two"),
        );

        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].sources.len(), 2);
        assert_eq!(pending[0].sources[0].name, "One");
        assert_eq!(pending[0].sources[1].name, "Two");
    }

    #[test]
    fn cached_esi_response_skips_a_fresh_network_request() {
        let server = MockServer::start();
        let request = server.mock(|when, then| {
            when.method(GET).path("/characters/2/");
            then.status(200)
                .header("content-type", "application/json")
                .header("expires", "Thu, 31 Dec 2099 23:59:59 GMT")
                .header("etag", "fixture-etag")
                .body(r#"{"name":"Cached Pilot","corporation_id":20}"#);
        });
        let path = temporary_cache_path();
        {
            let mut cache = Some(EsiCache::open_at(&path).unwrap());
            let url = format!("{}/characters/2/", server.base_url());
            let first: CharacterInfo = cached_get_json(
                &Client::new(),
                &mut cache,
                url.clone(),
                true,
                || Ok(None),
                "Character name lookup",
            )
            .unwrap();
            let second: CharacterInfo = cached_get_json(
                &Client::new(),
                &mut cache,
                url,
                true,
                || Ok(None),
                "Character name lookup",
            )
            .unwrap();

            assert_eq!(first.name, "Cached Pilot");
            assert_eq!(second.corporation_id, 20);
        }
        request.assert_calls(1);
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn expired_esi_response_is_revalidated_with_its_etag() {
        let server = MockServer::start();
        let request = server.mock(|when, then| {
            when.method(GET)
                .path("/characters/2/")
                .header("if-none-match", "fixture-etag");
            then.status(304)
                .header("expires", "Thu, 31 Dec 2099 23:59:59 GMT");
        });
        let path = temporary_cache_path();
        {
            let mut cache = Some(EsiCache::open_at(&path).unwrap());
            let url = format!("{}/characters/2/", server.base_url());
            cache
                .as_ref()
                .unwrap()
                .store(
                    &url,
                    br#"{"name":"Cached Pilot","corporation_id":20}"#,
                    Some("Sat, 01 Jan 2000 00:00:00 GMT"),
                    Some("fixture-etag"),
                    None,
                )
                .unwrap();

            let response: CharacterInfo = cached_get_json(
                &Client::new(),
                &mut cache,
                url.clone(),
                true,
                || Ok(None),
                "Character name lookup",
            )
            .unwrap();

            assert_eq!(response.name, "Cached Pilot");
            assert!(cache.as_ref().unwrap().load(&url).unwrap().unwrap().fresh);
        }
        request.assert_calls(1);
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn loads_killmails_through_the_configured_http_endpoint() {
        let server = MockServer::start();
        let recent = server.mock(|when, then| {
            when.method(GET)
                .path("/characters/1/killmails/recent/")
                .header("authorization", "Bearer synthetic-token");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"[{"killmail_id":42,"killmail_hash":"fixture-hash"}]"#);
        });
        let detail = server.mock(|when, then| {
            when.method(GET).path("/killmails/42/fixture-hash");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"killmail_time":"2026-08-16T10:00:00Z","solar_system_id":30000142,"victim":{"character_id":2,"corporation_id":20,"ship_type_id":3,"damage_taken":1200,"items":[{"flag":27,"item_type_id":4,"quantity_destroyed":2,"singleton":0}]},"attackers":[{"character_id":5,"corporation_id":50,"ship_type_id":6,"weapon_type_id":7,"damage_done":1200,"final_blow":true}]}"#);
        });
        let prices = server.mock(|when, then| {
            when.method(GET).path("/markets/prices/");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"[{"type_id":3,"adjusted_price":1000.0},{"type_id":4,"average_price":500.0}]"#);
        });
        let names = server.mock(|when, then| {
            when.method(POST).path("/universe/names/");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"[{"id":2,"name":"Fixture Victim","category":"character"},{"id":3,"name":"Fixture Ship","category":"inventory_type"},{"id":4,"name":"Fixture Module","category":"inventory_type"},{"id":5,"name":"Fixture Attacker","category":"character"},{"id":6,"name":"Fixture Attack Ship","category":"inventory_type"},{"id":7,"name":"Fixture Weapon","category":"inventory_type"},{"id":20,"name":"Fixture Victim Corp","category":"corporation"},{"id":50,"name":"Fixture Attacker Corp","category":"corporation"}]"#);
        });
        let system = server.mock(|when, then| {
            when.method(GET).path("/universe/systems/30000142/");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"name":"Jita","constellation_id":20000020}"#);
        });
        let constellation = server.mock(|when, then| {
            when.method(GET).path("/universe/constellations/20000020/");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"region_id":10000002}"#);
        });
        let region = server.mock(|when, then| {
            when.method(GET).path("/universe/regions/10000002/");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"name":"The Forge"}"#);
        });

        let mails = load_killmails_at(
            &server.base_url(),
            &[character(1, "Pilot")],
            &[],
            &HashSet::new(),
            &mut None,
            |_| Ok("synthetic-token".into()),
        )
        .unwrap();

        assert_eq!(mails.len(), 1);
        assert_eq!(mails[0].id, 42);
        assert_eq!(mails[0].victim, "Fixture Victim");
        assert_eq!(mails[0].ship, "Fixture Ship");
        assert_eq!(mails[0].estimated_value_isk, Some(2_000.0));
        let expanded = mails[0].detail.as_ref().unwrap();
        assert_eq!(expanded.location.solar_system_name, "Jita");
        assert_eq!(expanded.location.region_name.as_deref(), Some("The Forge"));
        assert_eq!(
            expanded.attackers[0].character_name.as_deref(),
            Some("Fixture Attacker")
        );
        assert_eq!(expanded.victim.items[0].name, "Fixture Module");
        recent.assert();
        detail.assert();
        prices.assert();
        names.assert();
        system.assert();
        constellation.assert();
        region.assert();
    }

    #[test]
    fn matching_cached_detail_is_reused_and_source_membership_is_refreshed() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/characters/1/killmails/recent/");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"[{"killmail_id":42,"killmail_hash":"fixture-hash"}]"#);
        });
        let detail = server.mock(|when, then| {
            when.method(GET).path("/killmails/42/fixture-hash");
            then.status(500);
        });
        server.mock(|when, then| {
            when.method(GET).path("/markets/prices/");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"[{"type_id":3,"average_price":1000.0}]"#);
        });
        server.mock(|when, then| {
            when.method(POST).path("/universe/names/");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"[{"id":2,"name":"Current Victim"},{"id":3,"name":"Current Ship"},{"id":20,"name":"Current Corp"}]"#);
        });
        let cached: Killmail = serde_json::from_str(
            r#"{
                "id":42,"hash":"fixture-hash","sources":[],
                "victim_id":2,"victim_corporation_id":20,
                "victim":"Old Victim","ship":"Old Ship","time":"2026-08-16T10:00:00Z",
                "detail":{
                    "victim":{"ship_type_id":3,"damage_taken":100,"items":[]},
                    "attackers":[],
                    "location":{"solar_system_id":30000142,"solar_system_name":"Jita","region_id":10000002,"region_name":"The Forge"}
                }
            }"#,
        )
        .unwrap();

        let mails = load_killmails_at(
            &server.base_url(),
            &[character(1, "New Source")],
            &[cached],
            &HashSet::new(),
            &mut None,
            |_| Ok("synthetic-token".into()),
        )
        .unwrap();

        detail.assert_calls(0);
        assert_eq!(mails[0].victim, "Current Victim");
        assert_eq!(mails[0].ship, "Current Ship");
        assert_eq!(mails[0].sources[0].name, "New Source");
        assert_eq!(mails[0].estimated_value_isk, Some(1_000.0));
    }

    #[test]
    fn positively_reported_ids_are_filtered_before_detail_and_price_requests() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/characters/1/killmails/recent/");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"[{"killmail_id":42,"killmail_hash":"fixture-hash"}]"#);
        });

        let mails = load_killmails_at(
            &server.base_url(),
            &[character(1, "Pilot")],
            &[],
            &HashSet::from([42]),
            &mut None,
            |_| Ok("synthetic-token".into()),
        )
        .unwrap();

        assert!(mails.is_empty());
    }

    #[test]
    fn recursive_estimate_prefers_average_prices_and_keeps_partial_totals() {
        let victim: Victim = serde_json::from_str(
            r#"{
                "ship_type_id":1,
                "items":[{
                    "item_type_id":2,"quantity_destroyed":2,
                    "items":[{"item_type_id":3,"quantity_dropped":4}]
                }]
            }"#,
        )
        .unwrap();
        let prices = HashMap::from([(1, 10.0), (2, 5.0), (3, 2.0)]);

        assert_eq!(estimate_killmail_value(&victim, &prices), Some(28.0));
    }

    #[test]
    fn protected_victim_lookup_uses_the_configured_http_endpoint() {
        let server = MockServer::start();
        let lookup = server.mock(|when, then| {
            when.method(POST).path("/universe/ids/");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"characters":[{"id":42,"name":"Fixture Pilot"}]}"#);
        });

        let result = resolve_protected_victim_name_at(
            &server.base_url(),
            ProtectedVictimKind::Character,
            "Fixture Pilot",
        )
        .unwrap();

        assert_eq!(result, (42, "Fixture Pilot".into()));
        lookup.assert();
    }
}
