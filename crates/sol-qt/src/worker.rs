//! The render worker thread: owns [`Offscreen`] off the GUI thread so a
//! GPU wait (an atlas rebuild, a slow readback) never contends with
//! Qt's relayout during a resize.
//!
//! One channel is the whole mailbox: [`WorkerHandle`]'s calls, the
//! internal shutdown request, and a transient atlas-builder thread's
//! result all arrive on it, so the loop blocks on exactly one receiver.
//! Each iteration drains everything queued at once: control messages
//! (scale adoptions, theme swaps, build results) apply in arrival
//! order, but of any render requests drained together only the newest
//! is ever drawn — frames are dropped, never queued. A build result
//! carries the theme
//! generation live when it was dispatched; one stamped with a generation
//! the worker has since left behind — a theme swapped away underneath a
//! slow build — is dropped before it can repaint old artwork onto the
//! freshly themed board. Atlas builds normally run on their own
//! transient thread, never on the caller's thread: a build can take
//! hundreds of milliseconds, long enough to stall frames if it shared
//! the render thread. The one exception is before the worker's first
//! delivered frame: `Offscreen` starts out built at a placeholder scale
//! (the real viewport is unknown until the first resize), so the first
//! fitting `AdoptScale` would otherwise render one stretched, blurry
//! frame while the real atlas rebuilds in the background. Until a frame
//! has been delivered, a build `adopt_scale` hands out runs inline
//! instead — blocking only this thread, never the GUI's — so the
//! opening deal simply appears a beat later, already sharp, the same
//! feel the app had before atlas builds were made asynchronous at all.
//! From the first delivered frame onward, builds go back to a transient
//! thread as described above.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use sol_frontend::previews;
use sol_presenter::DisplayList;
use sol_render_wgpu::{AtlasBuildJob, BuiltAtlas};
use sol_theme::{CardScaling, Theme};

use crate::offscreen::{Frame, Offscreen};

/// An event the render worker reports back through `deliver`, called
/// from the worker thread for every event — marshalling to the GUI
/// thread is the caller's job.
#[derive(Debug)]
pub enum WorkerEvent {
    /// A frame finished rendering.
    Frame(Frame),
    /// A render or atlas build failed; the previous frame stays on
    /// screen and the worker keeps running.
    Error(String),
}

/// One message in the worker's mailbox.
enum Msg {
    /// Draw `list` at this size; only the newest survives a drain.
    Render(DisplayList, u32, u32),
    /// A control message: applied in arrival order, never dropped.
    Control(Control),
    /// Stop the loop; cuts a drain immediately, discarding the rest of
    /// it (including any render request seen earlier in the same
    /// drain).
    Shutdown,
}

/// A control message: applied in arrival order, distinct from render
/// requests (of which only the newest in a drained batch survives).
enum Control {
    /// Adopt a continuous display scale.
    AdoptScale(f32),
    /// Swap the theme; the reply carries the rebuild verdict. Boxed: a
    /// `Theme` is by far the largest payload on this channel.
    SetTheme(
        Box<Theme>,
        CardScaling,
        f32,
        mpsc::Sender<Result<(), String>>,
    ),
    /// Render one card-back contact sheet immediately and reply with its
    /// pixels. A control message rather than a [`Msg::Render`] request:
    /// of a drained batch's render requests only the newest survives, but
    /// the Options dialog waiting on this one needs exactly this result,
    /// not whatever the playfield's own next frame happens to be.
    RenderSheet(
        DisplayList,
        u32,
        u32,
        f32,
        mpsc::Sender<Result<Vec<u8>, String>>,
    ),
    /// A transient builder thread finished the job for this factor
    /// (captured before `run` consumed it): the built atlas on success,
    /// or the error text on failure. The leading counter is the theme
    /// generation captured when the build was dispatched; a result whose
    /// generation no longer matches the worker's is for a theme swapped
    /// away since and is dropped in [`decide`] before it can touch the
    /// renderer.
    BuildDone(u64, u32, Result<BuiltAtlas, String>),
}

/// One drained render request: `list` at `width`×`height`.
#[derive(Debug, Clone)]
struct RenderReq {
    list: DisplayList,
    width: u32,
    height: u32,
}

/// What one drained batch resolves to, before any of it touches the GPU.
enum Decision {
    /// Apply `controls` in order, then run `render` if it is `Some` —
    /// the newest render request seen anywhere in the batch.
    Batch {
        controls: Vec<Control>,
        render: Option<RenderReq>,
    },
    /// A shutdown was seen; everything else in the batch is discarded.
    Shutdown,
}

