use crate::models::Store;
use std::{fs, path::PathBuf};

pub fn persist(store: &Store) -> Result<(), String> {
    let path = store_path()?;
    let data = serde_json::to_vec_pretty(store).map_err(|e| e.to_string())?;
    fs::write(path, data).map_err(|e| e.to_string())
}

pub fn load() -> Store {
    store_path()
        .ok()
        .and_then(|p| fs::read(p).ok())
        .and_then(|d| serde_json::from_slice(&d).ok())
        .unwrap_or_default()
}

fn store_path() -> Result<PathBuf, String> {
    config_dir().map(|p| p.join("akmp.json"))
}

fn config_dir() -> Result<PathBuf, String> {
    let path = std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|p| p.join(".config").join("akmp"))
        .ok_or("HOME is not set")?;
    fs::create_dir_all(&path).map_err(|e| e.to_string())?;
    Ok(path)
}
