use super::{
    character_info, check_cancelled, corporation_name, Character, Esi, ProtectedVictimKind,
    UniverseEntity, UniverseIds,
};
use crate::{
    integrations::{http::CooldownLog, ApiResult},
    models::ProtectedVictim,
};
use std::sync::atomic::AtomicBool;

pub fn refresh_character_affiliation(
    character: &mut Character,
    cancelled: &AtomicBool,
    cooldowns: &CooldownLog,
) -> ApiResult<()> {
    check_cancelled(cancelled)?;
    let esi = Esi::live(cooldowns)?;
    let info = character_info(&esi, character.id)?;
    check_cancelled(cancelled)?;
    let corporation_name = corporation_name(&esi, info.corporation_id)?;
    character.name = info.name;
    character.corporation_id = Some(info.corporation_id);
    character.corporation_name = Some(corporation_name);
    Ok(())
}

/// Resolves a protected victim from either a numeric EVE ID or an exact name.
pub fn resolve_protected_victim(
    kind: ProtectedVictimKind,
    query: &str,
    cooldowns: &CooldownLog,
) -> ApiResult<ProtectedVictim> {
    let esi = Esi::live(cooldowns)?;
    let (id, name) = match query.parse::<u64>() {
        Ok(id) if id > 0 => {
            let name = match kind {
                ProtectedVictimKind::Character => character_info(&esi, id)?.name,
                ProtectedVictimKind::Corporation => corporation_name(&esi, id)?,
            };
            (id, name)
        }
        _ => resolve_protected_victim_name_at(&esi, kind, query)?,
    };
    Ok(ProtectedVictim { id, name })
}

pub(super) fn resolve_protected_victim_name_at(
    esi: &Esi,
    kind: ProtectedVictimKind,
    name: &str,
) -> ApiResult<(u64, String)> {
    let response: UniverseIds = esi.post(
        "/universe/ids/",
        &[name],
        &format!("EVE name lookup for {name}"),
    )?;
    matching_protected_victim(response, kind, name)
        .map(|entity| (entity.id, entity.name))
        .ok_or_else(|| format!("no exact {} named {name} was found", kind.label()).into())
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
