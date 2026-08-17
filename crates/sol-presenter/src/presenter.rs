//! [`Presenter`]: the platform-neutral presentation facade native UIs
//! talk to.
//!
//! Owns the running [`Session`] (game, options, bankroll, save/load), the
//! playfield [`Layout`], drag state, and every animation. Frontends feed
//! it pointer/key events and `advance(dt)` ticks, call its command/query
//! API from their menus, and draw the [`DisplayList`] it emits each
//! frame. It never reads a clock, spawns a thread, or touches I/O: time
//! arrives only through [`Presenter::advance`], entropy only through the
//! seeds the host passes in.

use sol_engine::{Command, Event, PileId, RuleError, Seed};
use sol_session::{Options, SaveError, Session};
use sol_theme::Theme;

use crate::back_sheet::BackSheet;
use crate::backs;
use crate::cascade::Cascade;
use crate::deal_anim::DealAnimation;
use crate::display::Rgba;
use crate::drag::{Drag, SnapBack, drop_target, pick_up};
use crate::fit::Fit;
use crate::geometry::{Pt, Size};
use crate::hit::{HitTarget, card_pos, hit_test};
use crate::layout::Layout;
use crate::profile::ThemeProfile;
use crate::waste::fan_len;

mod frame;

/// Two presses on the same card within this window are a double-click —
/// the Windows default double-click time the original inherited from the
/// system.
const DOUBLE_CLICK_MS: u64 = 500;

/// How many `advance` ticks a post-resize board repaint stays owed for,
/// so that both ways a host drives its draw loop observe it (see the
/// [`Presenter`] `resize_repaint` field). A host that renders once
/// *without* advancing right after a refit spends one tick's worth on
/// that render; a host that advances before every frame only draws its
/// first post-resize frame *after* a tick, so it needs the repaint to
/// survive that first decrement. Two covers both.
const RESIZE_REPAINT_TICKS: u8 = 2;

/// A presenter operation failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum PresenterError {
    /// [`Presenter::set_back`] named a back the theme does not declare.
    #[error("no card back {index}: the theme declares {count}")]
    UnknownBack {
        /// The requested back index.
        index: usize,
        /// How many backs the theme declares.
        count: usize,
    },
}

/// What the presenter is currently animating.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Phase {
    /// Normal play.
    Idle,
    /// The deal animation is flying cards to the tableau.
    Dealing(DealAnimation),
    /// An illegally dropped run is sliding home.
    SnapBack(SnapBack),
    /// The win cascade is running.
    Cascade(Cascade),
}

/// The previous press, for double-click detection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PressMemo {
    at_ms: u64,
    pile: PileId,
    index: usize,
}

/// The presentation core: session, layout, drag, and animation state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Presenter {
    session: Session,
    profile: ThemeProfile,
    layout: Layout,
    viewport: Size,
    surface: Option<(u32, u32)>,
    clock_ms: u64,
    fan: usize,
    back_index: usize,
    phase: Phase,
    drag: Option<Drag>,
    last_press: Option<PressMemo>,
    /// Post-resize board-repaint countdown: set to
    /// [`RESIZE_REPAINT_TICKS`] by [`Presenter::fit_viewport`],
    /// decremented (saturating) at the top of [`Presenter::advance`], and
    /// read by [`Presenter::frame`], which repaints the full board while
    /// the count is nonzero. A resize overwrites the count outright, so
    /// several resizes between ticks are equivalent to one.
    ///
    /// Every frontend recreates its render target when the surface
    /// resizes, and a fresh target is blank; but a running win cascade's
    /// `frame` ordinarily draws only the newly stepped positions with
    /// `clear: None`, relying on its smear trail surviving frame to
    /// frame. Combined, a mid-cascade resize would leave the whole board
    /// black under the bouncing cards. While this count is nonzero,
    /// `frame` instead repaints the ordinary full board, with the
    /// cascade's pending faces layered on top, before letting the smear
    /// resume.
    ///
    /// It counts ticks rather than latching a single bool because the two
    /// ways a host drives its draw loop observe it at different moments,
    /// and that coupling is invisible from the frontends so it is spelled
    /// out here. One shape renders once without advancing right after the
    /// refit, then advances-then-draws on later ticks; the other advances
    /// before every draw, so even its first post-resize frame follows an
    /// `advance`. A one-shot bool cleared in `advance` would already be
    /// gone by that frame. Two ticks let the advance-free render consume
    /// one and still leave a nonzero count for the advance-first host's
    /// next frame; the only cost is one extra self-contained frame in the
    /// advance-free host, which is visually benign — the smear just
    /// restarts one frame later.
    resize_repaint: u8,
}

impl Presenter {
    /// Wraps a session for presentation with the given theme, at scale 1
    /// and the layout's design viewport.
    ///
    /// A brand-new session (empty log) starts with the deal animation; a
    /// restored one resumes quietly.
    #[must_use]
    pub fn new(session: Session, theme: &Theme) -> Self {
        let profile = ThemeProfile::from_theme(theme);
        let layout = Layout::new(profile.card, Layout::min_design(profile.card).w);
        let phase = if session.game().log().is_empty() {
            Phase::Dealing(DealAnimation::new())
        } else {
            Phase::Idle
        };
        let fan = fan_len(session.game().log());
        let clock_ms = u64::from(session.elapsed_secs()).saturating_mul(1000);
        Self {
            session,
            profile,
            layout,
            viewport: layout.design_size(),
            surface: None,
            clock_ms,
            fan,
            back_index: 0,
            phase,
            drag: None,
            last_press: None,
            resize_repaint: 0,
        }
    }

    /// Switches the active theme: relayouts to its card size and clamps
    /// the selected back. Transient animations and any drag are dropped.
    pub fn set_theme(&mut self, theme: &Theme) {
        self.profile = ThemeProfile::from_theme(theme);
        // The viewport is updated on both paths, not just the refit one: the
        // public viewport, the background's tiling bounds and the cascade's
        // exit bounds all read it, and none of them may be left describing
        // the theme that was just replaced. With no surface reported yet
        // there is nothing to re-fit against, so the board falls back to its
        // minimum design size — and the viewport follows it there too.
        self.viewport = match self.surface {
            Some((w, h)) => Fit::compute(self.profile.card, w, h).logical,
            None => Layout::min_design(self.profile.card),
        };
        self.layout = Layout::new(self.profile.card, self.viewport.w);
        if self.back_index >= self.profile.backs.len() {
            self.back_index = 0;
        }
        self.drag = None;
        self.skip_animations();
    }

    /// The playfield layout currently in effect.
    #[must_use]
    pub const fn layout(&self) -> &Layout {
        &self.layout
    }

    /// The current viewport in logical pixels.
    #[must_use]
    pub const fn viewport(&self) -> Size {
        self.viewport
    }

    /// Fits the playfield to a physical surface: computes the continuous
    /// scale, adopts the logical viewport, and re-spreads the columns.
    /// Interactions survive: a live drag keeps riding the pointer
    /// (logical coordinates are scale-independent), a running snap-back
    /// retargets to its pile's new position, and the deal re-derives its
    /// flight endpoints each frame anyway.
    ///
    /// Returns the fit; the host forwards `scale` to its renderer and
    /// divides pointer coordinates by it (see [`Fit::to_logical`]).
    pub fn fit_viewport(&mut self, width: u32, height: u32) -> Fit {
        let fit = Fit::compute(self.profile.card, width, height);
        self.surface = Some((width.max(1), height.max(1)));
        self.viewport = fit.logical;
        self.layout = Layout::new(self.profile.card, fit.logical.w);
        self.refresh_interaction_homes();
        // The host is about to recreate its render target at the new
        // size (see `resize_repaint` on the struct): arm the repaint
        // countdown unconditionally, overwriting any in-flight count so
        // repeated resizes collapse to one repaint. Non-cascade frames
        // are self-contained already, so this only changes anything when
        // a cascade happens to be running.
        self.resize_repaint = RESIZE_REPAINT_TICKS;
        fit
    }

    /// The fit currently in effect: identity (scale 1 over the minimum
    /// design viewport) until the host reports a surface.
    #[must_use]
    pub fn fit(&self) -> Fit {
        self.surface.map_or(
            Fit {
                scale: 1.0,
                logical: Layout::min_design(self.profile.card),
            },
            |(w, h)| Fit::compute(self.profile.card, w, h),
        )
    }

    /// Re-derives the home position (the source pile's slot under the
    /// current layout) for whatever interaction is holding cards: a
    /// running snap-back retargets its flight; a live drag refreshes the
    /// home a *future* snap-back would fly to.
    fn refresh_interaction_homes(&mut self) {
        let state = self.session.game().state();
        if let Some(drag) = &mut self.drag
            && let Some(home) = card_pos(state, &self.layout, self.fan, drag.from, drag.first_index)
        {
            drag.home = home;
        }
        if let Phase::SnapBack(snap) = &mut self.phase {
            let drag = snap.drag;
            // In-range piles always resolve; skipping on the impossible
            // miss merely keeps the lookup total (a stale home would
            // cost one visual frame at worst).
            if let Some(home) = card_pos(state, &self.layout, self.fan, drag.from, drag.first_index)
            {
                snap.retarget(home);
            }
        }
    }