/// Resolves one drained batch — `first` is the blocking `recv()`
/// result, `queue` is everything `try_recv` gathered right after it —
/// into what the worker loop must do. `generation` is the worker's
/// current theme generation: a build result stamped with any other
/// generation is for a theme already swapped away and is filtered out
/// here, so the loop never paints stale artwork onto the new board and
/// never touches the current renderer's bookkeeping for a job it never
/// issued.
///
/// Pure and GPU-free so the ordering rules are unit-testable on their
/// own: controls keep arrival order, only the newest render survives,
/// stale-generation build results are dropped, and a shutdown cuts the
/// batch immediately (nothing after it, and nothing recorded before it
/// either, is kept).
fn decide(first: Msg, queue: Vec<Msg>, generation: u64) -> Decision {
    let mut controls = Vec::new();
    let mut render = None;
    for msg in std::iter::once(first).chain(queue) {
        match msg {
            Msg::Shutdown => return Decision::Shutdown,
            Msg::Render(list, width, height) => {
                render = Some(RenderReq {
                    list,
                    width,
                    height,
                });
            }
            // A build result from a superseded theme generation is
            // dropped outright: its artwork is for a theme already
            // swapped away, and the current renderer never issued the
            // job, so its bookkeeping stays untouched (no `job_failed`)
            // and its obsolescence raises no error.
            Msg::Control(Control::BuildDone(build_generation, ..))
                if build_generation != generation => {}
            Msg::Control(control) => controls.push(control),
        }
    }
    Decision::Batch { controls, render }
}

/// The theme generation after a swap attempt. It advances only when the
/// swap succeeded — a failed swap leaves the previous theme (and its
/// renderer, and every build dispatched against it) in place, so the
/// generation must not move. Pulled out as its own step so the
/// success-only rule is unit-testable without a GPU.
fn generation_after_swap(generation: u64, swap_ok: bool) -> u64 {
    if swap_ok { generation + 1 } else { generation }
}

/// Whether an `AdoptScale`'s build job must run inline on the worker
/// thread rather than dispatch to a transient builder thread: true until
/// the worker's first frame is delivered, false forever after. Pulled
/// out as its own step so the inline-vs-dispatch rule is unit-testable
/// without a GPU, the same way [`generation_after_swap`] is.
fn builds_run_inline(delivered_frame: bool) -> bool {
    !delivered_frame
}

/// Spawns a transient thread to run `job`, off the render worker: a
/// build can take hundreds of milliseconds, long enough to stall frames
/// if it ran inline. The result reports back through `sender` as a
/// [`Control::BuildDone`] stamped with `generation` (the theme
/// generation live at dispatch), so the worker loop still blocks on
/// exactly one receiver and can discard the result if a theme swap has
/// superseded it in the meantime.
///
/// A failed SPAWN (not a failed build) is reported and fails the job
/// right here instead: no builder thread will ever exist to report it
/// otherwise, and the renderer must not wait for it forever. This path
/// runs against the live renderer for the current generation, so failing
/// the job here is always correct.
fn spawn_atlas_build(
    job: AtlasBuildJob,
    generation: u64,
    sender: &mpsc::Sender<Msg>,
    offscreen: &mut Offscreen,
    deliver: &mut dyn FnMut(WorkerEvent),
) {
    let factor = job.factor();
    let sender = sender.clone();
    let spawned = thread::Builder::new()
        .name(String::from("sol-qt-atlas"))
        .spawn(move || {
            let result = job.run().map_err(|error| error.to_string());
            let _ = sender.send(Msg::Control(Control::BuildDone(generation, factor, result)));
        });
    if let Err(error) = spawned {
        offscreen.job_failed(factor);
        deliver(WorkerEvent::Error(format!(
            "starting the atlas build thread: {error}"
        )));
    }
}

