//! [`EditedOptions`]: the Options dialog's edit state, in flight.

use sol_engine::ScoringMode;

/// The Options dialog's editable fields, as one value a dialog reads out of
/// its controls on OK (the theme and back live selections are already
/// applied by then).
///
/// A dialog seeds this from [`crate::app::App::options_snapshot`], mutates
/// the copy, and commits it back through
/// [`crate::app::App::commit_options`], so nothing ever observes a partially
/// edited option set.
///
/// `sounds` is persisted and honoured by the option set, but no frontend
/// plays a sound yet: the theme format carries a `[sounds]` section and the
/// session stores the preference, while the audio path is unbuilt. The
/// checkbox is therefore truthful about what it records and silent about
/// what it produces.
// Five bools is inherent to the dialog's five independent toggles,
// mirroring sol-session's Options (same documented opt-out there).
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditedOptions {
    /// Deal three cards per stock click rather than one.
    pub draw_three: bool,
    /// Which scoring rules apply.
    pub scoring: ScoringMode,
    /// Run the game clock and apply the timed bonus/decay.
    pub timed: bool,
    /// Drag an outline rather than the card artwork.
    pub outline_dragging: bool,
    /// Carry the Vegas bankroll across deals.
    pub keep_vegas_score: bool,
    /// Play sounds (recorded, not yet produced — see the type doc).
    pub sounds: bool,
}
