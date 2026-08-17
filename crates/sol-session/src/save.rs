//! Save format v1: a single versioned JSON document, platform-free — this
//! module touches no filesystem at all; see `storage.rs` for where saves
//! actually get written to disk.
//!
//! [`SaveGame`] is the serialization core. [`crate::Session::to_save`] and
//! [`crate::Session::from_save`] (plus the `_bytes` compositions) convert
//! between it and a running [`crate::Session`].
//!
//! Loading is deserialize, then re-deal from the seed and fold the log
//! (`Game::from_log`) — see [`crate::Session::from_save`]. The redo stack is
//! intentionally not part of the format: the log is canonical, so after a
//! load `Session::game().can_redo()` is always `false`, while undo
//! availability is restored exactly, since it derives from the log alone.
//!
//! v1 embeds `sol_engine`'s own serde representations of [`Seed`] and
//! [`LogEntry`] (which in turn embeds `Command`/`Event`) verbatim. Any
//! future change to those types' serde shape is a save-format break and
//! must land alongside a [`FORMAT_VERSION`] bump —
//! `tests/fixtures/save_v1.json` locks the exact bytes forever specifically
//! to catch this.

use serde::{Deserialize, Serialize};
use sol_engine::{LogEntry, Seed};

use crate::bankroll::Bankroll;
use crate::options::Options;

/// The save format version this build writes, and the only version it
/// accepts on load. Bumping it is a breaking format change.
pub const FORMAT_VERSION: u32 = 1;

/// The string written into every save's [`SaveGame::engine_version`]. Pinned
/// to the save format, not the crate: unlike [`sol_engine::VERSION`] it
/// changes only when the save format itself changes, so the committed
/// `tests/fixtures/save_v1.json` stays byte-identical across engine releases.
pub const ENGINE_VERSION: &str = "1.0.0";

/// A save-format v1 document: a single
/// versioned JSON object. Field order is locked forever —
/// `format_version`, `engine_version`, `seed`, `options`, `log`, `bankroll`,
/// `elapsed_secs` — this **is** format v1.
///
/// ```
/// use sol_engine::Seed;
/// use sol_session::{Bankroll, ENGINE_VERSION, Options, SaveGame, FORMAT_VERSION};
///
/// let save = SaveGame {
///     format_version: FORMAT_VERSION,
///     engine_version: ENGINE_VERSION.to_owned(),
///     seed: Seed::new(1).unwrap(),
///     options: Options::default(),
///     log: Vec::new(),
///     bankroll: Bankroll::default(),
///     elapsed_secs: 0,
/// };
/// let bytes = save.to_bytes()?;
/// assert_eq!(SaveGame::from_bytes(&bytes)?, save);
/// # Ok::<(), sol_session::SaveError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SaveGame {
    /// The save format version this document was written as.
    pub format_version: u32,
    /// The save format's engine version string, always [`ENGINE_VERSION`].
    /// Informational only — every `format_version`-1 document is read
    /// regardless of this value.
    pub engine_version: String,
    /// The deal seed.
    pub seed: Seed,
    /// The player's options at the time of saving.
    pub options: Options,
    /// The running game's full command log — replaying it from `seed`
    /// reconstructs the game exactly, undo availability included.
    pub log: Vec<LogEntry>,
    /// The committed Vegas bankroll.
    pub bankroll: Bankroll,
    /// The session clock's total elapsed play seconds.
    pub elapsed_secs: u32,
}

impl SaveGame {
    /// Serializes this document to its canonical pretty-printed JSON bytes
    /// (`serde_json::to_vec_pretty`) — the exact bytes
    /// `tests/fixtures/save_v1.json` locks forever.
    ///
    /// # Errors
    ///
    /// Returns [`SaveError::Malformed`] if serialization fails —
    /// unreachable in practice, since every field of `SaveGame` serializes
    /// infallibly.
    pub fn to_bytes(&self) -> Result<Vec<u8>, SaveError> {
        Ok(serde_json::to_vec_pretty(self)?)
    }

    /// Parses `bytes` as a save document, two-phase:
    /// phase 1 probes only `format_version`, via a private struct that
    /// tolerates any other shape, so an unsupported version is reported as
    /// [`SaveError::UnsupportedFormatVersion`] even when the rest of the
    /// document doesn't parse as v1 at all — the typed rejection always
    /// wins over a shape error a foreign version would otherwise cause.
    /// Only once the version is confirmed does phase 2 run the full typed
    /// parse.
    ///
    /// # Errors
    ///
    /// Returns [`SaveError::UnsupportedFormatVersion`] when `format_version`
    /// parses but is not [`FORMAT_VERSION`]. Returns [`SaveError::Malformed`]
    /// for anything else that keeps `bytes` from parsing as a v1
    /// [`SaveGame`] — invalid JSON, a missing or wrongly-typed
    /// `format_version`, a missing required field, or an unknown top-level
    /// field.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, SaveError> {
        let probe: FormatProbe = serde_json::from_slice(bytes)?;
        if probe.format_version != FORMAT_VERSION {
            return Err(SaveError::UnsupportedFormatVersion {
                found: probe.format_version,
            });
        }
        Ok(serde_json::from_slice(bytes)?)
    }
}

