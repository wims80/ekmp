use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Default, Serialize, Deserialize)]
pub struct Store {
    pub characters: Vec<Character>,
    #[serde(default)]
    pub zkill_cache: HashMap<u64, ZkillCacheEntry>,
    #[serde(default)]
    pub show_reported_killmails: bool,
    #[serde(default)]
    pub cached_killmails: Vec<Killmail>,
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
    pub refresh_token: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Killmail {
    pub id: u64,
    pub hash: String,
    pub sources: Vec<CharacterSource>,
    pub victim_id: Option<u64>,
    pub victim: String,
    pub ship: String,
    pub time: String,
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
        assert!(!store.show_reported_killmails);
        assert!(store.cached_killmails.is_empty());
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
            show_reported_killmails: true,
            cached_killmails: vec![Killmail {
                id: 7,
                hash: "hash".into(),
                sources: vec![CharacterSource {
                    id: 1,
                    name: "Pilot".into(),
                }],
                victim_id: Some(2),
                victim: "Victim".into(),
                ship: "Ship".into(),
                time: "Time".into(),
            }],
            ..Store::default()
        };
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
        assert!(restored.show_reported_killmails);
        assert_eq!(restored.cached_killmails.len(), 1);
        assert_eq!(restored.cached_killmails[0].id, 7);
    }
}
