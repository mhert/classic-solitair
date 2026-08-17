//! The frontends' shared theme discovery and loading.
//!
//! A theme id (the session's [`sol_session::ThemeId`] text) resolves to
//! a package on disk. `"default"` is the in-tree vector theme during
//! development and `<data>/themes/default` when installed; every other
//! id names a directory or `.zip` under `<data>/themes/` — which is
//! where `soltool extract` output naturally lands for local use.

use std::path::{Path, PathBuf};

use sol_theme::{Theme, ThemeError};

/// One selectable theme: its session id and its package location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeEntry {
    /// The session-visible theme id.
    pub id: String,
    /// Where the package lives: a directory, or a `.zip` file.
    pub path: PathBuf,
}

/// Errors from resolving or loading a theme by id.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ThemeLookupError {
    /// No discovered theme carries this id.
    #[error("no theme named \"{id}\" was found")]
    UnknownTheme {
        /// The id that matched nothing.
        id: String,
    },
    /// The package exists but fails to parse or validate.
    #[error("loading theme \"{id}\"")]
    Load {
        /// The id whose package failed to load.
        id: String,
        // Boxed: ThemeError is large, and this error travels through
        // startup Results where a slim Err variant matters to clippy.
        /// The underlying loader failure.
        #[source]
        source: Box<ThemeError>,
    },
}

/// The user theme directory, `<data>/themes`, next to the autosave.
#[must_use]
pub fn user_theme_dir() -> Option<PathBuf> {
    sol_session::paths::default_data_dir()
        .ok()
        .map(|dir| dir.join("themes"))
}

/// The in-tree default theme during development, resolved relative to
/// the cargo workspace.
#[must_use]
pub fn dev_default_dir() -> Option<PathBuf> {
    dev_default_dir_among(&default_dev_candidates())
}

/// The candidate locations [`dev_default_dir`] searches, in order.
///
/// Exposed so a frontend that ships inside a bundle can search its own
/// locations *in addition* to these rather than forking the whole function:
/// a macOS `.app` resolves `../Resources/themes/default` relative to its
/// executable, which no workspace-relative path can express.
#[must_use]
pub fn default_dev_candidates() -> Vec<PathBuf> {
    vec![
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../themes/default"),
        PathBuf::from("themes/default"),
    ]
}

/// The first of `candidates` that is a directory.
#[must_use]
pub fn dev_default_dir_among(candidates: &[PathBuf]) -> Option<PathBuf> {
    candidates.iter().find(|path| path.is_dir()).cloned()
}

/// Lists the selectable themes: `"default"` first, then every directory
/// or `.zip` under `<data>/themes` in name order. Unreadable entries are
/// skipped — discovery populates a picker and must not fail it.
#[must_use]
pub fn discover() -> Vec<ThemeEntry> {
    discover_among(&default_dev_candidates())
}

/// [`discover`] with an explicit in-tree/bundled default search path, for a
/// frontend whose default theme ships somewhere the workspace layout cannot
/// name.
#[must_use]
pub fn discover_among(default_candidates: &[PathBuf]) -> Vec<ThemeEntry> {
    let mut entries = Vec::new();
    let user_dir = user_theme_dir();
    if let Some(path) = dev_default_dir_among(default_candidates).or_else(|| {
        user_dir
            .as_ref()
            .map(|dir| dir.join("default"))
            .filter(|path| path.is_dir())
    }) {
        entries.push(ThemeEntry {
            id: String::from("default"),
            path,
        });
    }

    if let Some(dir) = user_dir {
        entries.extend(packages_in(&dir, &entries));
    }
    entries
}

/// Every theme package directly under `dir`, in id order, skipping any id
/// already in `known`.
///
/// An unreadable directory yields nothing rather than failing: discovery
/// populates a picker, and a missing `<data>/themes` is the ordinary state
/// of a machine whose owner has not added a theme yet.
fn packages_in(dir: &Path, known: &[ThemeEntry]) -> Vec<ThemeEntry> {
    let Ok(listing) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<ThemeEntry> = listing
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let id = theme_id_of(&path)?;
            known
                .iter()
                .all(|entry| entry.id != id)
                .then_some(ThemeEntry { id, path })
        })
        .collect();
    found.sort_by(|a, b| a.id.cmp(&b.id));
    found
}

/// The theme id a directory or `.zip` package at `path` would get, or
/// `None` when `path` is not a theme package.
#[must_use]
pub fn theme_id_of(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str().filter(|stem| !stem.is_empty())?;
    let is_package = path.join("theme.toml").is_file()
        || (path.is_file() && path.extension().is_some_and(|ext| ext == "zip"));
    is_package.then(|| String::from(stem))
}