    /// Advances time by `dt_ms` milliseconds: the presenter clock, the
    /// engine tick (score decay in timed games), and whatever animation
    /// runs. The clock freezes while a drag is in progress, exactly like
    /// the original's timer gate.
    pub fn advance(&mut self, dt_ms: u32) {
        // Spend one tick of any owed post-resize repaint
        // (`resize_repaint`). `frame` is `&self` and cannot decrement it,
        // so it happens here, before this tick's own frame is drawn.
        // Counting down rather than clearing outright keeps the repaint
        // owed across both host draw shapes — an advance-free post-resize
        // render and a host that advances before every frame — as that
        // field explains.
        self.resize_repaint = self.resize_repaint.saturating_sub(1);
        if self.drag.is_none() {
            self.clock_ms = self.clock_ms.saturating_add(u64::from(dt_ms));
            let secs = u32::try_from(self.clock_ms / 1000).unwrap_or(u32::MAX);
            // The clock is monotonic, so "differs" means "advanced".
            if secs != self.session.elapsed_secs() && !self.session.game().state().is_won() {
                // Monotonic and not won: the engine can only accept this.
                let _ = self.session.tick(secs);
            }
        }
        match &mut self.phase {
            Phase::Idle => {}
            Phase::Dealing(deal) => {
                deal.advance(dt_ms);
                if deal.is_done() {
                    self.phase = Phase::Idle;
                }
            }
            Phase::SnapBack(snap) => {
                snap.advance(dt_ms);
                if snap.is_done() {
                    self.phase = Phase::Idle;
                }
            }
            Phase::Cascade(cascade) => {
                cascade.advance(dt_ms);
                if cascade.is_done() {
                    self.phase = Phase::Idle;
                }
            }
        }
    }

    /// Whether any animation (deal, snap-back, cascade) is running.
    #[must_use]
    pub const fn is_animating(&self) -> bool {
        !matches!(self.phase, Phase::Idle)
    }

    /// Whether the win cascade is running.
    #[must_use]
    pub const fn is_cascade_running(&self) -> bool {
        matches!(self.phase, Phase::Cascade(_))
    }

    /// Lands every running animation instantly. Any input does this — the
    /// original's cascade (and this presenter's deal and snap-back) stop
    /// on the first key or click, which is then processed normally.
    fn skip_animations(&mut self) {
        match &mut self.phase {
            Phase::Idle => return,
            Phase::Dealing(deal) => deal.skip(),
            Phase::SnapBack(snap) => snap.skip(),
            Phase::Cascade(cascade) => cascade.skip(),
        }
        self.phase = Phase::Idle;
    }

    /// Applies a command after skipping animations, swallowing rule
    /// rejections: a rejected pointer command (exhausted stock, no
    /// eligible foundation, illegal drop) is a normal non-event the
    /// original answered with silence.
    fn try_command(&mut self, command: Command) {
        if let Ok(events) = self.session.apply(command) {
            self.command_applied(&events);
        }
    }

    /// Post-command bookkeeping: refold the waste fan; start the cascade
    /// on a win.
    fn command_applied(&mut self, events: &[Event]) {
        self.fan = fan_len(self.session.game().log());
        if events.contains(&Event::GameWon) {
            self.phase = Phase::Cascade(Cascade::new(
                self.session.game().state(),
                &self.layout,
                self.viewport,
                self.session.game().seed().get().into(),
            ));
        }
    }

    /// A pointer press at `pt` (logical pixels): skips animations, then
    /// draws from the stock, fires a double-click's `AutoToFoundation`,
    /// or picks up a draggable card.
    pub fn pointer_down(&mut self, pt: Pt) {
        self.skip_animations();
        if self.drag.is_some() {
            // Further buttons while dragging are ignored, as the original
            // ignored them under its mouse capture.
            return;
        }
        let hit = hit_test(self.session.game().state(), &self.layout, self.fan, pt);
        match hit {
            Some(HitTarget::Stock) => {
                self.last_press = None;
                self.try_command(Command::Draw);
            }
            Some(HitTarget::Card { pile, index }) => {
                if self.completes_double_click(pile, index) {
                    self.last_press = None;
                    self.try_command(Command::AutoToFoundation { pile });
                    return;
                }
                self.last_press = Some(PressMemo {
                    at_ms: self.clock_ms,
                    pile,
                    index,
                });
                self.drag = pick_up(
                    self.session.game().state(),
                    &self.layout,
                    self.fan,
                    pile,
                    index,
                    pt,
                );
            }
            None => {
                self.last_press = None;
            }
        }
    }

    /// Whether this press is the second of a double-click that the
    /// original would consume: same card as the previous press, within
    /// the double-click time, on the top card of the waste or a tableau
    /// pile (only those piles answer double-clicks).
    fn completes_double_click(&self, pile: PileId, index: usize) -> bool {
        let Some(memo) = self.last_press else {
            return false;
        };
        if memo.pile != pile || memo.index != index {
            return false;
        }
        if self.clock_ms.saturating_sub(memo.at_ms) > DOUBLE_CLICK_MS {
            return false;
        }
        let state = self.session.game().state();
        match pile {
            PileId::Waste => state.waste().len() == index + 1,
            PileId::Tableau(t) => state.tableau(t).is_some_and(|p| p.len() == index + 1),
            PileId::Stock | PileId::Foundation(_) => false,
        }
    }

    /// A pointer move to `pt`: the dragged run follows.
    pub fn pointer_move(&mut self, pt: Pt) {
        if let Some(drag) = &mut self.drag {
            drag.pos = pt;
        }
    }

    /// A pointer release at `pt`: drops the dragged run on its live
    /// target, or slides it back home.
    pub fn pointer_up(&mut self, pt: Pt) {
        let Some(mut drag) = self.drag.take() else {
            return;
        };
        drag.pos = pt;
        let target = drop_target(self.session.game().state(), &self.layout, self.fan, &drag);
        if let Some(to) = target {
            self.try_command(Command::MoveCards {
                from: drag.from,
                to,
                count: drag.count,
            });
        } else {
            let snap = SnapBack::new(drag);
            if !snap.is_done() {
                self.phase = Phase::SnapBack(snap);
            }
        }
    }

    /// A key press: skips any running animation. Keyboard shortcuts
    /// themselves are the frontend's business — they arrive here as
    /// command/query API calls.
    pub fn key_down(&mut self) {
        self.skip_animations();
    }

    /// Deals a new game from `seed` under the current options, playing
    /// the deal animation. The host supplies the seed — a random one for
    /// "Deal", the player's for "Select Game…" — because the presenter
    /// has no entropy source of its own.
    pub fn deal_new(&mut self, seed: Seed) {
        self.drag = None;
        self.last_press = None;
        self.session.new_game(seed);
        self.fan = 0;
        self.clock_ms = 0;
        self.phase = Phase::Dealing(DealAnimation::new());
    }

    /// Applies an engine command (the programmatic mirror of the pointer
    /// paths). Player commands skip animations first; ticks pass through
    /// untouched so a host driving time via [`Command::Tick`] does not
    /// constantly interrupt them.
    ///
    /// # Errors
    ///
    /// The engine's [`RuleError`], unchanged; the game is unchanged.
    pub fn apply(&mut self, command: Command) -> Result<(), RuleError> {
        if !matches!(command, Command::Tick { .. }) {
            self.skip_animations();
            self.drag = None;
        }
        let events = self.session.apply(command)?;
        self.command_applied(&events);
        Ok(())
    }

    /// Takes back the most recent player command.
    ///
    /// # Errors
    ///
    /// The engine's [`RuleError`], unchanged — undo is rejected in Vegas
    /// scoring and when nothing is logged.
    pub fn undo(&mut self) -> Result<(), RuleError> {
        self.skip_animations();
        self.drag = None;
        self.session.undo()?;
        self.fan = fan_len(self.session.game().log());
        Ok(())
    }

    /// Re-applies the most recently undone command.
    ///
    /// # Errors
    ///
    /// The engine's [`RuleError`], unchanged — redo is rejected in Vegas
    /// scoring and when nothing was undone.
    pub fn redo(&mut self) -> Result<(), RuleError> {
        self.skip_animations();
        self.drag = None;
        let events = self.session.redo()?;
        self.command_applied(&events);
        Ok(())
    }

    /// The current player options.
    #[must_use]
    pub const fn options(&self) -> &Options {
        self.session.options()
    }

    /// Stores new options. Rule options take effect at the next deal;
    /// outline dragging and sounds apply immediately.
    pub fn set_options(&mut self, options: Options) {
        self.session.set_options(options);
    }

    /// Serializes the session as save-format bytes.
    ///
    /// # Errors
    ///
    /// A [`SaveError`] from the session's serializer.
    pub fn save_bytes(&self) -> Result<Vec<u8>, SaveError> {
        self.session.to_save_bytes()
    }

