//! [`RelativeAssetPath`]: a theme-package-relative asset path that has been
//! parsed, not merely checked. Every path-valued manifest field (face
//! directory/sheet, back images, background image, sound files) holds one.
//!
//! The asset-loading layer resolves these against a directory or zip source,
//! so holding the parsed type — rather than a `String` that was validated
//! somewhere upstream — is what makes an unvalidated path unable to reach a
//! filesystem join at all.
//!
//! The rule is applied to the raw string and is identical on every platform.
//! `std::path` is deliberately not used: its parsing is target-dependent, so
//! `Path::new("C:/x")` yields two ordinary components on Unix and a drive
//! prefix on Windows. A theme package is a cross-platform artifact; validating
//! it under one platform's rules and extracting it under another's is exactly
//! how a package that looks relative becomes an absolute write.

use core::fmt;

use crate::error::ManifestError;

/// Names Windows reserves as devices regardless of extension or directory.
/// A theme carrying one cannot be extracted on Windows, so it is rejected
/// everywhere rather than loading on Linux and failing later.
const RESERVED_DOS_NAMES: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// A validated theme-package-relative path: `/`-separated, no absolute
/// prefix of any platform's form, no traversal, no reserved device name.
///
/// Construct with [`RelativeAssetPath::parse`]. There is no way to build one
/// from an unchecked string, which is the point: an asset source resolves
/// one of these against a directory or archive, so a manifest string that
/// was never parsed cannot reach a filesystem join.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RelativeAssetPath(String);

impl RelativeAssetPath {
    /// Parses `raw` as a package-relative asset path; `context` names the
    /// offending field (and, for backs/sounds, the entry) in the error.
    ///
    /// # Errors
    ///
    /// Returns [`ManifestError::InvalidPath`] if `raw` is empty, starts with
    /// a separator, contains a `\`, or has a segment that is empty, `.`,
    /// `..`, a reserved DOS device name, or contains a `:`.
    pub fn parse(context: String, raw: &str) -> Result<Self, ManifestError> {
        match violated_rule(raw) {
            Some(reason) => Err(ManifestError::InvalidPath {
                context,
                path: raw.to_owned(),
                reason,
            }),
            None => Ok(Self(raw.to_owned())),
        }
    }

    /// A path this crate fixes itself rather than reading from a manifest —
    /// the package's `theme.toml`, and nothing else.
    ///
    /// Taking `&'static str` is the guarantee: a value assembled at runtime
    /// from manifest text cannot be passed here, so this is not a way around
    /// [`RelativeAssetPath::parse`].
    pub(crate) fn generated(path: &'static str) -> Self {
        Self(path.to_owned())
    }

    /// Appends a file name this crate generates to a directory reference
    /// that has already been parsed.
    ///
    /// Both halves are known good: the prefix is a parsed path, and `name`
    /// is built from the closed set of canonical card stems and the format's
    /// two file extensions — never manifest text. Re-parsing the result
    /// would add an error branch no input can reach.
    pub(crate) fn join_generated(&self, name: &str) -> Self {
        Self(format!("{}{name}", self.0))
    }

    /// The validated path, as it appeared in the manifest.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RelativeAssetPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Which relative-path rule `raw` violates, or `None` if it violates none.
///
/// Separate from [`RelativeAssetPath::parse`] so the rule reads as a rule and
/// the error is assembled once, in one place, from the caller's context.
fn violated_rule(raw: &str) -> Option<&'static str> {
    if raw.is_empty() {
        return Some("must not be empty");
    }
    if raw.starts_with('/') {
        return Some("must be a relative path, not start with `/`");
    }
    if raw.contains('\\') {
        return Some("must use `/` separators, not `\\`");
    }

    for segment in raw.split('/') {
        // A trailing `/` (a directory reference such as `cards/`) yields a
        // final empty segment and is legal; an interior empty segment is
        // rejected below.
        if segment.is_empty() {
            continue;
        }
        if segment == "." || segment == ".." {
            return Some("must not contain a `.` or `..` segment");
        }
        // Rejects Windows drive prefixes (`C:/…`, `C:x`) and NTFS alternate
        // data streams (`ace.png:hidden`) in one rule.
        if segment.contains(':') {
            return Some("must not contain a `:`");
        }
        if is_reserved_dos_name(segment) {
            return Some("must not use a name Windows reserves for a device");
        }
    }

