//! Filesystem I/O for session saves and settings: the
//! platform-touching complement to the platform-free `save.rs` and
//! `settings.rs` formats. `std::fs` against caller-supplied paths, plus
//! thin default-path compositions built on [`crate::paths`].
//!
//! Frontend contract: call [`autosave`] on exit and [`load_autosave`] at
//! launch ("autosave on exit"); explicit File → Save / Load use
//! [`save_to`] / [`load_from`] with a user-chosen path. Settings mirror
//! this shape: [`store_settings`] / [`load_settings`] are the default-path
//! pair frontends restore at launch and rewrite on every settings commit;
//! [`save_settings_to`] / [`load_settings_from`] take a caller-chosen path.

use std::fs;
use std::path::{Path, PathBuf};

use crate::paths::{autosave_path, settings_path};
use crate::save::SaveError;
use crate::session::Session;
use crate::settings::{Settings, SettingsError};

/// Errors from this module's filesystem operations.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum StorageError {
    /// A filesystem operation failed (missing file, permissions, and so
    /// on).
    #[error("filesystem error: {0}")]
    Io(#[from] std::io::Error),
    /// The bytes read from disk were not a readable save document.
    #[error("invalid save data: {0}")]
    Save(#[from] SaveError),
    /// The bytes read from disk were not a readable settings document.
    #[error("invalid settings data: {0}")]
    Settings(#[from] SettingsError),
    /// No home or config directory could be determined for this platform,
    /// so [`crate::paths::default_data_dir`] has nowhere to point.
    #[error("could not determine a home or config directory for this platform")]
    NoHomeDirectory,
}

/// Writes `session`'s save bytes to `path`, atomically: creates `path`'s
/// parent directories as needed, serializes and writes to a sibling temp
/// file (`path` with `.tmp` appended, e.g. `save.json.tmp`), then renames
/// it over `path`. A reader can therefore never observe a partially
/// written save, and a successful call leaves no temp file behind.
///
/// The temp file's name is fixed rather than randomized: classic-solitair
/// is a single-instance application, so two writes never race for the same
/// `path`.
///
/// # Errors
///
/// Returns [`StorageError::Save`] if `session` fails to serialize (see
/// [`Session::to_save_bytes`]). Returns [`StorageError::Io`] if creating
/// the parent directory, writing the temp file, or renaming it over `path`
/// fails.
///
/// ```
/// use sol_engine::Seed;
/// use sol_session::{Options, Session, storage};
///
/// let dir = tempfile::tempdir()?;
/// let path = dir.path().join("save.json");
/// let session = Session::new(Options::default(), Seed::new(1).unwrap());
///
/// storage::save_to(&session, &path)?;
/// let loaded = storage::load_from(&path)?;
/// assert_eq!(loaded.to_save_bytes()?, session.to_save_bytes()?);
/// # Ok::<(), sol_session::StorageError>(())
/// ```
pub fn save_to(session: &Session, path: &Path) -> Result<(), StorageError> {
    let bytes = session.to_save_bytes()?;
    write_atomically(path, &bytes)?;
    Ok(())
}

/// Writes `settings` to `path`, atomically — the same mechanism [`save_to`]
/// uses (see its docs for the guarantee this gives a reader): creates
/// `path`'s parent directories as needed, writes to a sibling temp file
/// (`path` with `.tmp` appended), then renames it over `path`.
///
/// # Errors
///
/// Returns [`StorageError::Settings`] if `settings` fails to serialize
/// (see [`Settings::to_bytes`]). Returns [`StorageError::Io`] if creating
/// the parent directory, writing the temp file, or renaming it over `path`
/// fails.
///
/// ```
/// use sol_session::{Settings, storage};
///
/// let dir = tempfile::tempdir()?;
/// let path = dir.path().join("settings.json");
/// let settings = Settings::default();
///
/// storage::save_settings_to(&settings, &path)?;
/// let loaded = storage::load_settings_from(&path)?;
/// assert_eq!(loaded, settings);
/// # Ok::<(), sol_session::StorageError>(())
/// ```
pub fn save_settings_to(settings: &Settings, path: &Path) -> Result<(), StorageError> {
    let bytes = settings.to_bytes()?;
    write_atomically(path, &bytes)?;
    Ok(())
}

/// Reads `path` and parses it as a session save.
///
/// # Errors
///
/// Returns [`StorageError::Io`] if `path` cannot be read — for example, it
/// does not exist. Returns [`StorageError::Save`] if the bytes read are not
/// a valid save document (see [`Session::from_save_bytes`]).
pub fn load_from(path: &Path) -> Result<Session, StorageError> {
    let bytes = fs::read(path)?;
    Ok(Session::from_save_bytes(&bytes)?)
}

/// Reads `path` and parses it as a settings document.
///
/// # Errors
///
/// Returns [`StorageError::Io`] if `path` cannot be read — for example, it
/// does not exist. Returns [`StorageError::Settings`] if the bytes read
/// are not a valid settings document (see [`Settings::from_bytes`]).
pub fn load_settings_from(path: &Path) -> Result<Settings, StorageError> {
    let bytes = fs::read(path)?;
    Ok(Settings::from_bytes(&bytes)?)
}

/// Writes `session` to the platform's default autosave location
/// ([`crate::paths::autosave_path`]) — the frontend's exit-time contract
/// ("autosave on exit"). Returns the path written to.
///
/// # Errors
///
/// Returns [`StorageError::NoHomeDirectory`] if the autosave path cannot be
/// resolved. Otherwise, the same errors as [`save_to`].
///
/// ```no_run
/// use sol_engine::Seed;
/// use sol_session::{Options, Session, storage};
///
/// let session = Session::new(Options::default(), Seed::new(1).unwrap());
/// let path = storage::autosave(&session)?;
/// println!("wrote {}", path.display());
/// # Ok::<(), sol_session::StorageError>(())
/// ```
pub fn autosave(session: &Session) -> Result<PathBuf, StorageError> {
    let path = autosave_path()?;
    save_to(session, &path)?;
    Ok(path)
}

/// Writes `settings` to the platform's default settings location
/// ([`crate::paths::settings_path`]) — the frontend's settings-commit
/// contract. Returns the path written to.
///
/// # Errors
///
/// Returns [`StorageError::NoHomeDirectory`] if the settings path cannot be
/// resolved. Otherwise, the same errors as [`save_settings_to`].
///
/// ```no_run
/// use sol_session::{Settings, storage};
///
/// let settings = Settings::default();
/// let path = storage::store_settings(&settings)?;
/// println!("wrote {}", path.display());
/// # Ok::<(), sol_session::StorageError>(())
/// ```
pub fn store_settings(settings: &Settings) -> Result<PathBuf, StorageError> {
    let path = settings_path()?;
    save_settings_to(settings, &path)?;
    Ok(path)
}

/// Loads the session previously written by [`autosave`] — the frontend's
/// launch-time contract ("autosave on exit").
///
/// # Errors
///
/// Returns `Ok(None)` when no autosave file exists yet: a fresh machine or
/// a first launch is not an error. Returns [`StorageError::NoHomeDirectory`]
/// if the autosave path cannot be resolved. Returns [`StorageError::Save`]
/// if an autosave file exists but is not a valid save document.
///
/// ```no_run
/// use sol_session::storage;
///
/// match storage::load_autosave()? {
///     Some(session) => println!("resuming, score {}", session.game().state().score()),
///     None => println!("starting fresh"),
/// }
/// # Ok::<(), sol_session::StorageError>(())
/// ```
pub fn load_autosave() -> Result<Option<Session>, StorageError> {
    let path = autosave_path()?;
    missing_as_none(load_from(&path))
}

/// Loads the settings document previously written by [`store_settings`] —
/// the frontend's startup-restore contract.
///
/// # Errors
///
/// Returns `Ok(None)` when no settings file exists yet: a fresh machine or
/// a first launch is not an error. Returns [`StorageError::NoHomeDirectory`]
/// if the settings path cannot be resolved. Returns
/// [`StorageError::Settings`] if a settings file exists but is not a valid
/// settings document.
///
/// ```no_run
/// use sol_session::storage;
///
/// match storage::load_settings()? {
///     Some(settings) => println!("restoring back index {}", settings.back_index),
///     None => println!("using defaults"),
/// }
/// # Ok::<(), sol_session::StorageError>(())
/// ```
pub fn load_settings() -> Result<Option<Settings>, StorageError> {
    let path = settings_path()?;
    missing_as_none(load_settings_from(&path))
}

/// The sibling temp path [`save_to`] writes to before renaming over `path`:
/// `path`'s full file name with `.tmp` appended, in the same directory —
/// same-directory placement is what makes the following rename atomic
/// (renames across filesystems are not).
fn temp_sibling(path: &Path) -> PathBuf {
    let mut temp = path.as_os_str().to_owned();
    temp.push(".tmp");
    PathBuf::from(temp)
}

/// The parent directory [`save_to`] should ensure exists before writing to
/// `path`, or `None` when there is nothing meaningful to create: `path` has
/// no parent component at all (for example a filesystem root), or its
/// parent is the empty path — `Path::parent` returns `Some("")` for a bare
/// relative file name like `"save.json"`, and `fs::create_dir_all` would
/// fail on `""`.
fn parent_to_create(path: &Path) -> Option<&Path> {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
}

/// Creates `path`'s parent directory ahead of a write, or does nothing when
/// [`parent_to_create`] finds nothing worth creating.
fn create_parent_dir(path: &Path) -> std::io::Result<()> {
    match parent_to_create(path) {
        Some(parent) => fs::create_dir_all(parent),
        None => Ok(()),
    }
}

/// Writes `bytes` to `path` atomically: [`create_parent_dir`], then write
/// to [`temp_sibling`] and rename it over `path`. The mechanism shared by
/// [`save_to`] and [`save_settings_to`] — see [`save_to`]'s docs for the
/// atomicity contract this gives a reader.
fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    create_parent_dir(path)?;
    let temp_path = temp_sibling(path);
    fs::write(&temp_path, bytes)?;
    fs::rename(&temp_path, path)
}

/// Turns a [`load_from`]- or [`load_settings_from`]-shaped result into
/// what [`load_autosave`] and [`load_settings`] promise: a missing file is
/// not an error — a fresh machine or a first launch has none yet — so a
/// `NotFound` I/O error becomes `Ok(None)`. Every other outcome (a
/// successfully loaded value, or any other I/O or parse failure) passes
/// through unchanged, just wrapped in `Option`.
fn missing_as_none<T>(result: Result<T, StorageError>) -> Result<Option<T>, StorageError> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(StorageError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::options::Options;
    use crate::session::Session;
    use sol_engine::Seed;

    fn sample_session() -> Session {
        Session::new(Options::default(), Seed::new(1).unwrap())
    }

    /// A non-default settings document: `back_index: 2` distinguishes it
    /// from `Settings::default()` so round-trip tests actually prove the
    /// bytes on disk were read back, not just that `Settings::default()`
    /// was returned regardless.
    fn sample_settings() -> Settings {
        Settings {
            back_index: 2,
            ..Settings::default()
        }
    }

    #[test]
    fn save_to_then_load_from_round_trips_byte_identically() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("save.json");
        let session = sample_session();

        save_to(&session, &path).unwrap();
        let loaded = load_from(&path).unwrap();

        assert_eq!(
            loaded.to_save_bytes().unwrap(),
            session.to_save_bytes().unwrap()
        );
    }

    #[test]
    fn save_to_creates_missing_parent_directories() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("dir").join("save.json");
        let session = sample_session();

        save_to(&session, &path).unwrap();

        assert!(path.is_file());
    }

    #[test]
    fn save_to_leaves_a_valid_target_and_no_tmp_sibling() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("save.json");
        let session = sample_session();

        save_to(&session, &path).unwrap();

        assert!(path.is_file());
        load_from(&path).unwrap(); // valid content
        let temp_path = dir.path().join("save.json.tmp");
        assert!(!temp_path.exists(), "no leftover temp file");
    }

    #[test]
    fn save_to_overwrites_an_existing_save_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("save.json");
        let first = sample_session();
        save_to(&first, &path).unwrap();

        let second = Session::new(Options::default(), Seed::new(2).unwrap());
        save_to(&second, &path).unwrap();

        let loaded = load_from(&path).unwrap();
        assert_eq!(
            loaded.to_save_bytes().unwrap(),
            second.to_save_bytes().unwrap()
        );
    }

    #[test]
    fn load_from_a_missing_path_is_a_not_found_io_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing.json");

        let error = load_from(&path).unwrap_err();

        assert!(matches!(
            &error,
            StorageError::Io(io_error) if io_error.kind() == std::io::ErrorKind::NotFound
        ));
    }

    #[test]
    fn load_from_malformed_bytes_is_a_save_malformed_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.json");
        std::fs::write(&path, b"not json at all").unwrap();

        let error = load_from(&path).unwrap_err();

        assert!(matches!(error, StorageError::Save(SaveError::Malformed(_))));
    }

    #[test]
    fn load_from_an_unsupported_format_version_is_a_save_unsupported_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("future.json");
        let mut value = serde_json::to_value(sample_session().to_save()).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .insert("format_version".to_owned(), serde_json::json!(99));
        std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();

        let error = load_from(&path).unwrap_err();

        assert!(matches!(
            error,
            StorageError::Save(SaveError::UnsupportedFormatVersion { found: 99 })
        ));
    }

    #[test]
    fn save_settings_to_then_load_settings_from_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let settings = sample_settings();

        save_settings_to(&settings, &path).unwrap();
        let loaded = load_settings_from(&path).unwrap();

        assert_eq!(loaded, settings);
    }

    #[test]
    fn save_settings_to_creates_missing_parent_directories() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("dir").join("settings.json");
        let settings = sample_settings();

        save_settings_to(&settings, &path).unwrap();

        assert!(path.is_file());
    }

    #[test]
    fn save_settings_to_leaves_a_valid_target_and_no_tmp_sibling() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let settings = sample_settings();

        save_settings_to(&settings, &path).unwrap();

        assert!(path.is_file());
        load_settings_from(&path).unwrap(); // valid content
        let temp_path = dir.path().join("settings.json.tmp");
        assert!(!temp_path.exists(), "no leftover temp file");
    }

    #[test]
    fn save_settings_to_overwrites_an_existing_settings_file_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let first = sample_settings();
        save_settings_to(&first, &path).unwrap();

        let second = Settings {
            back_index: 3,
            ..Settings::default()
        };
        save_settings_to(&second, &path).unwrap();

        let loaded = load_settings_from(&path).unwrap();
        assert_eq!(loaded, second);
    }

    #[test]
    fn load_settings_from_a_missing_path_is_a_not_found_io_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing.json");

        let error = load_settings_from(&path).unwrap_err();

        assert!(matches!(
            &error,
            StorageError::Io(io_error) if io_error.kind() == std::io::ErrorKind::NotFound
        ));
    }

    #[test]
    fn load_settings_from_malformed_bytes_is_a_settings_malformed_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.json");
        std::fs::write(&path, b"not json at all").unwrap();

        let error = load_settings_from(&path).unwrap_err();

        assert!(matches!(
            error,
            StorageError::Settings(SettingsError::Malformed(_))
        ));
    }

    #[test]
    fn load_settings_from_an_unsupported_format_version_is_a_settings_unsupported_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("future.json");
        let mut value = serde_json::to_value(sample_settings()).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .insert("format_version".to_owned(), serde_json::json!(2));
        std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();

        let error = load_settings_from(&path).unwrap_err();

        assert!(matches!(
            error,
            StorageError::Settings(SettingsError::UnsupportedFormatVersion { found: 2 })
        ));
    }

    #[test]
    fn settings_variant_display_matches_exact_string() {
        let inner = Settings::from_bytes(b"not json at all").unwrap_err();
        let inner_display = inner.to_string();

        let error = StorageError::Settings(inner);

        assert_eq!(
            error.to_string(),
            format!("invalid settings data: {inner_display}")
        );
    }

    /// RAII guard that snapshots whatever bytes (if any) already live at
    /// `path` on construction, then restores them — or removes the file, if
    /// nothing was there to begin with — on drop, recreating `path`'s
    /// parent directory first if it went missing in between. `Drop::drop`
    /// still runs during unwinding, so even a panicking assertion inside
    /// the guarded window cannot leave the snapshotted file clobbered or a
    /// stray file behind. The tests that touch genuine platform paths
    /// (autosave, settings) use this to leave them exactly as found; a
    /// hermetic test below exercises the same restore logic against a
    /// `tempfile` path instead. Every other storage test goes through
    /// `save_to`/`load_from` or `save_settings_to`/`load_settings_from`
    /// with a `tempfile` path directly — workspace lints forbid
    /// `unsafe_code`, so `std::env::set_var` redirection is not an option.
    struct AutosaveGuard {
        path: PathBuf,
        original: Option<Vec<u8>>,
    }

    impl AutosaveGuard {
        /// Snapshots whatever bytes (if any) already live at `path`.
        fn snapshot(path: PathBuf) -> Self {
            let original = fs::read(&path).ok();
            Self { path, original }
        }
    }

    impl Drop for AutosaveGuard {
        fn drop(&mut self) {
            match &self.original {
                Some(bytes) => {
                    let _ = create_parent_dir(&self.path);
                    let _ = fs::write(&self.path, bytes);
                }
                None => {
                    let _ = fs::remove_file(&self.path);
                }
            }
        }
    }

    #[test]
    fn real_platform_autosave_round_trips_and_reports_none_when_absent() {
        let path = autosave_path().unwrap();
        let _guard = AutosaveGuard::snapshot(path.clone());
        let _ = fs::remove_file(&path);

        assert!(
            load_autosave().unwrap().is_none(),
            "no autosave file is not an error"
        );

        let session = sample_session();
        let written_path = autosave(&session).unwrap();
        assert_eq!(written_path, path);

        let loaded = load_autosave().unwrap().unwrap();
        assert_eq!(
            loaded.to_save_bytes().unwrap(),
            session.to_save_bytes().unwrap()
        );
    }

    #[test]
    fn real_platform_settings_round_trips_and_reports_none_when_absent() {
        let path = settings_path().unwrap();
        let _guard = AutosaveGuard::snapshot(path.clone());
        let _ = fs::remove_file(&path);

        assert!(
            load_settings().unwrap().is_none(),
            "no settings file is not an error"
        );

        let settings = sample_settings();
        let written_path = store_settings(&settings).unwrap();
        assert_eq!(written_path, path);

        let loaded = load_settings().unwrap().unwrap();
        assert_eq!(loaded, settings);
    }

    #[test]
    fn autosave_guard_restores_original_bytes_and_recreates_a_missing_parent_dir() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("autosave.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"original bytes").unwrap();

        let guard = AutosaveGuard::snapshot(path.clone());
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
        drop(guard);

        assert_eq!(
            fs::read(&path).unwrap(),
            b"original bytes",
            "drop recreates the parent directory and restores the snapshotted bytes"
        );
    }

    #[test]
    fn autosave_guard_removes_the_file_it_wrote_when_nothing_was_there_before() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("autosave.json");

        let guard = AutosaveGuard::snapshot(path.clone());
        fs::write(&path, b"written during the guarded window").unwrap();
        drop(guard);

        assert!(
            !path.exists(),
            "drop removes a file that did not exist at snapshot time"
        );
    }

    #[test]
    fn parent_to_create_is_none_for_a_bare_relative_file_name() {
        assert_eq!(parent_to_create(Path::new("save.json")), None);
    }

    #[test]
    fn parent_to_create_is_the_parent_for_a_path_with_a_real_directory_component() {
        assert_eq!(
            parent_to_create(Path::new("dir/save.json")),
            Some(Path::new("dir"))
        );
    }

    #[test]
    fn parent_to_create_is_none_for_a_filesystem_root() {
        assert_eq!(parent_to_create(Path::new("/")), None);
    }

    #[test]
    fn create_parent_dir_is_a_no_op_when_there_is_no_parent_to_create() {
        assert!(create_parent_dir(Path::new("save.json")).is_ok());
    }

    #[test]
    fn create_parent_dir_creates_a_missing_directory() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("nested").join("save.json");

        create_parent_dir(&target).unwrap();

        assert!(target.parent().unwrap().is_dir());
    }

    #[test]
    fn missing_as_none_turns_a_successful_load_into_some() {
        let session = sample_session();

        let result = missing_as_none(Ok(session.clone()));

        assert_eq!(result.unwrap(), Some(session));
    }

    #[test]
    fn missing_as_none_turns_a_not_found_io_error_into_ok_none() {
        let error = StorageError::Io(std::io::Error::from(std::io::ErrorKind::NotFound));

        let result = missing_as_none::<Session>(Err(error));

        assert_eq!(result.unwrap(), None);
    }

    #[test]
    fn missing_as_none_forwards_every_other_error_unchanged() {
        let error = StorageError::Io(std::io::Error::from(std::io::ErrorKind::PermissionDenied));

        let result = missing_as_none::<Session>(Err(error));

        assert!(matches!(
            result,
            Err(StorageError::Io(io_error))
                if io_error.kind() == std::io::ErrorKind::PermissionDenied
        ));
    }
}