    /// Replaces the session from save-format bytes and rebuilds the
    /// presentation (waste fan, clock) around it. No deal animation: the
    /// loaded table appears as saved.
    ///
    /// # Errors
    ///
    /// A [`SaveError`] from the session's loader; the presenter is
    /// unchanged.
    pub fn load_bytes(&mut self, bytes: &[u8]) -> Result<(), SaveError> {
        let session = Session::from_save_bytes(bytes)?;
        self.session = session;
        self.drag = None;
        self.last_press = None;
        self.fan = fan_len(self.session.game().log());
        self.clock_ms = u64::from(self.session.elapsed_secs()).saturating_mul(1000);
        self.phase = Phase::Idle;
        Ok(())
    }

    /// The owned session, read-only — for status displays (bankroll,
    /// elapsed time) and for the host's autosave via `sol-session`'s
    /// storage layer.
    #[must_use]
    pub const fn session(&self) -> &Session {
        &self.session
    }

    /// The running game's seed, for the status bar and "Select Game…".
    #[must_use]
    pub fn seed(&self) -> Seed {
        self.session.game().seed()
    }

    /// The current score (points, or Vegas dollars).
    #[must_use]
    pub fn score(&self) -> i32 {
        self.session.game().state().score()
    }

    /// Total elapsed play seconds, from the session clock.
    #[must_use]
    pub const fn elapsed_secs(&self) -> u32 {
        self.session.elapsed_secs()
    }

    /// Whether undo is currently available (for menu enabling).
    #[must_use]
    pub fn can_undo(&self) -> bool {
        self.session.game().can_undo()
    }

    /// Whether redo is currently available (for menu enabling).
    #[must_use]
    pub fn can_redo(&self) -> bool {
        self.session.game().can_redo()
    }

    /// Whether the game has been won.
    #[must_use]
    pub fn is_won(&self) -> bool {
        self.session.game().state().is_won()
    }

    /// How many backs the active theme declares.
    #[must_use]
    pub fn back_count(&self) -> usize {
        self.profile.backs.len()
    }

    /// The selected back's index (declaration order).
    #[must_use]
    pub const fn back_index(&self) -> usize {
        self.back_index
    }

    /// Selects the card back to show, by declaration order.
    ///
    /// # Errors
    ///
    /// [`PresenterError::UnknownBack`] if the theme has no such back; the
    /// selection is unchanged.
    pub fn set_back(&mut self, index: usize) -> Result<(), PresenterError> {
        if index >= self.profile.backs.len() {
            return Err(PresenterError::UnknownBack {
                index,
                count: self.profile.backs.len(),
            });
        }
        self.back_index = index;
        Ok(())
    }

    /// Every back the active theme declares, laid out as one contact
    /// sheet a frontend draws once and cuts apart into thumbnails: see
    /// [`BackSheet::build`] for the packing rules. `background` becomes
    /// the sheet's clear color; `max_side` bounds it on both axes.
    /// `None` when no such sheet exists.
    #[must_use]
    pub fn back_sheet(&self, background: Rgba, max_side: u32) -> Option<BackSheet> {
        BackSheet::build(
            &self.profile.backs,
            self.layout.card_base(),
            max_side,
            background,
        )
    }

    /// Which frame back `back` shows at the current clock reading — the
    /// same law [`Presenter::frame`] draws the board's own backs by, so
    /// a [`Presenter::back_sheet`] thumbnail and the card on the table
    /// are never a frame apart. `0` for an index the active theme does
    /// not declare.
    #[must_use]
    pub fn back_frame(&self, back: usize) -> u32 {
        self.profile
            .backs
            .get(back)
            .map_or(0, |meta| backs::frame_index(meta, self.clock_ms))
    }

    /// How many frames back `back` has: 1 for a static back, 0 for an
    /// index the active theme does not declare.
    #[must_use]
    pub fn back_frame_count(&self, back: usize) -> u32 {
        self.profile.backs.get(back).map_or(0, |meta| meta.frames)
    }

    /// The current waste-fan length (test observability).
    #[cfg(test)]
    pub(crate) const fn fan(&self) -> usize {
        self.fan
    }

    /// How many more `advance` ticks the post-resize board repaint is
    /// owed for (test observability).
    #[cfg(test)]
    pub(crate) const fn resize_repaint(&self) -> u8 {
        self.resize_repaint
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]

    use sol_engine::{FOUNDATION_COUNT, Rank, RuleError, Suit};
    use sol_theme::{CardSize, Theme};

    use super::*;
    use crate::deal_anim::DEAL_FLIGHT_MS;
    use crate::display::{DisplayList, PlaceholderSlot, Rgba, TextureId};
    use crate::geometry::Rect;
    use crate::testkit::{test_theme, test_theme_at, test_theme_image_bg, test_theme_single_back};
    use crate::testkit_engine::{almost_won_session, options as engine_test_options};

    fn theme() -> Theme {
        test_theme()
    }

    /// The deal these tests are written against, shared with the engine's
    /// rules suites: an ace face-up on tableau 0, a second on tableau 5.
    const SEED: u16 = 8622;

    /// A presenter over a fresh [`SEED`] session (Draw Three, Standard,
    /// timed — the Win98 defaults).
    fn fresh() -> Presenter {
        Presenter::new(
            Session::new(Options::default(), Seed::new(SEED).unwrap()),
            &theme(),
        )
    }

    /// [`fresh`], with the deal animation already skipped.
    fn settled() -> Presenter {
        let mut presenter = fresh();
        presenter.key_down();
        assert!(!presenter.is_animating());
        presenter
    }

    const ALL_SLOTS: [&str; 3] = ["empty_pile", "stock_recycle", "stock_blocked"];

    /// A presenter over a fresh session whose theme declares `slots`, with
    /// the deal animation already skipped.
    fn settled_with(slots: &[&str], options: Options) -> Presenter {
        let mut presenter = Presenter::new(
            Session::new(options, Seed::new(SEED).unwrap()),
            &crate::testkit::test_theme_with_placeholders(slots),
        );
        presenter.key_down();
        presenter
    }

    /// Every destination rectangle drawn for `slot`.
    fn placeholder_dsts(list: &DisplayList, slot: PlaceholderSlot) -> Vec<Rect> {
        list.sprites
            .iter()
            .filter(
                |sprite| matches!(sprite.texture, TextureId::Placeholder { slot: s } if s == slot),
            )
            .map(|sprite| sprite.dst)
            .collect()
    }

    /// Draws until the stock is empty.
    fn drain_stock(presenter: &mut Presenter) {
        while !presenter.session().game().state().stock().is_empty() {
            presenter.apply(Command::Draw).unwrap();
        }
    }

    #[test]
    fn a_fresh_deal_ghosts_its_four_empty_foundations() {
        let presenter = settled_with(&ALL_SLOTS, Options::default());
        let list = presenter.frame();
        let ghosts = placeholder_dsts(&list, PlaceholderSlot::EmptyPile);

        let expected: Vec<Rect> = (0..FOUNDATION_COUNT)
            .map(|index| {
                let pos = presenter
                    .layout()
                    .pile_origin(PileId::Foundation(index))
                    .unwrap();
                let card = presenter.layout().card();
                Rect::new(pos.x, pos.y, card.w, card.h)
            })
            .collect();
        assert_eq!(ghosts, expected);
    }

    /// A fresh deal fills every column and leaves a full stock, so neither
    /// a column ghost nor a stock indicator belongs on screen.
    fn stock_slots(list: &DisplayList) -> usize {
        placeholder_dsts(list, PlaceholderSlot::StockRecycle).len()
            + placeholder_dsts(list, PlaceholderSlot::StockBlocked).len()
    }

    #[test]
    fn a_fresh_deal_draws_no_stock_indicator_and_no_column_ghost() {
        let presenter = settled_with(&ALL_SLOTS, Options::default());
        let list = presenter.frame();
        assert_eq!(stock_slots(&list), 0);
        // Four ghosts exactly — the foundations, no tableau column.
        assert_eq!(
            placeholder_dsts(&list, PlaceholderSlot::EmptyPile).len(),
            usize::from(FOUNDATION_COUNT)
        );
    }

    #[test]
    fn an_empty_stock_that_can_still_recycle_shows_the_ring() {
        let mut presenter = settled_with(&ALL_SLOTS, Options::default());
        drain_stock(&mut presenter);
        let list = presenter.frame();

        let pos = presenter.layout().pile_origin(PileId::Stock).unwrap();
        let card = presenter.layout().card();
        assert_eq!(
            placeholder_dsts(&list, PlaceholderSlot::StockRecycle),
            vec![Rect::new(pos.x, pos.y, card.w, card.h)]
        );
        assert!(placeholder_dsts(&list, PlaceholderSlot::StockBlocked).is_empty());
    }

