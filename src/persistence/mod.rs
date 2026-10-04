pub(crate) mod esi_cache;
#[cfg(feature = "gui")]
pub(crate) mod image_cache;
pub(crate) mod paths;
pub(crate) mod secrets;
pub(crate) mod storage;

pub(crate) use paths::cache_dir;

use std::{io, path::Path};

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
