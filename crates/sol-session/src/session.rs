//! [`Session`]: the cross-game aggregate — player options, the Vegas
//! [`Bankroll`], the running [`sol_engine::Game`], and the session clock.
//!
//! All mutation goes through `Session`: [`Session::game`] is read-only, so a
//! Vegas bankroll commit can never be bypassed by reaching into the engine
//! directly.

use sol_engine::{Command, Event, Game, LogEntry, RuleError, ScoringMode, Seed};

use crate::bankroll::Bankroll;
use crate::options::Options;
use crate::save::{ENGINE_VERSION, FORMAT_VERSION, SaveError, SaveGame};

/// The cross-game session aggregate: owns the player's
/// [`Options`], the Vegas [`Bankroll`], the running [`Game`], and the
/// session clock. The session clock is the authoritative play timer for
/// every scoring mode — the engine itself tracks time only for timed
/// Standard games.
///
/// ```
/// use sol_engine::{Command, DrawMode, PileId, ScoringMode, Seed};
/// use sol_session::{Options, Session};
///
/// let options = Options {
///     scoring: ScoringMode::Vegas,
///     draw_mode: DrawMode::One,
///     keep_vegas_score: true,
///     ..Options::default()
/// };
/// let mut session = Session::new(options, Seed::new(8622).unwrap());
/// assert_eq!(session.vegas_provisional(), -52, "the Vegas buy-in");
/// assert_eq!(session.bankroll().dollars(), 0, "nothing committed yet");
///
/// // Tableau 0's ace to a foundation: +$5 in Vegas scoring.
/// session.apply(Command::MoveCards {
///     from: PileId::Tableau(0),
///     to: PileId::Foundation(0),
///     count: 1,
/// })?;
/// assert_eq!(session.vegas_provisional(), -47);
///
/// // Redealing commits the outgoing result; keep-score is on, so it
/// // survives into the bankroll.
/// session.new_game(Seed::new(2).unwrap());
/// assert_eq!(session.bankroll().dollars(), -47);
/// assert_eq!(session.vegas_provisional(), -52, "the new deal's buy-in");
/// # Ok::<(), sol_engine::RuleError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    options: Options,
    bankroll: Bankroll,
    game: Game,
    elapsed_secs: u32,
}

impl Session {
    /// Starts a new session: deals immediately from `seed` using
    /// `options.game_config()`, with an empty bankroll and a zeroed clock.
    #[must_use]
    pub fn new(options: Options, seed: Seed) -> Self {
        let game = Game::new(seed, options.game_config());
        Self {
            options,
            bankroll: Bankroll::default(),
            game,
            elapsed_secs: 0,
        }
    }

    /// Rebuilds a session from its canonical parts — the load-path
    /// reconstruction primitive that save/load builds on.
    /// Infallible: rebuilds the game via [`Game::from_log`], and — like
    /// that function — a hand-built log yields whatever state its events
    /// describe. `bankroll` and `elapsed_secs` are taken exactly as given,
    /// with no validation against the log.
    #[must_use]
    pub fn restore(
        options: Options,
        seed: Seed,
        log: Vec<LogEntry>,
        bankroll: Bankroll,
        elapsed_secs: u32,
    ) -> Self {
        let game = Game::from_log(seed, options.game_config(), log);
        Self {
            options,
            bankroll,
            game,
            elapsed_secs,
        }
    }

    /// The current player options.
    #[must_use]
    pub const fn options(&self) -> &Options {
        &self.options
    }

    /// The running game, read-only: every mutation goes through `Session`
    /// so a Vegas bankroll commit can never be bypassed.
    #[must_use]
    pub const fn game(&self) -> &Game {
        &self.game
    }

    /// The committed Vegas bankroll. Does not include the running game's
    /// provisional result — see [`Session::vegas_provisional`] and
    /// [`Session::vegas_total`].
    #[must_use]
    pub const fn bankroll(&self) -> Bankroll {
        self.bankroll
    }

    /// The session clock: total elapsed play seconds, as last recorded by
    /// [`Session::tick`]. Authoritative for every scoring mode — the engine
    /// itself only tracks time for timed Standard games.
    #[must_use]
    pub const fn elapsed_secs(&self) -> u32 {
        self.elapsed_secs
    }