    /// Vegas draw-one allows a single pass, so the very first recycle is
    /// already refused — the cross, not the ring.
    #[test]
    fn an_empty_stock_with_no_pass_left_shows_the_cross() {
        let options = Options {
            draw_mode: sol_engine::DrawMode::One,
            scoring: sol_engine::ScoringMode::Vegas,
            ..Options::default()
        };
        let mut presenter = settled_with(&ALL_SLOTS, options);
        drain_stock(&mut presenter);
        assert!(presenter.apply(Command::Draw).is_err(), "no pass remains");

        let list = presenter.frame();
        assert_eq!(
            placeholder_dsts(&list, PlaceholderSlot::StockBlocked).len(),
            1
        );
        assert!(placeholder_dsts(&list, PlaceholderSlot::StockRecycle).is_empty());
    }

    /// The two stock states are mutually exclusive: whichever is drawn,
    /// exactly one indicator occupies the slot.
    #[test]
    fn an_empty_stock_shows_exactly_one_indicator() {
        let mut presenter = settled_with(&ALL_SLOTS, Options::default());
        drain_stock(&mut presenter);
        assert_eq!(stock_slots(&presenter.frame()), 1);
    }

    /// A theme that declares nothing draws nothing, which is what every
    /// theme predating the section gets.
    #[test]
    fn a_theme_without_placeholders_draws_none_of_them() {
        let mut presenter = settled_with(&[], Options::default());
        drain_stock(&mut presenter);
        let list = presenter.frame();
        assert!(
            !list
                .sprites
                .iter()
                .any(|sprite| matches!(sprite.texture, TextureId::Placeholder { .. }))
        );
    }

    /// Declaring one slot must not conjure the others.
    #[test]
    fn only_declared_slots_are_drawn() {
        let mut presenter = settled_with(&["stock_recycle"], Options::default());
        drain_stock(&mut presenter);
        let list = presenter.frame();
        assert_eq!(
            placeholder_dsts(&list, PlaceholderSlot::StockRecycle).len(),
            1
        );
        assert!(placeholder_dsts(&list, PlaceholderSlot::EmptyPile).is_empty());
    }

    /// The ghost tracks what the frame draws, not what the state holds: a
    /// foundation emptied by lifting its card shows its ghost again while
    /// that card is still in hand.
    #[test]
    fn a_foundation_emptied_by_a_drag_ghosts_under_the_held_card() {
        let mut presenter = settled_with(&ALL_SLOTS, Options::default());
        let slot = {
            let origin = presenter
                .layout()
                .pile_origin(PileId::Foundation(0))
                .unwrap();
            let card = presenter.layout().card();
            Rect::new(origin.x, origin.y, card.w, card.h)
        };
        // Column 0's lone card is an ace, so foundation 0 fills and its
        // ghost goes away.
        presenter
            .apply(Command::AutoToFoundation {
                pile: PileId::Tableau(0),
            })
            .unwrap();
        assert!(!placeholder_dsts(&presenter.frame(), PlaceholderSlot::EmptyPile).contains(&slot));

        presenter.pointer_down(Pt::new(slot.x + 2, slot.y + 2));
        presenter.pointer_move(Pt::new(slot.x + 40, slot.y + 140));
        assert!(
            placeholder_dsts(&presenter.frame(), PlaceholderSlot::EmptyPile).contains(&slot),
            "the emptied foundation must show its ghost"
        );
    }

    /// The original leaves an emptied column as bare table — no ghost
    /// marks the slot a king may be dropped into.
    #[test]
    fn an_emptied_column_stays_bare() {
        let mut presenter = settled_with(&ALL_SLOTS, Options::default());
        let slot = {
            let origin = presenter.layout().pile_origin(PileId::Tableau(0)).unwrap();
            let card = presenter.layout().card();
            Rect::new(origin.x, origin.y, card.w, card.h)
        };
        // Column 0 holds exactly one (face-up) card after the deal; send
        // it to a foundation to empty the column for good.
        presenter
            .apply(Command::AutoToFoundation {
                pile: PileId::Tableau(0),
            })
            .unwrap();
        assert!(
            presenter
                .session()
                .game()
                .state()
                .tableaus()
                .next()
                .unwrap()
                .is_empty()
        );
        assert!(!placeholder_dsts(&presenter.frame(), PlaceholderSlot::EmptyPile).contains(&slot));
    }

    /// Placeholders belong under the cards: a card landing on a ghosted
    /// pile must cover it, never the other way round.
    #[test]
    fn placeholders_sort_behind_every_card() {
        let presenter = settled_with(&ALL_SLOTS, Options::default());
        let list = presenter.frame();
        let last_placeholder = list
            .sprites
            .iter()
            .filter(|sprite| matches!(sprite.texture, TextureId::Placeholder { .. }))
            .map(|sprite| sprite.z)
            .max()
            .unwrap();
        let first_card = list
            .sprites
            .iter()
            .filter(|sprite| {
                matches!(
                    sprite.texture,
                    TextureId::Face { .. } | TextureId::Back { .. }
                )
            })
            .map(|sprite| sprite.z)
            .min()
            .unwrap();
        assert!(
            last_placeholder < first_card,
            "placeholder z {last_placeholder} must precede card z {first_card}"
        );
    }

    fn face_dsts(list: &DisplayList) -> Vec<Rect> {
        list.sprites
            .iter()
            .filter(|sprite| matches!(sprite.texture, TextureId::Face { .. }))
            .map(|sprite| sprite.dst)
            .collect()
    }

    fn white_sprites(list: &DisplayList) -> Vec<crate::display::Sprite> {
        list.sprites
            .iter()
            .filter(|sprite| sprite.texture == TextureId::White)
            .copied()
            .collect()
    }

    fn first_back_sprite(list: &DisplayList) -> crate::display::Sprite {
        *list
            .sprites
            .iter()
            .find(|sprite| matches!(sprite.texture, TextureId::Back { .. }))
            .unwrap()
    }

    #[test]
    fn a_fresh_presenter_plays_the_deal_animation() {
        let presenter = fresh();
        assert!(presenter.is_animating());
        let frame = presenter.frame();
        assert_eq!(frame.clear, Some(Rgba::opaque(0x00, 0x80, 0x00)));
        // 24 stock backs plus the first card in flight, nothing dealt yet.
        assert_eq!(frame.sprites.len(), 25);
    }

    #[test]
    fn the_deal_flight_interpolates_and_lands_in_order() {
        let mut presenter = fresh();
        // Halfway through flight 0: column 0's face-up ace flying from
        // the stock (11, 5) to its slot (11, 107).
        presenter.advance(DEAL_FLIGHT_MS / 2);
        let faces = face_dsts(&presenter.frame());
        assert_eq!(faces, vec![Rect::new(11, 56, 71, 96)]);
        // After it lands, flight 1 is a face-down card: no faces in
        // flight, the ace resting dealt.
        presenter.advance(DEAL_FLIGHT_MS / 2);
        let frame = presenter.frame();
        let faces = face_dsts(&frame);
        assert_eq!(faces, vec![Rect::new(11, 107, 71, 96)]);
        // 24 stock + 1 dealt + 1 flying back.
        assert_eq!(frame.sprites.len(), 26);
        // The whole deal: 24 stock backs + 28 dealt cards, 7 face-up.
        presenter.advance(DEAL_FLIGHT_MS * 28);
        assert!(!presenter.is_animating());
        let frame = presenter.frame();
        assert_eq!(frame.sprites.len(), 52);
        assert_eq!(face_dsts(&frame).len(), 7);
    }

    #[test]
    fn any_input_skips_the_deal() {
        let mut presenter = fresh();
        presenter.pointer_down(Pt::new(400, 350));
        assert!(!presenter.is_animating());
        assert_eq!(presenter.frame().sprites.len(), 52);
    }

    #[test]
    fn a_restored_session_resumes_without_the_deal_animation() {
        let mut donor = settled();
        donor.pointer_down(Pt::new(30, 30));
        donor.pointer_up(Pt::new(30, 30));
        let bytes = donor.save_bytes().unwrap();
        let restored = Session::from_save_bytes(&bytes).unwrap();
        let presenter = Presenter::new(restored, &theme());
        assert!(!presenter.is_animating());
        assert_eq!(presenter.fan(), 3);
    }

    #[test]
    fn advance_drives_the_session_clock_whole_seconds() {
        let mut presenter = settled();
        presenter.advance(999);
        assert_eq!(presenter.elapsed_secs(), 0);
        presenter.advance(1);
        assert_eq!(presenter.elapsed_secs(), 1);
        presenter.advance(2000);
        assert_eq!(presenter.elapsed_secs(), 3);
    }

    #[test]
    fn the_clock_freezes_while_dragging() {
        let mut presenter = settled();
        presenter.pointer_down(Pt::new(20, 120));
        presenter.advance(5000);
        assert_eq!(presenter.elapsed_secs(), 0, "dragging gates the timer");
        presenter.pointer_up(Pt::new(20, 120));
        presenter.advance(1000);
        assert_eq!(presenter.elapsed_secs(), 1);
    }

