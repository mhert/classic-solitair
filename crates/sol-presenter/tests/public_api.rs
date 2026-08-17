//! Public-surface acceptance tests: the golden Win98 layout tables in
//! logical pixels, and a scripted stretch of play driven purely through
//! the published API.

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use sol_engine::{PileId, Seed};
use sol_presenter::{Fit, Layout, Presenter, Pt, Rect, Size, TextureId};
use sol_session::{Options, Session};
use sol_theme::{CardSize, MemSource, Theme, canonical_faces};

/// The deal these tests share with the engine's rules suites.
const SEED: u16 = 8622;

fn theme() -> Theme {
    let manifest = br##"
[theme]
name = "Acceptance"
render_mode = "vector"

[cards]
faces = "cards/"
base_size = [71, 96]

[backs]
plain = { image = "backs/plain.svg" }

[table]
background = { color = "#008000" }

[drag]
outline_color = "#000000"
"##;
    let svg = |w: u32, h: u32| format!(r#"<svg width="{w}" height="{h}"></svg>"#).into_bytes();
    let mut source = MemSource::new()
        .with_file("theme.toml", &manifest[..])
        .with_file("backs/plain.svg", svg(71, 96));
    for (suit, rank) in canonical_faces() {
        source = source.with_file(format!("cards/{}.svg", suit.stem(rank)), svg(71, 96));
    }
    Theme::from_source(&source).unwrap()
}

fn win98() -> Layout {
    Layout::new(
        CardSize {
            width: 71,
            height: 96,
        },
        585,
    )
}

/// The complete pile-origin table of the original's design layout, in
/// logical pixels.
#[test]
fn golden_pile_origins() {
    let expectations = [
        (PileId::Stock, 11, 5),
        (PileId::Waste, 93, 5),
        (PileId::Foundation(0), 257, 5),
        (PileId::Foundation(1), 339, 5),
        (PileId::Foundation(2), 421, 5),
        (PileId::Foundation(3), 503, 5),
        (PileId::Tableau(0), 11, 107),
        (PileId::Tableau(1), 93, 107),
        (PileId::Tableau(2), 175, 107),
        (PileId::Tableau(3), 257, 107),
        (PileId::Tableau(4), 339, 107),
        (PileId::Tableau(5), 421, 107),
        (PileId::Tableau(6), 503, 107),
    ];
    let layout = win98();
    for (pile, x, y) in expectations {
        assert_eq!(layout.pile_origin(pile), Some(Pt::new(x, y)), "{pile:?}");
    }
    assert_eq!(layout.design_size(), Size::new(585, 384), "design client");
    assert_eq!(layout.face_up_step(), 15);
    assert_eq!(layout.face_down_step(), 3);
    assert_eq!(layout.waste_fan_step(), Pt::new(14, 1));
}

/// The original's padded pile hit rectangles.
#[test]
fn golden_pile_rects() {
    let expectations = [
        (PileId::Stock, 11, 5, 81, 101),
        (PileId::Waste, 93, 5, 103, 101),
        (PileId::Foundation(0), 257, 5, 77, 101),
        (PileId::Foundation(3), 503, 5, 77, 101),
        (PileId::Tableau(0), 11, 107, 71, 294),
        (PileId::Tableau(6), 503, 107, 71, 294),
    ];
    let layout = win98();
    for (pile, x, y, w, h) in expectations {
        assert_eq!(
            layout.pile_rect(pile),
            Some(Rect::new(x, y, w, h)),
            "{pile:?}"
        );
    }
}

/// A stretch of real play, purely through the public API: deal, skip the
/// deal animation, draw, drag the known legal move of seed 1, undo it,
/// redo it, save and reload.
#[test]
fn a_scripted_game_through_the_public_api() {
    let theme = theme();
    let session = Session::new(Options::default(), Seed::new(SEED).unwrap());
    let mut presenter = Presenter::new(session, &theme);

    // The deal animates until skipped by input.
    assert!(presenter.is_animating());
    presenter.key_down();
    assert!(!presenter.is_animating());

    // Draw three.
    presenter.pointer_down(Pt::new(30, 30));
    presenter.pointer_up(Pt::new(30, 30));
    assert_eq!(presenter.session().game().state().waste().len(), 3);

    // This seed's known legal tableau move: S3 (column 6) onto H4 (column 2).
    presenter.pointer_down(Pt::new(510, 130));
    presenter.pointer_move(Pt::new(190, 130));
    presenter.pointer_up(Pt::new(190, 130));
    let state = presenter.session().game().state();
    assert_eq!(state.tableau(2).unwrap().face_up().len(), 2);
    assert!(presenter.can_undo());

    presenter.undo().unwrap();
    assert_eq!(
        presenter
            .session()
            .game()
            .state()
            .tableau(2)
            .unwrap()
            .face_up()
            .len(),
        1
    );
    assert!(presenter.can_redo());
    presenter.redo().unwrap();
    assert_eq!(
        presenter
            .session()
            .game()
            .state()
            .tableau(2)
            .unwrap()
            .face_up()
            .len(),
        2
    );

    // Save, load into a different presenter, compare frames.
    presenter.advance(1500);
    let bytes = presenter.save_bytes().unwrap();
    let mut other = Presenter::new(
        Session::new(Options::default(), Seed::new(7).unwrap()),
        &theme,
    );
    other.load_bytes(&bytes).unwrap();
    assert_eq!(other.seed(), presenter.seed());
    assert_eq!(other.elapsed_secs(), 1);
    assert_eq!(other.frame(), presenter.frame());

    // The frame is plain data a renderer can consume: sprites reference
    // theme assets by id only.
    let frame = presenter.frame();
    assert!(frame.sprites.iter().all(|sprite| matches!(
        sprite.texture,
        TextureId::White | TextureId::Background | TextureId::Face { .. } | TextureId::Back { .. }
    )));
    let stock_backs = frame
        .sprites
        .iter()
        .filter(|s| matches!(s.texture, TextureId::Back { .. }))
        .count();
    assert!(stock_backs > 0);
}

/// The continuous-fit surface: scale from the window, logical viewport,
/// spread layout — and interactions that survive every resize.
#[test]
fn fit_viewport_adopts_scale_viewport_and_spread() {
    let mut p = Presenter::new(
        Session::new(Options::default(), Seed::new(SEED).unwrap()),
        &theme(),
    );
    // Identity before any surface arrives.
    let idle: Fit = p.fit();
    assert!((idle.scale - 1.0).abs() < f32::EPSILON);
    assert_eq!(idle.logical, Size::new(585, 384));

    let fit = p.fit_viewport(1600, 768);
    assert!((fit.scale - 2.0).abs() < f32::EPSILON);
    assert_eq!(fit.logical, Size::new(800, 384));
    assert_eq!(p.viewport(), Size::new(800, 384));
    assert_eq!(p.fit(), fit, "fit() re-derives the same fit");
    // Columns spread: stock sits at the proportional xUnit.
    assert_eq!(p.layout().pile_origin(PileId::Stock), Some(Pt::new(37, 5)));
}

/// A drag picked up before a resize drops correctly after it — logical
/// coordinates stay valid across any scale change.
#[test]
fn a_drag_survives_a_mid_drag_resize() {
    let mut p = Presenter::new(
        Session::new(Options::default(), Seed::new(SEED).unwrap()),
        &theme(),
    );
    p.key_down();
    p.fit_viewport(585, 384);
    // This seed's legal move: S3 (column 6) onto H4 (column 2).
    p.pointer_down(Pt::new(510, 130));
    // The window doubles mid-drag: same logical geometry, new scale.
    p.fit_viewport(1170, 768);
    p.pointer_move(Pt::new(190, 130));
    p.pointer_up(Pt::new(190, 130));
    assert_eq!(
        p.session()
            .game()
            .state()
            .tableau(2)
            .unwrap()
            .face_up()
            .len(),
        2,
        "the drop lands after the resize"
    );
}

/// A resize that re-spreads the columns mid-drag: the drop target is
/// found at the columns' new positions.
#[test]
fn a_drag_survives_a_mid_drag_column_spread() {
    let mut p = Presenter::new(
        Session::new(Options::default(), Seed::new(SEED).unwrap()),
        &theme(),
    );
    p.key_down();
    p.fit_viewport(585, 384);
    p.pointer_down(Pt::new(510, 130));
    // Wider window: tableau 2 moves from x=175 to x=253.
    p.fit_viewport(1600, 768);
    p.pointer_move(Pt::new(260, 130));
    p.pointer_up(Pt::new(260, 130));
    assert_eq!(
        p.session()
            .game()
            .state()
            .tableau(2)
            .unwrap()
            .face_up()
            .len(),
        2,
        "the drop lands on the re-spread column"
    );
}

/// The live drag's snap-back home follows a mid-drag re-spread:
/// dropping illegally exactly at the pile's new position needs no
/// snap-back at all.
#[test]
fn a_mid_drag_respread_refreshes_the_snap_home() {
    let mut p = Presenter::new(
        Session::new(Options::default(), Seed::new(SEED).unwrap()),
        &theme(),
    );
    p.key_down();
    p.fit_viewport(585, 384);
    // Draw three, then pick up the waste top: fan slot (121, 7),
    // grabbed at (130, 20), so grab = (-9, -13).
    p.pointer_down(Pt::new(30, 30));
    p.pointer_up(Pt::new(30, 30));
    p.pointer_down(Pt::new(130, 20));
    // Re-spread: the waste fan's top slot moves to (173, 7).
    p.fit_viewport(1600, 768);
    // Release so the run sits exactly on the refreshed home:
    // pointer (182, 20) + grab (-9, -13) = (173, 7). The drop is
    // illegal (open felt), but the run is already home — no snap-back.
    p.pointer_up(Pt::new(182, 20));
    assert!(!p.is_animating(), "the run was already at its new home");
    assert_eq!(p.session().game().state().waste().len(), 3);
}

/// A snap-back caught by a resize retargets to the pile's new position.
#[test]
fn a_snap_back_retargets_after_a_resize() {
    let mut p = Presenter::new(
        Session::new(Options::default(), Seed::new(SEED).unwrap()),
        &theme(),
    );
    p.key_down();
    p.fit_viewport(585, 384);
    // Draw three, then drag the waste top over open felt and release:
    // illegal, so it snaps back.
    p.pointer_down(Pt::new(30, 30));
    p.pointer_up(Pt::new(30, 30));
    p.pointer_down(Pt::new(130, 20));
    p.pointer_move(Pt::new(300, 300));
    p.pointer_up(Pt::new(300, 300));
    assert!(p.is_animating(), "illegal waste drop snaps back");
    // Re-spread while the run is mid-flight, then let it land.
    p.fit_viewport(1600, 768);
    for _ in 0..200 {
        p.advance(16);
    }
    assert!(!p.is_animating());
    // The waste fan's top card sits at the *new* waste position:
    // waste origin x = 2·37 + 71 = 145; fan steps (14, 1) twice.
    let expected = Rect::new(145 + 28, 5 + 2, 71, 96);
    let frame = p.frame();
    assert!(
        frame
            .sprites
            .iter()
            .any(|s| s.dst == expected && matches!(s.texture, TextureId::Face { .. })),
        "waste top at the re-spread position"
    );
}

/// Reproduction of the dev-shell bug report: at a doubled window, every
/// drop bounced back even for the one legal move of this deal
/// (S3 onto H4), while clicks (stock draws) worked. Hosts now divide
/// physical pointer coordinates through the fit before forwarding.
#[test]
fn a_legal_drop_lands_in_a_doubled_window() {
    let mut presenter = Presenter::new(
        Session::new(Options::default(), Seed::new(SEED).unwrap()),
        &theme(),
    );
    presenter.key_down();
    let fit = presenter.fit_viewport(1170, 768);
    // The host contract: physical (200, 240) at scale 2 is logical
    // (100, 120).
    assert_eq!(fit.to_logical(200, 240), Pt::new(100, 120));
    presenter.pointer_down(fit.to_logical(1020, 260));
    presenter.pointer_move(fit.to_logical(800, 260));
    presenter.pointer_move(fit.to_logical(560, 260));
    presenter.pointer_move(fit.to_logical(380, 260));
    presenter.pointer_up(fit.to_logical(380, 260));
    let state = presenter.session().game().state();
    assert_eq!(
        state.tableau(2).unwrap().face_up().len(),
        2,
        "the black three lands on the red four in the doubled window"
    );
}

/// The dev shell's real event pattern around a drag: redraw-loop
/// `advance` ticks interleaved with every pointer event, cursor travel
/// before the press, stock clicks beforehand, and an immediate retry
/// after a failed drop — in a doubled window, with the host dividing
/// pointer coordinates to logical pixels through the fit.
#[test]
fn shell_like_event_stream_still_lands_drops_in_a_doubled_window() {
    let mut p = Presenter::new(
        Session::new(Options::default(), Seed::new(SEED).unwrap()),
        &theme(),
    );
    p.key_down();
    let fit = p.fit_viewport(1170, 768);
    let tick = |p: &mut Presenter, n: u32| {
        for _ in 0..n {
            p.advance(16);
        }
    };
    // The player first clicks through the stock once (draw three).
    tick(&mut p, 30);
    p.pointer_move(fit.to_logical(60, 60));
    p.pointer_down(fit.to_logical(60, 60));
    tick(&mut p, 3);
    p.pointer_up(fit.to_logical(60, 60));
    assert_eq!(p.session().game().state().waste().len(), 3);

    // Then drags S3 onto H4 with the cursor wandering in.
    tick(&mut p, 20);
    for x in [900, 940, 980, 1020] {
        p.pointer_move(fit.to_logical(x, 260));
        tick(&mut p, 1);
    }
    p.pointer_down(fit.to_logical(1020, 260));
    tick(&mut p, 2);
    for x in [900, 750, 600, 500, 420, 380] {
        p.pointer_move(fit.to_logical(x, 260));
        tick(&mut p, 2);
    }
    p.pointer_up(fit.to_logical(380, 260));
    tick(&mut p, 5);
    let state = p.session().game().state();
    assert_eq!(
        state.tableau(2).unwrap().face_up().len(),
        2,
        "S3 lands on H4 through the shell-like event stream"
    );

    // Then drags the waste top over open felt: the illegal drop snaps
    // back.
    p.pointer_move(fit.to_logical(260, 40));
    p.pointer_down(fit.to_logical(260, 40));
    tick(&mut p, 2);
    p.pointer_move(fit.to_logical(600, 600));
    tick(&mut p, 2);
    p.pointer_up(fit.to_logical(600, 600));
    assert!(p.is_animating(), "illegal waste drop snaps back");
    tick(&mut p, 10);

    // A fast second press on the same card is the original's
    // double-click (here: a rejected waste-to-foundation attempt, no
    // pickup) — the waste top never leaves its logical fan slot.
    let waste_top = Rect::new(121, 7, 71, 96);
    p.pointer_down(fit.to_logical(260, 40));
    p.pointer_move(fit.to_logical(600, 300));
    let dragged_away = !p
        .frame()
        .sprites
        .iter()
        .any(|s| s.dst == waste_top && matches!(s.texture, TextureId::Face { .. }));
    p.pointer_up(fit.to_logical(600, 300));
    assert!(
        !dragged_away,
        "the fast second press double-clicks, not drags"
    );

    // Past the double-click window the same press is a pickup again.
    tick(&mut p, 40);
    p.pointer_down(fit.to_logical(260, 40));
    p.pointer_move(fit.to_logical(600, 300));
    let dragged_away = !p
        .frame()
        .sprites
        .iter()
        .any(|s| s.dst == waste_top && matches!(s.texture, TextureId::Face { .. }));
    p.pointer_up(fit.to_logical(600, 300));
    assert!(dragged_away, "a later press drags the waste top normally");
}