    /// The running game's not-yet-committed Vegas result: `state.score()`
    /// while a Vegas game is in progress and unwon, `0` otherwise —
    /// including every non-Vegas mode and a Vegas game that has already
    /// been won (its result is committed at that point; see
    /// [`Session::apply`]).
    #[must_use]
    pub fn vegas_provisional(&self) -> i64 {
        let state = self.game.state();
        if state.config().scoring == ScoringMode::Vegas && !state.is_won() {
            i64::from(state.score())
        } else {
            0
        }
    }

    /// The bankroll plus the running game's provisional result, saturating
    /// — what the player would end up with if the current game ended right
    /// now.
    #[must_use]
    pub fn vegas_total(&self) -> i64 {
        self.bankroll
            .dollars()
            .saturating_add(self.vegas_provisional())
    }

    /// Stores new options; they take effect at the next
    /// [`Session::new_game`] — the running game's rule configuration is
    /// fixed at deal time and never changes mid-game.
    pub fn set_options(&mut self, options: Options) {
        self.options = options;
    }

    /// Ends the running game and deals a new one from `seed`, using the
    /// current options. There is no separate `abandon`:
    /// dealing the next game is exactly how a game ends without a win.
    ///
    /// In order: if the outgoing game was Vegas and not already won, its
    /// net result commits into the bankroll (saturating); then, if the
    /// current options have `keep_vegas_score` off, the bankroll resets to
    /// `0` — each game stands alone — regardless of whether a commit just
    /// happened. Only then is the new game dealt and the session clock
    /// zeroed.
    pub fn new_game(&mut self, seed: Seed) {
        let outgoing = self.game.state();
        let should_commit = outgoing.config().scoring == ScoringMode::Vegas && !outgoing.is_won();
        let outgoing_score = outgoing.score();
        if should_commit {
            self.bankroll = commit_vegas_score(self.bankroll, outgoing_score);
        }
        if !self.options.keep_vegas_score {
            self.bankroll = Bankroll::default();
        }
        self.game = Game::new(seed, self.options.game_config());
        self.elapsed_secs = 0;
    }

    /// Runs `command` through the engine and returns the events it
    /// produced. [`Command::Tick`] is routed through [`Session::tick`]
    /// first, so the session clock stays authoritative no matter how
    /// `apply` is called. On an [`Event::GameWon`] in a Vegas game, the
    /// game's net result commits into the bankroll (saturating) before
    /// returning — exactly once, since the engine rejects every further
    /// command against a won game.
    ///
    /// # Errors
    ///
    /// Returns the engine's [`RuleError`] unchanged; the session is
    /// unchanged.
    pub fn apply(&mut self, command: Command) -> Result<Vec<Event>, RuleError> {
        if let Command::Tick { total_elapsed_secs } = command {
            return self.tick(total_elapsed_secs);
        }
        let events = self.game.apply(command)?.to_vec();
        let state = self.game.state();
        let is_vegas_win =
            state.config().scoring == ScoringMode::Vegas && events.contains(&Event::GameWon);
        let score = state.score();
        if is_vegas_win {
            self.bankroll = commit_vegas_score(self.bankroll, score);
        }
        Ok(events)
    }

    /// Takes back the most recent player command.
    ///
    /// # Errors
    ///
    /// Returns the engine's [`RuleError`] unchanged —
    /// [`RuleError::UndoNotAllowed`] in Vegas scoring,
    /// [`RuleError::NothingToUndo`] when nothing is logged.
    pub fn undo(&mut self) -> Result<(), RuleError> {
        self.game.undo()
    }

    /// Re-applies the most recently undone command.
    ///
    /// Unlike [`Session::apply`], this never scans the returned events for
    /// [`Event::GameWon`]: Vegas scoring rejects undo and redo
    /// unconditionally, so a redo can only ever happen in a non-Vegas game,
    /// where a win never touches the bankroll — there is no case where a
    /// redo could need a commit.
    ///
    /// # Errors
    ///
    /// Returns the engine's [`RuleError`] unchanged —
    /// [`RuleError::UndoNotAllowed`] in Vegas scoring,
    /// [`RuleError::NothingToRedo`] when nothing was undone.
    pub fn redo(&mut self) -> Result<Vec<Event>, RuleError> {
        Ok(self.game.redo()?.to_vec())
    }