/// Runs `job` — and any follow-up job [`Offscreen::apply_atlas`] hands
/// back — inline on the worker thread, applying each result as soon as
/// it lands instead of dispatching to a transient builder thread. Used
/// only [before the worker's first delivered frame](builds_run_inline):
/// the placeholder-scale startup atlas would otherwise sit on screen,
/// stretched, for however long the real build takes in the background.
/// Blocking here blocks only the worker thread, never the GUI thread —
/// the playfield simply appears a beat later, exactly the pre-async
/// startup feel. A resize storm before the first frame can retarget
/// mid-sequence, chaining several follow-up jobs; each runs and applies
/// in turn before control returns to the batch loop.
///
/// Bypasses the generation machinery entirely: running on this same
/// thread, nothing can interleave and swap the theme out from under it,
/// so there is no stale result to guard against the way
/// [`spawn_atlas_build`]'s dispatch must.
///
/// A failed run reports [`job_failed`](Offscreen::job_failed) (the
/// factor captured before `run` consumes the job, same as the async
/// path) and emits an error event, then stops — there is no built atlas
/// left to feed onward.
fn run_build_inline(
    mut job: AtlasBuildJob,
    offscreen: &mut Offscreen,
    deliver: &mut dyn FnMut(WorkerEvent),
) {
    loop {
        let factor = job.factor();
        match job.run() {
            Ok(built) => match offscreen.apply_atlas(built) {
                Some(next) => job = next,
                None => break,
            },
            Err(error) => {
                offscreen.job_failed(factor);
                deliver(WorkerEvent::Error(error.to_string()));
                break;
            }
        }
    }
}

/// The worker thread body: owns `offscreen` until a [`Msg::Shutdown`]
/// arrives, blocking on `receiver` between batches. `self_sender` is a
/// clone of the same channel's sender, held for the whole loop so that
/// spawned atlas-builder threads can report back on it — and, as a
/// consequence, so the channel can never disconnect on its own; only an
/// explicit shutdown message ends the loop.
///
/// It also owns the theme generation counter, bumped on every successful
/// swap and stamped onto each build it dispatches, so a build result for
/// a since-swapped theme is recognized as stale and dropped rather than
/// painting old artwork onto the new board.
///
/// After any delivered build result, the most recent render request (if
/// any) replays so fresh texels reach `deliver` without waiting for the
/// next external request — unless the same drain already carried a fresh
/// request, which wins instead. The replay is armed for every `Ok`
/// result, not only those that swap a new atlas in: `apply_atlas`
/// reports the follow-up step, not whether it applied, and may discard a
/// result as redundant (the want moved on while it built, or that factor
/// is already on screen), in which case the replay just re-renders the
/// same pixels — one harmless extra frame. Stale-generation results
/// never reach this point (`decide` drops them), so a replay never
/// repaints old artwork. This merge (and the atlas-apply/build-dispatch
/// behavior above) is exercised by the GPU-gated end-to-end test rather
/// than a pure unit test, since it depends on real render/build
/// outcomes.
///
/// It also owns the `delivered_frame` latch: false until the first
/// `Ok` render hands a frame to `deliver`, true forever after. While
/// false, [`builds_run_inline`] routes every `AdoptScale` build through
/// [`run_build_inline`] instead of [`spawn_atlas_build`] — see the
/// module docs for why.
fn worker_loop(
    mut offscreen: Offscreen,
    receiver: &mpsc::Receiver<Msg>,
    self_sender: &mpsc::Sender<Msg>,
    deliver: &mut dyn FnMut(WorkerEvent),
) {
    let mut last_render: Option<RenderReq> = None;
    // The theme generation, bumped on every successful swap. Builds are
    // stamped with it at dispatch so their results can be recognized as
    // stale (from a theme since swapped away) and dropped.
    let mut generation: u64 = 0;
    // Latches true the first time a render actually delivers a frame;
    // gates whether an AdoptScale's build runs inline or dispatches to a
    // transient thread (see `builds_run_inline`).
    let mut delivered_frame = false;
    loop {
        let Ok(first) = receiver.recv() else {
            // `self_sender` keeps one Sender alive for this whole loop,
            // so the channel disconnecting here would mean this
            // function had already returned. Kept as a safe exit
            // rather than an infinite spin if that is ever violated.
            break;
        };
        let mut queue = Vec::new();
        while let Ok(msg) = receiver.try_recv() {
            queue.push(msg);
        }
        let Decision::Batch { controls, render } = decide(first, queue, generation) else {
            break;
        };

        let mut replay_wanted = false;
        for control in controls {
            match control {
                Control::AdoptScale(scale) => match offscreen.adopt_scale(scale) {
                    Ok(Some(job)) if builds_run_inline(delivered_frame) => {
                        run_build_inline(job, &mut offscreen, deliver);
                    }
                    Ok(Some(job)) => {
                        spawn_atlas_build(job, generation, self_sender, &mut offscreen, deliver);
                    }
                    Ok(None) => {}
                    Err(error) => deliver(WorkerEvent::Error(error.to_string())),
                },
                Control::SetTheme(theme, scaling, scale, reply) => {
                    let verdict = offscreen
                        .set_theme(*theme, scaling, scale)
                        .map_err(|error| error.to_string());
                    // A successful swap replaces the whole renderer, so
                    // every build dispatched against the previous theme is
                    // now obsolete. Advancing the generation is what lets
                    // their results be recognized as stale and dropped.
                    generation = generation_after_swap(generation, verdict.is_ok());
                    let _ = reply.send(verdict);
                }
                Control::RenderSheet(list, width, height, scale, reply) => {
                    let result = offscreen
                        .render_sheet(&list, (width, height), scale)
                        .map_err(|error| error.to_string());
                    let _ = reply.send(result);
                }
                // A straggler whose generation a swap earlier in THIS
                // batch has already left behind — one `decide` could not
                // know was stale, since it cannot see whether a swap
                // succeeds. Disown it silently against the live
                // generation: no apply, no `job_failed`, no error, so an
                // obsolete build never disturbs the freshly swapped
                // renderer's bookkeeping.
                Control::BuildDone(build_generation, ..) if build_generation != generation => {}
                Control::BuildDone(_, _, Ok(built)) => {
                    if let Some(job) = offscreen.apply_atlas(built) {
                        spawn_atlas_build(job, generation, self_sender, &mut offscreen, deliver);
                    }
                    // Armed for every delivered result: `apply_atlas`
                    // reports the follow-up step, not whether it swapped
                    // the atlas in, so there is nothing to branch on. When
                    // it discarded the result as redundant (the want moved
                    // on, or that factor is already on screen) the replay
                    // just re-renders the same pixels — one harmless extra
                    // frame, never stale artwork.
                    replay_wanted = true;
                }
                Control::BuildDone(_, factor, Err(reason)) => {
                    offscreen.job_failed(factor);
                    deliver(WorkerEvent::Error(reason));
                }
            }
        }

        let to_render = match render {
            Some(request) => Some(request),
            None if replay_wanted => last_render.clone(),
            None => None,
        };
        if let Some(request) = to_render {
            match offscreen.render(request.width, request.height, &request.list) {
                Ok(frame) => {
                    delivered_frame = true;
                    deliver(WorkerEvent::Frame(frame));
                }
                Err(error) => deliver(WorkerEvent::Error(error.to_string())),
            }
            last_render = Some(request);
        }
    }
}

