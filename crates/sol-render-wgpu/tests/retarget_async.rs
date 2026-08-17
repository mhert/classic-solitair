//! Tests for the asynchronous atlas-retargeting API: `adopt_scale` plans
//! and hands out a self-contained [`AtlasBuildJob`] only when the loaded
//! atlas actually needs to change, `apply_atlas` resolves a finished
//! build (applying it if still wanted, discarding it if stale), and the
//! one-slot previous-atlas cache absorbs oscillation across a single
//! factor boundary without ever rebuilding. `set_display_scale` (covered
//! by `scaling_smoke.rs`) is this API reimplemented as a synchronous
//! adopt/run/apply loop; these tests drive the split API directly, the
//! way an off-thread frontend would.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use sol_engine::{Rank, Suit};
use sol_presenter::{DisplayList, Rect, Rgba, Sprite, TextureId};
use sol_render_wgpu::{AtlasBuildJob, BuiltAtlas, Renderer};
use sol_theme::CardScaling;

const FELT: [u8; 4] = [0, 128, 0, 255];

#[allow(clippy::cast_precision_loss)] // tiny test factors
fn scale(n: u32) -> f32 {
    n as f32
}

/// One ace-of-spades sprite over a felt clear, matching `scaling_smoke`'s
/// fixture so the pixel math here reuses its known-good coordinates.
fn ace_frame(card: Rect) -> DisplayList {
    DisplayList {
        clear: Some(Rgba::opaque(0, 128, 0)),
        sprites: vec![Sprite {
            texture: TextureId::Face {
                suit: Suit::Spades,
                rank: Rank::Ace,
            },
            src: Rect::new(0, 0, 4, 6),
            dst: card,
            z: 0,
            tint: Rgba::WHITE,
        }],
    }
}

/// Compile-time proof of the crate's thread-free contract: both new
/// public types cross a thread boundary on their own (`Send + 'static`),
/// which is what lets a frontend run [`AtlasBuildJob::run`] on a thread
/// of its own and hand the resulting [`BuiltAtlas`] back — the renderer
/// never spawns anything itself. Never called; a violation would fail to
/// compile.
fn assert_send_and_static<T: Send + 'static>() {}

#[test]
fn build_job_and_built_atlas_cross_a_thread_boundary() {
    assert_send_and_static::<AtlasBuildJob>();
    assert_send_and_static::<BuiltAtlas>();
}

/// Adopting within the loaded factor's ceiling (atlas at 2, a scale that
/// still ceils to 2) needs no rebuild.
#[test]
fn adopt_within_the_loaded_factor_returns_no_job() {
    let Some(gpu) = common::gpu() else { return };
    let mut renderer = Renderer::new(
        &gpu.device,
        &gpu.queue,
        wgpu::TextureFormat::Rgba8Unorm,
        common::tiny_vector_theme(),
        CardScaling::Original,
        scale(2),
    )
    .expect("renderer");
    assert_eq!(renderer.atlas_factor(), 2);

    let job = renderer
        .adopt_scale(&gpu.device, &gpu.queue, 1.7)
        .expect("adopt_scale");
    assert!(job.is_none(), "1.7 still ceils within the loaded factor 2");
    assert_eq!(renderer.atlas_factor(), 2, "atlas unchanged");
}

/// Crossing a factor boundary hands out a job naming the right factor;
/// the loaded atlas stays put until the built result is applied, and a
/// rendered frame afterward shows the new factor's card art.
#[test]
fn adopt_across_a_boundary_yields_a_job_that_applies() {
    let Some(gpu) = common::gpu() else { return };
    let mut renderer = Renderer::new(
        &gpu.device,
        &gpu.queue,
        wgpu::TextureFormat::Rgba8Unorm,
        common::tiny_vector_theme(),
        CardScaling::Original,
        scale(1),
    )
    .expect("renderer");
    assert_eq!(renderer.atlas_factor(), 1);

    let job: AtlasBuildJob = renderer
        .adopt_scale(&gpu.device, &gpu.queue, 2.0)
        .expect("adopt_scale")
        .expect("crossing to factor 2 yields a job");
    assert_eq!(job.factor(), 2);
    assert_eq!(renderer.atlas_factor(), 1, "unchanged until applied");

    let built: BuiltAtlas = job.run().expect("run");
    let follow_up = renderer.apply_atlas(&gpu.device, &gpu.queue, built);
    assert!(follow_up.is_none(), "the want is satisfied");
    assert_eq!(renderer.atlas_factor(), 2, "applied");

    // The 4×6 card at logical (8, 8): the scene transform doubles it,
    // mapping the factor-2 atlas texels 1:1 onto physical pixels (x
    // 16..24, y 16..28).
    let pixels = common::render_and_read(
        &gpu,
        &mut renderer,
        64,
        64,
        &[ace_frame(Rect::new(8, 8, 4, 6))],
    );
    assert_eq!(common::pixel_at(&pixels, 64, 18, 20), [255, 0, 0, 255]);
    assert_eq!(common::pixel_at(&pixels, 64, 50, 50), FELT, "felt clear");
}

