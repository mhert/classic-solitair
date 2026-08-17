//! [`ThemeProfile`]: the slice of a loaded [`sol_theme::Theme`] the
//! presenter actually needs — sizes, colors, and back-animation metadata.
//! Asset bytes stay with the theme; the renderer reads those.

use sol_theme::{BackLayout, BackTiming, CardSize, LoadedBackground, Theme};

use crate::display::{PlaceholderSlot, Rgba};
use crate::geometry::{Size, saturate};

/// Presentation metadata extracted from a loaded theme.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ThemeProfile {
    /// The theme's logical card size (`[cards] base_size`).
    pub card: CardSize,
    /// The outline-dragging rectangle color (`[drag] outline_color`).
    pub outline: Rgba,
    /// The table background.
    pub background: ProfileBackground,
    /// One entry per `[backs]` back, declaration order; a validated theme
    /// has at least one.
    pub backs: Vec<BackMeta>,
    /// Which `[placeholders]` slots the theme supplies. A theme may declare
    /// any subset, including none.
    pub placeholders: PlaceholderSet,
}

/// Which placeholder slots a theme supplies. Only presence matters here —
/// the pixels live with the theme, and the renderer reads those.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct PlaceholderSet {
    /// Whether `[placeholders] empty_pile` is declared.
    pub empty_pile: bool,
    /// Whether `[placeholders] stock_recycle` is declared.
    pub stock_recycle: bool,
    /// Whether `[placeholders] stock_blocked` is declared.
    pub stock_blocked: bool,
}

impl PlaceholderSet {
    /// Whether the theme supplies `slot`. A slot it does not supply is
    /// simply not drawn.
    pub(crate) const fn has(self, slot: PlaceholderSlot) -> bool {
        match slot {
            PlaceholderSlot::EmptyPile => self.empty_pile,
            PlaceholderSlot::StockRecycle => self.stock_recycle,
            PlaceholderSlot::StockBlocked => self.stock_blocked,
        }
    }
}

/// The table background, reduced to what a display list needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProfileBackground {
    /// A flat fill color: becomes the display list's clear color.
    Color(Rgba),
    /// An image: becomes background sprites over a black clear.
    Image {
        /// The image size in its own (unscaled) pixels.
        size: Size,
        /// Whether the image tiles at native size rather than stretching.
        tile: bool,
    },
}

/// Animation metadata of one theme back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BackMeta {
    /// Number of frames (1 for a static back).
    pub frames: u32,
    /// Frame timing; `None` for a static back.
    pub timing: Option<BackTiming>,
    /// Strip axis for strip-shaped backs; list-form backs keep the
    /// default, which is never consulted for them.
    pub layout: BackLayout,
    /// Number of assets: 1 for static and strip backs, one per frame for
    /// the list form.
    pub assets: usize,
}