/// Raises the `stopped` flag when dropped, so the worker thread ending
/// sets it on every exit path — a clean return or a panic-unwind (e.g.
/// wgpu's uncaptured-error panic) alike. Armed before the loop runs so no
/// exit can bypass it; without it, [`WorkerHandle::drop`] would burn its
/// whole bounded wait on a worker that has already gone.
struct StoppedGuard(Arc<AtomicBool>);

impl Drop for StoppedGuard {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

/// The GUI thread's grip on the render worker. Every method returns
/// promptly: render requests are dropped rather than queued when the
/// worker falls behind, and the one call that needs an answer (a theme
/// swap) waits with a bounded timeout.
pub struct WorkerHandle {
    sender: mpsc::Sender<Msg>,
    /// Raised when the worker thread ends — a clean shutdown or a
    /// panic-unwind alike (a [`StoppedGuard`] arms it before the loop
    /// runs). The bounded shutdown handshake on [`Drop`] waits on this.
    stopped: Arc<AtomicBool>,
    /// Latched once a fire-and-forget send is seen to fail: the worker's
    /// receiver is dropped, so its thread is gone. Once set, further
    /// sends are skipped and [`Self::is_gone`] stays true.
    gone: AtomicBool,
    /// The device's texture size ceiling, captured once by the worker
    /// thread right after it takes ownership of `offscreen` — `0` until
    /// then, resolved through [`previews::resolve_max_texture_dim`] by
    /// [`Self::max_texture_dim`].
    max_texture_dim: Arc<AtomicU32>,
}

impl WorkerHandle {
    /// Spawns the render worker thread, moving `offscreen` onto it.
    /// `deliver` is called from the worker thread for every
    /// [`WorkerEvent`]; marshalling it to the GUI thread is the
    /// caller's job.
    ///
    /// # Errors
    ///
    /// A [`std::io::Error`] if the thread cannot be spawned. `offscreen`
    /// is already built by the caller, so startup itself never fails
    /// here.
    pub fn start(
        offscreen: Offscreen,
        mut deliver: impl FnMut(WorkerEvent) + Send + 'static,
    ) -> std::io::Result<Self> {
        let (sender, receiver) = mpsc::channel::<Msg>();
        let self_sender = sender.clone();
        let stopped = Arc::new(AtomicBool::new(false));
        let thread_stopped = Arc::clone(&stopped);
        let max_texture_dim = Arc::new(AtomicU32::new(0));
        let thread_max_texture_dim = Arc::clone(&max_texture_dim);
        thread::Builder::new()
            .name(String::from("sol-qt-render"))
            .spawn(move || {
                // Armed before the loop so `stopped` is raised on every
                // exit, a panic-unwind included, not just a clean return.
                let _stopped_guard = StoppedGuard(thread_stopped);
                // Captured once, here, rather than per render: `offscreen`
                // never changes its device, so the ceiling never changes
                // either.
                thread_max_texture_dim.store(offscreen.max_texture_dim(), Ordering::Release);
                worker_loop(offscreen, &receiver, &self_sender, &mut deliver);
            })?;
        Ok(Self {
            sender,
            stopped,
            gone: AtomicBool::new(false),
            max_texture_dim,
        })
    }