    /// Reports the host's total elapsed play time, advancing the session
    /// clock — the authoritative play timer for every scoring mode.
    /// Forwards [`Command::Tick`] to the engine, which
    /// additionally tracks time (and its score decay) for timed Standard
    /// games only.
    ///
    /// Invariant: the session clock is never behind the engine's own
    /// `state.elapsed_secs()`. A hand-edited or corrupted save could
    /// violate that on [`Session::restore`] — the next `tick` then simply
    /// yields a typed [`RuleError::TickInPast`] against the session's
    /// (higher) clock, never a panic or silent corruption. The mirror
    /// corruption — a restored log whose events put the engine's clock
    /// above the session's — degrades just as safely: a tick clearing the
    /// session's (lower) guard surfaces the engine's own
    /// [`RuleError::TickInPast`], its `current` naming the engine's
    /// (higher) clock.
    ///
    /// # Errors
    ///
    /// Returns [`RuleError::TickInPast`] when `total_elapsed_secs` is
    /// behind the session clock — checked here independently of the
    /// engine's own, narrower guard. Otherwise returns the engine's own
    /// [`RuleError`] unchanged.
    pub fn tick(&mut self, total_elapsed_secs: u32) -> Result<Vec<Event>, RuleError> {
        if total_elapsed_secs < self.elapsed_secs {
            return Err(RuleError::TickInPast {
                reported: total_elapsed_secs,
                current: self.elapsed_secs,
            });
        }
        let events = self
            .game
            .apply(Command::Tick { total_elapsed_secs })?
            .to_vec();
        self.elapsed_secs = total_elapsed_secs;
        Ok(events)
    }

    /// Snapshots this session into a save-format v1 document
    /// (`crate::save`): [`FORMAT_VERSION`], the format-pinned
    /// [`ENGINE_VERSION`], the running game's seed and full log, this
    /// session's options, bankroll, and elapsed seconds.
    #[must_use]
    pub fn to_save(&self) -> SaveGame {
        SaveGame {
            format_version: FORMAT_VERSION,
            engine_version: ENGINE_VERSION.to_owned(),
            seed: self.game.seed(),
            options: self.options.clone(),
            log: self.game.log().to_vec(),
            bankroll: self.bankroll,
            elapsed_secs: self.elapsed_secs,
        }
    }

    /// Rebuilds a session from a save document: delegates to
    /// [`Session::restore`], so it shares that exact reconstruction —
    /// re-deal from `save.seed`, then fold `save.log`. `save.engine_version`
    /// is ignored: it is informational only, and
    /// `format_version` — the only field that gates readability — has
    /// already been checked before a [`SaveGame`] value can exist.
    ///
    /// The redo stack is never persisted, since the log is canonical, so
    /// the restored session always reports `game().can_redo() == false`,
    /// even when the session that was saved had a redo available. Undo
    /// availability is restored exactly, since `game().can_undo()` derives
    /// from the log alone.
    #[must_use]
    pub fn from_save(save: SaveGame) -> Self {
        Self::restore(
            save.options,
            save.seed,
            save.log,
            save.bankroll,
            save.elapsed_secs,
        )
    }

    /// Composes [`Session::to_save`] with [`SaveGame::to_bytes`] — the
    /// bytes to write to a save file.
    ///
    /// # Errors
    ///
    /// Returns [`SaveError`] on serialization failure; see
    /// [`SaveGame::to_bytes`].
    pub fn to_save_bytes(&self) -> Result<Vec<u8>, SaveError> {
        self.to_save().to_bytes()
    }

    /// Composes [`SaveGame::from_bytes`] with [`Session::from_save`] — loads
    /// a session from previously saved bytes.
    ///
    /// # Errors
    ///
    /// Returns [`SaveError`] when `bytes` is not a readable save document;
    /// see [`SaveGame::from_bytes`].
    pub fn from_save_bytes(bytes: &[u8]) -> Result<Self, SaveError> {
        Ok(Self::from_save(SaveGame::from_bytes(bytes)?))
    }
}

