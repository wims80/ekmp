use super::{
    character_info, corporation_name, Character, Client, EsiCache, ProtectedVictimKind,
    UniverseEntity, UniverseIds, ESI, USER_AGENT, USER_AGENT_VALUE,
};

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

pub(super) fn resolve_protected_victim_name_at(
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