    #[test]
    fn clicking_the_stock_draws_a_fanned_three() {
        let mut presenter = settled();
        presenter.pointer_down(Pt::new(30, 30));
        presenter.pointer_up(Pt::new(30, 30));
        assert_eq!(presenter.session().game().state().waste().len(), 3);
        assert_eq!(presenter.fan(), 3);
        let faces = face_dsts(&presenter.frame());
        assert!(faces.contains(&Rect::new(93, 5, 71, 96)));
        assert!(faces.contains(&Rect::new(107, 6, 71, 96)));
        assert!(faces.contains(&Rect::new(121, 7, 71, 96)));
    }

    #[test]
    fn clicking_the_empty_stock_recycles_the_waste() {
        let mut presenter = settled();
        for _ in 0..8 {
            presenter.pointer_down(Pt::new(30, 30));
            presenter.pointer_up(Pt::new(30, 30));
        }
        let state = presenter.session().game().state();
        assert!(state.stock().is_empty());
        assert_eq!(state.waste().len(), 24);
        presenter.pointer_down(Pt::new(30, 30));
        let state = presenter.session().game().state();
        assert_eq!(state.stock().len(), 24);
        assert!(state.waste().is_empty());
        assert_eq!(state.passes_completed(), 1);
        assert_eq!(presenter.fan(), 0);
    }

    #[test]
    fn double_clicking_the_ace_sends_it_to_a_foundation() {
        let mut presenter = settled();
        presenter.pointer_down(Pt::new(20, 120));
        presenter.pointer_up(Pt::new(20, 120));
        presenter.pointer_down(Pt::new(20, 120));
        let state = presenter.session().game().state();
        assert_eq!(state.foundation_card_count(), 1);
        assert!(state.tableau(0).unwrap().is_empty());
        assert_eq!(presenter.score(), 10, "tableau → foundation scores +10");
    }

    #[test]
    fn a_slow_second_click_is_not_a_double_click() {
        let mut presenter = settled();
        presenter.pointer_down(Pt::new(20, 120));
        presenter.pointer_up(Pt::new(20, 120));
        presenter.advance(u32::try_from(DOUBLE_CLICK_MS).unwrap() + 1);
        presenter.pointer_down(Pt::new(20, 120));
        presenter.pointer_up(Pt::new(20, 120));
        let state = presenter.session().game().state();
        assert_eq!(state.foundation_card_count(), 0);
    }

    #[test]
    fn double_clicks_only_fire_on_waste_and_tableau_tops() {
        let mut presenter = settled();
        // Put the ace on a foundation, then double-click it there:
        // foundations never answer double-clicks, and the press instead
        // picks the card up (and drops it back).
        presenter
            .apply(Command::AutoToFoundation {
                pile: PileId::Tableau(0),
            })
            .unwrap();
        presenter.pointer_down(Pt::new(270, 30));
        presenter.pointer_up(Pt::new(270, 30));
        presenter.pointer_down(Pt::new(270, 30));
        presenter.pointer_up(Pt::new(270, 30));
        let state = presenter.session().game().state();
        assert_eq!(state.foundation_card_count(), 1, "the ace stays put");
    }

    #[test]
    fn dragging_a_legal_move_applies_it_and_auto_flips() {
        let mut presenter = settled();
        presenter.pointer_down(Pt::new(510, 130));
        presenter.pointer_move(Pt::new(190, 130));
        assert!(
            white_sprites(&presenter.frame()).is_empty(),
            "pixel-mode dragging never highlights the target"
        );
        presenter.pointer_up(Pt::new(190, 130));
        let state = presenter.session().game().state();
        let t2 = state.tableau(2).unwrap();
        assert_eq!(t2.face_up().len(), 2);
        assert_eq!(
            t2.face_up()[1],
            sol_engine::Card::new(Suit::Spades, Rank::Three)
        );
        let t6 = state.tableau(6).unwrap();
        assert_eq!(t6.face_down().len(), 5, "S10 auto-flipped");
        assert_eq!(t6.face_up().len(), 1);
        assert_eq!(presenter.score(), 5, "the flip scores +5");
        assert!(presenter.can_undo());
    }

    #[test]
    fn the_dragged_card_rides_the_pointer_and_hides_at_home() {
        let mut presenter = settled();
        presenter.pointer_down(Pt::new(100, 120));
        presenter.pointer_move(Pt::new(300, 250));
        let frame = presenter.frame();
        let faces = face_dsts(&frame);
        assert!(
            !faces.contains(&Rect::new(93, 110, 71, 96)),
            "D7 is hidden at its home slot"
        );
        let riding = frame
            .sprites
            .iter()
            .find(|s| s.dst == Rect::new(293, 240, 71, 96))
            .unwrap();
        assert_eq!(
            riding.texture,
            TextureId::Face {
                suit: Suit::Diamonds,
                rank: Rank::Seven
            },
            "the ridden sprite is the picked card itself"
        );
        assert!(
            white_sprites(&frame).is_empty(),
            "pixel-mode dragging draws no outline and no highlight"
        );
        presenter.pointer_up(Pt::new(300, 250));
    }

    #[test]
    fn a_dragged_two_card_run_stacks_its_own_cards() {
        let mut presenter = settled();
        presenter
            .apply(Command::MoveCards {
                from: PileId::Tableau(6),
                to: PileId::Tableau(2),
                count: 1,
            })
            .unwrap();
        // Grab H4 (with S3 riding on it) and hold it over open felt.
        presenter.pointer_down(Pt::new(182, 118));
        presenter.pointer_move(Pt::new(300, 300));
        let frame = presenter.frame();
        let texture_at = |dst: Rect| {
            frame
                .sprites
                .iter()
                .find(|s| s.dst == dst)
                .map(|s| s.texture)
        };
        assert_eq!(
            texture_at(Rect::new(293, 295, 71, 96)),
            Some(TextureId::Face {
                suit: Suit::Hearts,
                rank: Rank::Four
            })
        );
        assert_eq!(
            texture_at(Rect::new(293, 310, 71, 96)),
            Some(TextureId::Face {
                suit: Suit::Spades,
                rank: Rank::Three
            })
        );
        presenter.pointer_up(Pt::new(300, 300));
    }

    #[test]
    fn an_illegal_drop_snaps_back_home() {
        let mut presenter = settled();
        presenter.pointer_down(Pt::new(100, 120));
        presenter.pointer_move(Pt::new(300, 300));
        presenter.pointer_up(Pt::new(300, 300));
        assert!(presenter.is_animating(), "snap-back runs");
        // One step in, the card sits exactly 36 px along the line home:
        // from (293, 290) toward (93, 110), length 200 → (257, 258).
        presenter.advance(crate::drag::SNAP_STEP_MS);
        let faces = face_dsts(&presenter.frame());
        assert!(!faces.contains(&Rect::new(93, 110, 71, 96)));
        assert!(
            faces.contains(&Rect::new(257, 258, 71, 96)),
            "the run is drawn mid-slide"
        );
        // The run lands; the board is unchanged.
        presenter.advance(1000);
        assert!(!presenter.is_animating());
        let faces = face_dsts(&presenter.frame());
        assert!(faces.contains(&Rect::new(93, 110, 71, 96)));
        let state = presenter.session().game().state();
        assert_eq!(state.tableau(1).unwrap().face_up().len(), 1);
    }

    #[test]
    fn outline_dragging_draws_outline_and_target_highlight() {
        let mut presenter = settled();
        let options = Options {
            outline_dragging: true,
            ..presenter.options().clone()
        };
        presenter.set_options(options);
        presenter.pointer_down(Pt::new(510, 130));
        // Over the legal target: 4 highlight edges + 4 run edges, no
        // separators for a single card — all tinted the theme outline
        // color, and no face rides the pointer.
        presenter.pointer_move(Pt::new(190, 130));
        let frame = presenter.frame();
        let whites = white_sprites(&frame);
        assert_eq!(whites.len(), 8);
        assert!(whites.iter().all(|s| s.tint == Rgba::opaque(0, 0, 0)));
        assert!(!face_dsts(&frame).contains(&Rect::new(183, 125, 71, 96)));
        // Away from any target: only the 4 run edges, hugging the
        // single-card run rect (293, 295, 71, 96) at one pixel thick.
        presenter.pointer_move(Pt::new(300, 300));
        let whites: Vec<Rect> = white_sprites(&presenter.frame())
            .iter()
            .map(|s| s.dst)
            .collect();
        assert_eq!(whites.len(), 4);
        assert!(whites.contains(&Rect::new(293, 295, 71, 1)), "top edge");
        assert!(whites.contains(&Rect::new(293, 390, 71, 1)), "bottom edge");
        assert!(whites.contains(&Rect::new(293, 296, 1, 94)), "left edge");
        assert!(whites.contains(&Rect::new(363, 296, 1, 94)), "right edge");
        presenter.pointer_up(Pt::new(300, 300));
    }

