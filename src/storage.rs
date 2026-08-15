use crate::models::Store;
use std::{fs, path::PathBuf};

const CONFIG_DIR_NAME: &str = "ekmp";
const STORE_FILE_NAME: &str = "ekmp.json";

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
    config_dir().map(|path| path.join(STORE_FILE_NAME))
}

fn config_dir() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or("HOME is not set")?;
    let path = config_dir_path(home);
    fs::create_dir_all(&path).map_err(|e| e.to_string())?;
    Ok(path)
}

fn config_dir_path(home: PathBuf) -> PathBuf {
    home.join(".config").join(CONFIG_DIR_NAME)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_uses_the_ekmp_configuration_location() {
        let path = config_dir_path(PathBuf::from("/home/tester")).join(STORE_FILE_NAME);

        assert_eq!(path, PathBuf::from("/home/tester/.config/ekmp/ekmp.json"));
    }
}
