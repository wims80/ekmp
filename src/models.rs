use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const ZKILL_STATUS_CACHE_VERSION: u8 = 2;

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Store {
    pub characters: Vec<Character>,
    #[serde(default)]
    pub zkill_cache: HashMap<u64, ZkillCacheEntry>,
    #[serde(default)]
    pub zkill_status_cache_version: u8,
    #[serde(default)]
    pub show_protected_killmails: bool,
    #[serde(default)]
    pub cached_killmails: Vec<Killmail>,
    #[serde(default)]
    pub manually_protected_characters: Vec<ProtectedVictim>,
    #[serde(default)]
    pub manually_protected_corporations: Vec<ProtectedVictim>,
    #[serde(default)]
    pub manually_protected_killmail_ids: Vec<u64>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct ZkillCacheEntry {
    pub reported: bool,
    pub checked_at: u64,
}

impl ZkillCacheEntry {
    pub fn is_fresh(self, now: u64, negative_ttl: u64) -> bool {
        self.reported || now.saturating_sub(self.checked_at) < negative_ttl
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Character {
    pub id: u64,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub corporation_id: Option<u64>,
    #[serde(default)]
    pub corporation_name: Option<String>,
}

impl Character {
    pub fn uses_json_refresh_token_fallback(&self) -> bool {
        self.refresh_token.is_some()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtectedVictim {
    pub id: u64,
    pub name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtectedVictimKind {
    Character,
    Corporation,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Killmail {
    pub id: u64,
    pub hash: String,
    pub sources: Vec<CharacterSource>,
    pub victim_id: Option<u64>,
    #[serde(default)]
    pub victim_corporation_id: Option<u64>,
    pub victim: String,
    pub ship: String,
    pub time: String,
    #[serde(default)]
    pub estimated_value_isk: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<KillmailDetail>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KillmailDetail {
    pub victim: KillmailVictimDetail,
    #[serde(default)]
    pub attackers: Vec<KillmailAttacker>,
    pub location: KillmailLocation,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KillmailVictimDetail {
    #[serde(default)]
    pub corporation_name: Option<String>,
    #[serde(default)]
    pub alliance_id: Option<u64>,
    #[serde(default)]
    pub alliance_name: Option<String>,
    #[serde(default)]
    pub ship_type_id: Option<u64>,
    pub damage_taken: u64,
    #[serde(default)]
    pub items: Vec<KillmailItem>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KillmailAttacker {
    #[serde(default)]
    pub character_id: Option<u64>,
    #[serde(default)]
    pub character_name: Option<String>,
    #[serde(default)]
    pub corporation_id: Option<u64>,
    #[serde(default)]
    pub corporation_name: Option<String>,
    #[serde(default)]
    pub alliance_id: Option<u64>,
    #[serde(default)]
    pub alliance_name: Option<String>,
    #[serde(default)]
    pub faction_id: Option<u64>,
    #[serde(default)]
    pub faction_name: Option<String>,
    #[serde(default)]
    pub ship_type_id: Option<u64>,
    #[serde(default)]
    pub ship_name: Option<String>,
    #[serde(default)]
    pub weapon_type_id: Option<u64>,
    #[serde(default)]
    pub weapon_name: Option<String>,
    pub damage_done: u64,
    pub final_blow: bool,
    #[serde(default)]
    pub security_status: Option<f32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KillmailLocation {
    pub solar_system_id: u64,
    pub solar_system_name: String,
    #[serde(default)]
    pub region_id: Option<u64>,
    #[serde(default)]
    pub region_name: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KillmailItem {
    pub item_type_id: u64,
    pub name: String,
    pub flag: u32,
    #[serde(default)]
    pub quantity_destroyed: u64,
    #[serde(default)]
    pub quantity_dropped: u64,
    #[serde(default)]
    pub singleton: u32,
    #[serde(default)]
    pub items: Vec<KillmailItem>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CharacterSource {
    pub id: u64,
    pub name: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_store_without_cache_still_deserializes() {
        let store: Store = serde_json::from_str(
            r#"{"characters":[{"id":1,"name":"Pilot","refresh_token":"token"}]}"#,
        )
        .unwrap();

        assert_eq!(store.characters.len(), 1);
        assert!(store.zkill_cache.is_empty());
        assert_eq!(store.zkill_status_cache_version, 0);
        assert!(!store.show_protected_killmails);
        assert!(store.cached_killmails.is_empty());
        assert!(store.manually_protected_characters.is_empty());
        assert!(store.manually_protected_corporations.is_empty());
        assert!(store.manually_protected_killmail_ids.is_empty());
        assert_eq!(store.characters[0].refresh_token.as_deref(), Some("token"));
        assert_eq!(store.characters[0].corporation_id, None);
    }

    #[test]
    fn positive_cache_entries_do_not_expire() {
        let entry = ZkillCacheEntry {
            reported: true,
            checked_at: 1,
        };

        assert!(entry.is_fresh(u64::MAX, 900));
    }

    #[test]
    fn negative_cache_entries_expire_at_the_ttl() {
        let entry = ZkillCacheEntry {
            reported: false,
            checked_at: 100,
        };

        assert!(entry.is_fresh(999, 900));
        assert!(!entry.is_fresh(1_000, 900));
    }

    #[test]
    fn store_round_trips_cache_entries() {
        let mut store = Store {
            show_protected_killmails: true,
            cached_killmails: vec![Killmail {
                id: 7,
                hash: "hash".into(),
                sources: vec![CharacterSource {
                    id: 1,
                    name: "Pilot".into(),
                }],
                victim_id: Some(2),
                victim_corporation_id: Some(3),
                victim: "Victim".into(),
                ship: "Ship".into(),
                time: "Time".into(),
                estimated_value_isk: Some(1_250_000.0),
                detail: None,
            }],
            ..Store::default()
        };
        store.manually_protected_characters.push(ProtectedVictim {
            id: 8,
            name: "Protected Pilot".into(),
        });
        store.manually_protected_corporations.push(ProtectedVictim {
            id: 9,
            name: "Protected Corp".into(),
        });
        store.manually_protected_killmail_ids.push(10);
        store.zkill_cache.insert(
            42,
            ZkillCacheEntry {
                reported: true,
                checked_at: 123,
            },
        );

        let json = serde_json::to_string(&store).unwrap();
        let restored: Store = serde_json::from_str(&json).unwrap();

        let entry = restored.zkill_cache[&42];
        assert!(entry.reported);
        assert_eq!(entry.checked_at, 123);
        assert!(restored.show_protected_killmails);
        assert_eq!(restored.cached_killmails.len(), 1);
        assert_eq!(restored.cached_killmails[0].id, 7);
        assert_eq!(
            restored.cached_killmails[0].estimated_value_isk,
            Some(1_250_000.0)
        );
        assert_eq!(restored.manually_protected_characters[0].id, 8);
        assert_eq!(restored.manually_protected_corporations[0].id, 9);
        assert_eq!(restored.manually_protected_killmail_ids, vec![10]);
        assert_eq!(restored.zkill_status_cache_version, 0);
    }

    #[test]
    fn secure_store_tokens_are_not_serialized() {
        let store = Store {
            characters: vec![Character {
                id: 1,
                name: "Pilot".into(),
                refresh_token: None,
                corporation_id: None,
                corporation_name: None,
            }],
            ..Store::default()
        };

        let json = serde_json::to_string(&store).unwrap();

        assert!(!json.contains("refresh_token"));
    }

    #[test]
    fn json_fallback_tokens_are_serialized() {
        let store = Store {
            characters: vec![Character {
                id: 1,
                name: "Pilot".into(),
                refresh_token: Some("token".into()),
                corporation_id: None,
                corporation_name: None,
            }],
            ..Store::default()
        };

        let json = serde_json::to_string(&store).unwrap();

        assert!(json.contains("refresh_token"));
        assert!(store.characters[0].uses_json_refresh_token_fallback());
    }
}
