use crate::models::Store;
use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

const CONFIG_DIR_NAME: &str = "ekmp";
const STORE_FILE_NAME: &str = "ekmp.json";

pub fn persist(store: &Store) -> Result<(), String> {
    let path = store_path()?;
    let data = serde_json::to_vec_pretty(store).map_err(|error| error.to_string())?;
    persist_to_path(&path, &data)
        .map_err(|error| format!("could not atomically write {}: {error}", path.display()))
}

pub fn load() -> Result<Store, String> {
    let path = store_path()?;
    load_from_path(&path)
}

fn load_from_path(path: &Path) -> Result<Store, String> {
    match fs::read(path) {
        Ok(data) => serde_json::from_slice(&data)
            .map_err(|error| format!("{} contains invalid JSON: {error}", path.display())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => load_missing_store(path),
        Err(error) => Err(format!("could not read {}: {error}", path.display())),
    }
}

#[cfg(not(windows))]
fn load_missing_store(_path: &Path) -> Result<Store, String> {
    Ok(Store::default())
}

#[cfg(windows)]
fn load_missing_store(path: &Path) -> Result<Store, String> {
    let backup_path = sibling_path(path, "bak");
    match fs::read(&backup_path) {
        Ok(data) => serde_json::from_slice(&data).map_err(|error| {
            format!(
                "recovery file {} contains invalid JSON: {error}",
                backup_path.display()
            )
        }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Store::default()),
        Err(error) => Err(format!(
            "could not read recovery file {}: {error}",
            backup_path.display()
        )),
    }
}

fn persist_to_path(path: &Path, data: &[u8]) -> io::Result<()> {
    let temporary_path = sibling_path(path, "tmp");
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    let mut temporary_file = options.open(&temporary_path)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temporary_file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }

    temporary_file.write_all(data)?;
    temporary_file.sync_all()?;
    drop(temporary_file);
    replace_file(&temporary_path, path)
}

#[cfg(not(windows))]
fn replace_file(temporary_path: &Path, path: &Path) -> io::Result<()> {
    fs::rename(temporary_path, path)
}

#[cfg(windows)]
fn replace_file(temporary_path: &Path, path: &Path) -> io::Result<()> {
    let backup_path = sibling_path(path, "bak");
    let had_previous_file = path.exists();
    if had_previous_file {
        if backup_path.exists() {
            fs::remove_file(&backup_path)?;
        }
        fs::rename(path, &backup_path)?;
    }

    match fs::rename(temporary_path, path) {
        Ok(()) => {
            if had_previous_file {
                fs::remove_file(backup_path)?;
            }
            Ok(())
        }
        Err(error) => {
            if had_previous_file {
                let _ = fs::rename(backup_path, path);
            }
            Err(error)
        }
    }
}

fn sibling_path(path: &Path, suffix: &str) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(STORE_FILE_NAME);
    path.with_file_name(format!("{file_name}.{suffix}"))
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

    fs::create_dir_all(&path).map_err(|error| error.to_string())?;
    Ok(path)
}

fn config_dir_path(base: PathBuf) -> PathBuf {
    base.join(CONFIG_DIR_NAME)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST_DIRECTORY: AtomicU64 = AtomicU64::new(0);

    fn temporary_store_path() -> PathBuf {
        let sequence = NEXT_TEST_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "ekmp-storage-test-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&directory).unwrap();
        directory.join(STORE_FILE_NAME)
    }

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

    #[test]
    fn missing_store_loads_as_default() {
        let path = temporary_store_path();

        let store = load_from_path(&path).unwrap();

        assert!(store.characters.is_empty());
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn malformed_store_is_reported_instead_of_silently_discarded() {
        let path = temporary_store_path();
        fs::write(&path, b"not json").unwrap();

        let error = load_from_path(&path).err().unwrap();

        assert!(error.contains("contains invalid JSON"));
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn persisted_store_replaces_the_destination_and_cleans_up_temporary_file() {
        let path = temporary_store_path();
        fs::write(&path, b"old data").unwrap();
        let store = Store {
            show_protected_killmails: true,
            ..Store::default()
        };
        let data = serde_json::to_vec_pretty(&store).unwrap();

        persist_to_path(&path, &data).unwrap();

        assert!(load_from_path(&path).unwrap().show_protected_killmails);
        assert!(!sibling_path(&path, "tmp").exists());
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn persisted_store_is_private_to_the_user() {
        use std::os::unix::fs::PermissionsExt;

        let path = temporary_store_path();
        persist_to_path(&path, b"{}").unwrap();

        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
