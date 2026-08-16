use crate::{
    integrations::auth,
    models::{Character, Killmail, ProtectedVictimKind},
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
use std::collections::HashMap;

const ESI: &str = "https://esi.evetech.net/latest";
const USER_AGENT_VALUE: &str = concat!(
    "ekmp/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/wims80/ekmp)"
);

pub fn load_killmails(chars: &[Character]) -> Result<Vec<Killmail>, String> {
    let mut cache = EsiCache::open().ok();
    load_killmails_at(ESI, chars, &mut cache, auth::access_token)
}

fn load_killmails_at(
    esi: &str,
    chars: &[Character],
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
            add_pending(&mut pending, &mut positions, recent, c);
        }
    }
    let market_prices = market_prices(&client, esi, cache).unwrap_or_default();
    pending
        .into_iter()
        .map(|pending| {
            let recent = pending.recent;
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
            let victim = detail
                .victim
                .character_id
                .map(|id| character_name(&client, esi, id, cache))
                .transpose()?
                .unwrap_or_else(|| "Unknown character".into());
            let ship = detail
                .victim
                .ship_type_id
                .map(|id| ship_name(&client, esi, id, cache))
                .transpose()?
                .unwrap_or_else(|| "Unknown ship".into());
            let estimated_value_isk = estimate_killmail_value(&detail.victim, &market_prices);
            Ok(Killmail {
                id: recent.killmail_id,
                hash: recent.killmail_hash,
                sources: pending.sources,
                victim_id: detail.victim.character_id,
                victim_corporation_id: detail.victim.corporation_id,
                victim,
                ship,
                time: detail.killmail_time,
                estimated_value_isk,
            })
        })
        .collect()
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

fn character_name(
    client: &Client,
    esi: &str,
    id: u64,
    cache: &mut Option<EsiCache>,
) -> Result<String, String> {
    character_info(client, esi, id, cache).map(|info| info.name)
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
fn ship_name(
    client: &Client,
    esi: &str,
    id: u64,
    cache: &mut Option<EsiCache>,
) -> Result<String, String> {
    let info: Name = cached_get_json(
        client,
        cache,
        format!("{esi}/universe/types/{id}/"),
        true,
        || Ok(None),
        &format!("Ship name lookup for type {id}"),
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
                .adjusted_price
                .or(price.average_price)
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

    let response = request
        .send()
        .map_err(|error| format!("{request_description} failed: {error}"))?;
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
    if response.status() == StatusCode::TOO_MANY_REQUESTS {
        let retry_after = header_value(response.headers(), RETRY_AFTER)
            .unwrap_or_else(|| "an unspecified delay".into());
        return Err(format!(
            "{request_description} was rate limited; retry after {retry_after}"
        ));
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
    let ship_value = victim
        .ship_type_id
        .and_then(|type_id| market_prices.get(&type_id))?;
    let item_value = victim
        .items
        .iter()
        .filter_map(|item| {
            let quantity =
                item.quantity_destroyed.unwrap_or(0) + item.quantity_dropped.unwrap_or(0);
            market_prices
                .get(&item.item_type_id)
                .map(|price| *price * f64::from(quantity))
        })
        .sum::<f64>();
    Some(ship_value + item_value)
}

#[derive(Deserialize)]
struct Recent {
    killmail_id: u64,
    killmail_hash: String,
}
#[derive(Deserialize)]
struct Detail {
    killmail_time: String,
    victim: Victim,
}
#[derive(Deserialize)]
struct Victim {
    character_id: Option<u64>,
    corporation_id: Option<u64>,
    ship_type_id: Option<u64>,
    #[serde(default)]
    items: Vec<Item>,
}
#[derive(Deserialize)]
struct Item {
    item_type_id: u64,
    quantity_destroyed: Option<u32>,
    quantity_dropped: Option<u32>,
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
                .body(r#"{"killmail_time":"2026-08-16T10:00:00Z","victim":{"character_id":2,"corporation_id":20,"ship_type_id":3,"items":[{"item_type_id":4,"quantity_destroyed":2}]}}"#);
        });
        let prices = server.mock(|when, then| {
            when.method(GET).path("/markets/prices/");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"[{"type_id":3,"adjusted_price":1000.0},{"type_id":4,"average_price":500.0}]"#);
        });
        let victim = server.mock(|when, then| {
            when.method(GET).path("/characters/2/");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"name":"Fixture Victim","corporation_id":20}"#);
        });
        let ship = server.mock(|when, then| {
            when.method(GET).path("/universe/types/3/");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"name":"Fixture Ship"}"#);
        });

        let mails = load_killmails_at(
            &server.base_url(),
            &[character(1, "Pilot")],
            &mut None,
            |_| Ok("synthetic-token".into()),
        )
        .unwrap();

        assert_eq!(mails.len(), 1);
        assert_eq!(mails[0].id, 42);
        assert_eq!(mails[0].victim, "Fixture Victim");
        assert_eq!(mails[0].ship, "Fixture Ship");
        assert_eq!(mails[0].estimated_value_isk, Some(2_000.0));
        recent.assert();
        detail.assert();
        prices.assert();
        victim.assert();
        ship.assert();
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