impl ThemeProfile {
    /// Extracts the presentation metadata from a loaded theme.
    pub(crate) fn from_theme(theme: &Theme) -> Self {
        let background = match theme.background() {
            LoadedBackground::Color(color) => ProfileBackground::Color((*color).into()),
            LoadedBackground::Image { asset, tile } => ProfileBackground::Image {
                size: Size::new(
                    saturate(i64::from(asset.size.width)),
                    saturate(i64::from(asset.size.height)),
                ),
                tile: *tile,
            },
        };
        let backs = theme
            .backs()
            .iter()
            .map(|(_, back)| BackMeta {
                frames: back.frame_count,
                timing: back.timing.clone(),
                layout: back.layout.unwrap_or_default(),
                assets: back.assets.len(),
            })
            .collect();
        let loaded = theme.placeholders();
        Self {
            card: theme.manifest.base_size,
            outline: theme.manifest.outline_color.into(),
            background,
            backs,
            placeholders: PlaceholderSet {
                empty_pile: loaded.empty_pile.is_some(),
                stock_recycle: loaded.stock_recycle.is_some(),
                stock_blocked: loaded.stock_blocked.is_some(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use sol_theme::Color;

    use super::*;
    use crate::display::PlaceholderSlot;
    use crate::testkit::{test_theme, test_theme_with_placeholders};

    const SLOTS: [PlaceholderSlot; 3] = [
        PlaceholderSlot::EmptyPile,
        PlaceholderSlot::StockRecycle,
        PlaceholderSlot::StockBlocked,
    ];

    fn name(slot: PlaceholderSlot) -> &'static str {
        match slot {
            PlaceholderSlot::EmptyPile => "empty_pile",
            PlaceholderSlot::StockRecycle => "stock_recycle",
            PlaceholderSlot::StockBlocked => "stock_blocked",
        }
    }

    /// The test's own slot-to-field oracle, kept separate from the
    /// production mapping so a crossed assignment in `from_theme` shows up
    /// as a failure rather than being mirrored by the assertion.
    fn declared(set: PlaceholderSet, slot: PlaceholderSlot) -> bool {
        match slot {
            PlaceholderSlot::EmptyPile => set.empty_pile,
            PlaceholderSlot::StockRecycle => set.stock_recycle,
            PlaceholderSlot::StockBlocked => set.stock_blocked,
        }
    }

    #[test]
    fn a_theme_declaring_no_placeholders_supplies_none() {
        let profile = ThemeProfile::from_theme(&test_theme());
        assert_eq!(profile.placeholders, PlaceholderSet::default());
        for slot in SLOTS {
            assert!(!declared(profile.placeholders, slot), "{slot:?}");
        }
    }

    #[test]
    fn a_theme_declaring_every_placeholder_supplies_all_three() {
        let theme = test_theme_with_placeholders(&SLOTS.map(name));
        let profile = ThemeProfile::from_theme(&theme);
        for slot in SLOTS {
            assert!(declared(profile.placeholders, slot), "{slot:?}");
        }
    }

    /// Each slot is wired to its own field: declaring exactly one must
    /// light up that one and leave the other two dark, so a crossed pair of
    /// assignments cannot pass.
    #[test]
    fn each_slot_is_reported_independently_of_the_others() {
        for target in SLOTS {
            let theme = test_theme_with_placeholders(&[name(target)]);
            let profile = ThemeProfile::from_theme(&theme);
            for slot in SLOTS {
                assert_eq!(
                    declared(profile.placeholders, slot),
                    slot == target,
                    "declared {target:?}, asked {slot:?}"
                );
            }
        }
    }

    #[test]
    fn profile_extracts_card_outline_background_and_backs() {
        let theme = test_theme();
        let profile = ThemeProfile::from_theme(&theme);
        assert_eq!(profile.card.width, 71);
        assert_eq!(profile.card.height, 96);
        assert_eq!(profile.outline, Rgba::from(Color::new(0, 0, 0)));
        assert_eq!(
            profile.background,
            ProfileBackground::Color(Rgba::opaque(0x00, 0x80, 0x00))
        );
        // plain (static), strip (2 frames, fps), steps (2 frames,
        // durations, list form), tall (2 frames, vertical strip).
        assert_eq!(profile.backs.len(), 4);
        assert_eq!(profile.backs.first().map(|b| b.frames), Some(1));
        assert_eq!(profile.backs.first().and_then(|b| b.timing.clone()), None);
        assert_eq!(profile.backs.get(1).map(|b| b.frames), Some(2));
        assert_eq!(
            profile.backs.get(1).and_then(|b| b.timing.clone()),
            Some(BackTiming::Fps(2))
        );
        assert_eq!(profile.backs.get(1).map(|b| b.assets), Some(1));
        assert_eq!(
            profile.backs.get(2).and_then(|b| b.timing.clone()),
            Some(BackTiming::DurationsMs(vec![250, 750]))
        );
        assert_eq!(profile.backs.get(2).map(|b| b.assets), Some(2));
        assert_eq!(
            profile.backs.get(3).map(|b| b.layout),
            Some(BackLayout::Vertical)
        );
    }
}
