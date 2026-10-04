use super::{cache_dir, restrict_permissions};
use crate::clock::{http_date, unix_time};
use rusqlite::{params, Connection, OptionalExtension};
use std::{fs, path::Path};

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
        Self::open_at(&cache_dir()?.join(DATABASE_FILE_NAME))
    }

    pub(crate) fn open_at(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("could not create ESI cache directory: {error}"))?;
            restrict_permissions(parent, 0o700)
                .map_err(|error| format!("could not secure ESI cache directory: {error}"))?;
        }
        let connection = Connection::open(path)
            .map_err(|error| format!("could not open ESI cache {}: {error}", path.display()))?;
        restrict_permissions(path, 0o600)
            .map_err(|error| format!("could not secure ESI cache: {error}"))?;
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
        let Some(expires_at) = expires.and_then(http_date) else {
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
        self.prune(MAX_ENTRIES, MAX_BYTES)
    }

    pub(crate) fn revalidate(
        &self,
        key: &str,
        expires: Option<&str>,
        etag: Option<&str>,
        last_modified: Option<&str>,
    ) -> Result<(), String> {
        let Some(expires_at) = expires.and_then(http_date) else {
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

    /// Evicts the least recently used entries beyond the entry and byte limits.
    fn prune(&self, max_entries: usize, max_bytes: usize) -> Result<(), String> {
        self.connection
            .execute(
                "
                DELETE FROM responses WHERE cache_key IN (
                    SELECT cache_key FROM (
                        SELECT
                            cache_key,
                            ROW_NUMBER() OVER newest AS position,
                            SUM(length(body)) OVER newest AS retained_bytes
                        FROM responses
                        WINDOW newest AS (ORDER BY accessed_at DESC, cache_key DESC)
                    )
                    WHERE position > ?1 OR retained_bytes > ?2
                )
                ",
                params![max_entries, max_bytes],
            )
            .map_err(|error| format!("could not prune ESI cache: {error}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

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
        drop(cache);
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
        drop(cache);
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn prune_keeps_the_most_recently_used_entries_within_limits() {
        let path = temporary_database_path();
        let cache = EsiCache::open_at(&path).unwrap();
        for (key, accessed_at) in [("a", 1), ("b", 2), ("c", 3), ("d", 4)] {
            cache
                .connection
                .execute(
                    "INSERT INTO responses (cache_key, body, expires_at, accessed_at)
                     VALUES (?1, ?2, 0, ?3)",
                    params![key, b"1234".as_slice(), accessed_at],
                )
                .unwrap();
        }
        let keys = |cache: &EsiCache| {
            let mut statement = cache
                .connection
                .prepare("SELECT cache_key FROM responses ORDER BY cache_key")
                .unwrap();
            statement
                .query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };

        cache.prune(3, 1_000).unwrap();
        assert_eq!(keys(&cache), ["b", "c", "d"]);
        cache.prune(3, 8).unwrap();
        assert_eq!(keys(&cache), ["c", "d"]);
        drop(cache);
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
        drop(_cache);
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
