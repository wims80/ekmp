use serde::Deserialize;

#[derive(Deserialize)]
pub(super) struct Recent {
    pub(super) killmail_id: u64,
    pub(super) killmail_hash: String,
}
#[derive(Deserialize)]
pub(super) struct Detail {
    pub(super) killmail_time: String,
    pub(super) solar_system_id: u64,
    pub(super) victim: Victim,
    #[serde(default)]
    pub(super) attackers: Vec<Attacker>,
}
#[derive(Deserialize)]
pub(super) struct Victim {
    pub(super) character_id: Option<u64>,
    pub(super) corporation_id: Option<u64>,
    pub(super) alliance_id: Option<u64>,
    pub(super) ship_type_id: Option<u64>,
    #[serde(default)]
    pub(super) damage_taken: u64,
    #[serde(default)]
    pub(super) items: Vec<Item>,
}
#[derive(Deserialize)]
pub(super) struct Item {
    pub(super) item_type_id: u64,
    #[serde(default)]
    pub(super) flag: u32,
    pub(super) quantity_destroyed: Option<u64>,
    pub(super) quantity_dropped: Option<u64>,
    #[serde(default)]
    pub(super) singleton: u32,
    #[serde(default)]
    pub(super) items: Vec<Item>,
}
#[derive(Deserialize)]
pub(super) struct Attacker {
    pub(super) character_id: Option<u64>,
    pub(super) corporation_id: Option<u64>,
    pub(super) alliance_id: Option<u64>,
    pub(super) faction_id: Option<u64>,
    pub(super) ship_type_id: Option<u64>,
    pub(super) weapon_type_id: Option<u64>,
    #[serde(default)]
    pub(super) damage_done: u64,
    #[serde(default)]
    pub(super) final_blow: bool,
    pub(super) security_status: Option<f32>,
}
#[derive(Deserialize)]
pub(super) struct MarketPrice {
    pub(super) type_id: u64,
    pub(super) adjusted_price: Option<f64>,
    pub(super) average_price: Option<f64>,
}
#[derive(Deserialize)]
pub(super) struct CharacterInfo {
    pub(super) name: String,
    pub(super) corporation_id: u64,
}
#[derive(Deserialize)]
pub(super) struct Name {
    pub(super) name: String,
}

#[derive(Deserialize)]
pub(super) struct SolarSystemInfo {
    pub(super) name: String,
    pub(super) constellation_id: u64,
}

#[derive(Deserialize)]
pub(super) struct ConstellationInfo {
    pub(super) region_id: u64,
}

#[derive(Deserialize)]
pub(super) struct UniverseName {
    pub(super) id: u64,
    pub(super) name: String,
}

#[derive(Deserialize)]
pub(super) struct UniverseIds {
    #[serde(default)]
    pub(super) characters: Vec<UniverseEntity>,
    #[serde(default)]
    pub(super) corporations: Vec<UniverseEntity>,
}

#[derive(Deserialize)]
pub(super) struct UniverseEntity {
    pub(super) id: u64,
    pub(super) name: String,
}
