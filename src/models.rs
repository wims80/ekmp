use serde::{Deserialize, Serialize};

#[derive(Default, Serialize, Deserialize)]
pub struct Store {
    pub client_id: String,
    pub characters: Vec<Character>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Character {
    pub id: u64,
    pub name: String,
    pub refresh_token: String,
}

#[derive(Clone)]
pub struct Killmail {
    pub id: u64,
    pub hash: String,
    pub character: String,
    pub victim_id: Option<u64>,
    pub victim: String,
    pub ship: String,
    pub time: String,
}
