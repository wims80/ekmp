//! Per-user directories following the XDG Base Directory Specification.

use std::{
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
};

const APP_DIR_NAME: &str = "ekmp";

/// `$XDG_CONFIG_HOME/ekmp`, for user preferences.
pub(crate) fn config_dir() -> Result<PathBuf, String> {
    xdg_dir("XDG_CONFIG_HOME", ".config")
}

/// `$XDG_STATE_HOME/ekmp`, for cached killmails, schedules, and fallback credentials.
pub(crate) fn state_dir() -> Result<PathBuf, String> {
    xdg_dir("XDG_STATE_HOME", ".local/state")
}

/// `$XDG_CACHE_HOME/ekmp`, for data that can be deleted and fetched again.
pub(crate) fn cache_dir() -> Result<PathBuf, String> {
    xdg_dir("XDG_CACHE_HOME", ".cache")
}

/// Creates `path` and any missing parents, restricting `path` itself to the current user.
pub(crate) fn create_private_dir(path: &Path) -> io::Result<()> {
    if path.is_dir() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    match builder.create(path) {
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        result => result,
    }
}

fn xdg_dir(variable: &str, home_fallback: &str) -> Result<PathBuf, String> {
    resolve(
        variable,
        std::env::var_os(variable),
        std::env::var_os("HOME"),
        home_fallback,
    )
}

/// The spec requires relative `XDG_*` values to be ignored.
fn resolve(
    variable: &str,
    value: Option<OsString>,
    home: Option<OsString>,
    home_fallback: &str,
) -> Result<PathBuf, String> {
    value
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| home.map(|home| PathBuf::from(home).join(home_fallback)))
        .map(|base| base.join(APP_DIR_NAME))
        .ok_or_else(|| format!("neither {variable} nor HOME is set"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_xdg_value_is_used() {
        let path = resolve(
            "XDG_STATE_HOME",
            Some("/xdg/state".into()),
            Some("/home/tester".into()),
            ".local/state",
        );

        assert_eq!(path.unwrap(), PathBuf::from("/xdg/state/ekmp"));
    }

    #[test]
    fn missing_or_relative_xdg_value_falls_back_to_home() {
        for value in [None, Some("relative/state".into())] {
            let path = resolve(
                "XDG_STATE_HOME",
                value,
                Some("/home/tester".into()),
                ".local/state",
            );

            assert_eq!(
                path.unwrap(),
                PathBuf::from("/home/tester/.local/state/ekmp")
            );
        }
    }

    #[test]
    fn missing_xdg_value_and_home_is_an_error() {
        let error = resolve("XDG_CONFIG_HOME", None, None, ".config").unwrap_err();

        assert!(error.contains("XDG_CONFIG_HOME"));
    }

    #[cfg(unix)]
    #[test]
    fn created_directory_is_private_to_the_user() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!("ekmp-paths-test-{}", std::process::id()));
        let path = root.join("nested").join("ekmp");

        create_private_dir(&path).unwrap();
        create_private_dir(&path).unwrap();

        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o700
        );
        fs::remove_dir_all(root).unwrap();
    }
}
