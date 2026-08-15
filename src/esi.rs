use crate::{
    auth,
    models::{Character, Killmail},
};
use reqwest::blocking::Client;
use serde::Deserialize;

const ESI: &str = "https://esi.evetech.net/latest";

pub fn load_killmails(chars: &[Character], client_id: &str) -> Result<Vec<Killmail>, String> {
    let client = Client::new();
    let mut result = Vec::new();
    for c in chars {
        let response: Vec<Recent> = client
            .get(format!("{ESI}/characters/{}/killmails/recent/", c.id))
            .bearer_auth(auth::access_token(c, client_id)?)
            .send()
            .map_err(|e| e.to_string())?
            .error_for_status()
            .map_err(|e| e.to_string())?
            .json()
            .map_err(|e| e.to_string())?;
        for recent in response {
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
            result.push((recent, detail, c.name.clone()));
        }
    }
    result
        .into_iter()
        .map(|(recent, detail, character)| {
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
                character,
                victim_id: detail.victim.character_id,
                victim,
                ship,
                time: detail.killmail_time,
            })
        })
        .collect()
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
