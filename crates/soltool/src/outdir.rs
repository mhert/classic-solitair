//! Output-directory guard for `extract`.
//!
//! A fresh theme is only ever written into an absent or empty `-o` target —
//! an existing, non-empty one is refused rather than clobbered.

use std::path::Path;

/// The output target already holds something.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum OutDirError {
    /// `output` exists and is a non-empty directory, or any non-directory.
    #[error("output directory {path} already exists and is not empty; refusing to overwrite it")]
    NotEmpty {
        /// The rejected output path.
        path: String,
    },
}

/// Confirms `output` is safe to write a fresh theme into: it must be absent
/// or an existing empty directory. A non-empty directory or a regular file at
/// the path is refused here; a path that simply cannot exist yet (e.g. its
/// parent is a file) is allowed through so the later write surfaces the more
/// precise failure.
///
/// [`std::fs::read_dir`] classifies the path: an `Ok` iterator is a real
/// directory (occupied iff it yields an entry); an `Err` means "not a
/// readable directory", and then [`Path::exists`] distinguishes a file that
/// is present (occupied) from a path that is genuinely absent (available).
///
/// # Errors
///
/// Returns [`OutDirError::NotEmpty`] if `output` is a non-empty directory or
/// an existing non-directory.
pub(crate) fn ensure_available(output: &Path) -> Result<(), OutDirError> {
    let occupied = match std::fs::read_dir(output) {
        Ok(mut entries) => entries.next().is_some(),
        Err(_) => output.exists(),
    };
    if occupied {
        Err(OutDirError::NotEmpty {
            path: output.display().to_string(),
        })
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn an_absent_path_is_available() {
        let dir = tempfile::tempdir().unwrap();
        assert!(ensure_available(&dir.path().join("does-not-exist")).is_ok());
    }

    #[test]
    fn an_existing_empty_directory_is_available() {
        let dir = tempfile::tempdir().unwrap();
        let empty = dir.path().join("empty");
        std::fs::create_dir(&empty).unwrap();
        assert!(ensure_available(&empty).is_ok());
    }

    #[test]
    fn a_non_empty_directory_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("keep.txt"), b"x").unwrap();
        assert!(matches!(
            ensure_available(dir.path()).unwrap_err(),
            OutDirError::NotEmpty { .. }
        ));
    }

    #[test]
    fn a_regular_file_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a-file");
        std::fs::write(&file, b"x").unwrap();
        assert!(matches!(
            ensure_available(&file).unwrap_err(),
            OutDirError::NotEmpty { .. }
        ));
    }

    #[test]
    fn the_error_message_names_the_path() {
        let error = OutDirError::NotEmpty {
            path: "/some/target".to_owned(),
        };
        assert!(error.to_string().contains("/some/target"));
    }
}
