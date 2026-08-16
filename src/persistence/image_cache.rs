use crate::integrations::images::IdentityImageKey;
use std::{
    fs, io,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

const CACHE_DIR_NAME: &str = "ekmp";
const IMAGE_DIR_NAME: &str = "images";
const CACHE_FRESHNESS: Duration = Duration::from_secs(7 * 24 * 60 * 60);

pub(crate) struct CachedImage {
    pub bytes: Vec<u8>,
    pub fresh: bool,
}

pub(crate) fn load(key: IdentityImageKey) -> Result<Option<CachedImage>, String> {
    let path = image_path(key)?;
    match fs::read(&path) {
        Ok(bytes) => {
            let fresh = fs::metadata(&path)
                .and_then(|metadata| metadata.modified())
                .ok()
                .and_then(|modified| SystemTime::now().duration_since(modified).ok())
                .is_some_and(|age| age < CACHE_FRESHNESS);
            Ok(Some(CachedImage { bytes, fresh }))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("could not read {}: {error}", path.display())),
    }
}

pub(crate) fn store(key: IdentityImageKey, bytes: &[u8]) -> Result<(), String> {
    let path = image_path(key)?;
    fs::write(&path, bytes).map_err(|error| format!("could not write {}: {error}", path.display()))
}

fn image_path(key: IdentityImageKey) -> Result<PathBuf, String> {
    cache_dir().map(|path| path.join(key.cache_file_name()))
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

    let path = cache_dir_path(&base);
    fs::create_dir_all(&path).map_err(|error| error.to_string())?;
    Ok(path)
}

fn cache_dir_path(base: &Path) -> PathBuf {
    base.join(CACHE_DIR_NAME).join(IMAGE_DIR_NAME)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_cache_has_its_own_subdirectory() {
        assert_eq!(
            cache_dir_path(Path::new("/cache")),
            PathBuf::from("/cache/ekmp/images")
        );
    }
}