    #[test]
    fn a_two_card_outline_run_draws_a_separator() {
        let mut presenter = settled();
        presenter
            .apply(Command::MoveCards {
                from: PileId::Tableau(6),
                to: PileId::Tableau(2),
                count: 1,
            })
            .unwrap();
        let options = Options {
            outline_dragging: true,
            ..presenter.options().clone()
        };
        presenter.set_options(options);
        // Grab H4 with S3 on it: a 2-card run.
        presenter.pointer_down(Pt::new(182, 118));
        presenter.pointer_move(Pt::new(300, 300));
        let whites = white_sprites(&presenter.frame());
        assert_eq!(whites.len(), 5, "4 edges + 1 separator");
        let dsts: Vec<Rect> = whites.iter().map(|s| s.dst).collect();
        assert!(
            dsts.contains(&Rect::new(293, 310, 71, 1)),
            "separator at the card boundary"
        );
        // Run height 1·15 + 96 = 111: the bottom edge pins it.
        assert!(dsts.contains(&Rect::new(293, 295, 71, 1)), "top edge");
        assert!(
            dsts.contains(&Rect::new(293, 405, 71, 1)),
            "bottom edge at the two-card run height"
        );
        presenter.pointer_up(Pt::new(300, 300));
    }

    #[test]
    fn a_quick_press_on_another_card_of_the_same_pile_is_no_double_click() {
        let mut presenter = settled();
        presenter
            .apply(Command::MoveCards {
                from: PileId::Tableau(6),
                to: PileId::Tableau(2),
                count: 1,
            })
            .unwrap();
        // Click H4's exposed sliver (column 2, index 2), then immediately
        // the pile top S3 (index 3): same pile, different card — a normal
        // pickup of S3, never a double-click on the top.
        presenter.pointer_down(Pt::new(182, 118));
        presenter.pointer_up(Pt::new(182, 118));
        presenter.pointer_down(Pt::new(182, 133));
        presenter.pointer_move(Pt::new(300, 300));
        let faces = face_dsts(&presenter.frame());
        assert!(
            faces.contains(&Rect::new(293, 295, 71, 96)),
            "S3 rides the pointer: the press was a pickup"
        );
        presenter.pointer_up(Pt::new(300, 300));
    }

    #[test]
    fn a_second_click_exactly_at_the_window_still_double_clicks() {
        let mut presenter = settled();
        presenter.pointer_down(Pt::new(20, 120));
        presenter.pointer_up(Pt::new(20, 120));
        presenter.advance(u32::try_from(DOUBLE_CLICK_MS).unwrap());
        presenter.pointer_down(Pt::new(20, 120));
        assert_eq!(
            presenter.session().game().state().foundation_card_count(),
            1,
            "exactly the double-click time still counts"
        );
    }

    #[test]
    fn the_winning_move_starts_the_cascade() {
        let (session, winning) = almost_won_session();
        let mut presenter = Presenter::new(session, &theme());
        assert!(!presenter.is_animating());
        presenter.apply(winning).unwrap();
        assert!(presenter.is_won());
        assert!(presenter.is_cascade_running());
        // Before the first physics step the frame still shows the full
        // won board over a normal clear.
        let frame = presenter.frame();
        assert!(frame.clear.is_some());
        assert_eq!(face_dsts(&frame).len(), 52);
        // Each advance yields only the stepped trail, unclear.
        presenter.advance(crate::cascade::CASCADE_STEP_MS);
        let frame = presenter.frame();
        assert_eq!(frame.clear, None);
        assert_eq!(frame.sprites.len(), 1);
        // The session clock never ticks a won game.
        presenter.advance(5000);
        assert_eq!(presenter.elapsed_secs(), 0);
    }

    #[test]
    fn a_resize_mid_cascade_repaints_the_board_before_the_next_smear() {
        let (session, winning) = almost_won_session();
        let mut presenter = Presenter::new(session, &theme());
        assert_eq!(
            presenter.resize_repaint(),
            0,
            "a fresh presenter owes nothing"
        );
        presenter.apply(winning).unwrap();
        presenter.advance(crate::cascade::CASCADE_STEP_MS);
        assert!(presenter.is_cascade_running());
        // The smear frame right before the resize is exactly the
        // cascade's one pending sprite; keep it so the post-resize frame
        // can be shown to still contain that very sprite — identity and
        // rect, not just some face — on top of the repainted board.
        let before = presenter.frame();
        assert_eq!(before.clear, None);
        assert_eq!(before.sprites.len(), 1);
        let pending_sprite = before.sprites[0];

        presenter.fit_viewport(800, 600);
        assert_eq!(
            presenter.resize_repaint(),
            2,
            "the resize arms the countdown to its full value"
        );
        let frame = presenter.frame();
        assert!(frame.clear.is_some(), "the repaint frame clears normally");
        assert_eq!(
            face_dsts(&frame).len(),
            53,
            "the 52 board faces plus the cascade's one flying duplicate"
        );
        assert!(
            frame
                .sprites
                .iter()
                .any(|s| s.texture == pending_sprite.texture && s.dst == pending_sprite.dst),
            "the cascade's pending face is drawn on top of the repainted board"
        );
    }

    #[test]
    fn the_repaint_outlives_the_advance_then_frame_host_shape() {
        // sol-shell and sol-win32 always `advance` before they `frame`,
        // so their first post-resize frame follows a tick. The repaint
        // must still be owed on that frame — a one-shot latch cleared in
        // `advance` would already be gone, which is the case review found
        // unreachable — and give way to the smear only after the tick
        // that follows it.
        let (session, winning) = almost_won_session();
        let mut presenter = Presenter::new(session, &theme());
        presenter.apply(winning).unwrap();
        presenter.advance(crate::cascade::CASCADE_STEP_MS);
        assert!(presenter.is_cascade_running());
        presenter.fit_viewport(800, 600);

        // First post-resize tick: the countdown is only spent down to
        // one, so this frame is STILL self-contained — the board repaints
        // with the flying card on top rather than smearing onto a blank
        // target.
        presenter.advance(crate::cascade::CASCADE_STEP_MS);
        assert_eq!(presenter.resize_repaint(), 1);
        let frame = presenter.frame();
        assert!(
            frame.clear.is_some(),
            "the repaint survives the advance-then-frame tick"
        );
        assert_eq!(
            face_dsts(&frame).len(),
            53,
            "board plus the cascade's flying card"
        );

        // Second tick: the countdown reaches zero, so the smear resumes.
        presenter.advance(crate::cascade::CASCADE_STEP_MS);
        assert_eq!(presenter.resize_repaint(), 0);
        assert_eq!(
            presenter.frame().clear,
            None,
            "the smear resumes only after the second advance"
        );
    }

    #[test]
    fn fit_viewport_without_a_cascade_leaves_frames_unaffected() {
        let mut presenter = settled();
        presenter.fit_viewport(800, 600);
        assert_eq!(
            presenter.resize_repaint(),
            2,
            "the countdown arms unconditionally even with no cascade running"
        );
        let frame = presenter.frame();
        assert!(
            frame.clear.is_some(),
            "non-cascade frames are always self-contained regardless of the countdown"
        );
        assert_eq!(frame.sprites.len(), 52);

        // The countdown drains over its two ticks with nothing observable
        // changing, since a non-cascade frame is self-contained either
        // way.
        presenter.advance(16);
        assert_eq!(presenter.resize_repaint(), 1);
        assert_eq!(presenter.frame().sprites.len(), 52);
        presenter.advance(16);
        assert_eq!(presenter.resize_repaint(), 0);
        let frame = presenter.frame();
        assert!(frame.clear.is_some());
        assert_eq!(
            frame.sprites.len(),
            52,
            "draining the countdown changed nothing observable"
        );
    }

    #[test]
    fn two_resizes_between_ticks_re_arm_the_countdown_in_full() {
        let (session, winning) = almost_won_session();
        let mut presenter = Presenter::new(session, &theme());
        presenter.apply(winning).unwrap();
        presenter.advance(crate::cascade::CASCADE_STEP_MS);
        presenter.fit_viewport(800, 600);
        presenter.fit_viewport(700, 500);
        assert_eq!(
            presenter.resize_repaint(),
            2,
            "a second resize re-arms to the full value, it does not accumulate"
        );
        assert!(
            presenter.frame().clear.is_some(),
            "still self-contained after two resizes"
        );
        // Draining takes the whole countdown no matter how many resizes
        // re-armed it: still owed after one tick, spent after the second.
        presenter.advance(crate::cascade::CASCADE_STEP_MS);
        assert_eq!(presenter.resize_repaint(), 1);
        assert!(
            presenter.frame().clear.is_some(),
            "still owed after one tick"
        );
        presenter.advance(crate::cascade::CASCADE_STEP_MS);
        assert_eq!(presenter.resize_repaint(), 0);
        assert_eq!(
            presenter.frame().clear,
            None,
            "the smear resumes once the countdown is spent"
        );
    }

