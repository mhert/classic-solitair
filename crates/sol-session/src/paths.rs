//! Platform-specific save/data directory resolution: the ONLY module in
//! this crate that imports `directories`. Pure
//! path resolution — nothing here touches the filesystem; see
//! [`crate::storage`] for the I/O that actually reads and writes at these
//! paths.

use std::path::PathBuf;

use directories::ProjectDirs;

use crate::storage::StorageError;

/// The autosave file's name within [`default_data_dir`].
const AUTOSAVE_FILE_NAME: &str = "autosave.json";

/// The settings file's name within [`default_data_dir`].
const SETTINGS_FILE_NAME: &str = "settings.json";

/// Resolves this platform's data directory for classic-solitair —
/// `ProjectDirs::from("", "", "classic-solitair")` (no qualifier or
/// organization: a single, non-corporate application), data dir. Pure
/// resolution: creates nothing on disk.
///
/// # Errors
///
/// Returns [`StorageError::NoHomeDirectory`] when no home directory can be
/// determined for the current platform/user (see
/// `directories::ProjectDirs::from`).
///
/// ```no_run
/// let dir = sol_session::paths::default_data_dir()?;
/// println!("{}", dir.display());
/// # Ok::<(), sol_session::StorageError>(())
/// ```
pub fn default_data_dir() -> Result<PathBuf, StorageError> {
    ProjectDirs::from("", "", "classic-solitair")
        .map(|dirs| dirs.data_dir().to_path_buf())
        .ok_or(StorageError::NoHomeDirectory)
}

/// The autosave file's path: [`default_data_dir`] joined with
/// `autosave.json`.
///
/// # Errors
///
/// Returns [`StorageError::NoHomeDirectory`] under the same conditions as
/// [`default_data_dir`].
///
/// ```no_run
/// let path = sol_session::paths::autosave_path()?;
/// assert!(path.ends_with("autosave.json"));
/// # Ok::<(), sol_session::StorageError>(())
/// ```
pub fn autosave_path() -> Result<PathBuf, StorageError> {
    Ok(default_data_dir()?.join(AUTOSAVE_FILE_NAME))
}

/// The settings file's path: [`default_data_dir`] joined with
/// `settings.json`.
///
/// # Errors
///
/// Returns [`StorageError::NoHomeDirectory`] under the same conditions as
/// [`default_data_dir`].
///
/// ```no_run
/// let path = sol_session::paths::settings_path()?;
/// assert!(path.ends_with("settings.json"));
/// # Ok::<(), sol_session::StorageError>(())
/// ```
pub fn settings_path() -> Result<PathBuf, StorageError> {
    Ok(default_data_dir()?.join(SETTINGS_FILE_NAME))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn default_data_dir_succeeds_on_this_machine_and_is_absolute() {
        let dir = default_data_dir().unwrap();
        assert!(dir.is_absolute());
    }

    #[test]
    fn autosave_path_is_the_data_dir_joined_with_autosave_json() {
        let data_dir = default_data_dir().unwrap();

        let autosave = autosave_path().unwrap();

        assert_eq!(autosave, data_dir.join("autosave.json"));
        assert!(autosave.ends_with("autosave.json"));
    }

    #[test]
    fn settings_path_is_the_data_dir_joined_with_settings_json() {
        let data_dir = default_data_dir().unwrap();

        let settings = settings_path().unwrap();

        assert_eq!(settings, data_dir.join("settings.json"));
        assert!(settings.ends_with("settings.json"));
    }
}
