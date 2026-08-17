//! Save-format v1 integration tests: byte-
//! identical round trips, mid-game restore fidelity, the committed fixture
//! lock, and an end-to-end continue-after-load flow. Format-version
//! rejection and malformed-data rejection are covered as unit tests in
//! `src/save.rs` alongside the code they pin.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use sol_engine::{Command, DrawMode, PileId, ScoringMode, Seed};
use sol_session::{
    Bankroll, ENGINE_VERSION, FORMAT_VERSION, Options, SaveError, SaveGame, Session, ThemeId,
};

/// Non-default options, every field away from `Options::default()` — mirrors
/// `options.rs`'s own `non_default_options` fixture.
fn fixture_options() -> Options {
    Options {
        draw_mode: DrawMode::One,
        scoring: ScoringMode::Standard,
        timed: true,
        outline_dragging: true,
        keep_vegas_score: true,
        theme: ThemeId::try_from("midnight".to_owned()).unwrap(),
        sounds: false,
    }
}

const FIXTURE_SEED: u16 = 8622;

/// Tableau 0's single face-up card onto foundation 0 — an ace under
/// [`FIXTURE_SEED`], legal with no setup (the same deal `sol-engine`'s own
/// rules suites use). Standard scoring: +10, no flip, since tableau 0 has no
/// face-down cards underneath it.
const MOVE_ONE: Command = Command::MoveCards {
    from: PileId::Tableau(0),
    to: PileId::Foundation(0),
    count: 1,
};

/// Tableau 5's single face-up card — also an ace under [`FIXTURE_SEED`] (see
/// `sol-engine`'s `rules_scoring.rs`,
/// `standard_tableau_to_foundation_scores_10_and_a_flip_scores_5`) — onto
/// foundation 1. Standard scoring: +10 plus a +5 flip bonus, since tableau 5
/// has 5 face-down cards underneath it.
const MOVE_TWO: Command = Command::MoveCards {
    from: PileId::Tableau(5),
    to: PileId::Foundation(1),
    count: 1,
};

/// The deterministic "fixture session": a timed Standard game with a draw,
/// two foundation moves, an undo, and a tick — in that order, so the tick
/// lands *after* the undo instead of being discarded by it (undo always
/// discards a trailing tick; see `sol-engine`'s `Game::undo`). That keeps
/// this session's persisted log actually carrying `TimeAdvanced` and
/// decay-`ScoreChanged` entries, not just move entries. Backs
/// `tests/fixtures/save_v1.json` and is reused by the round-trip and
/// mid-game-restore tests below, so every test in this file agrees on the
/// same known-good numbers.
///
/// Hand-computed trace (cross-checked against `sol-engine`'s own
/// `rules_scoring.rs` tests):
/// - `Draw`: the waste gains 1 card; drawing itself never scores.
/// - `MOVE_ONE`: +10 → score 10.
/// - `MOVE_TWO`: +10 base, +5 flip → score 25.
/// - `undo()`: takes back `MOVE_TWO` (the log is only `[Draw, MOVE_ONE,
///   MOVE_TWO]` so far — no trailing tick to discard yet). The log is left
///   as `[Draw, MOVE_ONE]`, `MOVE_TWO` waits on the redo stack, and
///   replaying returns the score to 10.
/// - `tick(10)`: one 10s decay boundary, -2 → score 8. Ticks never clear the
///   redo stack, so `MOVE_TWO` is still waiting afterwards — the *original*
///   session has a redo available going into the save.
fn fixture_session() -> Session {
    let mut session = Session::new(fixture_options(), Seed::new(FIXTURE_SEED).unwrap());
    session.apply(Command::Draw).unwrap();
    session.apply(MOVE_ONE).unwrap();
    session.apply(MOVE_TWO).unwrap();
    session.undo().unwrap();
    session.tick(10).unwrap();
    session
}

#[test]
fn fixture_session_matches_its_hand_computed_trace() {
    let session = fixture_session();
    assert_eq!(
        session.game().state().score(),
        8,
        "10 from MOVE_ONE, -2 decay"
    );
    assert_eq!(session.game().state().passes_completed(), 0);
    assert_eq!(
        session.game().state().elapsed_secs(),
        10,
        "the tick was applied after the undo, so it survives"
    );
    assert_eq!(session.elapsed_secs(), 10);
    assert_eq!(session.bankroll(), Bankroll::default());
    assert!(session.game().can_undo());
    assert!(
        session.game().can_redo(),
        "MOVE_TWO is waiting on the redo stack; ticks never clear it"
    );
}