/// The phase-1 version probe: reads only
/// `format_version`; every other field — present, absent, or malformed — is
/// tolerated, so this alone can never produce a shape error.
#[derive(Deserialize)]
struct FormatProbe {
    format_version: u32,
}

/// Errors from [`SaveGame::from_bytes`].
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SaveError {
    /// The document's `format_version` was read successfully but is not one
    /// this build can load.
    #[error("unsupported save format version {found}: this build reads version {FORMAT_VERSION}")]
    UnsupportedFormatVersion {
        /// The `format_version` found in the document.
        found: u32,
    },
    /// The document did not parse as a v1 [`SaveGame`] — invalid JSON, a
    /// missing or extra field, or a field of the wrong shape.
    #[error("malformed save data: {0}")]
    Malformed(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use sol_engine::{Command, Event, PileId};

    fn sample_save() -> SaveGame {
        SaveGame {
            format_version: FORMAT_VERSION,
            engine_version: ENGINE_VERSION.to_owned(),
            seed: Seed::new(1).unwrap(),
            options: Options::default(),
            log: vec![LogEntry {
                command: Command::MoveCards {
                    from: PileId::Tableau(0),
                    to: PileId::Foundation(0),
                    count: 1,
                },
                events: vec![Event::ScoreChanged { delta: 10 }],
            }],
            bankroll: Bankroll::from(-52_i64),
            elapsed_secs: 42,
        }
    }

    #[test]
    fn round_trips_through_bytes() {
        let save = sample_save();
        let bytes = save.to_bytes().unwrap();
        assert_eq!(SaveGame::from_bytes(&bytes).unwrap(), save);
    }

    #[test]
    fn to_bytes_is_pretty_printed_json() {
        let bytes = sample_save().to_bytes().unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.starts_with("{\n"), "pretty-printed, not compact");
        assert!(text.contains("\"format_version\": 1"));
    }

    #[test]
    fn rejects_an_unsupported_format_version_even_with_a_garbage_shape() {
        let mut value = serde_json::to_value(sample_save()).unwrap();
        let object = value.as_object_mut().unwrap();
        object.insert("format_version".to_owned(), serde_json::json!(2));
        // Wreck everything else: the probe must not care.
        object.insert("options".to_owned(), serde_json::json!("not an object"));
        object.remove("seed");
        let bytes = serde_json::to_vec(&value).unwrap();

        let error = SaveGame::from_bytes(&bytes).unwrap_err();

        assert!(matches!(
            error,
            SaveError::UnsupportedFormatVersion { found: 2 }
        ));
    }

    #[test]
    fn rejects_format_version_zero() {
        let mut value = serde_json::to_value(sample_save()).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .insert("format_version".to_owned(), serde_json::json!(0));
        let bytes = serde_json::to_vec(&value).unwrap();

        let error = SaveGame::from_bytes(&bytes).unwrap_err();

        assert!(matches!(
            error,
            SaveError::UnsupportedFormatVersion { found: 0 }
        ));
    }

    #[test]
    fn unsupported_format_version_message_names_found_and_this_builds_version() {
        let found: u32 = 7;
        let error = SaveError::UnsupportedFormatVersion { found };
        let message = error.to_string();
        assert!(message.contains(&found.to_string()), "{message}");
        assert!(message.contains(&FORMAT_VERSION.to_string()), "{message}");
    }

    #[test]
    fn rejects_non_json_bytes_as_malformed() {
        let error = SaveGame::from_bytes(b"not json at all").unwrap_err();
        assert!(matches!(error, SaveError::Malformed(_)));
    }

    #[test]
    fn rejects_a_json_array_as_malformed() {
        let error = SaveGame::from_bytes(b"[1,2,3]").unwrap_err();
        assert!(matches!(error, SaveError::Malformed(_)));
    }

    #[test]
    fn rejects_a_missing_required_field_as_malformed() {
        let mut value = serde_json::to_value(sample_save()).unwrap();
        value.as_object_mut().unwrap().remove("elapsed_secs");
        let bytes = serde_json::to_vec(&value).unwrap();

        let error = SaveGame::from_bytes(&bytes).unwrap_err();

        assert!(matches!(error, SaveError::Malformed(_)));
    }

    #[test]
    fn rejects_an_unknown_top_level_field_as_malformed() {
        let mut value = serde_json::to_value(sample_save()).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .insert("unknown_field".to_owned(), serde_json::json!(1));
        let bytes = serde_json::to_vec(&value).unwrap();

        let error = SaveGame::from_bytes(&bytes).unwrap_err();

        assert!(matches!(error, SaveError::Malformed(_)));
    }
}