/// Two adopts that both plan to the same factor must not hand out two
/// jobs — the second is a no-op until the first is resolved.
#[test]
fn two_adopts_toward_the_same_factor_do_not_duplicate_the_job() {
    let Some(gpu) = common::gpu() else { return };
    let mut renderer = Renderer::new(
        &gpu.device,
        &gpu.queue,
        wgpu::TextureFormat::Rgba8Unorm,
        common::tiny_vector_theme(),
        CardScaling::Original,
        scale(1),
    )
    .expect("renderer");

    let first = renderer
        .adopt_scale(&gpu.device, &gpu.queue, 2.0)
        .expect("adopt_scale")
        .expect("crossing to factor 2 yields a job");
    assert_eq!(first.factor(), 2);

    // 1.6 also ceils to 2: the same planned factor, via a different
    // scale value.
    let second = renderer
        .adopt_scale(&gpu.device, &gpu.queue, 1.6)
        .expect("adopt_scale");
    assert!(second.is_none(), "an identical job is already out");
    assert_eq!(renderer.atlas_factor(), 1, "still unapplied");
}

/// Adopting toward 2 and then toward 3 before either build lands: the
/// stale factor-2 result must be discarded without touching the loaded
/// atlas or duplicating the factor-3 job that is already out, and the
/// factor-3 result lands cleanly once it arrives.
#[test]
fn a_stale_apply_is_discarded_while_a_newer_job_is_already_out() {
    let Some(gpu) = common::gpu() else { return };
    let mut renderer = Renderer::new(
        &gpu.device,
        &gpu.queue,
        wgpu::TextureFormat::Rgba8Unorm,
        common::tiny_vector_theme(),
        CardScaling::Original,
        scale(1),
    )
    .expect("renderer");

    let job2 = renderer
        .adopt_scale(&gpu.device, &gpu.queue, 2.0)
        .expect("adopt_scale")
        .expect("crossing to factor 2 yields a job");
    assert_eq!(job2.factor(), 2);

    let job3 = renderer
        .adopt_scale(&gpu.device, &gpu.queue, 3.0)
        .expect("adopt_scale")
        .expect("crossing to factor 3 yields a job");
    assert_eq!(job3.factor(), 3);

    let built2 = job2.run().expect("run factor 2");
    let follow_up = renderer.apply_atlas(&gpu.device, &gpu.queue, built2);
    assert!(
        follow_up.is_none(),
        "the factor-3 job is already out; no duplicate follow-up"
    );
    assert_eq!(
        renderer.atlas_factor(),
        1,
        "stale factor-2 result discarded, loaded factor unchanged"
    );

    let built3 = job3.run().expect("run factor 3");
    let follow_up = renderer.apply_atlas(&gpu.device, &gpu.queue, built3);
    assert!(follow_up.is_none());
    assert_eq!(renderer.atlas_factor(), 3, "the fresh result lands");
}

