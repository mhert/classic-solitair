//! Sound loading: validation matrix row 7 — existence only.

use crate::path::RelativeAssetPath;
use crate::source::AssetSource;
use crate::theme_error::ThemeError;

/// Loads every `[sounds]` entry's bytes, in declaration order — the first
/// unreadable sound wins.
///
/// Sounds are not [`crate::Asset`]s (existence only; no dimensions to
/// probe), so their bytes are returned directly.
///
/// # Errors
///
/// Returns [`ThemeError::SoundUnreadable`] if a sound's bytes cannot be
/// read from `source`.
pub(crate) fn load(
    source: &impl AssetSource,
    sounds: &[(String, RelativeAssetPath)],
) -> Result<Vec<(String, Vec<u8>)>, ThemeError> {
    sounds
        .iter()
        .map(|(name, path)| {
            let bytes = source
                .read(path)
                .map_err(|source| ThemeError::SoundUnreadable {
                    name: name.clone(),
                    path: path.as_str().to_owned(),
                    source,
                })?;
            Ok((name.clone(), bytes))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::mem_source::MemSource;
    use crate::testkit::asset_path;

    #[test]
    fn loads_every_sounds_bytes_in_declaration_order() {
        let source = MemSource::new()
            .with_file("sounds/deal.ogg", b"deal-bytes".to_vec())
            .with_file("sounds/win.ogg", b"win-bytes".to_vec());
        let sounds = vec![
            ("deal".to_owned(), asset_path("sounds/deal.ogg")),
            ("win".to_owned(), asset_path("sounds/win.ogg")),
        ];

        let loaded = load(&source, &sounds).unwrap();
        assert_eq!(
            loaded,
            vec![
                ("deal".to_owned(), b"deal-bytes".to_vec()),
                ("win".to_owned(), b"win-bytes".to_vec()),
            ]
        );
    }

    #[test]
    fn an_empty_sounds_list_loads_as_empty() {
        let source = MemSource::new();
        assert_eq!(load(&source, &[]).unwrap(), Vec::new());
    }

    #[test]
    fn a_missing_sound_names_it_by_key_and_path() {
        let source = MemSource::new().with_file("sounds/deal.ogg", b"deal-bytes".to_vec());
        let sounds = vec![
            ("deal".to_owned(), asset_path("sounds/deal.ogg")),
            ("win".to_owned(), asset_path("sounds/win.ogg")),
        ];

        let error = load(&source, &sounds).unwrap_err();
        assert!(matches!(
            error,
            ThemeError::SoundUnreadable { name, path, .. }
                if name == "win" && path == "sounds/win.ogg"
        ));
    }

    #[test]
    fn the_first_missing_sound_in_declaration_order_wins() {
        let source = MemSource::new();
        let sounds = vec![
            ("deal".to_owned(), asset_path("sounds/deal.ogg")),
            ("win".to_owned(), asset_path("sounds/win.ogg")),
        ];

        let error = load(&source, &sounds).unwrap_err();
        assert!(matches!(
            error,
            ThemeError::SoundUnreadable { name, .. } if name == "deal"
        ));
    }
}