    #[test]
    fn any_input_skips_the_cascade_and_repaints() {
        let (session, winning) = almost_won_session();
        let mut presenter = Presenter::new(session, &theme());
        presenter.apply(winning).unwrap();
        presenter.advance(crate::cascade::CASCADE_STEP_MS * 5);
        assert!(presenter.is_cascade_running());
        presenter.pointer_down(Pt::new(5, 380));
        assert!(!presenter.is_cascade_running());
        let frame = presenter.frame();
        assert!(frame.clear.is_some());
        assert_eq!(face_dsts(&frame).len(), 52);
    }

    #[test]
    fn undo_then_redo_rewins_and_recascades() {
        let (session, winning) = almost_won_session();
        let mut presenter = Presenter::new(session, &theme());
        presenter.apply(winning).unwrap();
        presenter.undo().unwrap();
        assert!(!presenter.is_won());
        assert!(!presenter.is_cascade_running());
        presenter.redo().unwrap();
        assert!(presenter.is_won());
        assert!(presenter.is_cascade_running());
    }

    #[test]
    fn save_and_load_round_trip_restores_the_presentation() {
        let mut presenter = settled();
        presenter.pointer_down(Pt::new(30, 30));
        presenter.pointer_up(Pt::new(30, 30));
        presenter.advance(3000);
        let bytes = presenter.save_bytes().unwrap();

        let mut other = Presenter::new(
            Session::new(engine_test_options(), Seed::new(9).unwrap()),
            &theme(),
        );
        other.load_bytes(&bytes).unwrap();
        assert_eq!(other.seed().get(), SEED);
        assert_eq!(other.fan(), 3);
        assert_eq!(other.elapsed_secs(), 3);
        assert!(!other.is_animating());
        assert_eq!(other.frame(), presenter.frame());
    }

    #[test]
    fn loading_garbage_fails_and_changes_nothing() {
        let mut presenter = settled();
        let before = presenter.frame();
        assert!(presenter.load_bytes(b"not a save").is_err());
        assert_eq!(presenter.seed().get(), SEED);
        assert_eq!(presenter.frame(), before);
    }

    #[test]
    fn back_selection_is_validated_and_animates() {
        let mut presenter = settled();
        assert_eq!(presenter.back_count(), 4);
        assert_eq!(presenter.back_index(), 0);
        // The default static back never changes frames.
        presenter.advance(5000);
        let back = first_back_sprite(&presenter.frame());
        assert_eq!(back.texture, TextureId::Back { back: 0, asset: 0 });
        assert_eq!(back.src, Rect::new(0, 0, 71, 96));

        // The 2 fps strip advances every 500 ms; clock is at 5000.
        presenter.set_back(1).unwrap();
        presenter.advance(500);
        let back = first_back_sprite(&presenter.frame());
        assert_eq!(back.texture, TextureId::Back { back: 1, asset: 0 });
        assert_eq!(back.src, Rect::new(71, 0, 71, 96));

        // The list-form back switches whole assets: clock 5500 → cycle
        // position 500 lies in frame 1's 250..1000 hold.
        presenter.set_back(2).unwrap();
        let back = first_back_sprite(&presenter.frame());
        assert_eq!(back.texture, TextureId::Back { back: 2, asset: 1 });
        assert_eq!(back.src, Rect::new(0, 0, 71, 96));

        let error = presenter.set_back(9).unwrap_err();
        assert_eq!(error, PresenterError::UnknownBack { index: 9, count: 4 });
        assert!(error.to_string().contains('9'));
        assert!(error.to_string().contains('4'));
        assert_eq!(presenter.back_index(), 2, "a rejected selection sticks");
    }

    #[test]
    fn back_frame_follows_the_clock_and_back_frame_count_reports_shape() {
        let mut presenter = settled();
        // The 2 fps strip (index 1) ticks over at 500 ms, not before.
        assert_eq!(presenter.back_frame(1), 0);
        presenter.advance(499);
        assert_eq!(presenter.back_frame(1), 0);
        presenter.advance(1);
        assert_eq!(presenter.back_frame(1), 1);
        // The static back (index 0) never moves off frame 0.
        assert_eq!(presenter.back_frame(0), 0);
        // An index the theme does not declare reports 0, not a panic.
        assert_eq!(presenter.back_frame(9), 0);

        assert_eq!(presenter.back_frame_count(0), 1, "static: one frame");
        assert_eq!(
            presenter.back_frame_count(1),
            2,
            "animated: its frame count"
        );
        assert_eq!(presenter.back_frame_count(9), 0, "unknown index");
    }

    #[test]
    fn back_sheet_wraps_the_active_theme_profile_and_base_card_size() {
        let presenter = settled();
        let sheet = presenter.back_sheet(Rgba::opaque(9, 9, 9), 1000).unwrap();
        assert_eq!(sheet.cell, presenter.layout().card_base());
        assert_eq!(sheet.cells.len(), 7, "four backs, seven frames total");
        assert_eq!(sheet.list.clear, Some(Rgba::opaque(9, 9, 9)));
        // A limit narrower than one cell refuses, exactly like `BackSheet::build`.
        assert!(presenter.back_sheet(Rgba::WHITE, 1).is_none());
    }

    #[test]
    fn image_backgrounds_stretch_or_tile() {
        let mut presenter = settled();
        presenter.set_theme(&test_theme_image_bg(false));
        let frame = presenter.frame();
        assert_eq!(frame.clear, Some(Rgba::opaque(0, 0, 0)));
        let backgrounds: Vec<_> = frame
            .sprites
            .iter()
            .filter(|s| s.texture == TextureId::Background)
            .collect();
        assert_eq!(backgrounds.len(), 1);
        assert_eq!(backgrounds[0].dst, Rect::new(0, 0, 585, 384));
        assert_eq!(backgrounds[0].src, Rect::new(0, 0, 100, 50));

        presenter.set_theme(&test_theme_image_bg(true));
        let frame = presenter.frame();
        let tiles: Vec<_> = frame
            .sprites
            .iter()
            .filter(|s| s.texture == TextureId::Background)
            .collect();
        assert_eq!(tiles.len(), 48, "6×8 native-size tiles over 585×384");
        assert_eq!(tiles[0].dst, Rect::new(0, 0, 100, 50));
        assert_eq!(tiles[47].dst, Rect::new(500, 350, 100, 50));

        // Exact-multiple viewports must not spill an extra column/row
        // past the edge: 600 is six whole 100-wide tiles.
        presenter.fit_viewport(1200, 768);
        let count = |presenter: &Presenter| {
            presenter
                .frame()
                .sprites
                .iter()
                .filter(|s| s.texture == TextureId::Background)
                .count()
        };
        assert_eq!(count(&presenter), 48, "6×8 tiles exactly fill 600×384");
        // 400 is eight whole 50-tall rows.
        presenter.fit_viewport(585, 400);
        assert_eq!(count(&presenter), 48, "6×8 tiles exactly fill 585×400");
    }

    #[test]
    fn set_theme_refits_to_the_known_surface() {
        let mut presenter = settled();
        presenter.fit_viewport(1600, 768);
        presenter.set_theme(&test_theme_image_bg(false));
        assert_eq!(presenter.viewport(), Size::new(800, 384));
        assert_eq!(
            presenter.layout().pile_origin(PileId::Stock),
            Some(Pt::new(37, 5)),
            "the column spread survives the theme switch"
        );
    }

    /// `set_theme` relayouts to the new card size. When no surface has been
    /// reported yet there is nothing to re-fit against, but the viewport still
    /// described the *previous* theme — and the public viewport, the
    /// background's tiling bounds and the cascade's exit bounds all read it.
    #[test]
    fn adopting_a_theme_before_any_surface_updates_the_viewport() {
        let big = CardSize {
            width: 142,
            height: 192,
        };
        let mut presenter = fresh();
        presenter.set_theme(&test_theme_at(big));

        assert_eq!(
            presenter.viewport(),
            Layout::min_design(big),
            "the viewport must describe the theme the layout describes"
        );
        assert_eq!(
            presenter.viewport().w,
            presenter.layout().design_size().w,
            "viewport and layout must agree on the board's width"
        );
    }

    #[test]
    fn set_theme_keeps_a_valid_back_selection() {
        let mut presenter = settled();
        presenter.set_back(3).unwrap();
        presenter.set_theme(&test_theme_image_bg(false));
        assert_eq!(presenter.back_index(), 3, "same back count: kept");
    }

    #[test]
    fn a_repeated_fit_keeps_the_drag() {
        // Hosts forward window configures as fit_viewport; a same-size
        // configure (window activation) landing mid-drag must not eat
        // the drag — the drop still lands.
        let mut presenter = settled();
        presenter.fit_viewport(585, 384);
        presenter.pointer_down(Pt::new(510, 130));
        presenter.fit_viewport(585, 384);
        presenter.pointer_move(Pt::new(190, 130));
        presenter.pointer_up(Pt::new(190, 130));
        let state = presenter.session().game().state();
        assert_eq!(
            state.tableau(2).unwrap().face_up().len(),
            2,
            "S3 still lands on H4 after a same-size configure"
        );
    }