/// Building and applying factor 2 from 1, then oscillating back across
/// the same boundary in both directions, needs no further job either
/// way — the one-slot cache absorbs it — and a render after a cache
/// swap draws correctly.
#[test]
fn the_cache_absorbs_oscillation_across_one_boundary() {
    let Some(gpu) = common::gpu() else { return };
    let mut renderer = Renderer::new(
        &gpu.device,
        &gpu.queue,
        wgpu::TextureFormat::Rgba8Unorm,
        common::tiny_vector_theme(),
        CardScaling::Original,
        scale(1),
    )
    .expect("renderer");

    let job = renderer
        .adopt_scale(&gpu.device, &gpu.queue, 2.0)
        .expect("adopt_scale")
        .expect("crossing to factor 2 yields a job");
    let built = job.run().expect("run");
    assert!(
        renderer
            .apply_atlas(&gpu.device, &gpu.queue, built)
            .is_none()
    );
    assert_eq!(renderer.atlas_factor(), 2);

    // Back below the boundary: the outgoing factor-1 atlas was retained
    // by the apply above, so this is an immediate cache swap, no job.
    let back = renderer
        .adopt_scale(&gpu.device, &gpu.queue, 1.0)
        .expect("adopt_scale");
    assert!(back.is_none(), "the factor-1 atlas is cached");
    assert_eq!(renderer.atlas_factor(), 1, "swapped back immediately");

    // Up again: the factor-2 atlas is now the cached one, in its place.
    let up = renderer
        .adopt_scale(&gpu.device, &gpu.queue, 2.0)
        .expect("adopt_scale");
    assert!(up.is_none(), "the factor-2 atlas is cached in its place");
    assert_eq!(renderer.atlas_factor(), 2, "swapped forward immediately");

    let pixels = common::render_and_read(
        &gpu,
        &mut renderer,
        64,
        64,
        &[ace_frame(Rect::new(8, 8, 4, 6))],
    );
    assert_eq!(common::pixel_at(&pixels, 64, 18, 20), [255, 0, 0, 255]);
    assert_eq!(common::pixel_at(&pixels, 64, 50, 50), FELT, "felt clear");
}

/// While a build is pending (adopted across a boundary, deliberately
/// left unapplied), a render call still stretches the current atlas by
/// the new continuous scale — the scene transform is scale-driven
/// independent of which atlas is loaded.
#[test]
fn render_stretches_the_current_atlas_while_a_job_is_pending() {
    let Some(gpu) = common::gpu() else { return };
    let mut renderer = Renderer::new(
        &gpu.device,
        &gpu.queue,
        wgpu::TextureFormat::Rgba8Unorm,
        common::tiny_vector_theme(),
        CardScaling::Original,
        scale(1),
    )
    .expect("renderer");

    let job = renderer
        .adopt_scale(&gpu.device, &gpu.queue, 1.5)
        .expect("adopt_scale");
    assert!(job.is_some(), "1.5 crosses the boundary to factor 2");
    assert_eq!(renderer.atlas_factor(), 1, "not applied yet");

    // Same geometry as scaling_smoke's fractional-scale test: a 4×6 card
    // at logical (10, 10) covers physical x 15..21, y 15..24 at 1.5x.
    let pixels = common::render_and_read(
        &gpu,
        &mut renderer,
        64,
        64,
        &[ace_frame(Rect::new(10, 10, 4, 6))],
    );
    let center = common::pixel_at(&pixels, 64, 18, 19);
    assert!(center[0] > 200, "card interior is red at 1.5x: {center:?}");
    assert_eq!(
        common::pixel_at(&pixels, 64, 12, 12),
        FELT,
        "left of the scaled card stays felt"
    );
    assert_eq!(
        common::pixel_at(&pixels, 64, 40, 40),
        FELT,
        "far felt stays felt"
    );
}

/// Crossing two boundaries before either build lands leaves *both* jobs
/// outstanding; a third adopt back toward the first factor must still find
/// its job in flight and hand out nothing. Tracking only the most recent
/// handed-out factor would forget the first job and duplicate it here.
#[test]
fn a_readopt_across_an_earlier_boundary_does_not_duplicate_its_job() {
    let Some(gpu) = common::gpu() else { return };
    let mut renderer = Renderer::new(
        &gpu.device,
        &gpu.queue,
        wgpu::TextureFormat::Rgba8Unorm,
        common::tiny_vector_theme(),
        CardScaling::Original,
        scale(1),
    )
    .expect("renderer");

    let job2 = renderer
        .adopt_scale(&gpu.device, &gpu.queue, 2.0)
        .expect("adopt_scale")
        .expect("crossing to factor 2 yields a job");
    assert_eq!(job2.factor(), 2);

    let job3 = renderer
        .adopt_scale(&gpu.device, &gpu.queue, 3.0)
        .expect("adopt_scale")
        .expect("crossing to factor 3 yields a job");
    assert_eq!(job3.factor(), 3);

    // Factor 2's job is still out from the first adopt; readopting toward
    // it must not hand out a second one.
    let again = renderer
        .adopt_scale(&gpu.device, &gpu.queue, 2.0)
        .expect("adopt_scale");
    assert!(again.is_none(), "the factor-2 job is still outstanding");
    assert_eq!(renderer.atlas_factor(), 1, "nothing applied yet");
}

