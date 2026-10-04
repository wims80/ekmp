use super::paths::{self, create_private_dir};
use crate::models::{Store, DEFAULT_REFRESH_INTERVAL_SECS, MIN_REFRESH_INTERVAL_SECS};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

const CONFIG_FILE_NAME: &str = "config.toml";
const STATE_FILE_NAME: &str = "state.json";
const CREDENTIALS_FILE_NAME: &str = "credentials.json";
const CONFIG_HEADER: &str = "\
# EVE Killmail Publisher settings.
# Change them with `ekmp config set` or edit this file; ekmp rewrites it only
# when a setting changes.
#
# refresh-interval-secs: minimum seconds between refreshes.
# show-protected-killmails: whether lists include protected victims.

";
const OPERATION_LOCK_SUFFIX: &str = "lock";
const SERVICE_LOCK_SUFFIX: &str = "service.lock";

/// The files that together persist one `Store`.
///
/// Locks are siblings of the state file, which therefore identifies the store.
#[derive(Clone, Debug)]
pub(crate) struct StorePaths {
    /// User preferences.
    pub config: PathBuf,
    /// Everything else, without preferences or refresh tokens.
    pub state: PathBuf,
    /// Refresh tokens that the system credential store could not accept.
    pub credentials: PathBuf,
}

impl StorePaths {
    /// The per-user XDG locations, with their directories created.
    pub(crate) fn live() -> Result<Self, String> {
        let config_dir = paths::config_dir()?;
        let state_dir = paths::state_dir()?;
        for directory in [&config_dir, &state_dir] {
            create_private_dir(directory)
                .map_err(|error| format!("could not create {}: {error}", directory.display()))?;
        }
        Ok(Self {
            config: config_dir.join(CONFIG_FILE_NAME),
            state: state_dir.join(STATE_FILE_NAME),
            credentials: state_dir.join(CREDENTIALS_FILE_NAME),
        })
    }

    #[cfg(any(test, feature = "dev-tools"))]
    /// Keeps all files next to `state`, e.g. `dev.json`, `dev.config.toml`, and
    /// `dev.credentials.json`.
    pub(crate) fn beside(state: PathBuf) -> Self {
        Self {
            config: state.with_extension(CONFIG_FILE_NAME),
            credentials: state.with_extension(CREDENTIALS_FILE_NAME),
            state,
        }
    }
}

/// The contents of the hand-editable config file.
///
/// Unknown keys are rejected so a misspelled setting is reported rather than ignored.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
struct Config {
    refresh_interval_secs: u64,
    show_protected_killmails: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            refresh_interval_secs: DEFAULT_REFRESH_INTERVAL_SECS,
            show_protected_killmails: false,
        }
    }
}

/// The contents of the credentials file.
#[derive(Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Credentials {
    refresh_tokens: BTreeMap<u64, String>,
}

/// Loads the store from its config, state, and credentials files.
///
/// Missing files load as defaults; unreadable or malformed files are errors.
pub(crate) fn load(paths: &StorePaths) -> Result<Store, String> {
    let mut store: Store = read_json(&paths.state)?.unwrap_or_default();
    let config: Config = read_toml(&paths.config)?.unwrap_or_default();
    if config.refresh_interval_secs < MIN_REFRESH_INTERVAL_SECS {
        return Err(format!(
            "{}: refresh-interval-secs must be at least {MIN_REFRESH_INTERVAL_SECS}",
            paths.config.display()
        ));
    }
    store.refresh_interval_secs = config.refresh_interval_secs;
    store.show_protected_killmails = config.show_protected_killmails;
    let credentials: Credentials = read_json(&paths.credentials)?.unwrap_or_default();
    for character in &mut store.characters {
        character.refresh_token = credentials.refresh_tokens.get(&character.id).cloned();
    }
    Ok(store)
}