    /// Queues one render request. Only the newest request queued at the
    /// moment the worker next drains its mailbox is ever drawn; older
    /// ones in the same drain are dropped. Never blocks.
    ///
    /// A no-op once the worker is [gone](Self::is_gone); a send that
    /// fails (the worker's thread ended or panicked) latches that state.
    pub fn render(&self, list: DisplayList, width: u32, height: u32) {
        self.send_or_mark_gone(Msg::Render(list, width, height));
    }

    /// Queues a continuous display scale adoption. Applied before any
    /// render request queued after it on this handle.
    ///
    /// A no-op once the worker is [gone](Self::is_gone); a send that
    /// fails (the worker's thread ended or panicked) latches that state.
    pub fn adopt_scale(&self, scale: f32) {
        self.send_or_mark_gone(Msg::Control(Control::AdoptScale(scale)));
    }

    /// Sends one fire-and-forget message, latching [`Self::is_gone`] if
    /// the worker's receiver has been dropped (its thread ended or
    /// panicked). A no-op once the worker is already known gone, so a
    /// dead worker is never chased with more sends.
    fn send_or_mark_gone(&self, msg: Msg) {
        if self.is_gone() {
            return;
        }
        if self.sender.send(msg).is_err() {
            self.gone.store(true, Ordering::Release);
        }
    }

    /// Whether the render worker is gone: its thread has ended (a clean
    /// shutdown or a panic-unwind both raise `stopped`), or a
    /// fire-and-forget send has failed. Latches — once true, always true
    /// — and once true every send on this handle is skipped.
    #[must_use]
    pub fn is_gone(&self) -> bool {
        self.stopped.load(Ordering::Acquire) || self.gone.load(Ordering::Acquire)
    }

    /// The device's texture size ceiling, for laying out a card-back
    /// contact sheet that must fit one texture. Conservatively the
    /// guaranteed floor (see [`previews::resolve_max_texture_dim`]) until
    /// the worker thread has captured the real value.
    #[must_use]
    pub fn max_texture_dim(&self) -> u32 {
        previews::resolve_max_texture_dim(self.max_texture_dim.load(Ordering::Acquire))
    }

    /// Swaps the theme and waits (bounded) for the verdict.
    ///
    /// # Errors
    ///
    /// The rebuild's error text; a note that the worker did not answer
    /// in time (a stalled build or render); or that the worker is
    /// [gone](Self::is_gone) — the caller keeps the previous theme
    /// active in every case.
    pub fn set_theme(&self, theme: Theme, scaling: CardScaling, scale: f32) -> Result<(), String> {
        if self.is_gone() {
            return Err(String::from("the render worker is gone"));
        }
        let (reply_sender, reply_receiver) = mpsc::channel();
        if self
            .sender
            .send(Msg::Control(Control::SetTheme(
                Box::new(theme),
                scaling,
                scale,
                reply_sender,
            )))
            .is_err()
        {
            self.gone.store(true, Ordering::Release);
            return Err(String::from("the render worker is gone"));
        }
        match reply_receiver.recv_timeout(Duration::from_secs(2)) {
            Ok(verdict) => verdict,
            Err(_) => Err(String::from(
                "the render worker did not respond; keeping the current theme",
            )),
        }
    }