#[test]
fn byte_round_trip_is_identical_for_a_timed_standard_session_with_history() {
    let session = fixture_session();

    let first = session.to_save_bytes().unwrap();
    let loaded = Session::from_save_bytes(&first).unwrap();
    let second = loaded.to_save_bytes().unwrap();

    assert_eq!(first, second, "save -> load -> save must be byte-identical");
}

/// Non-default Vegas options, distinct from `fixture_options` so the two
/// scenarios don't collide.
fn vegas_options() -> Options {
    Options {
        draw_mode: DrawMode::Three,
        scoring: ScoringMode::Vegas,
        timed: true,
        outline_dragging: false,
        keep_vegas_score: true,
        theme: ThemeId::try_from("vegas".to_owned()).unwrap(),
        sounds: true,
    }
}

/// A Vegas session with a nonzero committed bankroll and a completed waste
/// pass. Hand-computed trace:
/// - game 1 (seed 1): the deal charges the -52 buy-in; `MOVE_ONE` (tableau
///   0's ace, an eligible Vegas foundation play) pays +5 → score -47 (see
///   `sol-engine`'s `rules_scoring.rs::vegas_pays_5_per_foundation_card_and_refunds_5_when_one_leaves`).
/// - `new_game(seed 2)`: commits -47 into the bankroll (`keep_vegas_score`
///   is on, so it survives the redeal); game 2 deals with its own fresh -52
///   buy-in.
/// - game 2, Draw Three: 8 draws empty the 24-card stock; a 9th recycles it
///   — Vegas allows recycling under Draw Three, up to 2 passes (see
///   `rules_draw.rs::vegas_draw_three_gets_three_passes_then_rejects`), with
///   no score effect, so `passes_completed` becomes 1.
/// - `tick(30)`: Vegas games are never engine-timed (see
///   `rules_scoring.rs::untimed_vegas_and_none_games_ignore_ticks_entirely`),
///   so the engine clock stays at 0, but the session clock — authoritative
///   for every scoring mode — advances to 30.
fn vegas_session_with_history() -> Session {
    let mut session = Session::new(vegas_options(), Seed::new(FIXTURE_SEED).unwrap());
    session.apply(MOVE_ONE).unwrap();
    assert_eq!(session.game().state().score(), -47);

    session.new_game(Seed::new(2).unwrap());
    assert_eq!(session.bankroll(), Bankroll::from(-47_i64));

    for _ in 0..8 {
        session.apply(Command::Draw).unwrap();
    }
    session.apply(Command::Draw).unwrap();
    assert_eq!(session.game().state().passes_completed(), 1);

    session.tick(30).unwrap();
    session
}

#[test]
fn byte_round_trip_is_identical_for_a_vegas_session_with_nonzero_bankroll_and_passes() {
    let session = vegas_session_with_history();
    assert_ne!(session.bankroll(), Bankroll::default());
    assert!(session.game().state().passes_completed() > 0);

    let first = session.to_save_bytes().unwrap();
    let loaded = Session::from_save_bytes(&first).unwrap();
    let second = loaded.to_save_bytes().unwrap();

    assert_eq!(first, second, "save -> load -> save must be byte-identical");
}

#[test]
fn mid_game_load_restores_score_passes_undo_availability_elapsed_bankroll_and_options_exactly() {
    let original = fixture_session();
    assert!(
        original.game().can_redo(),
        "the original session has a redo available"
    );

    let bytes = original.to_save_bytes().unwrap();
    let loaded = Session::from_save_bytes(&bytes).unwrap();

    assert_eq!(
        loaded.game().state().score(),
        original.game().state().score()
    );
    assert_eq!(
        loaded.game().state().passes_completed(),
        original.game().state().passes_completed()
    );
    assert!(
        loaded.game().can_undo(),
        "undo availability derives from the log alone and is restored exactly"
    );
    assert!(
        !loaded.game().can_redo(),
        "the redo stack is intentionally not persisted (see save.rs's module docs)"
    );
    assert_eq!(loaded.elapsed_secs(), original.elapsed_secs());
    assert_eq!(loaded.bankroll(), original.bankroll());
    assert_eq!(loaded.options(), original.options());
    assert_eq!(
        loaded.game().state(),
        original.game().state(),
        "the folded states must be identical"
    );
}

