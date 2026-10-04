use crate::{
    integrations::{auth, backend::LoadKillmailsOutcome, http::CooldownLog, ApiError, ApiResult},
    models::{
        Character, Killmail, KillmailAttacker, KillmailDetail, KillmailItem, KillmailLocation,
        KillmailVictimDetail, ProtectedVictimKind,
    },
};
use std::{
    collections::{HashMap, HashSet},
    sync::atomic::{AtomicBool, Ordering},
};

mod client;
mod killmails;
mod market;
mod types;
mod universe;

use client::Esi;
#[cfg(test)]
use killmails::add_pending;
use killmails::load_killmails_at;
use market::estimate_killmail_value;
use types::*;
#[cfg(test)]
use universe::{matching_protected_victim, resolve_protected_victim_name_at};
pub use universe::{refresh_character_affiliation, resolve_protected_victim};

const ESI: &str = "https://esi.evetech.net/latest";

pub fn load_killmails(
    chars: &[Character],
    cached_killmails: &[Killmail],
    reported_ids: &HashSet<u64>,
    cancelled: &AtomicBool,
    cooldowns: &CooldownLog,
    on_character_updated: &mut dyn FnMut(&Character) -> ApiResult<()>,
) -> ApiResult<LoadKillmailsOutcome> {
    load_killmails_at(
        &Esi::live(cooldowns)?,
        chars,
        cached_killmails,
        reported_ids,
        cancelled,
        |character| auth::access_token(character, on_character_updated),
    )
}

