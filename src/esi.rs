use crate::{
    auth,
    models::{Character, Killmail},
};
use reqwest::blocking::Client;
use serde::Deserialize;
use std::collections::HashMap;

const ESI: &str = "https://esi.evetech.net/latest";

pub fn load_killmails(chars: &[Character]) -> Result<Vec<Killmail>, String> {
    let client = Client::new();
    let mut pending = Vec::new();
    let mut positions = HashMap::new();
    for c in chars {
        let response: Vec<Recent> = client
            .get(format!("{ESI}/characters/{}/killmails/recent/", c.id))
            .bearer_auth(auth::access_token(c)?)
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
                    "{ESI}/killmails/{}/{}",
                    recent.killmail_id, recent.killmail_hash
                ))
                .send()
                .map_err(|e| format!("Killmail {} request failed: {e}", recent.killmail_id))?
                .error_for_status()
                .map_err(|e| format!("Killmail {} request failed: {e}", recent.killmail_id))?
                .json()
                .map_err(|e| format!("Killmail {} response invalid: {e}", recent.killmail_id))?;
            let victim = detail
                .victim
                .character_id
                .map(|id| character_name(&client, id))
                .transpose()?
                .unwrap_or_else(|| "Unknown character".into());
            let ship = detail
                .victim
                .ship_type_id
                .map(|id| ship_name(&client, id))
                .transpose()?
                .unwrap_or_else(|| "Unknown ship".into());
            Ok(Killmail {
                id: recent.killmail_id,
                hash: recent.killmail_hash,
                sources: pending.sources,
                victim_id: detail.victim.character_id,
                victim,
                ship,
                time: detail.killmail_time,
            })
        })
        .collect()
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

fn character_name(client: &Client, id: u64) -> Result<String, String> {
    let info: Name = client
        .get(format!("{ESI}/characters/{id}/"))
        .send()
        .map_err(|e| format!("Character name lookup failed for {id}: {e}"))?
        .error_for_status()
        .map_err(|e| format!("Character name lookup failed for {id}: {e}"))?
        .json()
        .map_err(|e| format!("Character name response invalid for {id}: {e}"))?;
    Ok(info.name)
}
fn ship_name(client: &Client, id: u64) -> Result<String, String> {
    let info: Name = client
        .get(format!("{ESI}/universe/types/{id}/"))
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
    ship_type_id: Option<u64>,
}
#[derive(Deserialize)]
struct Name {
    name: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn character(id: u64, name: &str) -> Character {
        Character {
            id,
            name: name.into(),
            refresh_token: String::new(),
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
}
