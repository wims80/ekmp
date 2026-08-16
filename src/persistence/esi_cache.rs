use rusqlite::{params, Connection, OptionalExtension};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const CACHE_DIR_NAME: &str = "ekmp";
const DATABASE_FILE_NAME: &str = "esi-cache.sqlite3";
const MAX_ENTRIES: usize = 5_000;
const MAX_BYTES: usize = 64 * 1024 * 1024;

pub(crate) struct EsiCache {
    connection: Connection,
}

pub(crate) struct CachedResponse {
    pub body: Vec<u8>,
    pub fresh: bool,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

impl EsiCache {
    pub(crate) fn open() -> Result<Self, String> {
        Self::open_at(&cache_path()?)
    }

    #[cfg(test)]
    pub(crate) fn open_at(path: &Path) -> Result<Self, String> {
        Self::open_path(path)
    }

    #[cfg(not(test))]
    fn open_at(path: &Path) -> Result<Self, String> {
        Self::open_path(path)
    }

    fn open_path(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("could not create ESI cache directory: {error}"))?;
            set_private_directory_permissions(parent)?;
        }
        let connection = Connection::open(path)
            .map_err(|error| format!("could not open ESI cache {}: {error}", path.display()))?;
        set_private_file_permissions(path)?;
        connection
            .execute_batch(
                "
                CREATE TABLE IF NOT EXISTS responses (
                    cache_key TEXT PRIMARY KEY,
                    body BLOB NOT NULL,
                    expires_at INTEGER NOT NULL,
                    etag TEXT,
                    last_modified TEXT,
                    accessed_at INTEGER NOT NULL
                );
                CREATE INDEX IF NOT EXISTS responses_accessed_at ON responses(accessed_at);
                ",
            )
            .map_err(|error| format!("could not initialize ESI cache: {error}"))?;
        Ok(Self { connection })
    }

    pub(crate) fn load(&self, key: &str) -> Result<Option<CachedResponse>, String> {
        let now = unix_time();
        let entry = self
            .connection
            .query_row(
                "SELECT body, expires_at, etag, last_modified FROM responses WHERE cache_key = ?1",
                [key],
                |row| {
                    Ok(CachedResponse {
                        body: row.get(0)?,
                        fresh: row.get::<_, u64>(1)? > now,
                        etag: row.get(2)?,
                        last_modified: row.get(3)?,
                    })
                },
            )
            .optional()
            .map_err(|error| format!("could not read ESI cache: {error}"))?;
        if entry.is_some() {
            self.connection
                .execute(
                    "UPDATE responses SET accessed_at = ?2 WHERE cache_key = ?1",
                    params![key, now],
                )
                .map_err(|error| format!("could not update ESI cache: {error}"))?;
        }
        Ok(entry)
    }

    pub(crate) fn store(
        &self,
        key: &str,
        body: &[u8],
        expires: Option<&str>,
        etag: Option<&str>,
        last_modified: Option<&str>,
    ) -> Result<(), String> {
        let Some(expires_at) = expires.and_then(http_date_to_unix_time) else {
            return Ok(());
        };
        if body.len() > MAX_BYTES {
            return Ok(());
        }
        let now = unix_time();
        self.connection
            .execute(
                "
                INSERT INTO responses (cache_key, body, expires_at, etag, last_modified, accessed_at)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                ON CONFLICT(cache_key) DO UPDATE SET
                    body = excluded.body,
                    expires_at = excluded.expires_at,
                    etag = excluded.etag,
                    last_modified = excluded.last_modified,
                    accessed_at = excluded.accessed_at
                ",
                params![key, body, expires_at, etag, last_modified, now],
            )
            .map_err(|error| format!("could not write ESI cache: {error}"))?;
        self.prune()
    }

    pub(crate) fn revalidate(
        &self,
        key: &str,
        expires: Option<&str>,
        etag: Option<&str>,
        last_modified: Option<&str>,
    ) -> Result<(), String> {
        let Some(expires_at) = expires.and_then(http_date_to_unix_time) else {
            return Ok(());
        };
        self.connection
            .execute(
                "
                UPDATE responses SET
                    expires_at = ?2,
                    etag = COALESCE(?3, etag),
                    last_modified = COALESCE(?4, last_modified),
                    accessed_at = ?5
                WHERE cache_key = ?1
                ",
                params![key, expires_at, etag, last_modified, unix_time()],
            )
            .map_err(|error| format!("could not revalidate ESI cache: {error}"))?;
        Ok(())
    }

    fn prune(&self) -> Result<(), String> {
        let count = self
            .connection
            .query_row("SELECT COUNT(*) FROM responses", [], |row| {
                row.get::<_, usize>(0)
            })
            .map_err(|error| format!("could not count ESI cache entries: {error}"))?;
        if count > MAX_ENTRIES {
            self.connection
                .execute(
                    "DELETE FROM responses WHERE cache_key IN (
                        SELECT cache_key FROM responses
                        ORDER BY accessed_at ASC, cache_key ASC
                        LIMIT ?1
                    )",
                    [count - MAX_ENTRIES],
                )
                .map_err(|error| format!("could not prune ESI cache: {error}"))?;
        }

        loop {
            let bytes = self
                .connection
                .query_row(
                    "SELECT COALESCE(SUM(length(body)), 0) FROM responses",
                    [],
                    |row| row.get::<_, usize>(0),
                )
                .map_err(|error| format!("could not measure ESI cache: {error}"))?;
            if bytes <= MAX_BYTES {
                return Ok(());
            }
            let removed = self
                .connection
                .execute(
                    "DELETE FROM responses WHERE cache_key = (
                        SELECT cache_key FROM responses ORDER BY accessed_at ASC, cache_key ASC LIMIT 1
                    )",
                    [],
                )
                .map_err(|error| format!("could not prune ESI cache: {error}"))?;
            if removed == 0 {
                return Ok(());
            }
        }
    }
}