/// Atomically replaces each file whose contents changed.
///
/// Config and credentials are compared by value, so comments and formatting in a
/// hand-edited config file survive until a setting changes.
///
/// Credentials are written first so a state file never names a character whose fallback
/// token was lost. The credentials file is removed once no fallback tokens remain.
pub(crate) fn save(paths: &StorePaths, store: &Store) -> Result<(), String> {
    let credentials = Credentials {
        refresh_tokens: store
            .characters
            .iter()
            .filter_map(|character| Some((character.id, character.refresh_token.clone()?)))
            .collect(),
    };
    if credentials.refresh_tokens.is_empty() {
        match fs::remove_file(&paths.credentials) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => {
                return Err(write_error(&paths.credentials, &error));
            }
            _ => {}
        }
    } else if read_json(&paths.credentials).ok().flatten().as_ref() != Some(&credentials) {
        let data = serde_json::to_vec_pretty(&credentials).map_err(|error| error.to_string())?;
        persist_to_path(&paths.credentials, &data)
            .map_err(|error| write_error(&paths.credentials, &error))?;
    }
    let config = Config {
        refresh_interval_secs: store.refresh_interval_secs,
        show_protected_killmails: store.show_protected_killmails,
    };
    if read_toml(&paths.config).ok().flatten().as_ref() != Some(&config) {
        let data = format!(
            "{CONFIG_HEADER}{}",
            toml::to_string(&config).map_err(|error| error.to_string())?
        );
        persist_to_path(&paths.config, data.as_bytes())
            .map_err(|error| write_error(&paths.config, &error))?;
    }
    let state = serde_json::to_vec_pretty(store).map_err(|error| error.to_string())?;
    persist_to_path(&paths.state, &state).map_err(|error| write_error(&paths.state, &error))
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, String> {
    let Some(data) = read_file(path)? else {
        return Ok(None);
    };
    serde_json::from_slice(&data).map(Some).map_err(|error| {
        format!(
            "{} contains invalid JSON at line {}, column {}",
            path.display(),
            error.line(),
            error.column()
        )
    })
}

fn read_toml<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, String> {
    let Some(data) = read_file(path)? else {
        return Ok(None);
    };
    let text =
        String::from_utf8(data).map_err(|_| format!("{} is not valid UTF-8", path.display()))?;
    toml::from_str(&text)
        .map(Some)
        .map_err(|error| format!("{} is invalid: {error}", path.display()))
}

fn read_file(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match fs::read(path) {
        Ok(data) => Ok(Some(data)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("could not read {}: {error}", path.display())),
    }
}

fn write_error(path: &Path, error: &io::Error) -> String {
    format!("could not atomically write {}: {error}", path.display())
}

#[derive(Debug)]
pub(crate) enum LockError {
    Busy,
    Io(String),
}

/// An advisory lock held until dropped.
pub(crate) struct FileLock {
    _file: fs::File,
}

pub(crate) fn try_operation_lock_for(path: &Path) -> Result<FileLock, LockError> {
    let file = open_lock_file(&sibling_path(path, OPERATION_LOCK_SUFFIX))?;
    file.try_lock().map_err(map_lock_error)?;
    Ok(FileLock { _file: file })
}

pub(crate) fn try_snapshot_lock_for(path: &Path) -> Result<FileLock, LockError> {
    let file = open_lock_file(&sibling_path(path, OPERATION_LOCK_SUFFIX))?;
    file.try_lock_shared().map_err(map_lock_error)?;
    Ok(FileLock { _file: file })
}

pub(crate) fn try_service_lock_for(path: &Path) -> Result<FileLock, LockError> {
    let file = open_lock_file(&sibling_path(path, SERVICE_LOCK_SUFFIX))?;
    file.try_lock().map_err(map_lock_error)?;
    Ok(FileLock { _file: file })
}

fn open_lock_file(path: &Path) -> Result<fs::File, LockError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            LockError::Io(format!("could not create {}: {error}", parent.display()))
        })?;
    }
    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .map_err(|error| LockError::Io(format!("could not open {}: {error}", path.display())))
}

fn map_lock_error(error: fs::TryLockError) -> LockError {
    match error {
        fs::TryLockError::WouldBlock => LockError::Busy,
        fs::TryLockError::Error(error) => LockError::Io(error.to_string()),
    }
}

#[cfg(any(test, feature = "dev-tools"))]
pub(crate) fn state_exists(paths: &StorePaths) -> bool {
    paths.state.exists()
}

