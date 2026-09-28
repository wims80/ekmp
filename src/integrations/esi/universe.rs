use super::{
    active_cooldown_error, character_info,
    client::{esi_limit_error, observe_rate_limit},
    corporation_name, http_client, Character, EsiCache, ProtectedVictimKind, UniverseEntity,
    UniverseIds, ESI, USER_AGENT, USER_AGENT_VALUE,
};
use std::sync::atomic::AtomicBool;

pub fn refresh_character_affiliation(
    character: &mut Character,
    cancelled: &AtomicBool,
) -> Result<(), String> {
    let mut cache = EsiCache::open().ok();
    refresh_character_affiliation_at(ESI, character, &mut cache, cancelled)
}

fn refresh_character_affiliation_at(
    esi: &str,
    character: &mut Character,
    cache: &mut Option<EsiCache>,
    cancelled: &AtomicBool,
) -> Result<(), String> {
    super::check_cancelled(cancelled)?;
    let client = http_client()?;
    let info = character_info(&client, esi, character.id, cache)?;
    super::check_cancelled(cancelled)?;
    let corporation_name = corporation_name(&client, esi, info.corporation_id, cache)?;
    character.name = info.name;
    character.corporation_id = Some(info.corporation_id);
    character.corporation_name = Some(corporation_name);
    Ok(())
}

pub fn resolve_character_name(id: u64) -> Result<String, String> {
    let mut cache = EsiCache::open().ok();
    character_info(&http_client()?, ESI, id, &mut cache).map(|info| info.name)
}

pub fn resolve_corporation_name(id: u64) -> Result<String, String> {
    let mut cache = EsiCache::open().ok();
    corporation_name(&http_client()?, ESI, id, &mut cache)
}

pub fn resolve_protected_victim_name(
    kind: ProtectedVictimKind,
    name: &str,
) -> Result<(u64, String), String> {
    if let Some(error) = active_cooldown_error() {
        return Err(error);
    }
    resolve_protected_victim_name_at(ESI, kind, name)
}

pub(super) fn resolve_protected_victim_name_at(
    esi: &str,
    kind: ProtectedVictimKind,
    name: &str,
) -> Result<(u64, String), String> {
    let response = http_client()?
        .post(format!("{esi}/universe/ids/"))
        .header(USER_AGENT, USER_AGENT_VALUE)
        .json(&[name])
        .send()
        .map_err(|error| {
            if error.is_timeout() {
                format!("EVE name lookup timed out for {name}")
            } else if error.is_connect() {
                format!("EVE name lookup could not connect for {name}")
            } else {
                format!("EVE name lookup transport failed for {name}")
            }
        })?;
    observe_rate_limit(&response);
    if let Some(error) = esi_limit_error(&response, "EVE name lookup") {
        return Err(error);
    }
    if !response.status().is_success() {
        return Err(format!(
            "EVE name lookup failed for {name} (HTTP {})",
            response.status().as_u16()
        ));
    }
    let response: UniverseIds = response
        .json()
        .map_err(|_| format!("EVE name response was invalid for {name}"))?;
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

pub(super) fn matching_protected_victim(
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