fn cache_path() -> Result<PathBuf, String> {
    cache_dir().map(|path| path.join(DATABASE_FILE_NAME))
}

fn cache_dir() -> Result<PathBuf, String> {
    #[cfg(windows)]
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .ok_or("LOCALAPPDATA is not set")?;

    #[cfg(target_os = "macos")]
    let base = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or("HOME is not set")?
        .join("Library")
        .join("Caches");

    #[cfg(all(not(windows), not(target_os = "macos")))]
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .ok_or("neither XDG_CACHE_HOME nor HOME is set")?;

    Ok(base.join(CACHE_DIR_NAME))
}

fn http_date_to_unix_time(value: &str) -> Option<u64> {
    httpdate::parse_http_date(value)
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
}

fn unix_time() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(unix)]
fn set_private_directory_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("could not secure ESI cache directory: {error}"))
}

#[cfg(not(unix))]
fn set_private_directory_permissions(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(unix)]
fn set_private_file_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("could not secure ESI cache: {error}"))
}

#[cfg(not(unix))]
fn set_private_file_permissions(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST_DATABASE: AtomicU64 = AtomicU64::new(0);

    fn temporary_database_path() -> PathBuf {
        let sequence = NEXT_TEST_DATABASE.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir()
            .join(format!(
                "ekmp-esi-cache-test-{}-{sequence}",
                std::process::id()
            ))
            .join(DATABASE_FILE_NAME)
    }

    #[test]
    fn stores_and_loads_a_fresh_response() {
        let path = temporary_database_path();
        let cache = EsiCache::open_at(&path).unwrap();
        cache
            .store(
                "https://esi.example/prices/",
                b"[1]",
                Some("Thu, 31 Dec 2099 23:59:59 GMT"),
                Some("etag"),
                Some("Thu, 31 Dec 2099 22:59:59 GMT"),
            )
            .unwrap();

        let entry = cache.load("https://esi.example/prices/").unwrap().unwrap();

        assert!(entry.fresh);
        assert_eq!(entry.body, b"[1]");
        assert_eq!(entry.etag.as_deref(), Some("etag"));
        assert_eq!(
            entry.last_modified.as_deref(),
            Some("Thu, 31 Dec 2099 22:59:59 GMT")
        );
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn ignores_responses_without_an_expiry() {
        let path = temporary_database_path();
        let cache = EsiCache::open_at(&path).unwrap();
        cache
            .store("https://esi.example/no-expiry/", b"[1]", None, None, None)
            .unwrap();

        assert!(cache
            .load("https://esi.example/no-expiry/")
            .unwrap()
            .is_none());
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn database_is_private_to_the_user() {
        use std::os::unix::fs::PermissionsExt;

        let path = temporary_database_path();
        let _cache = EsiCache::open_at(&path).unwrap();

        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