    // Every segment but the last: a trailing `/` is a directory reference and
    // legal, so only interior emptiness (`a//b`) is a malformed path.
    if raw.split('/').rev().skip(1).any(str::is_empty) {
        return Some("must not contain an empty segment");
    }

    None
}

/// Is `segment` a reserved DOS device name, ignoring case and any extension?
/// Windows treats `NUL`, `nul.txt` and `NUL.png` alike.
fn is_reserved_dos_name(segment: &str) -> bool {
    let stem = match segment.split_once('.') {
        Some((stem, _)) => stem,
        None => segment,
    };
    RESERVED_DOS_NAMES
        .iter()
        .any(|reserved| stem.eq_ignore_ascii_case(reserved))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn parse(raw: &str) -> Result<RelativeAssetPath, ManifestError> {
        RelativeAssetPath::parse("cards.faces".to_owned(), raw)
    }

    #[test]
    fn accepts_a_relative_forward_slash_path() {
        assert_eq!(parse("cards/").unwrap().as_str(), "cards/");
        assert_eq!(
            parse("backs/robot.png").unwrap().as_str(),
            "backs/robot.png"
        );
    }

    #[test]
    fn rejects_an_absolute_path() {
        assert!(matches!(
            parse("/etc/passwd").unwrap_err(),
            ManifestError::InvalidPath { .. }
        ));
    }

    #[test]
    fn rejects_a_parent_segment() {
        assert!(parse("../secret").is_err());
        assert!(parse("a/../b").is_err());
    }

    #[test]
    fn rejects_a_backslash() {
        assert!(parse("backs\\robot.png").is_err());
        assert!(parse("\\\\server\\share").is_err());
    }

    #[test]
    fn does_not_reject_a_double_dot_that_is_not_its_own_segment() {
        // ".." must be a whole segment to be a parent reference; a filename
        // that merely contains two dots is legal.
        assert!(parse("weird..name.png").is_ok());
    }

    /// The reason this rule reads the raw string instead of `Path::components`:
    /// on Unix, `Path::new("C:/x")` yields two `Normal` components, so a
    /// components-based check passes on the machine that validates and fails on
    /// the machine that writes. A theme package is a cross-platform artifact
    /// and gets one rule everywhere.
    #[test]
    fn rejects_a_windows_drive_prefix_on_every_platform() {
        assert!(parse("C:/Windows/Temp/evil.exe").is_err());
        assert!(parse("c:/x").is_err());
        assert!(parse("C:x").is_err());
    }

    #[test]
    fn rejects_an_alternate_data_stream_suffix() {
        assert!(parse("cards/ace.png:hidden").is_err());
    }

    #[test]
    fn rejects_a_current_directory_segment() {
        assert!(parse("./cards/ace.png").is_err());
        assert!(parse("cards/./ace.png").is_err());
    }

    #[test]
    fn rejects_an_empty_segment() {
        assert!(parse("").is_err());
        assert!(parse("cards//ace.png").is_err());
    }

    #[test]
    fn rejects_reserved_dos_device_names() {
        for raw in ["CON", "nul", "cards/COM1.png", "aux.svg", "LPT9"] {
            assert!(parse(raw).is_err(), "expected {raw} to be rejected");
        }
    }

    #[test]
    fn accepts_a_name_that_merely_starts_like_a_device_name() {
        assert!(parse("console.png").is_ok());
        assert!(parse("nullable.svg").is_ok());
    }

    #[test]
    fn error_names_context_and_path() {
        let error = RelativeAssetPath::parse("back `robot` image".to_owned(), "/x").unwrap_err();
        let message = error.to_string();
        assert!(message.contains("back `robot` image"), "{message}");
        assert!(message.contains("/x"), "{message}");
    }

    #[test]
    fn displays_as_its_raw_path() {
        assert_eq!(
            parse("backs/robot.png").unwrap().to_string(),
            "backs/robot.png"
        );
    }

    #[test]
    fn a_crate_fixed_path_is_taken_as_written() {
        assert_eq!(
            RelativeAssetPath::generated("theme.toml").as_str(),
            "theme.toml"
        );
    }

    #[test]
    fn joining_a_generated_name_extends_a_parsed_directory() {
        let dir = parse("cards/").unwrap();
        assert_eq!(
            dir.join_generated("spades_01.png").as_str(),
            "cards/spades_01.png"
        );
    }
}