    /// Renders one card-back contact sheet and waits (bounded) for its
    /// pixels — modeled on [`Self::set_theme`]'s bounded request/reply
    /// shape: a control message, not a render request, because the
    /// dialog waiting on it needs this exact result rather than whatever
    /// the newest queued render happens to be.
    ///
    /// # Errors
    ///
    /// The render's error text; a note that the worker did not respond
    /// in time and card back previews are unavailable; or that the
    /// worker is [gone](Self::is_gone).
    pub fn render_sheet(
        &self,
        list: DisplayList,
        width: u32,
        height: u32,
        scale: f32,
    ) -> Result<Vec<u8>, String> {
        if self.is_gone() {
            return Err(String::from("the render worker is gone"));
        }
        let (reply_sender, reply_receiver) = mpsc::channel();
        if self
            .sender
            .send(Msg::Control(Control::RenderSheet(
                list,
                width,
                height,
                scale,
                reply_sender,
            )))
            .is_err()
        {
            self.gone.store(true, Ordering::Release);
            return Err(String::from("the render worker is gone"));
        }
        match reply_receiver.recv_timeout(Duration::from_secs(2)) {
            Ok(result) => result,
            Err(_) => Err(String::from(
                "the render worker did not respond; card back previews are unavailable",
            )),
        }
    }
}

impl Drop for WorkerHandle {
    /// Requests shutdown and waits (bounded) for the worker to confirm
    /// it stopped. A worker wedged in a GPU wait is abandoned, not
    /// waited on forever — process exit reclaims it. A worker already
    /// known [gone](Self::is_gone) has no live thread left to confirm, so
    /// the shutdown request and the wait are both skipped.
    fn drop(&mut self) {
        if self.is_gone() {
            return;
        }
        let _ = self.sender.send(Msg::Shutdown);
        let deadline = Instant::now() + Duration::from_secs(2);
        while !self.stopped.load(Ordering::Acquire) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
    }
}

#[cfg(test)]
mod tests {
    // float_cmp: the scales asserted below pass through the mailbox
    // unchanged (no arithmetic) — pinning the exact value is correct,
    // an epsilon comparison would be the wrong tool here.
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::float_cmp
    )]

    use super::*;
    use crate::offscreen::OffscreenError;

    /// The in-tree default theme, like the renderer's and offscreen's
    /// own tests use. Disk and CPU only — no GPU touched — so this is
    /// fine to reuse from the pure, GPU-free decision tests too.
    fn default_theme() -> Theme {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../themes/default");
        Theme::load_dir(dir).unwrap()
    }

    // ---- pure drain/decision tests (no GPU) ----

    #[test]
    fn latest_render_wins_with_interleaved_controls() {
        let (reply_tx, _reply_rx) = mpsc::channel();
        let first = Msg::Control(Control::AdoptScale(1.5));
        let queue = vec![
            Msg::Render(DisplayList::default(), 100, 100),
            Msg::Control(Control::SetTheme(
                Box::new(default_theme()),
                CardScaling::Original,
                2.0,
                reply_tx,
            )),
            Msg::Render(DisplayList::default(), 200, 200),
            Msg::Control(Control::BuildDone(0, 3, Err(String::from("boom")))),
            Msg::Render(DisplayList::default(), 300, 300),
        ];

        let Decision::Batch { controls, render } = decide(first, queue, 0) else {
            panic!("expected a batch, not a shutdown");
        };
        let mut controls = controls.into_iter();

        match controls.next() {
            Some(Control::AdoptScale(scale)) => assert_eq!(scale, 1.5),
            _ => panic!("expected the first control to be an AdoptScale"),
        }
        match controls.next() {
            Some(Control::SetTheme(_, _, scale, _)) => assert_eq!(scale, 2.0),
            _ => panic!("expected the second control to be a SetTheme"),
        }
        match controls.next() {
            Some(Control::BuildDone(_, factor, Err(_))) => assert_eq!(factor, 3),
            _ => panic!("expected the third control to be a failed BuildDone"),
        }
        assert!(controls.next().is_none(), "no extra controls");

        let render = render.expect("the newest render request survives the drain");
        assert_eq!((render.width, render.height), (300, 300));
    }

    #[test]
    fn shutdown_mid_drain_cuts_the_batch_immediately() {
        let first = Msg::Render(DisplayList::default(), 50, 50);
        let queue = vec![
            Msg::Control(Control::AdoptScale(1.0)),
            Msg::Shutdown,
            Msg::Render(DisplayList::default(), 999, 999),
            Msg::Control(Control::AdoptScale(9.0)),
        ];

        assert!(matches!(decide(first, queue, 0), Decision::Shutdown));
    }

    #[test]
    fn a_lone_control_message_yields_no_render() {
        let decision = decide(Msg::Control(Control::AdoptScale(1.25)), Vec::new(), 0);
        let Decision::Batch { controls, render } = decision else {
            panic!("expected a batch, not a shutdown");
        };
        let mut controls = controls.into_iter();
        match controls.next() {
            Some(Control::AdoptScale(scale)) => assert_eq!(scale, 1.25),
            _ => panic!("expected a lone AdoptScale control"),
        }
        assert!(controls.next().is_none(), "no extra controls");
        assert!(
            render.is_none(),
            "a lone control message has no render to run"
        );
    }

    #[test]
    fn stale_generation_build_results_are_dropped_from_the_batch() {
        let current = 4;
        let first = Msg::Control(Control::AdoptScale(1.0));
        let queue = vec![
            // A result from a theme swapped away two generations ago: it
            // must vanish, taking its `job_failed`/error with it.
            Msg::Control(Control::BuildDone(2, 8, Err(String::from("stale")))),
            Msg::Render(DisplayList::default(), 120, 90),
            // A result stamped with the live generation: kept, in order.
            Msg::Control(Control::BuildDone(4, 5, Err(String::from("fresh")))),
        ];

        let Decision::Batch { controls, render } = decide(first, queue, current) else {
            panic!("expected a batch, not a shutdown");
        };
        let mut controls = controls.into_iter();
        match controls.next() {
            Some(Control::AdoptScale(scale)) => assert_eq!(scale, 1.0),
            _ => panic!("expected the leading AdoptScale to survive"),
        }
        match controls.next() {
            Some(Control::BuildDone(generation, factor, Err(reason))) => {
                assert_eq!((generation, factor), (4, 5));
                assert_eq!(reason, "fresh");
            }
            _ => panic!("expected only the current-generation BuildDone to survive"),
        }
        assert!(
            controls.next().is_none(),
            "the stale-generation build result is gone, no controls left"
        );
        let render = render.expect("the render request still survives the drain");
        assert_eq!((render.width, render.height), (120, 90));
    }

    #[test]
    fn a_successful_theme_swap_advances_the_generation_a_failed_one_holds() {
        // Only a successful swap replaces the renderer, so only it may
        // obsolete the builds dispatched against the old theme.
        assert_eq!(generation_after_swap(0, true), 1);
        assert_eq!(generation_after_swap(41, true), 42);
        assert_eq!(generation_after_swap(41, false), 41);
    }

    #[test]
    fn builds_run_inline_only_before_the_first_delivered_frame() {
        // No frame delivered yet: a crossed-boundary build must not touch
        // a transient thread, or the pre-first-frame stretched atlas could
        // reach the screen exactly as the bug report describes.
        assert!(builds_run_inline(false));
        // Once a frame has shipped, every later build goes back to the
        // transient path — the steady-state behavior must not regress.
        assert!(!builds_run_inline(true));
    }

    // ---- GPU-gated end-to-end test ----

    /// Blocks until a [`WorkerEvent::Frame`] with exactly `width`×`height`
    /// arrives, tolerating stale/replay frames at other sizes in between.
    /// Any [`WorkerEvent::Error`] fails the test immediately: nothing in
    /// this test's scenario is expected to fail.
    fn wait_for_frame(rx: &mpsc::Receiver<WorkerEvent>, width: u32, height: u32) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(
                !remaining.is_zero(),
                "timed out waiting for a {width}x{height} frame"
            );
            match rx.recv_timeout(remaining) {
                Ok(WorkerEvent::Frame(frame)) if (frame.width, frame.height) == (width, height) => {
                    return;
                }
                // A stale/replay frame, or a bare timeout: both just
                // loop back around, and the `assert!` above catches
                // true exhaustion against `deadline`.
                Ok(WorkerEvent::Frame(_)) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                Ok(WorkerEvent::Error(reason)) => panic!("unexpected worker error: {reason}"),
                Err(mpsc::RecvTimeoutError::Disconnected) => panic!(
                    "worker event channel disconnected while waiting for a {width}x{height} frame"
                ),
            }
        }
    }

    /// Blocks for exactly the next event off `rx` and requires it to
    /// already be the `width`×`height` frame — the strict counterpart to
    /// [`wait_for_frame`], usable only for the very first frame off a
    /// freshly started worker (nothing else could have been queued
    /// ahead of it). Pins the pre-first-frame inline build path: an
    /// `Error` here fails with a distinct message, and no stale/replay
    /// frame is tolerated either, since none can legitimately exist yet.
    fn expect_first_frame(rx: &mpsc::Receiver<WorkerEvent>, width: u32, height: u32) {
        match rx.recv_timeout(Duration::from_secs(10)) {
            Ok(WorkerEvent::Frame(frame)) => assert_eq!(
                (frame.width, frame.height),
                (width, height),
                "the very first event off a fresh worker must already be this frame"
            ),
            Ok(WorkerEvent::Error(reason)) => {
                panic!("unexpected worker error before the first frame: {reason}")
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                panic!("timed out waiting for the first frame")
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                panic!("worker event channel disconnected while waiting for the first frame")
            }
        }
    }

    /// End-to-end: start the worker, adopt a scale, render at two
    /// different sizes (crossing an atlas factor boundary in between,
    /// which may interleave a replay of the first size), swap the
    /// theme, and drop the handle — skips cleanly without a graphics
    /// adapter, like the offscreen tests.
    #[test]
    fn worker_renders_rescales_and_swaps_theme_then_shuts_down_promptly() {
        let theme = default_theme();
        let offscreen = match Offscreen::new(theme.clone(), CardScaling::Original, 1.0) {
            Ok(offscreen) => offscreen,
            Err(OffscreenError::NoAdapter) => {
                eprintln!("skipping: no graphics adapter");
                return;
            }
            Err(error) => panic!("offscreen setup failed: {error}"),
        };

        let (tx, rx) = mpsc::channel();
        let handle = WorkerHandle::start(offscreen, move |event| {
            let _ = tx.send(event);
        })
        .expect("starting the render worker");

        // The default theme is vector mode, so 1.0 -> 2.0 always crosses
        // an atlas factor boundary. Whether that rebuild runs inline or
        // is dispatched to a thread is pinned separately by the pure
        // `builds_run_inline` test; what `expect_first_frame` guarantees
        // here is only that the first event off the worker is already
        // this exact-size frame — no stray `Error` event and no
        // out-of-order replay of the prior size ahead of it.
        handle.adopt_scale(2.0);
        handle.render(DisplayList::default(), 300, 200);
        expect_first_frame(&rx, 300, 200);

        handle.render(DisplayList::default(), 150, 150);
        wait_for_frame(&rx, 150, 150);

        let verdict = handle.set_theme(theme, CardScaling::Original, 2.0);
        assert_eq!(verdict, Ok(()));

        // A healthy worker that has served every request is never
        // reported gone.
        assert!(!handle.is_gone(), "a live worker must not report gone");

        let start = Instant::now();
        drop(handle);
        assert!(
            start.elapsed() < Duration::from_secs(3),
            "dropping the handle must return promptly"
        );
    }

    /// A [`Control::RenderSheet`] round trip: the pixels come back
    /// correctly sized and cleared to the requested color, an ordinary
    /// render still works afterward, and the texture ceiling — the
    /// fallback or the real value, whichever [`WorkerHandle::max_texture_dim`]
    /// happens to observe — is guaranteed to be the real device limit by
    /// the time a render has round-tripped, since the worker thread
    /// stores it before `worker_loop` ever starts reading its mailbox.
    #[test]
    fn worker_renders_a_back_sheet_and_reports_its_real_texture_ceiling() {
        use sol_presenter::Rgba;

        let theme = default_theme();
        let offscreen = match Offscreen::new(theme, CardScaling::Original, 1.0) {
            Ok(offscreen) => offscreen,
            Err(OffscreenError::NoAdapter) => {
                eprintln!("skipping: no graphics adapter");
                return;
            }
            Err(error) => panic!("offscreen setup failed: {error}"),
        };
        let (tx, rx) = mpsc::channel();
        let handle = WorkerHandle::start(offscreen, move |event| {
            let _ = tx.send(event);
        })
        .expect("starting the render worker");

        let list = DisplayList {
            clear: Some(Rgba::opaque(9, 8, 7)),
            sprites: Vec::new(),
        };
        let pixels = handle
            .render_sheet(list, 4, 4, 1.0)
            .expect("rendering a back sheet");
        assert_eq!(pixels.len(), 4 * 4 * 4, "tightly packed RGBA8 at 4x4");
        assert!(
            pixels.chunks_exact(4).all(|px| px == [9, 8, 7, 255]),
            "the sheet clears to the requested color"
        );

        // The worker thread stores the real ceiling before `worker_loop`
        // ever starts reading its mailbox, and the render above just
        // round-tripped through that same mailbox, so the real value —
        // guaranteed at least the wgpu floor — is visible here now.
        assert!(handle.max_texture_dim() >= 2048);

        // A render_sheet round trip must not disturb the ordinary render
        // path this worker also serves.
        handle.render(DisplayList::default(), 32, 24);
        wait_for_frame(&rx, 32, 24);
    }
}