fn enrich_locations(esi: &Esi, mails: &mut [Killmail], cancelled: &AtomicBool) -> ApiResult<()> {
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
        check_cancelled(cancelled)?;
        if locations.contains_key(&system_id) {
            continue;
        }
        match location(esi, system_id, cancelled) {
            Ok(location) => {
                locations.insert(system_id, location);
            }
            Err(error) if error.is_fatal() => return Err(error),
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

fn location(esi: &Esi, system_id: u64, cancelled: &AtomicBool) -> ApiResult<KillmailLocation> {
    let system: SolarSystemInfo = esi.get(
        &format!("/universe/systems/{system_id}/"),
        &format!("Solar system lookup for {system_id}"),
    )?;
    check_cancelled(cancelled)?;
    let constellation_id = system.constellation_id;
    let constellation: ConstellationInfo = esi.get(
        &format!("/universe/constellations/{constellation_id}/"),
        &format!("Constellation lookup for {constellation_id}"),
    )?;
    check_cancelled(cancelled)?;
    let region_id = constellation.region_id;
    let region: Name = esi.get(
        &format!("/universe/regions/{region_id}/"),
        &format!("Region lookup for {region_id}"),
    )?;
    Ok(KillmailLocation {
        solar_system_id: system_id,
        solar_system_name: system.name,
        region_id: Some(region_id),
        region_name: Some(region.name),
    })
}

fn check_cancelled(cancelled: &AtomicBool) -> ApiResult<()> {
    if cancelled.load(Ordering::Relaxed) {
        Err(ApiError::Cancelled)
    } else {
        Ok(())
    }
}

fn character_info(esi: &Esi, id: u64) -> ApiResult<CharacterInfo> {
    esi.get(
        &format!("/characters/{id}/"),
        &format!("Character name lookup for {id}"),
    )
}

fn corporation_name(esi: &Esi, id: u64) -> ApiResult<String> {
    let info: Name = esi.get(
        &format!("/corporations/{id}/"),
        &format!("Corporation name lookup for {id}"),
    )?;
    Ok(info.name)
}

fn market_prices(esi: &Esi) -> ApiResult<HashMap<u64, f64>> {
    let prices: Vec<MarketPrice> = esi.get("/markets/prices/", "Market price request")?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{integrations::backend::LoadKillmailsOutcome, persistence::esi_cache::EsiCache};
    use httpmock::prelude::*;
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicBool, AtomicU64, Ordering},
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

    fn load(
        server: &MockServer,
        chars: &[Character],
        cached_killmails: &[Killmail],
        reported_ids: &HashSet<u64>,
        cooldowns: &CooldownLog,
    ) -> ApiResult<LoadKillmailsOutcome> {
        load_killmails_at(
            &Esi::new(&server.base_url(), None, cooldowns).unwrap(),
            chars,
            cached_killmails,
            reported_ids,
            &AtomicBool::new(false),
            |_| Ok("sentinel-token".into()),
        )
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
            let cooldowns = CooldownLog::default();
            let base_url = server.base_url();
            let esi = Esi::new(
                &base_url,
                Some(EsiCache::open_at(&path).unwrap()),
                &cooldowns,
            )
            .unwrap();
            let first: CharacterInfo = esi.get("/characters/2/", "Character name lookup").unwrap();
            let second: CharacterInfo = esi.get("/characters/2/", "Character name lookup").unwrap();

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
            let cache = EsiCache::open_at(&path).unwrap();
            let url = format!("{}/characters/2/", server.base_url());
            cache
                .store(
                    &url,
                    br#"{"name":"Cached Pilot","corporation_id":20}"#,
                    Some("Sat, 01 Jan 2000 00:00:00 GMT"),
                    Some("fixture-etag"),
                    None,
                )
                .unwrap();

            let cooldowns = CooldownLog::default();
            let base_url = server.base_url();
            let esi = Esi::new(&base_url, Some(cache), &cooldowns).unwrap();
            let response: CharacterInfo =
                esi.get("/characters/2/", "Character name lookup").unwrap();

            assert_eq!(response.name, "Cached Pilot");
            let cache = EsiCache::open_at(&path).unwrap();
            assert!(cache.load(&url).unwrap().unwrap().fresh);
        }
        request.assert_calls(1);
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn loads_killmails_through_the_configured_http_endpoint() {
        let server = MockServer::start();
        let cooldowns = CooldownLog::default();
        let recent = server.mock(|when, then| {
            when.method(GET)
                .path("/characters/1/killmails/recent/")
                .header("authorization", "Bearer sentinel-token");
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

        let mails = load(
            &server,
            &[character(1, "Pilot")],
            &[],
            &HashSet::new(),
            &cooldowns,
        )
        .unwrap()
        .killmails;

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
    fn rate_budget_during_location_lookup_stops_the_refresh() {
        let server = MockServer::start();
        let cooldowns = CooldownLog::default();
        server.mock(|when, then| {
            when.method(GET).path("/characters/1/killmails/recent/");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"[{"killmail_id":42,"killmail_hash":"fixture-hash"}]"#);
        });
        server.mock(|when, then| {
            when.method(GET).path("/killmails/42/fixture-hash");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"killmail_time":"2026-08-16T10:00:00Z","solar_system_id":30000142,"victim":{"damage_taken":1},"attackers":[]}"#);
        });
        server.mock(|when, then| {
            when.method(GET).path("/markets/prices/");
            then.status(200)
                .header("content-type", "application/json")
                .body("[]");
        });
        server.mock(|when, then| {
            when.method(GET).path("/universe/systems/30000142/");
            then.status(200)
                .header("content-type", "application/json")
                .header("x-ratelimit-limit", "10/15m")
                .header("x-ratelimit-remaining", "0")
                .body(r#"{"name":"Jita","constellation_id":20000020}"#);
        });
        let names = server.mock(|when, then| {
            when.method(POST).path("/universe/names/");
            then.status(200)
                .header("content-type", "application/json")
                .body("[]");
        });

        let result = load(
            &server,
            &[character(1, "Pilot")],
            &[],
            &HashSet::new(),
            &cooldowns,
        );

        assert!(
            matches!(&result, Err(ApiError::RateLimited(error)) if error.contains("rate budget"))
        );
        names.assert_calls(0);
        cooldowns.take();
    }

    #[test]
    fn matching_cached_detail_is_reused_and_source_membership_is_refreshed() {
        let server = MockServer::start();
        let cooldowns = CooldownLog::default();
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

        let mails = load(
            &server,
            &[character(1, "New Source")],
            &[cached],
            &HashSet::new(),
            &cooldowns,
        )
        .unwrap()
        .killmails;

        detail.assert_calls(0);
        assert_eq!(mails[0].victim, "Current Victim");
        assert_eq!(mails[0].ship, "Current Ship");
        assert_eq!(mails[0].sources[0].name, "New Source");
        assert_eq!(mails[0].estimated_value_isk, Some(1_000.0));
    }

    #[test]
    fn positively_reported_ids_are_filtered_before_detail_and_price_requests() {
        let server = MockServer::start();
        let cooldowns = CooldownLog::default();
        server.mock(|when, then| {
            when.method(GET).path("/characters/1/killmails/recent/");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"[{"killmail_id":42,"killmail_hash":"fixture-hash"}]"#);
        });

        let mails = load(
            &server,
            &[character(1, "Pilot")],
            &[],
            &HashSet::from([42]),
            &cooldowns,
        )
        .unwrap()
        .killmails;

        assert!(mails.is_empty());
    }

    #[test]
    fn market_rate_limit_stops_before_killmail_detail_requests() {
        let server = MockServer::start();
        let cooldowns = CooldownLog::default();
        server.mock(|when, then| {
            when.method(GET).path("/characters/1/killmails/recent/");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"[{"killmail_id":42,"killmail_hash":"sentinel-secret-hash"}]"#);
        });
        server.mock(|when, then| {
            when.method(GET).path("/markets/prices/");
            then.status(429)
                .header("retry-after", "60")
                .header("x-ratelimit-group", "market")
                .body("sentinel response body");
        });
        let detail = server.mock(|when, then| {
            when.method(GET).path("/killmails/42/sentinel-secret-hash");
            then.status(200).body("{}");
        });

        let error = match load(
            &server,
            &[character(1, "Pilot")],
            &[],
            &HashSet::new(),
            &cooldowns,
        ) {
            Err(ApiError::RateLimited(error)) => error,
            other => panic!("expected a rate-limit error, got {:?}", other.err()),
        };

        assert!(error.contains("rate limited"));
        assert!(!error.contains("sentinel"));
        detail.assert_calls(0);
        let cooldowns = cooldowns.take();
        assert_eq!(cooldowns.len(), 1);
        assert_eq!(cooldowns[0].scope.as_deref(), Some("market"));
    }

    #[test]
    fn exhausted_esi_error_budget_stops_the_next_character_request() {
        let server = MockServer::start();
        let cooldowns = CooldownLog::default();
        server.mock(|when, then| {
            when.method(GET).path("/characters/1/killmails/recent/");
            then.status(200)
                .header("content-type", "application/json")
                .header("x-esi-error-limit-remain", "0")
                .header("x-esi-error-limit-reset", "60")
                .body("[]");
        });
        let second = server.mock(|when, then| {
            when.method(GET).path("/characters/2/killmails/recent/");
            then.status(200)
                .header("content-type", "application/json")
                .body("[]");
        });

        let result = load(
            &server,
            &[character(1, "One"), character(2, "Two")],
            &[],
            &HashSet::new(),
            &cooldowns,
        );

        assert!(
            matches!(result, Err(ApiError::RateLimited(error)) if error.contains("rate budget"))
        );
        second.assert_calls(0);
        let cooldowns = cooldowns.take();
        assert_eq!(cooldowns.len(), 1);
        assert_eq!(
            cooldowns[0].reason.as_deref(),
            Some("error budget exhausted")
        );
    }

    #[test]
    fn low_group_budget_defers_detail_requests() {
        let server = MockServer::start();
        let cooldowns = CooldownLog::default();
        server.mock(|when, then| {
            when.method(GET).path("/characters/1/killmails/recent/");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"[{"killmail_id":42,"killmail_hash":"sentinel-secret-hash"}]"#);
        });
        server.mock(|when, then| {
            when.method(GET).path("/markets/prices/");
            then.status(200)
                .header("content-type", "application/json")
                .header("x-ratelimit-limit", "150/15m")
                .header("x-ratelimit-remaining", "10")
                .header("x-ratelimit-group", "market")
                .body("[]");
        });
        let detail = server.mock(|when, then| {
            when.method(GET).path("/killmails/42/sentinel-secret-hash");
            then.status(200).body("{}");
        });

        let result = load(
            &server,
            &[character(1, "Pilot")],
            &[],
            &HashSet::new(),
            &cooldowns,
        );

        assert!(
            matches!(result, Err(ApiError::RateLimited(error)) if error.contains("rate budget"))
        );
        detail.assert_calls(0);
        let cooldowns = cooldowns.take();
        assert_eq!(cooldowns[0].scope.as_deref(), Some("market"));
        assert_eq!(cooldowns[0].reason.as_deref(), Some("low rate budget"));
    }

    #[test]
    fn recursive_estimate_prefers_average_prices_and_keeps_partial_totals() {
        let detail: Detail = serde_json::from_str(
            r#"{
                "killmail_time":"2026-08-16T10:00:00Z",
                "solar_system_id":1,
                "victim":{
                    "ship_type_id":1,
                    "items":[{
                        "item_type_id":2,"quantity_destroyed":2,
                        "items":[{"item_type_id":3,"quantity_dropped":4}]
                    }]
                }
            }"#,
        )
        .unwrap();
        let detail = killmails::convert_detail(detail);
        let prices = HashMap::from([(1, 10.0), (2, 5.0), (3, 2.0)]);

        assert_eq!(estimate_killmail_value(&detail, &prices), Some(28.0));
        assert_eq!(estimate_killmail_value(&detail, &HashMap::new()), None);
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

        let cooldowns = CooldownLog::default();
        let base_url = server.base_url();
        let result = resolve_protected_victim_name_at(
            &Esi::new(&base_url, None, &cooldowns).unwrap(),
            ProtectedVictimKind::Character,
            "Fixture Pilot",
        )
        .unwrap();

        assert_eq!(result, (42, "Fixture Pilot".into()));
        lookup.assert();
    }
}
