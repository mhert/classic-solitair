//! Pure event-sourced Klondike Solitaire rules engine (Gameplay context).
//!
//! No I/O, no clock, no OS RNG: game state is a deterministic function of a
//! sequence of events, so it can be replayed, tested, and driven by any
//! frontend without touching the outside world.