#[test]
fn loading_a_mid_play_vegas_session_then_redealing_commits_the_bankroll_like_the_original() {
    let mut original = Session::new(vegas_options(), Seed::new(FIXTURE_SEED).unwrap());
    original.apply(MOVE_ONE).unwrap();
    assert_eq!(
        original.bankroll(),
        Bankroll::default(),
        "provisional, not committed while game 1 is still running"
    );

    let bytes = original.to_save_bytes().unwrap();
    let mut loaded = Session::from_save_bytes(&bytes).unwrap();

    original.new_game(Seed::new(2).unwrap());
    loaded.new_game(Seed::new(2).unwrap());

    assert_eq!(
        loaded.bankroll(),
        original.bankroll(),
        "the loaded session commits exactly as continuing the original would"
    );
    assert_eq!(loaded.bankroll(), Bankroll::from(-47_i64));
}

#[test]
fn a_fresh_saves_engine_version_is_the_pinned_format_version() {
    let session = Session::new(Options::default(), Seed::new(1).unwrap());

    // Pinned to the save format, not the crate version, so the fixture bytes
    // stay stable across engine releases.
    assert_eq!(session.to_save().engine_version, ENGINE_VERSION);
    assert_eq!(ENGINE_VERSION, "1.0.0");
}

#[test]
fn session_from_save_bytes_surfaces_an_unsupported_format_version() {
    let mut value = serde_json::to_value(fixture_session().to_save()).unwrap();
    value
        .as_object_mut()
        .unwrap()
        .insert("format_version".to_owned(), serde_json::json!(99));
    let bytes = serde_json::to_vec(&value).unwrap();

    let error = Session::from_save_bytes(&bytes).unwrap_err();

    assert!(matches!(
        error,
        SaveError::UnsupportedFormatVersion { found: 99 }
    ));
}

// ------------------------------------------------------------ fixture lock

/// The committed fixture's path, resolved from the crate root so this works
/// regardless of the working directory `cargo test` was invoked from.
fn fixture_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/save_v1.json")
}

/// Regenerates `tests/fixtures/save_v1.json` from `fixture_session()`.
/// The byte lock's purpose is to catch unintended serde-shape drift in what
/// v1 embeds. The stamped `engine_version` is pinned to `ENGINE_VERSION`
/// rather than the crate version, so routine engine releases no longer change
/// these bytes — only a deliberate save-format change does, and that is
/// exactly when this fixture should be regenerated (the file must be that raw
/// output, with no editor-added trailing newline). Run it explicitly with:
///
/// ```text
/// cargo test -p sol-session --test save write_fixture -- --ignored
/// ```
#[test]
#[ignore = "regenerates the committed fixture; see this test's doc comment"]
fn write_fixture() {
    let bytes = fixture_session().to_save_bytes().unwrap();
    std::fs::write(fixture_path(), bytes).unwrap();
}

#[test]
fn fixture_lock_reads_expected_values_and_reproduces_the_committed_bytes() {
    let fixture_bytes = std::fs::read(fixture_path()).unwrap();

    let loaded = SaveGame::from_bytes(&fixture_bytes).unwrap();
    assert_eq!(loaded.format_version, FORMAT_VERSION);
    assert_eq!(loaded.seed, Seed::new(FIXTURE_SEED).unwrap());
    assert_eq!(loaded.options, fixture_options());
    assert_eq!(loaded.bankroll, Bankroll::default());
    assert_eq!(loaded.elapsed_secs, 10);

    let session = Session::from_save(loaded);
    assert_eq!(session.game().state().score(), 8);
    assert_eq!(session.game().state().passes_completed(), 0);
    assert!(session.game().can_undo());
    assert!(!session.game().can_redo());

    let rebuilt = fixture_session().to_save_bytes().unwrap();
    assert_eq!(
        rebuilt, fixture_bytes,
        "to_bytes must reproduce the committed fixture byte-for-byte"
    );
}