/// Folds `score` dollars into `bankroll`, saturating instead of
/// overflowing.
fn commit_vegas_score(bankroll: Bankroll, score: i32) -> Bankroll {
    Bankroll::from(bankroll.dollars().saturating_add(i64::from(score)))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    /// The deal these tests share with the engine's rules suites: an ace
    /// face-up on tableau 0, so a game can open with a foundation move.
    const SEED: u16 = 8622;

    use super::*;
    use sol_engine::{DrawMode, PileId};

    fn standard_options() -> Options {
        Options::default()
    }

    fn none_scoring_options() -> Options {
        Options {
            scoring: ScoringMode::None,
            ..Options::default()
        }
    }

    fn vegas_options(keep_vegas_score: bool) -> Options {
        Options {
            scoring: ScoringMode::Vegas,
            keep_vegas_score,
            ..Options::default()
        }
    }

    /// The events that stage 51 cards onto foundation 0, leaving tableau 0's
    /// single face-up card as the only one still off the foundations —
    /// mirrors sol-engine's own fixtures (see
    /// `sol-engine/tests/undo_redo.rs::undo_reopens_a_won_game` and
    /// `rules_scoring.rs::stage_51_cards_on_foundations`). Deal-invariant:
    /// by the documented layout (`sol_engine::deal`), tableau `p` always
    /// holds exactly `p` face-down cards under one face-up card, for every
    /// seed — so this needs no live state to probe.
    fn stage_51_cards_on_foundation_0() -> Vec<Event> {
        let mut events = vec![Event::CardsMoved {
            from: PileId::Stock,
            to: PileId::Foundation(0),
            count: 24,
        }];
        for index in 1..7_u8 {
            events.push(Event::CardsMoved {
                from: PileId::Tableau(index),
                to: PileId::Foundation(0),
                count: 1,
            });
            for _ in 0..index {
                events.push(Event::CardFlipped {
                    pile: PileId::Tableau(index),
                });
                events.push(Event::CardsMoved {
                    from: PileId::Tableau(index),
                    to: PileId::Foundation(0),
                    count: 1,
                });
            }
        }
        events
    }

    /// A restorable log leaving exactly one legal winning move: for seed 1,
    /// tableau 0's remaining card is the ace of clubs, so
    /// `Tableau(0) -> Foundation(1)` wins (see sol-engine's
    /// `rules_scoring.rs::a_vegas_win_pays_the_last_5_and_no_bonus`).
    fn near_won_log() -> Vec<LogEntry> {
        vec![LogEntry {
            command: Command::Draw,
            events: stage_51_cards_on_foundation_0(),
        }]
    }

    const WINNING_MOVE: Command = Command::MoveCards {
        from: PileId::Tableau(0),
        to: PileId::Foundation(1),
        count: 1,
    };

    #[test]
    fn new_deals_immediately_with_an_empty_bankroll_and_a_zeroed_clock() {
        let options = standard_options();
        let session = Session::new(options.clone(), Seed::new(7).unwrap());
        assert_eq!(session.bankroll(), Bankroll::default());
        assert_eq!(session.elapsed_secs(), 0);
        assert_eq!(session.options(), &options);
        assert_eq!(session.game().state().stock().len(), 24, "a fresh deal");
    }

    #[test]
    fn restore_rebuilds_the_game_from_the_log_and_takes_bankroll_and_elapsed_as_given() {
        let options = standard_options();
        let seed = Seed::new(SEED).unwrap();
        let mut source = Game::new(seed, options.game_config());
        source.apply(Command::Draw).unwrap();
        let log = source.log().to_vec();

        let session = Session::restore(options.clone(), seed, log, Bankroll::from(250_i64), 90);

        assert_eq!(session.game().state(), source.state());
        assert_eq!(session.bankroll().dollars(), 250);
        assert_eq!(session.elapsed_secs(), 90);
        assert_eq!(session.options(), &options);
    }

    #[test]
    fn vegas_provisional_is_zero_for_standard_and_none_scoring() {
        for options in [standard_options(), none_scoring_options()] {
            let session = Session::new(options.clone(), Seed::new(SEED).unwrap());
            assert_eq!(session.vegas_provisional(), 0, "{options:?}");
            assert_eq!(session.vegas_total(), 0, "{options:?}");
        }
    }

    #[test]
    fn fresh_vegas_deal_is_provisional_at_the_buy_in_and_bankroll_stays_untouched() {
        let options = Options {
            scoring: ScoringMode::Vegas,
            ..Options::default()
        };
        let session = Session::restore(
            options,
            Seed::new(SEED).unwrap(),
            Vec::new(),
            Bankroll::from(500_i64),
            0,
        );
        assert_eq!(session.vegas_provisional(), -52, "the Vegas buy-in");
        assert_eq!(
            session.bankroll().dollars(),
            500,
            "untouched, still provisional"
        );
        assert_eq!(session.vegas_total(), 448, "bankroll 500 + provisional -52");
    }

    #[test]
    fn standard_win_does_not_touch_the_bankroll() {
        let mut session = Session::restore(
            standard_options(),
            Seed::new(SEED).unwrap(),
            near_won_log(),
            Bankroll::default(),
            0,
        );
        assert_eq!(session.vegas_provisional(), 0, "never Vegas");

        let events = session.apply(WINNING_MOVE).unwrap();

        assert!(events.contains(&Event::GameWon));
        assert!(session.game().state().is_won());
        assert_eq!(
            session.bankroll().dollars(),
            0,
            "Standard wins never commit"
        );
        assert_eq!(session.vegas_provisional(), 0);
    }

    #[test]
    fn standard_new_game_does_not_touch_the_bankroll() {
        let mut session = Session::new(standard_options(), Seed::new(SEED).unwrap());
        session
            .apply(Command::MoveCards {
                from: PileId::Tableau(0),
                to: PileId::Foundation(0),
                count: 1,
            })
            .unwrap();
        assert_eq!(session.game().state().score(), 10, "nonzero, but not Vegas");

        session.new_game(Seed::new(2).unwrap());

        assert_eq!(session.bankroll().dollars(), 0);
    }

    #[test]
    fn apply_forwards_a_rejected_command_error_unchanged_and_the_session_is_untouched() {
        let mut session = Session::new(standard_options(), Seed::new(SEED).unwrap());
        let before = session.clone();

        let error = session
            .apply(Command::MoveCards {
                from: PileId::Waste,
                to: PileId::Tableau(0),
                count: 1,
            })
            .unwrap_err();

        assert_eq!(error, RuleError::NothingToMove);
        assert_eq!(session, before, "a rejected command changes nothing");
    }

    #[test]
    fn redeal_with_keep_on_commits_the_outgoing_result_and_accumulates() {
        let mut session = Session::new(vegas_options(true), Seed::new(SEED).unwrap());

        session.new_game(Seed::new(2).unwrap());

        assert_eq!(
            session.bankroll().dollars(),
            -52,
            "game 1's net result committed"
        );
        assert_eq!(session.vegas_provisional(), -52, "game 2's fresh buy-in");
        assert_eq!(session.vegas_total(), -104);
    }

    #[test]
    fn redeal_with_keep_off_does_not_carry_the_outgoing_result() {
        let mut session = Session::new(vegas_options(false), Seed::new(SEED).unwrap());

        session.new_game(Seed::new(2).unwrap());

        assert_eq!(session.bankroll().dollars(), 0, "each game stands alone");
        assert_eq!(session.vegas_provisional(), -52, "game 2's fresh buy-in");
    }

    #[test]
    fn toggling_keep_off_mid_game_zeroes_the_bankroll_at_the_next_redeal() {
        let mut session = Session::new(vegas_options(true), Seed::new(SEED).unwrap());
        session.new_game(Seed::new(2).unwrap());
        assert_eq!(session.bankroll().dollars(), -52, "kept from game 1");

        session.set_options(vegas_options(false));
        session.new_game(Seed::new(3).unwrap());

        assert_eq!(
            session.bankroll().dollars(),
            0,
            "evaluated with the current options"
        );
        assert_eq!(session.vegas_provisional(), -52, "game 3's fresh buy-in");
    }

    #[test]
    fn vegas_win_commits_the_bankroll_exactly_once_and_new_game_does_not_double_commit() {
        let mut session = Session::restore(
            vegas_options(true),
            Seed::new(SEED).unwrap(),
            near_won_log(),
            Bankroll::from(100_i64),
            0,
        );
        assert_eq!(session.game().state().foundation_card_count(), 51);

        let events = session.apply(WINNING_MOVE).unwrap();

        assert!(events.contains(&Event::GameWon));
        assert_eq!(
            session.bankroll().dollars(),
            53,
            "100 + (-47) net Vegas result"
        );
        assert_eq!(
            session.vegas_provisional(),
            0,
            "committed, not provisional anymore"
        );
        assert_eq!(session.vegas_total(), 53);

        session.new_game(Seed::new(2).unwrap());

        assert_eq!(
            session.bankroll().dollars(),
            53,
            "the is_won guard stops new_game from committing the same result again"
        );
        assert_eq!(session.vegas_provisional(), -52, "the new deal's buy-in");
    }

    #[test]
    fn a_redeal_commit_that_would_overflow_the_bankroll_saturates() {
        let log = vec![LogEntry {
            command: Command::Draw,
            events: vec![Event::ScoreChanged { delta: 1_000 }],
        }];
        let mut session = Session::restore(
            vegas_options(true),
            Seed::new(SEED).unwrap(),
            log,
            Bankroll::from(i64::MAX),
            0,
        );
        assert_eq!(session.game().state().score(), 948, "buy-in -52 + 1000");

        session.new_game(Seed::new(2).unwrap());

        assert_eq!(
            session.bankroll().dollars(),
            i64::MAX,
            "clamps, never overflows"
        );
    }

    #[test]
    fn undo_and_redo_forward_to_the_engine_in_standard_scoring() {
        let mut session = Session::new(standard_options(), Seed::new(SEED).unwrap());
        let move_command = Command::MoveCards {
            from: PileId::Tableau(0),
            to: PileId::Foundation(0),
            count: 1,
        };
        session.apply(move_command).unwrap();
        assert_eq!(session.game().state().score(), 10);

        session.undo().unwrap();
        assert_eq!(session.game().state().score(), 0);

        let redone = session.redo().unwrap();
        assert_eq!(
            redone,
            vec![
                Event::CardsMoved {
                    from: PileId::Tableau(0),
                    to: PileId::Foundation(0),
                    count: 1,
                },
                Event::ScoreChanged { delta: 10 },
            ]
        );
        assert_eq!(session.game().state().score(), 10);
    }

    #[test]
    fn vegas_undo_and_redo_surface_the_engines_rejection_unchanged() {
        let mut session = Session::new(vegas_options(false), Seed::new(SEED).unwrap());
        session
            .apply(Command::MoveCards {
                from: PileId::Tableau(0),
                to: PileId::Foundation(0),
                count: 1,
            })
            .unwrap();

        assert_eq!(session.undo().unwrap_err(), RuleError::UndoNotAllowed);
        assert_eq!(session.redo().unwrap_err(), RuleError::UndoNotAllowed);
    }

    #[test]
    fn tick_advances_the_session_clock_even_when_the_engine_stays_silent() {
        let untimed_standard = Options {
            timed: false,
            ..Options::default()
        };
        for options in [vegas_options(false), untimed_standard] {
            let mut session = Session::new(options.clone(), Seed::new(SEED).unwrap());

            let events = session.tick(50).unwrap();

            assert_eq!(events, Vec::new(), "{options:?}");
            assert_eq!(session.elapsed_secs(), 50, "{options:?}");
        }
    }

    #[test]
    fn tick_on_a_timed_standard_game_forwards_time_advance_and_decay() {
        let mut session = Session::new(standard_options(), Seed::new(SEED).unwrap());
        session
            .apply(Command::MoveCards {
                from: PileId::Tableau(0),
                to: PileId::Foundation(0),
                count: 1,
            })
            .unwrap();
        assert_eq!(session.game().state().score(), 10);

        let events = session.tick(10).unwrap();

        assert_eq!(
            events,
            vec![
                Event::TimeAdvanced {
                    total_elapsed_secs: 10
                },
                Event::ScoreChanged { delta: -2 },
            ]
        );
        assert_eq!(session.elapsed_secs(), 10);
    }

    #[test]
    fn an_equal_second_tick_is_ok_and_a_backwards_tick_is_rejected() {
        let mut session = Session::new(standard_options(), Seed::new(SEED).unwrap());
        session.tick(20).unwrap();

        let events = session.tick(20).unwrap();
        assert_eq!(events, Vec::new(), "no boundary crossed a second time");
        assert_eq!(session.elapsed_secs(), 20);

        let error = session.tick(19).unwrap_err();
        assert_eq!(
            error,
            RuleError::TickInPast {
                reported: 19,
                current: 20
            }
        );
        assert_eq!(session.elapsed_secs(), 20, "unchanged by a rejected tick");
    }

    #[test]
    fn apply_routes_command_tick_through_the_clock_aware_tick_path() {
        let mut via_apply = Session::new(standard_options(), Seed::new(SEED).unwrap());
        let mut via_tick = Session::new(standard_options(), Seed::new(SEED).unwrap());

        let apply_events = via_apply
            .apply(Command::Tick {
                total_elapsed_secs: 50,
            })
            .unwrap();
        let tick_events = via_tick.tick(50).unwrap();

        assert_eq!(
            apply_events, tick_events,
            "apply(Command::Tick) forwards to tick() exactly"
        );
        assert_eq!(via_apply.elapsed_secs(), 50, "the session clock advanced");
        assert_eq!(via_apply, via_tick, "identical resulting session state");

        let error = via_apply
            .apply(Command::Tick {
                total_elapsed_secs: 10,
            })
            .unwrap_err();

        assert_eq!(
            error,
            RuleError::TickInPast {
                reported: 10,
                current: 50
            },
            "a backwards tick through apply surfaces TickInPast, same as tick()"
        );
        assert_eq!(via_apply.elapsed_secs(), 50, "unchanged by a rejected tick");
    }

    #[test]
    fn a_corrupted_restore_surfaces_the_engines_own_tick_in_past() {
        // The mirror image of the invariant documented on `Session::tick`:
        // a hand-built log whose `TimeAdvanced` puts the ENGINE clock at
        // 100 while the session clock is restored below it, at 10.
        let log = vec![LogEntry {
            command: Command::Tick {
                total_elapsed_secs: 100,
            },
            events: vec![Event::TimeAdvanced {
                total_elapsed_secs: 100,
            }],
        }];
        let mut session = Session::restore(
            standard_options(),
            Seed::new(SEED).unwrap(),
            log,
            Bankroll::default(),
            10,
        );
        assert_eq!(session.game().state().elapsed_secs(), 100, "engine clock");
        assert_eq!(session.elapsed_secs(), 10, "session clock, behind it");

        // 50 clears the session's own guard (>= 10) but still trails the
        // engine's clock, so the engine's guard fires.
        let error = session.tick(50).unwrap_err();

        assert_eq!(
            error,
            RuleError::TickInPast {
                reported: 50,
                current: 100
            },
            "the engine's clock, not the session's"
        );
        assert_eq!(session.elapsed_secs(), 10, "unchanged by a rejected tick");
    }

    #[test]
    fn new_game_resets_the_session_clock_to_zero() {
        let mut session = Session::new(standard_options(), Seed::new(SEED).unwrap());
        session.tick(42).unwrap();
        assert_eq!(session.elapsed_secs(), 42);

        session.new_game(Seed::new(2).unwrap());

        assert_eq!(session.elapsed_secs(), 0);
    }

    #[test]
    fn set_options_takes_effect_only_at_the_next_new_game() {
        let mut session = Session::new(Options::default(), Seed::new(SEED).unwrap());
        assert_eq!(session.game().state().config().draw_mode, DrawMode::Three);

        session.set_options(Options {
            draw_mode: DrawMode::One,
            ..Options::default()
        });
        assert_eq!(
            session.game().state().config().draw_mode,
            DrawMode::Three,
            "the running game's config is unaffected"
        );

        session.new_game(Seed::new(2).unwrap());

        assert_eq!(session.game().state().config().draw_mode, DrawMode::One);
    }
}