/// Loads the theme named `id` from `entries`.
///
/// # Errors
///
/// [`ThemeLookupError::UnknownTheme`] when no entry carries `id`;
/// [`ThemeLookupError::Load`] when the package fails to load.
pub fn load(entries: &[ThemeEntry], id: &str) -> Result<Theme, ThemeLookupError> {
    let entry = entries.iter().find(|entry| entry.id == id).ok_or_else(|| {
        ThemeLookupError::UnknownTheme {
            id: String::from(id),
        }
    })?;
    Theme::load_path(&entry.path).map_err(|source| ThemeLookupError::Load {
        id: String::from(id),
        source: Box::new(source),
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn theme_id_of_accepts_dirs_with_manifest_and_zips() {
        let dir = tempfile::tempdir().unwrap();
        let package = dir.path().join("winter");
        std::fs::create_dir(&package).unwrap();
        std::fs::write(package.join("theme.toml"), b"").unwrap();
        assert_eq!(theme_id_of(&package), Some(String::from("winter")));

        let zip = dir.path().join("cards.zip");
        std::fs::write(&zip, b"").unwrap();
        assert_eq!(theme_id_of(&zip), Some(String::from("cards")));

        let plain = dir.path().join("readme.txt");
        std::fs::write(&plain, b"").unwrap();
        assert_eq!(theme_id_of(&plain), None);

        let empty_dir = dir.path().join("empty");
        std::fs::create_dir(&empty_dir).unwrap();
        assert_eq!(theme_id_of(&empty_dir), None);
    }

    #[test]
    fn load_reports_unknown_ids() {
        let error = load(&[], "nope").unwrap_err();
        assert!(matches!(
            error,
            ThemeLookupError::UnknownTheme { id } if id == "nope"
        ));
    }

    #[test]
    fn load_reports_broken_packages() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("theme.toml"), b"not a manifest").unwrap();
        let entries = vec![ThemeEntry {
            id: String::from("broken"),
            path: dir.path().to_path_buf(),
        }];
        assert!(matches!(
            load(&entries, "broken"),
            Err(ThemeLookupError::Load { id, .. }) if id == "broken"
        ));
    }

    #[test]
    fn discover_finds_the_in_tree_default() {
        let entries = discover();
        assert!(entries.iter().any(|entry| entry.id == "default"));
    }

    /// A frontend whose default theme ships inside a bundle passes its own
    /// candidates; the first that exists wins, and nothing else changes.
    #[test]
    fn an_extra_candidate_directory_can_supply_the_default() {
        let dir = tempfile::tempdir().unwrap();
        let bundled = dir.path().join("Resources").join("themes").join("default");
        std::fs::create_dir_all(&bundled).unwrap();

        assert_eq!(
            dev_default_dir_among(&[PathBuf::from("/no/such/dir"), bundled.clone()]),
            Some(bundled)
        );
    }

    /// The picker lists user packages in id order and never repeats an id
    /// the in-tree default already claims — a user directory named `default`
    /// must not appear twice.
    #[test]
    fn user_packages_are_listed_in_order_and_never_shadow_a_known_id() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["winter", "arcade", "default"] {
            let package = dir.path().join(name);
            std::fs::create_dir(&package).unwrap();
            std::fs::write(package.join("theme.toml"), b"").unwrap();
        }
        std::fs::write(dir.path().join("notes.txt"), b"not a theme").unwrap();

        let known = vec![ThemeEntry {
            id: String::from("default"),
            path: PathBuf::from("/in/tree"),
        }];
        let ids: Vec<String> = packages_in(dir.path(), &known)
            .into_iter()
            .map(|entry| entry.id)
            .collect();
        assert_eq!(ids, vec![String::from("arcade"), String::from("winter")]);
    }

    /// A machine whose owner has added no themes has no such directory, and
    /// that is not a failure worth surfacing in a picker.
    #[test]
    fn an_absent_user_theme_directory_yields_nothing() {
        assert!(packages_in(Path::new("/no/such/directory"), &[]).is_empty());
    }

    /// Without any in-tree candidate, discovery falls back to the installed
    /// `<data>/themes/default` — the shape a packaged build has.
    #[test]
    fn discovery_falls_back_to_the_installed_default() {
        // No candidate exists, so the only "default" that could appear is
        // the installed one; on a dev machine there is none, and discovery
        // must then simply offer no default rather than inventing one.
        let entries = discover_among(&[PathBuf::from("/no/such/dir")]);
        let installed = user_theme_dir().map(|dir| dir.join("default"));
        let expected = installed.is_some_and(|path| path.is_dir());
        assert_eq!(
            entries.iter().any(|entry| entry.id == "default"),
            expected,
            "a default appears exactly when an installed one exists"
        );
    }

    #[test]
    fn no_candidate_directory_yields_no_default() {
        assert_eq!(
            dev_default_dir_among(&[PathBuf::from("/no/such/dir")]),
            None
        );
    }

    /// The workspace-relative candidate is what makes `cargo run` find the
    /// in-tree theme without an install step.
    #[test]
    fn the_default_candidates_include_the_in_tree_theme() {
        assert!(dev_default_dir_among(&default_dev_candidates()).is_some());
        assert!(dev_default_dir().is_some());
    }

    #[test]
    fn the_user_theme_dir_sits_under_the_data_dir() {
        let dir = user_theme_dir().unwrap();
        assert!(dir.ends_with("themes"), "{}", dir.display());
    }
}