/// A reported build failure ([`Renderer::job_failed`]) drops the job from
/// the outstanding set but damps retry: the same factor is not re-issued
/// until an adopt plans a *different* factor first, after which returning to
/// the once-failed factor builds again — even while another factor's job is
/// still outstanding. Meanwhile the renderer keeps drawing on the atlas it
/// already has, stretched by the adopted continuous scale.
#[test]
fn a_reported_failure_damps_retry_until_the_plan_moves_away_and_back() {
    let Some(gpu) = common::gpu() else { return };
    let mut renderer = Renderer::new(
        &gpu.device,
        &gpu.queue,
        wgpu::TextureFormat::Rgba8Unorm,
        common::tiny_vector_theme(),
        CardScaling::Original,
        scale(1),
    )
    .expect("renderer");

    let job = renderer
        .adopt_scale(&gpu.device, &gpu.queue, 2.0)
        .expect("adopt_scale")
        .expect("crossing to factor 2 yields a job");
    assert_eq!(job.factor(), 2);

    // The frontend ran the job off-thread and it failed; report it instead
    // of applying a built atlas. The factor leaves the outstanding set.
    renderer.job_failed(job.factor());

    // Same plan again (1.9 also ceils to 2): the failed factor is damped, so
    // no retry until the plan actually moves away from it.
    let damped = renderer
        .adopt_scale(&gpu.device, &gpu.queue, 1.9)
        .expect("adopt_scale");
    assert!(damped.is_none(), "factor 2 is damped after its failure");
    assert_eq!(renderer.atlas_factor(), 1, "still on the factor-1 atlas");

    // Plan a different factor: crossing to 3 lifts the damping on 2.
    let job3 = renderer
        .adopt_scale(&gpu.device, &gpu.queue, 3.0)
        .expect("adopt_scale")
        .expect("crossing to factor 3 yields a job");
    assert_eq!(job3.factor(), 3);

    // Back to the once-failed factor: the plan moved away and returned, so a
    // fresh job is handed out again. Factor 3's job is still outstanding but
    // must not block factor 2.
    let retry = renderer
        .adopt_scale(&gpu.device, &gpu.queue, 2.0)
        .expect("adopt_scale")
        .expect("factor 2 builds again once the plan moved away and back");
    assert_eq!(retry.factor(), 2);
    assert_eq!(renderer.atlas_factor(), 1, "nothing applied through it all");

    // The renderer still draws sanely on its current (factor-1) atlas after
    // the reported failure: the scene transform stretches it by the adopted
    // 2.0 scale, so the 4×6 card at logical (8, 8) covers physical x 16..24,
    // y 16..28 — probe its interior and a far felt pixel.
    let pixels = common::render_and_read(
        &gpu,
        &mut renderer,
        64,
        64,
        &[ace_frame(Rect::new(8, 8, 4, 6))],
    );
    let center = common::pixel_at(&pixels, 64, 18, 20);
    assert!(center[0] > 200, "card interior is red: {center:?}");
    assert_eq!(common::pixel_at(&pixels, 64, 50, 50), FELT, "felt clear");
}

