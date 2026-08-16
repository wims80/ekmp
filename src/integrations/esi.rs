use crate::{
    integrations::auth,
    models::{Character, Killmail, ProtectedVictimKind},
};
use reqwest::blocking::Client;
use reqwest::header::USER_AGENT;
use serde::Deserialize;
use std::collections::HashMap;

const ESI: &str = "https://esi.evetech.net/latest";
const USER_AGENT_VALUE: &str = concat!(
    "ekmp/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/wims80/ekmp)"
);

pub fn load_killmails(chars: &[Character]) -> Result<Vec<Killmail>, String> {
    load_killmails_at(ESI, chars, auth::access_token)
}

fn load_killmails_at(
    esi: &str,
    chars: &[Character],
    mut access_token: impl FnMut(&Character) -> Result<String, String>,
) -> Result<Vec<Killmail>, String> {
    let client = Client::new();
    let mut pending = Vec::new();
    let mut positions = HashMap::new();
    for c in chars {
        let response: Vec<Recent> = client
            .get(format!("{esi}/characters/{}/killmails/recent/", c.id))
            .header(USER_AGENT, USER_AGENT_VALUE)
            .bearer_auth(access_token(c)?)
            .send()
            .map_err(|e| e.to_string())?
            .error_for_status()
            .map_err(|e| e.to_string())?
            .json()
            .map_err(|e| e.to_string())?;
        for recent in response {
            add_pending(&mut pending, &mut positions, recent, c);
        }
    }
    pending
        .into_iter()
        .map(|pending| {
            let recent = pending.recent;
            let detail: Detail = client
                .get(format!(
                    "{esi}/killmails/{}/{}",
                    recent.killmail_id, recent.killmail_hash
                ))
                .header(USER_AGENT, USER_AGENT_VALUE)
                .send()
                .map_err(|e| format!("Killmail {} request failed: {e}", recent.killmail_id))?
                .error_for_status()
                .map_err(|e| format!("Killmail {} request failed: {e}", recent.killmail_id))?
                .json()
                .map_err(|e| format!("Killmail {} response invalid: {e}", recent.killmail_id))?;
            let victim = detail
                .victim
                .character_id
                .map(|id| character_name(&client, esi, id))
                .transpose()?
                .unwrap_or_else(|| "Unknown character".into());
            let ship = detail
                .victim
                .ship_type_id
                .map(|id| ship_name(&client, esi, id))
                .transpose()?
                .unwrap_or_else(|| "Unknown ship".into());
            Ok(Killmail {
                id: recent.killmail_id,
                hash: recent.killmail_hash,
                sources: pending.sources,
                victim_id: detail.victim.character_id,
                victim_corporation_id: detail.victim.corporation_id,
                victim,
                ship,
                time: detail.killmail_time,
            })
        })
        .collect()
}

pub fn refresh_character_affiliation(character: &mut Character) -> Result<(), String> {
    refresh_character_affiliation_at(ESI, character)
}

fn refresh_character_affiliation_at(esi: &str, character: &mut Character) -> Result<(), String> {
    let client = Client::new();
    let info = character_info(&client, esi, character.id)?;
    let corporation_name = corporation_name(&client, esi, info.corporation_id)?;
    character.name = info.name;
    character.corporation_id = Some(info.corporation_id);
    character.corporation_name = Some(corporation_name);
    Ok(())
}

pub fn resolve_character_name(id: u64) -> Result<String, String> {
    character_info(&Client::new(), ESI, id).map(|info| info.name)
}

pub fn resolve_corporation_name(id: u64) -> Result<String, String> {
    corporation_name(&Client::new(), ESI, id)
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

fn character_name(client: &Client, esi: &str, id: u64) -> Result<String, String> {
    character_info(client, esi, id).map(|info| info.name)
}

fn character_info(client: &Client, esi: &str, id: u64) -> Result<CharacterInfo, String> {
    client
        .get(format!("{esi}/characters/{id}/"))
        .header(USER_AGENT, USER_AGENT_VALUE)
        .send()
        .map_err(|e| format!("Character name lookup failed for {id}: {e}"))?
        .error_for_status()
        .map_err(|e| format!("Character name lookup failed for {id}: {e}"))?
        .json()
        .map_err(|e| format!("Character response invalid for {id}: {e}"))
}

fn corporation_name(client: &Client, esi: &str, id: u64) -> Result<String, String> {
    let info: Name = client
        .get(format!("{esi}/corporations/{id}/"))
        .header(USER_AGENT, USER_AGENT_VALUE)
        .send()
        .map_err(|e| format!("Corporation name lookup failed for {id}: {e}"))?
        .error_for_status()
        .map_err(|e| format!("Corporation name lookup failed for {id}: {e}"))?
        .json()
        .map_err(|e| format!("Corporation name response invalid for {id}: {e}"))?;
    Ok(info.name)
}
fn ship_name(client: &Client, esi: &str, id: u64) -> Result<String, String> {
    let info: Name = client
        .get(format!("{esi}/universe/types/{id}/"))
        .header(USER_AGENT, USER_AGENT_VALUE)
        .send()
        .map_err(|e| format!("Ship name lookup failed for type {id}: {e}"))?
        .error_for_status()
        .map_err(|e| format!("Ship name lookup failed for type {id}: {e}"))?
        .json()
        .map_err(|e| format!("Ship name response invalid for type {id}: {e}"))?;
    Ok(info.name)
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
                .body(r#"{"killmail_time":"2026-08-16T10:00:00Z","victim":{"character_id":2,"corporation_id":20,"ship_type_id":3}}"#);
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

        let mails = load_killmails_at(&server.base_url(), &[character(1, "Pilot")], |_| {
            Ok("synthetic-token".into())
        })
        .unwrap();

        assert_eq!(mails.len(), 1);
        assert_eq!(mails[0].id, 42);
        assert_eq!(mails[0].victim, "Fixture Victim");
        assert_eq!(mails[0].ship, "Fixture Ship");
        recent.assert();
        detail.assert();
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
