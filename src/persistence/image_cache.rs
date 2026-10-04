use super::cache_dir;
use crate::integrations::images::IdentityImageKey;
use std::{
    fs, io,
    path::PathBuf,
    time::{Duration, SystemTime},
};

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
    let directory = cache_dir()?.join(IMAGE_DIR_NAME);
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    Ok(directory.join(key.cache_file_name()))
}
