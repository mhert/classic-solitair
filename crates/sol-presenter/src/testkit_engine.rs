//! Test-only helpers: engine states that would take a full played game to
//! reach — a won game, and a game one move short of winning.
//!
//! Built by folding a hand-constructed event log with the engine's own
//! (total, rules-free) `evolve`, mirroring the live state at every step so
//! each emitted event is well-formed pile mechanics. The final state is a
//! real won position: every foundation holds one suit, ace through king.

// Test fixtures: a broken fixture must abort the suite loudly.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use sol_engine::{
    Command, DrawMode, Event, FOUNDATION_COUNT, Game, GameConfig, GameState, LogEntry, PileId,
    ScoringMode, Seed, deal, evolve,
};
use sol_session::{Bankroll, Options, Session};

/// The seed and config every helper here uses.
const SEED: u16 = 1;

fn config() -> GameConfig {
    GameConfig {
        draw_mode: DrawMode::One,
        scoring: ScoringMode::None,
        timed: false,
    }
}

/// Options whose `game_config()` equals [`config`].
pub(crate) fn options() -> Options {
    Options {
        draw_mode: DrawMode::One,
        scoring: ScoringMode::None,
        timed: false,
        ..Options::default()
    }
}

fn emit(state: &mut GameState, events: &mut Vec<Event>, event: Event) {
    evolve(state, event);
    events.push(event);
}

/// Dumps the stock through the waste onto tableau 0, then flips and
/// consolidates every other pile onto it: all 52 cards face-up in one
/// place.
fn consolidate_onto_tableau_0(state: &mut GameState, events: &mut Vec<Event>) {
    let stock = u8::try_from(state.stock().len()).expect("a deal's stock fits u8");
    emit(
        state,
        events,
        Event::CardsMoved {
            from: PileId::Stock,
            to: PileId::Waste,
            count: stock,
        },
    );
    while !state.waste().is_empty() {
        emit(
            state,
            events,
            Event::CardsMoved {
                from: PileId::Waste,
                to: PileId::Tableau(0),
                count: 1,
            },
        );
    }
    for column in 1..7_u8 {
        loop {
            let pile = state.tableau(column).expect("column exists");
            if pile.face_down().is_empty() {
                break;
            }
            emit(
                state,
                events,
                Event::CardFlipped {
                    pile: PileId::Tableau(column),
                },
            );
        }
        let run = state
            .tableau(column)
            .map(|pile| pile.face_up().len())
            .unwrap_or_default();
        if run > 0 {
            emit(
                state,
                events,
                Event::CardsMoved {
                    from: PileId::Tableau(column),
                    to: PileId::Tableau(0),
                    count: u8::try_from(run).expect("a pile fits u8"),
                },
            );
        }
    }
}

/// Builds the event list that turns the deal of [`SEED`] into a won game
/// (52 cards on suit-sorted foundations), ending with the winning
/// foundation move and `GameWon`. Also returns that winning move's
/// `(from, to)` piles, recorded as it was emitted.
fn winning_events() -> (Vec<Event>, (PileId, PileId)) {
    let mut state = deal(Seed::new(SEED).unwrap(), config());
    let mut events = Vec::new();
    consolidate_onto_tableau_0(&mut state, &mut events);

    // Distribute tableau 0 by suit onto parking piles 1..=4.
    while let Some(&top) = state.tableau(0).and_then(|pile| pile.face_up().last()) {
        emit(
            &mut state,
            &mut events,
            Event::CardsMoved {
                from: PileId::Tableau(0),
                to: PileId::Tableau(1 + top.suit.index()),
                count: 1,
            },
        );
    }

    // Two-stack selection sort per suit: park pile ↔ tableau 5, placing
    // each next-needed rank onto the suit's foundation. The last
    // foundation placement emitted is, by construction, the winning move.
    let mut winning_move = None;
    for suit in 0..FOUNDATION_COUNT {
        let mut source = 1 + suit;
        let mut aux = 5_u8;
        for _guard in 0..10_000 {
            let placed = state
                .foundation(suit)
                .map(<[sol_engine::Card]>::len)
                .unwrap_or_default();
            if placed == 13 {
                break;
            }
            let top = state.tableau(source).and_then(|pile| pile.face_up().last());
            let Some(&top) = top else {
                core::mem::swap(&mut source, &mut aux);
                continue;
            };
            let needed = u8::try_from(placed + 1).expect("rank fits u8");
            let to = if top.rank.value() == needed {
                winning_move = Some((PileId::Tableau(source), PileId::Foundation(suit)));
                PileId::Foundation(suit)
            } else {
                PileId::Tableau(aux)
            };
            emit(
                &mut state,
                &mut events,
                Event::CardsMoved {
                    from: PileId::Tableau(source),
                    to,
                    count: 1,
                },
            );
        }
        let placed = state
            .foundation(suit)
            .map(<[sol_engine::Card]>::len)
            .unwrap_or_default();
        assert_eq!(placed, 13, "suit {suit} sorted onto its foundation");
    }

    events.push(Event::GameWon);
    evolve(&mut state, Event::GameWon);
    assert!(state.is_won());
    let winning = winning_move.expect("52 placements include a last one");
    (events, winning)
}

/// A won game: all 52 cards on suit-sorted foundations, `is_won() == true`.
pub(crate) fn won_game() -> Game {
    let (events, _) = winning_events();
    Game::from_log(
        Seed::new(SEED).unwrap(),
        config(),
        vec![LogEntry {
            command: Command::Draw,
            events,
        }],
    )
}

/// A session one legal move short of winning, plus that winning command.
///
/// The returned session's state has 51 cards placed and the last king on a
/// tableau pile; applying the returned command through the real rules
/// emits the final `CardsMoved` and `GameWon`.
pub(crate) fn almost_won_session() -> (Session, Command) {
    let (mut events, (from, to)) = winning_events();
    let won = events.pop();
    assert_eq!(won, Some(Event::GameWon));
    let last_move = events.pop();
    assert_eq!(last_move, Some(Event::CardsMoved { from, to, count: 1 }));
    let session = Session::restore(
        options(),
        Seed::new(SEED).unwrap(),
        vec![LogEntry {
            command: Command::Draw,
            events,
        }],
        Bankroll::default(),
        0,
    );
    assert!(!session.game().state().is_won());
    (session, Command::MoveCards { from, to, count: 1 })
}
