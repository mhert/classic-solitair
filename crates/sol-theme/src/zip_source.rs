//! [`ZipSource`]: an [`crate::AssetSource`] over a zip archive's bytes,
//! decompressed once at construction — no filesystem.

use std::io::{Cursor, Read};

use crate::mem_source::MemSource;
use crate::path::RelativeAssetPath;
use crate::source::{AssetSource, SourceError};
use crate::theme_error::ThemeError;

/// A theme package stored as a zip archive, decompressed into memory once
/// at construction (mirrors [`crate::MemSource`] internally: every
/// subsequent [`AssetSource::read`] is a plain map lookup, no further zip
/// decoding).
#[derive(Debug, Clone)]
pub struct ZipSource {
    files: MemSource,
}

/// Ceiling on the total inflated size of a theme archive. The in-tree default
/// theme is well under a megabyte; this leaves room for a large png theme
/// with animated backs while keeping a decompression bomb from exhausting
/// memory before `theme.toml` has even been parsed.
const TOTAL_BUDGET_BYTES: u64 = 256 * 1024 * 1024;

impl ZipSource {
    /// Opens `bytes` as a zip archive and eagerly decompresses every entry
    /// it contains (directory entries are skipped).
    ///
    /// An entry whose stored name is not a valid [`RelativeAssetPath`] is
    /// dropped rather than stored: an archive is untrusted input, and a name
    /// that could not have come from a valid manifest must not become a
    /// lookup key. The manifest's own paths are parsed by the same rule, so
    /// a dropped entry is simply one nothing can ask for.
    ///
    /// # Errors
    ///
    /// Returns [`ThemeError::MalformedZip`] if `bytes` is not a valid zip
    /// archive, or an entry fails to decompress; [`ThemeError::ZipTooLarge`]
    /// if the entries together inflate past [`TOTAL_BUDGET_BYTES`].
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ThemeError> {
        Self::from_bytes_within(bytes, TOTAL_BUDGET_BYTES)
    }

    /// [`ZipSource::from_bytes`] against an explicit inflation budget.
    ///
    /// Split out so the budget can be exercised with a small archive: the
    /// shipped ceiling is a quarter of a gigabyte, and a test that had to
    /// reach it would allocate that much on every run.
    fn from_bytes_within(bytes: &[u8], budget: u64) -> Result<Self, ThemeError> {
        let malformed = |message: String| ThemeError::MalformedZip { message };

        let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
            .map_err(|error| malformed(error.to_string()))?;

        let mut files = MemSource::new();
        let mut inflated_total: u64 = 0;
        for index in 0..archive.len() {
            let mut entry = archive
                .by_index(index)
                .map_err(|error| malformed(error.to_string()))?;
            if entry.is_dir() {
                continue;
            }
            let name = entry.name().to_owned();
            let Ok(name) = RelativeAssetPath::parse("zip entry".to_owned(), &name) else {
                continue;
            };

            // `take` rather than trusting the header: the declared
            // uncompressed size is attacker-controlled and the zip crate does
            // not enforce it for deflate, so the reader itself is the bound.
            // Reading one byte past what remains is what makes an
            // over-budget entry detectable rather than silently truncated.
            let remaining = budget.saturating_sub(inflated_total);
            let mut contents = Vec::new();
            let read = entry
                .by_ref()
                .take(remaining.saturating_add(1))
                .read_to_end(&mut contents)
                .map_err(|error| malformed(error.to_string()))?;
            let read = u64::try_from(read).unwrap_or(u64::MAX);
            if read > remaining {
                return Err(ThemeError::ZipTooLarge { limit: budget });
            }
            inflated_total = inflated_total.saturating_add(read);

            files = files.with_file(name.as_str().to_owned(), contents);
        }

        Ok(Self { files })
    }
}

