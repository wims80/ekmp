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
    #[cfg(windows)]
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .ok_or("APPDATA is not set")?;

    #[cfg(not(windows))]
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or("HOME is not set")?;

    #[cfg(windows)]
    let path = config_dir_path(base);

    #[cfg(not(windows))]
    let path = config_dir_path(home.join(".config"));

    fs::create_dir_all(&path).map_err(|e| e.to_string())?;
    Ok(path)
}

fn config_dir_path(base: PathBuf) -> PathBuf {
    base.join(CONFIG_DIR_NAME)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(not(windows))]
    fn store_uses_the_unix_configuration_location() {
        let path = config_dir_path(PathBuf::from("/home/tester/.config")).join(STORE_FILE_NAME);

        assert_eq!(path, PathBuf::from("/home/tester/.config/ekmp/ekmp.json"));
    }

    #[cfg(windows)]
    #[test]
    fn store_uses_the_windows_roaming_app_data_location() {
        let path = config_dir_path(PathBuf::from(r"C:\Users\tester\AppData\Roaming"))
            .join(STORE_FILE_NAME);

        assert_eq!(
            path,
            PathBuf::from(r"C:\Users\tester\AppData\Roaming\ekmp\ekmp.json")
        );
    }
}