pub(crate) fn persist_to_path(path: &Path, data: &[u8]) -> io::Result<()> {
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
    fs::rename(temporary_path, path)
}

fn sibling_path(path: &Path, suffix: &str) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(STATE_FILE_NAME);
    path.with_file_name(format!("{file_name}.{suffix}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Character;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST_DIRECTORY: AtomicU64 = AtomicU64::new(0);

    fn temporary_paths() -> StorePaths {
        let sequence = NEXT_TEST_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "ekmp-storage-test-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&directory).unwrap();
        StorePaths::beside(directory.join(STATE_FILE_NAME))
    }

    fn remove(paths: &StorePaths) {
        fs::remove_dir_all(paths.state.parent().unwrap()).unwrap();
    }

    fn character(id: u64, refresh_token: Option<&str>) -> Character {
        Character {
            id,
            name: format!("Pilot {id}"),
            refresh_token: refresh_token.map(Into::into),
            corporation_id: None,
            corporation_name: None,
        }
    }

    #[test]
    fn sibling_files_share_the_state_file_stem() {
        let paths = StorePaths::beside(PathBuf::from("/tmp/dev.json"));

        assert_eq!(paths.config, PathBuf::from("/tmp/dev.config.toml"));
        assert_eq!(
            paths.credentials,
            PathBuf::from("/tmp/dev.credentials.json")
        );
    }

    #[test]
    fn missing_store_loads_as_default() {
        let paths = temporary_paths();

        let store = load(&paths).unwrap();

        assert!(store.characters.is_empty());
        assert_eq!(store.refresh_interval_secs, DEFAULT_REFRESH_INTERVAL_SECS);
        remove(&paths);
    }

    #[test]
    fn malformed_files_are_reported_instead_of_silently_discarded() {
        for corrupt in [
            |paths: &StorePaths| paths.state.clone(),
            |paths: &StorePaths| paths.config.clone(),
            |paths: &StorePaths| paths.credentials.clone(),
        ] {
            let paths = temporary_paths();
            let path = corrupt(&paths);
            fs::write(&path, b"not = valid = data").unwrap();

            let error = load(&paths).err().unwrap();

            assert!(error.contains(&path.display().to_string()));
            assert!(error.contains("invalid"));
            remove(&paths);
        }
    }

    #[test]
    fn misspelled_config_key_is_rejected() {
        let paths = temporary_paths();
        fs::write(&paths.config, "refresh-interval = 900\n").unwrap();

        let error = load(&paths).err().unwrap();

        assert!(error.contains("refresh-interval"));
        remove(&paths);
    }

    #[test]
    fn config_refresh_interval_below_the_minimum_is_rejected() {
        let paths = temporary_paths();
        fs::write(&paths.config, "refresh-interval-secs = 299\n").unwrap();

        let error = load(&paths).err().unwrap();

        assert!(error.contains("at least 300"));
        fs::write(&paths.config, "refresh-interval-secs = 300\n").unwrap();
        assert_eq!(load(&paths).unwrap().refresh_interval_secs, 300);
        remove(&paths);
    }

    #[test]
    fn hand_edited_config_is_read_and_kept_until_a_setting_changes() {
        let paths = temporary_paths();
        let edited = "# my settings\nrefresh-interval-secs = 3600\n";
        fs::write(&paths.config, edited).unwrap();

        let mut store = load(&paths).unwrap();
        assert_eq!(store.refresh_interval_secs, 3600);
        assert!(!store.show_protected_killmails);
        save(&paths, &store).unwrap();
        assert_eq!(fs::read_to_string(&paths.config).unwrap(), edited);

        store.show_protected_killmails = true;
        save(&paths, &store).unwrap();
        let rewritten = fs::read_to_string(&paths.config).unwrap();
        assert!(rewritten.contains("refresh-interval-secs = 3600"));
        assert!(rewritten.contains("show-protected-killmails = true"));
        remove(&paths);
    }

    #[test]
    fn store_round_trips_across_config_state_and_credentials() {
        let paths = temporary_paths();
        let store = Store {
            characters: vec![character(1, Some("fallback-token")), character(2, None)],
            show_protected_killmails: true,
            refresh_interval_secs: 3600,
            ..Store::default()
        };

        save(&paths, &store).unwrap();
        let restored = load(&paths).unwrap();

        assert!(restored.show_protected_killmails);
        assert_eq!(restored.refresh_interval_secs, 3600);
        assert_eq!(
            restored.characters[0].refresh_token.as_deref(),
            Some("fallback-token")
        );
        assert_eq!(restored.characters[1].refresh_token, None);
        remove(&paths);
    }

    #[test]
    fn state_file_contains_no_preferences_or_tokens() {
        let paths = temporary_paths();
        let store = Store {
            characters: vec![character(1, Some("sentinel-token"))],
            show_protected_killmails: true,
            ..Store::default()
        };

        save(&paths, &store).unwrap();
        let state = fs::read_to_string(&paths.state).unwrap();

        assert!(!state.contains("sentinel-token"));
        assert!(!state.contains("refresh_token"));
        assert!(!state.contains("show_protected_killmails"));
        assert!(!state.contains("refresh_interval_secs"));
        assert!(fs::read_to_string(&paths.credentials)
            .unwrap()
            .contains("sentinel-token"));
        remove(&paths);
    }

    #[test]
    fn credentials_file_is_removed_when_no_fallback_tokens_remain() {
        let paths = temporary_paths();
        let mut store = Store {
            characters: vec![character(1, Some("fallback-token"))],
            ..Store::default()
        };
        save(&paths, &store).unwrap();
        assert!(paths.credentials.exists());

        store.characters[0].refresh_token = None;
        save(&paths, &store).unwrap();

        assert!(!paths.credentials.exists());
        remove(&paths);
    }

    #[test]
    fn unchanged_config_is_not_rewritten() {
        let paths = temporary_paths();
        save(&paths, &Store::default()).unwrap();
        let written = fs::metadata(&paths.config).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));

        save(&paths, &Store::default()).unwrap();

        assert_eq!(
            fs::metadata(&paths.config).unwrap().modified().unwrap(),
            written
        );
        remove(&paths);
    }

    #[test]
    fn persisted_file_replaces_the_destination_and_cleans_up_temporary_file() {
        let paths = temporary_paths();
        fs::write(&paths.state, b"old data").unwrap();

        persist_to_path(&paths.state, b"{}").unwrap();

        assert_eq!(fs::read(&paths.state).unwrap(), b"{}");
        assert!(!sibling_path(&paths.state, "tmp").exists());
        remove(&paths);
    }

    #[test]
    fn operation_lock_reports_contention_and_is_released_on_drop() {
        let paths = temporary_paths();
        let first = try_operation_lock_for(&paths.state).unwrap();

        assert!(matches!(
            try_operation_lock_for(&paths.state),
            Err(LockError::Busy)
        ));

        drop(first);
        assert!(try_operation_lock_for(&paths.state).is_ok());
        remove(&paths);
    }

    #[test]
    fn snapshot_reads_contend_with_an_active_operation() {
        let paths = temporary_paths();
        let operation = try_operation_lock_for(&paths.state).unwrap();

        assert!(matches!(
            try_snapshot_lock_for(&paths.state),
            Err(LockError::Busy)
        ));

        drop(operation);
        assert!(try_snapshot_lock_for(&paths.state).is_ok());
        remove(&paths);
    }

    #[test]
    fn service_lock_is_independent_from_operation_lock() {
        let paths = temporary_paths();
        let operation = try_operation_lock_for(&paths.state).unwrap();
        let service = try_service_lock_for(&paths.state).unwrap();

        assert!(matches!(
            try_service_lock_for(&paths.state),
            Err(LockError::Busy)
        ));

        drop(service);
        assert!(try_service_lock_for(&paths.state).is_ok());
        drop(operation);
        remove(&paths);
    }

    #[cfg(unix)]
    #[test]
    fn persisted_files_are_private_to_the_user() {
        use std::os::unix::fs::PermissionsExt;

        let paths = temporary_paths();
        let store = Store {
            characters: vec![character(1, Some("fallback-token"))],
            ..Store::default()
        };
        save(&paths, &store).unwrap();

        for path in [&paths.config, &paths.state, &paths.credentials] {
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        remove(&paths);
    }
}
