pub(crate) mod esi_cache;
#[cfg(feature = "gui")]
pub(crate) mod image_cache;
pub(crate) mod secrets;
pub(crate) mod storage;

use std::{
    io,
    path::{Path, PathBuf},
};

/// The per-user cache directory for ekmp, which callers create as needed.
pub(crate) fn cache_dir() -> Result<PathBuf, String> {
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

    Ok(base.join("ekmp"))
}

/// Restricts a file or directory to the current user on Unix; elsewhere this does nothing.
pub(crate) fn restrict_permissions(path: &Path, mode: u32) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
        Ok(())
    }
}