    #[test]
    fn a_repeated_fit_keeps_a_snap_back_running() {
        let mut presenter = settled();
        presenter.fit_viewport(585, 384);
        presenter.pointer_down(Pt::new(100, 120));
        presenter.pointer_move(Pt::new(400, 300));
        presenter.pointer_up(Pt::new(400, 300));
        assert!(presenter.is_animating());
        presenter.fit_viewport(585, 384);
        assert!(presenter.is_animating(), "the slide home carries on");
    }

    #[test]
    fn ticks_pass_through_without_skipping_animations() {
        let mut presenter = fresh();
        assert!(presenter.is_animating());
        presenter
            .apply(Command::Tick {
                total_elapsed_secs: 1,
            })
            .unwrap();
        assert!(presenter.is_animating(), "a tick never skips the deal");
        assert_eq!(presenter.elapsed_secs(), 1);
        presenter.apply(Command::Draw).unwrap();
        assert!(!presenter.is_animating(), "a player command does");
    }

    #[test]
    fn deal_new_resets_clock_fan_and_animates() {
        let mut presenter = settled();
        presenter.pointer_down(Pt::new(30, 30));
        presenter.pointer_up(Pt::new(30, 30));
        presenter.advance(2000);
        presenter.deal_new(Seed::new(2).unwrap());
        assert_eq!(presenter.seed().get(), 2);
        assert_eq!(presenter.fan(), 0);
        assert_eq!(presenter.elapsed_secs(), 0);
        assert!(presenter.is_animating());
        assert!(presenter.session().game().log().is_empty());
        assert!(!presenter.can_undo());
        assert!(!presenter.can_redo());
    }

    #[test]
    fn undo_and_redo_surface_engine_errors() {
        let mut presenter = settled();
        assert_eq!(presenter.undo(), Err(RuleError::NothingToUndo));
        assert_eq!(presenter.redo(), Err(RuleError::NothingToRedo));
    }

    #[test]
    fn apply_surfaces_engine_rejections_unchanged() {
        let mut presenter = settled();
        let error = presenter
            .apply(Command::MoveCards {
                from: PileId::Tableau(0),
                to: PileId::Tableau(1),
                count: 1,
            })
            .unwrap_err();
        assert_eq!(error, RuleError::IllegalTableauMove);
    }

    #[test]
    fn pressing_felt_or_a_facedown_card_does_nothing() {
        let mut presenter = settled();
        presenter.pointer_down(Pt::new(400, 370));
        presenter.pointer_up(Pt::new(400, 370));
        // Face-down region of column 6.
        presenter.pointer_down(Pt::new(510, 110));
        presenter.pointer_up(Pt::new(510, 110));
        // Second press while a drag is somehow active is ignored: start a
        // drag, press again, then release cleanly.
        presenter.pointer_down(Pt::new(20, 120));
        presenter.pointer_down(Pt::new(30, 30));
        assert!(
            presenter.session().game().state().waste().is_empty(),
            "the press while dragging is swallowed"
        );
        presenter.pointer_up(Pt::new(20, 120));
        presenter.pointer_move(Pt::new(50, 50));
        presenter.pointer_up(Pt::new(50, 50));
        assert!(presenter.session().game().log().is_empty());
    }

    #[test]
    fn input_during_snap_back_lands_it_instantly() {
        let mut presenter = settled();
        presenter.pointer_down(Pt::new(100, 120));
        presenter.pointer_move(Pt::new(400, 300));
        presenter.pointer_up(Pt::new(400, 300));
        assert!(presenter.is_animating());
        presenter.pointer_down(Pt::new(400, 370));
        assert!(!presenter.is_animating());
        let faces = face_dsts(&presenter.frame());
        assert!(faces.contains(&Rect::new(93, 110, 71, 96)), "H4 back home");
    }

    #[test]
    fn the_viewport_defaults_to_the_design_size_and_follows_the_fit() {
        let mut presenter = settled();
        assert_eq!(presenter.viewport(), Size::new(585, 384));
        presenter.fit_viewport(1600, 768);
        assert_eq!(presenter.viewport(), Size::new(800, 384));
    }

    #[test]
    fn the_cascade_runs_to_completion_and_settles() {
        let (session, winning) = almost_won_session();
        let mut presenter = Presenter::new(session, &theme());
        presenter.apply(winning).unwrap();
        for _ in 0..6_000 {
            presenter.advance(10_000);
            if !presenter.is_cascade_running() {
                break;
            }
        }
        assert!(!presenter.is_cascade_running());
        assert!(!presenter.is_animating());
        let frame = presenter.frame();
        assert!(frame.clear.is_some());
        assert_eq!(face_dsts(&frame).len(), 52);
    }

    #[test]
    fn dragging_the_waste_top_hides_it_at_its_fan_slot() {
        let mut presenter = settled();
        presenter.pointer_down(Pt::new(30, 30));
        presenter.pointer_up(Pt::new(30, 30));
        presenter.pointer_down(Pt::new(130, 20));
        presenter.pointer_move(Pt::new(300, 250));
        let faces = face_dsts(&presenter.frame());
        assert!(
            !faces.contains(&Rect::new(121, 7, 71, 96)),
            "hidden at the fan"
        );
        assert!(
            faces.contains(&Rect::new(93, 5, 71, 96)),
            "the flat waste stays"
        );
        presenter.pointer_up(Pt::new(300, 250));
        assert_eq!(presenter.fan(), 3, "nothing moved");
    }

    #[test]
    fn dragging_a_foundation_top_hides_it_there() {
        let mut presenter = settled();
        presenter
            .apply(Command::AutoToFoundation {
                pile: PileId::Tableau(0),
            })
            .unwrap();
        presenter.pointer_down(Pt::new(270, 30));
        presenter.pointer_move(Pt::new(300, 250));
        let faces = face_dsts(&presenter.frame());
        assert!(
            !faces.contains(&Rect::new(257, 5, 71, 96)),
            "hidden on the foundation"
        );
        presenter.pointer_up(Pt::new(300, 250));
        assert_eq!(
            presenter.session().game().state().foundation_card_count(),
            1,
            "the ace snapped back"
        );
    }

    #[test]
    fn outline_dragging_highlights_an_empty_foundation() {
        let mut presenter = settled();
        let options = Options {
            outline_dragging: true,
            ..presenter.options().clone()
        };
        presenter.set_options(options);
        // Drag the ace over empty foundation 0: the highlight outlines
        // the foundation's base card slot.
        presenter.pointer_down(Pt::new(20, 120));
        presenter.pointer_move(Pt::new(270, 30));
        let whites = white_sprites(&presenter.frame());
        assert_eq!(whites.len(), 8, "4 highlight edges + 4 run edges");
        assert!(
            whites.iter().any(|s| s.dst == Rect::new(257, 5, 71, 1)),
            "the highlight hugs the empty foundation slot"
        );
        presenter.pointer_up(Pt::new(270, 30));
        assert_eq!(
            presenter.session().game().state().foundation_card_count(),
            1,
            "the outlined drop still lands"
        );
    }

    #[test]
    fn switching_to_a_theme_with_fewer_backs_clamps_the_selection() {
        let mut presenter = settled();
        presenter.set_back(3).unwrap();
        presenter.set_theme(&test_theme_single_back());
        assert_eq!(presenter.back_count(), 1);
        assert_eq!(presenter.back_index(), 0);
    }

    #[test]
    fn quick_clicks_on_different_cards_are_not_a_double_click() {
        let mut presenter = settled();
        presenter.pointer_down(Pt::new(20, 120));
        presenter.pointer_up(Pt::new(20, 120));
        // Immediately press a different card: no auto-to-foundation, a
        // normal pickup instead, proven by completing the S3 → H4 move.
        presenter.pointer_down(Pt::new(510, 130));
        presenter.pointer_move(Pt::new(190, 130));
        presenter.pointer_up(Pt::new(190, 130));
        let state = presenter.session().game().state();
        assert_eq!(state.foundation_card_count(), 0);
        assert_eq!(state.tableau(2).unwrap().face_up().len(), 2);
    }

    #[test]
    fn double_clicking_the_waste_top_is_consumed_even_when_rejected() {
        let mut presenter = settled();
        presenter.pointer_down(Pt::new(30, 30));
        presenter.pointer_up(Pt::new(30, 30));
        // This seed's first Draw-Three top is no ace: the auto move is
        // rejected silently and nothing is picked up.
        presenter.pointer_down(Pt::new(130, 20));
        presenter.pointer_up(Pt::new(130, 20));
        presenter.pointer_down(Pt::new(130, 20));
        presenter.pointer_move(Pt::new(300, 250));
        let state = presenter.session().game().state();
        assert_eq!(state.waste().len(), 3);
        assert_eq!(state.foundation_card_count(), 0);
        let faces = face_dsts(&presenter.frame());
        assert!(
            faces.contains(&Rect::new(121, 7, 71, 96)),
            "no drag: the waste top never left its fan slot"
        );
        presenter.pointer_up(Pt::new(300, 250));
    }
}