/// Damping is a per-factor set, not a single slot: several jobs can be
/// outstanding and fail independently, and one failure must not clobber
/// another's damping. Failing factor 2 and then factor 3 must leave *both*
/// damped, so a readopt toward 2 with no different plan in between still
/// returns nothing. A single slot would keep only the last-failed factor, so
/// the readopt toward 2 would wrongly re-issue its job.
#[test]
fn independent_failures_damp_each_factor_separately() {
    let Some(gpu) = common::gpu() else { return };
    let mut renderer = Renderer::new(
        &gpu.device,
        &gpu.queue,
        wgpu::TextureFormat::Rgba8Unorm,
        common::tiny_vector_theme(),
        CardScaling::Original,
        scale(1),
    )
    .expect("renderer");

    let job2 = renderer
        .adopt_scale(&gpu.device, &gpu.queue, 2.0)
        .expect("adopt_scale")
        .expect("crossing to factor 2 yields a job");
    assert_eq!(job2.factor(), 2);

    let job3 = renderer
        .adopt_scale(&gpu.device, &gpu.queue, 3.0)
        .expect("adopt_scale")
        .expect("crossing to factor 3 yields a job");
    assert_eq!(job3.factor(), 3);

    // Both jobs failed off-thread; report them in order. The second report
    // must not clobber the first factor's damping.
    renderer.job_failed(job2.factor());
    renderer.job_failed(job3.factor());

    // Factor 2 is still damped — no different factor has been planned since
    // its failure — so no retry. A single-slot tracker would have forgotten
    // factor 2 when factor 3 failed and re-issued a job here.
    let still_damped = renderer
        .adopt_scale(&gpu.device, &gpu.queue, 1.9)
        .expect("adopt_scale");
    assert!(
        still_damped.is_none(),
        "factor 2 stays damped even though factor 3 also failed"
    );
    assert_eq!(renderer.atlas_factor(), 1, "nothing applied");

    // Planning factor 2 just now lifted factor 3's damping, so returning to
    // factor 3 builds again.
    let retry3 = renderer
        .adopt_scale(&gpu.device, &gpu.queue, 3.0)
        .expect("adopt_scale")
        .expect("planning 2 in between lifted factor 3's damping");
    assert_eq!(retry3.factor(), 3);

    // And planning factor 3 just now lifted factor 2's damping, so returning
    // to factor 2 builds again too — its still-outstanding factor-3 job does
    // not block it.
    let retry2 = renderer
        .adopt_scale(&gpu.device, &gpu.queue, 2.0)
        .expect("adopt_scale")
        .expect("planning 3 in between lifted factor 2's damping");
    assert_eq!(retry2.factor(), 2);
    assert_eq!(renderer.atlas_factor(), 1, "nothing applied through it all");
}

/// [`Renderer::job_failed`] for a factor that is not currently outstanding
/// is caller misuse and must be a complete no-op: it neither damps a factor
/// that was never issued nor disturbs the genuine outstanding set. Reporting
/// an unrelated factor while factor 2 is building leaves factor 2's job
/// outstanding (a readopt still dedups to `None`) and lets a never-touched
/// factor 3 still issue; and on a fresh renderer, reporting a factor before
/// any job exists plants no phantom damping, so adopting toward it still
/// builds.
#[test]
fn job_failed_for_a_never_outstanding_factor_is_a_no_op() {
    let Some(gpu) = common::gpu() else { return };
    let mut renderer = Renderer::new(
        &gpu.device,
        &gpu.queue,
        wgpu::TextureFormat::Rgba8Unorm,
        common::tiny_vector_theme(),
        CardScaling::Original,
        scale(1),
    )
    .expect("renderer");

    let job2 = renderer
        .adopt_scale(&gpu.device, &gpu.queue, 2.0)
        .expect("adopt_scale")
        .expect("crossing to factor 2 yields a job");
    assert_eq!(job2.factor(), 2);

    // A factor that was never handed out: a complete no-op, so the real
    // factor-2 job stays outstanding and no phantom damping appears.
    renderer.job_failed(99);

    // A never-touched factor still issues its job...
    let job3 = renderer
        .adopt_scale(&gpu.device, &gpu.queue, 3.0)
        .expect("adopt_scale")
        .expect("factor 3 is untouched and still issues");
    assert_eq!(job3.factor(), 3);

    // ...and re-adopting factor 2 still dedups to `None` against its still
    // outstanding job (the stray report neither resolved nor damped it).
    let readopt = renderer
        .adopt_scale(&gpu.device, &gpu.queue, 2.0)
        .expect("adopt_scale");
    assert!(readopt.is_none(), "factor 2's job is still outstanding");
    assert_eq!(renderer.atlas_factor(), 1, "nothing applied");

    // On a fresh renderer, reporting a factor before any job exists must
    // plant no phantom damping: adopting toward it still builds.
    let mut fresh = Renderer::new(
        &gpu.device,
        &gpu.queue,
        wgpu::TextureFormat::Rgba8Unorm,
        common::tiny_vector_theme(),
        CardScaling::Original,
        scale(1),
    )
    .expect("renderer");
    fresh.job_failed(4);
    let job4 = fresh
        .adopt_scale(&gpu.device, &gpu.queue, 4.0)
        .expect("adopt_scale")
        .expect("no phantom damping: factor 4 still builds");
    assert_eq!(job4.factor(), 4);
    assert_eq!(fresh.atlas_factor(), 1, "unchanged until applied");
}