impl AssetSource for ZipSource {
    fn read(&self, path: &RelativeAssetPath) -> Result<Vec<u8>, SourceError> {
        self.files.read(path)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::io::Write;

    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    use super::*;
    use crate::testkit::asset_path;

    /// Builds a zip archive's bytes in memory from `(path, contents)`
    /// pairs, via the `zip` crate's writer.
    fn build_zip(files: &[(&str, &[u8])]) -> Vec<u8> {
        let cursor = std::io::Cursor::new(Vec::new());
        let mut writer = ZipWriter::new(cursor);
        let options = SimpleFileOptions::default();
        for (name, contents) in files {
            writer.start_file(*name, options).unwrap();
            writer.write_all(contents).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn reads_back_a_file_stored_in_the_archive() {
        let bytes = build_zip(&[("theme.toml", b"hello")]);
        let source = ZipSource::from_bytes(&bytes).unwrap();
        assert_eq!(source.read(&asset_path("theme.toml")).unwrap(), b"hello");
    }

    #[test]
    fn a_directory_entry_in_the_archive_is_skipped_not_stored_as_a_file() {
        let cursor = std::io::Cursor::new(Vec::new());
        let mut writer = ZipWriter::new(cursor);
        let options = SimpleFileOptions::default();
        writer.add_directory("cards/", options).unwrap();
        writer.start_file("cards/spades_01.png", options).unwrap();
        writer.write_all(b"png").unwrap();
        let bytes = writer.finish().unwrap().into_inner();

        let source = ZipSource::from_bytes(&bytes).unwrap();
        assert_eq!(
            source.read(&asset_path("cards/spades_01.png")).unwrap(),
            b"png"
        );
        let error = source.read(&asset_path("cards/")).unwrap_err();
        assert!(matches!(error, SourceError::NotFound { .. }));
    }

    /// An archive is untrusted input, so an entry whose name could never
    /// have come from a valid manifest is dropped at construction rather
    /// than stored under a name nothing is allowed to ask for.
    #[test]
    fn an_entry_whose_name_is_not_package_relative_is_dropped() {
        let bytes = build_zip(&[
            ("C:/Users/Public/escaped.bat", b"attacker bytes"),
            ("theme.toml", b"hello"),
        ]);
        let source = ZipSource::from_bytes(&bytes).unwrap();
        assert_eq!(source.read(&asset_path("theme.toml")).unwrap(), b"hello");
        assert!(!source.files.contains_raw_key("C:/Users/Public/escaped.bat"));
    }

    /// A theme package is untrusted input and is inflated eagerly, so the
    /// archive's compressed size is no bound on the memory it costs: a
    /// megabyte of zeros deflates to a few hundred bytes.
    #[test]
    fn rejects_an_archive_that_inflates_past_the_budget() {
        let bytes = deflated_zeros("bomb.png", 1024 * 1024);
        let error = ZipSource::from_bytes_within(&bytes, 4096).unwrap_err();
        assert!(matches!(error, ThemeError::ZipTooLarge { limit } if limit == 4096));
    }

    /// The budget is a total across entries, not a per-entry cap: several
    /// individually-legal entries must not add up past it.
    #[test]
    fn the_budget_is_shared_across_entries() {
        let cursor = std::io::Cursor::new(Vec::new());
        let mut writer = ZipWriter::new(cursor);
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for name in ["a.png", "b.png", "c.png"] {
            writer.start_file(name, options).unwrap();
            writer.write_all(&vec![0_u8; 3000]).unwrap();
        }
        let bytes = writer.finish().unwrap().into_inner();

        assert!(ZipSource::from_bytes_within(&bytes, 9000).is_ok());
        let error = ZipSource::from_bytes_within(&bytes, 8999).unwrap_err();
        assert!(matches!(error, ThemeError::ZipTooLarge { .. }));
    }

    /// The shipped ceiling has to be generous enough for a real theme, so an
    /// archive far larger than any test fixture still loads through the
    /// public entry point.
    #[test]
    fn an_ordinary_sized_archive_is_within_the_shipped_budget() {
        assert_eq!(TOTAL_BUDGET_BYTES, 268_435_456, "256 MiB");
        let bytes = deflated_zeros("big.png", 8 * 1024 * 1024);
        assert!(ZipSource::from_bytes(&bytes).is_ok());
    }

    /// An archive holding one deflated run of `len` zero bytes — highly
    /// compressible, so the stored archive stays small.
    fn deflated_zeros(name: &str, len: usize) -> Vec<u8> {
        let cursor = std::io::Cursor::new(Vec::new());
        let mut writer = ZipWriter::new(cursor);
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        writer.start_file(name, options).unwrap();
        writer.write_all(&vec![0_u8; len]).unwrap();
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn reads_back_a_nested_path() {
        let bytes = build_zip(&[("cards/spades_01.png", b"png")]);
        let source = ZipSource::from_bytes(&bytes).unwrap();
        assert_eq!(
            source.read(&asset_path("cards/spades_01.png")).unwrap(),
            b"png"
        );
    }

    #[test]
    fn reads_back_multiple_files() {
        let bytes = build_zip(&[("a.txt", b"a"), ("b.txt", b"b")]);
        let source = ZipSource::from_bytes(&bytes).unwrap();
        assert_eq!(source.read(&asset_path("a.txt")).unwrap(), b"a");
        assert_eq!(source.read(&asset_path("b.txt")).unwrap(), b"b");
    }

    #[test]
    fn a_missing_entry_is_not_found() {
        let bytes = build_zip(&[("theme.toml", b"hello")]);
        let source = ZipSource::from_bytes(&bytes).unwrap();
        let error = source.read(&asset_path("nope.png")).unwrap_err();
        assert!(matches!(error, SourceError::NotFound { path } if path == "nope.png"));
    }

    #[test]
    fn a_corrupt_local_header_is_a_malformed_zip_error() {
        // `ZipArchive::new` only parses the central directory; a corrupt
        // local file header is only discovered when `by_index` seeks to and
        // parses it for that specific entry. Corrupting the *second*
        // entry's local header signature (leaving the first entry and the
        // central directory untouched) reaches that per-entry parse.
        let mut bytes = build_zip(&[("a.txt", b"first"), ("b.txt", b"second")]);
        let signature = [0x50, 0x4B, 0x03, 0x04];
        let first = bytes.windows(4).position(|w| w == signature).unwrap();
        let after_first = bytes.get(first + 4..).unwrap();
        let second_offset = after_first.windows(4).position(|w| w == signature).unwrap();
        if let Some(byte) = bytes.get_mut(first + 4 + second_offset) {
            *byte = 0x00;
        }

        let error = ZipSource::from_bytes(&bytes).unwrap_err();
        assert!(matches!(error, ThemeError::MalformedZip { .. }));
    }

    #[test]
    fn corrupt_compressed_data_is_a_malformed_zip_error() {
        // A valid local header (so `by_index` succeeds) but corrupted
        // compressed bytes (so decompression fails when `read_to_end`
        // reads them) — a different failure point than a bad signature.
        let cursor = std::io::Cursor::new(Vec::new());
        let mut writer = ZipWriter::new(cursor);
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        writer.start_file("a.txt", options).unwrap();
        writer
            .write_all(&[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10].repeat(20))
            .unwrap();
        let mut bytes = writer.finish().unwrap().into_inner();

        // Local file header: 30 fixed bytes, then the filename, then the
        // compressed data — corrupt its first few bytes.
        let signature = [0x50, 0x4B, 0x03, 0x04];
        let header_start = bytes.windows(4).position(|w| w == signature).unwrap();
        let data_start = header_start + 30 + "a.txt".len();
        for offset in data_start..data_start + 4 {
            if let Some(byte) = bytes.get_mut(offset) {
                *byte ^= 0xFF;
            }
        }

        let error = ZipSource::from_bytes(&bytes).unwrap_err();
        assert!(matches!(error, ThemeError::MalformedZip { .. }));
    }

    #[test]
    fn corrupt_bytes_are_a_malformed_zip_error() {
        let error = ZipSource::from_bytes(b"not a zip archive at all").unwrap_err();
        assert!(matches!(error, ThemeError::MalformedZip { .. }));
    }

    #[test]
    fn empty_bytes_are_a_malformed_zip_error() {
        let error = ZipSource::from_bytes(&[]).unwrap_err();
        assert!(matches!(error, ThemeError::MalformedZip { .. }));
    }
}
