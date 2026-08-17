//! `sol-qt` — the Linux frontend: Qt6/QML chrome around a wgpu-rendered
//! playfield, integrated via cxx-qt. All chrome (menus, dialogs, status
//! bar) is QML calling the presenter's API through one bridge object;
//! this binary contains zero game logic.
//!
//! # Playfield embedding (decided)
//!
//! Two candidate approaches were probed for putting wgpu output under
//! QML chrome:
//!
//! (a) **A native child window** created from the window's
//! `raw-window-handle`. Works on X11 (`QWindow::winId` is public API and
//! wgpu accepts an xcb window id), but is not robust on Wayland: Qt's
//! public native interfaces stop at the application level
//! (`QNativeInterface::QWaylandApplication` — display/seat only); the
//! per-window `wl_surface *` that a wgpu Wayland surface needs is only
//! reachable through the private `QtWaylandClient` headers
//! (`qwaylandwindow_p.h`), which have no compatibility guarantees and
//! are not shipped by every distribution. On top of that, a child
//! `QWindow` becomes a `wl_subsurface` whose buffers both Qt and wgpu
//! would attach to (a protocol-error hazard Qt has no "foreign renderer"
//! escape hatch for), and any child window occludes in-scene QML popups
//! and overlays stacked over the playfield region (the classic airspace
//! problem).
//!
//! (b) **Offscreen wgpu, texture imported into the QML scene graph.**
//! The playfield renders on a headless wgpu device into a persistent
//! canvas texture (the same device/readback path the renderer's
//! golden-image tests exercise on lavapipe, llvmpipe, and real
//! hardware), is read back, and enters the scene graph as an ordinary
//! image texture through a `QQuickPaintedItem`. No windowing-system
//! code at all, so Wayland and X11 (and a pure-software GL stack)
//! behave identically; QML popups, overlays, and dialogs compose over
//! the playfield like over any other item.
//!
//! **Decision: (b).** The cost is a per-frame GPU→CPU→GPU copy of the
//! playfield (a few MB at typical sizes — well within what raster Qt
//! UIs move per frame); the win is that the platform-specific failure
//! surface is zero. True zero-copy sharing (dma-buf / external-memory
//! import into the scene graph) was rejected for the same reason (a)
//! was: it trades a bounded, portable cost for driver- and
//! compositor-specific fragility.

mod app;
mod bridge;
mod offscreen;
mod worker;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::OnceLock;

use anyhow::{Context, anyhow};
use cxx_qt_lib::{QGuiApplication, QQmlApplicationEngine, QUrl};
use sol_engine::Seed;

const HELP: &str = "\
classic-solitair (Qt frontend)

USAGE: sol-qt [--theme <dir-or-zip>] [--seed <0-32767>]

  --theme <path>   theme package to load (default: the \"default\" theme)
  --seed <0-32767> deal this exact game instead of a random one
  --smoke          open every dialog once and exit (self-test)

Menus and dialogs cover everything else; the current game's seed is
always visible (and copyable) in the status bar.
";

/// Parsed command-line arguments.
#[derive(Debug, Default, PartialEq, Eq)]
struct Cli {
    theme: Option<PathBuf>,
    seed: Option<Seed>,
    smoke: bool,
    help: bool,
}

impl Cli {
    fn parse(args: impl Iterator<Item = String>) -> anyhow::Result<Self> {
        let mut parsed = Self::default();
        let mut args = args.peekable();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--help" | "-h" => parsed.help = true,
                "--smoke" => parsed.smoke = true,
                "--theme" => {
                    let value = args.next().ok_or_else(|| anyhow!("--theme needs a path"))?;
                    parsed.theme = Some(PathBuf::from(value));
                }
                "--seed" => {
                    let value = args
                        .next()
                        .ok_or_else(|| anyhow!("--seed needs a number"))?;
                    let seed = value
                        .parse::<Seed>()
                        .with_context(|| format!("--seed {value} is not a game number"))?;
                    parsed.seed = Some(seed);
                }
                other => return Err(anyhow!("unknown argument {other} (try --help)")),
            }
        }
        Ok(parsed)
    }
}

/// The parsed CLI, for the bridge's `Initialize` (QML instantiates the
/// playfield, so constructor arguments cannot carry these).
static CLI: OnceLock<Cli> = OnceLock::new();

/// The process's parsed CLI arguments.
pub(crate) fn cli() -> &'static Cli {
    CLI.get_or_init(Cli::default)
}

fn main() -> anyhow::Result<ExitCode> {
    let cli = Cli::parse(std::env::args().skip(1))?;
    if cli.help {
        print!("{HELP}");
        return Ok(ExitCode::SUCCESS);
    }
    let _ = CLI.set(cli);

    let mut app = QGuiApplication::new();
    let mut engine = QQmlApplicationEngine::new();
    if let Some(engine) = engine.as_mut() {
        engine.load(&QUrl::from("qrc:/qt/qml/ClassicSolitair/qml/Main.qml"));
    }
    let code = app.as_mut().map_or(1, |app| app.exec());
    Ok(u8::try_from(code).map_or(ExitCode::FAILURE, ExitCode::from))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    fn args(list: &[&str]) -> anyhow::Result<Cli> {
        Cli::parse(list.iter().map(ToString::to_string))
    }

    #[test]
    fn cli_parses_theme_seed_and_help() {
        assert_eq!(args(&[]).unwrap(), Cli::default());
        let parsed = args(&["--theme", "/tmp/t", "--seed", "42"]).unwrap();
        assert_eq!(parsed.theme, Some(PathBuf::from("/tmp/t")));
        assert_eq!(parsed.seed, Some(Seed::new(42).unwrap()));
        assert!(args(&["--help"]).unwrap().help);
        assert!(args(&["-h"]).unwrap().help);
        assert!(args(&["--smoke"]).unwrap().smoke);
    }

    #[test]
    fn cli_rejects_garbage() {
        assert!(args(&["--seed"]).is_err(), "missing value");
        assert!(args(&["--seed", "x"]).is_err(), "not a number");
        assert!(args(&["--seed", "32768"]).is_err(), "beyond the last game");
        assert!(args(&["--theme"]).is_err(), "missing value");
        assert!(args(&["--frobnicate"]).is_err(), "unknown flag");
    }

    /// End-to-end QML smoke test: boots the real binary under Qt's
    /// `offscreen` platform in `--smoke` mode, which instantiates the
    /// full chrome — window, menus, and all four dialogs (they load
    /// lazily, so only this exercises them) — while the wgpu → `QImage`
    /// render loop runs, then exits. Any QML error (bad property,
    /// missing type, broken binding) surfaces as a `<file>.qml:<line>`
    /// diagnostic on stderr and fails the test.
    ///
    /// Lives in the binary's unit tests because the cxx-qt C++ archive
    /// references bridge symbols that only the binary target carries —
    /// a `tests/` integration target cannot link.
    #[test]
    fn smoke_boots_chrome_and_dialogs_headless() {
        /// The card back the run boots on: not index 0, so that "the
        /// dialog left the choice alone" is distinguishable from "the
        /// dialog reset the choice".
        const SEEDED_BACK: usize = 1;

        // `cargo test` only compiles and runs this test's own harness
        // binary — that harness is a separate build artifact from
        // `target/debug/sol-qt`, the plain bin this test launches below, so
        // a green `cargo test -p sol-qt` does not by itself prove the bin
        // was rebuilt. This smoke lives in the bin's unit tests (rather
        // than a `tests/` integration test cargo would rebuild for free)
        // because cxx-qt's generated C++ archive references bridge symbols
        // that only the bin target carries, and a `tests/` target cannot
        // link against it. Without an explicit build here, an edit to the
        // QML, this crate, or a dependency like `sol-session` could leave a
        // stale `sol-qt` binary in place and this test would keep passing
        // against old code — do not "simplify" this call away. Driving a
        // real `cargo build` uses cargo's own dependency graph instead of
        // an mtime heuristic, so cross-crate staleness is covered too, and
        // it is safe to call from within a test: the build lock is free
        // once test binaries are running, and on CI (which always builds
        // before testing) this is a no-op.
        let build = std::process::Command::new(env!("CARGO"))
            .args(["build", "-p", "sol-qt", "--bin", "sol-qt"])
            .output()
            .expect("running cargo build -p sol-qt --bin sol-qt");
        assert!(
            build.status.success(),
            "cargo build -p sol-qt --bin sol-qt failed ({:?})\nstderr:\n{}",
            build.status,
            String::from_utf8_lossy(&build.stderr)
        );

        // target/debug/deps/sol_qt-<hash> → target/debug/sol-qt
        let mut binary = std::env::current_exe().expect("test binary path");
        binary.pop();
        if binary.ends_with("deps") {
            binary.pop();
        }
        binary.push("sol-qt");
        assert!(
            binary.is_file(),
            "expected the sol-qt binary at {}",
            binary.display()
        );

        let data_dir = tempfile::tempdir().expect("temp data dir");

        // Boot the run on a card back that is not the first one, through
        // the same settings document the frontend reads at startup:
        // nothing can show that opening the Options dialog leaves the
        // player's card back alone while that back is index 0, which is
        // exactly what a picker resetting its own selection would land on.
        let state_dir = data_dir.path().join("classic-solitair");
        std::fs::create_dir_all(&state_dir).expect("creating the state directory");
        let seeded = sol_session::Settings {
            back_index: SEEDED_BACK,
            ..sol_session::Settings::default()
        };
        std::fs::write(
            state_dir.join("settings.json"),
            seeded.to_bytes().expect("serializing the seeded settings"),
        )
        .expect("writing the seeded settings");

        let output = std::process::Command::new(&binary)
            .args(["--smoke", "--seed", "1"])
            // Offscreen platform: no display server needed (CI-safe),
            // and an isolated XDG data dir so the self-test can never
            // touch a real autosave slot.
            .env("QT_QPA_PLATFORM", "offscreen")
            .env("XDG_DATA_HOME", data_dir.path())
            // Qt sends its messages to the systemd journal instead of
            // stderr whenever it is built against libsystemd and the
            // process was started from a journal-backed session — which
            // is the ordinary case on a Linux desktop, and would leave
            // every scan below reading an empty stream. This pins the
            // destination so QML diagnostics are actually observable.
            .env("QT_FORCE_STDERR_LOGGING", "1")
            .output()
            .expect("running sol-qt --smoke");

        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "smoke run failed ({:?})\nstdout:\n{stdout}\nstderr:\n{stderr}",
            output.status
        );
        // A QML diagnostic always carries "<name>.qml:<line>".
        let qml_diagnostic = stderr.lines().find(|line| line.contains(".qml:"));
        assert_eq!(
            qml_diagnostic, None,
            "QML errors during smoke run:\n{stderr}"
        );
        // Rust-side failures (startup, render, autosave) log "sol-qt:".
        let rust_failure = stderr.lines().find(|line| line.contains("sol-qt:"));
        assert_eq!(
            rust_failure, None,
            "frontend errors during smoke run:\n{stderr}"
        );
        // The QML self-checks Main.qml runs in `--smoke` mode all report
        // under one prefix (see its `smokeFail`); console.error prints a
        // bare message, so neither scan above would catch them.
        let smoke_failure = stderr
            .lines()
            .find(|line| line.contains("smoke check failed"));
        assert_eq!(
            smoke_failure, None,
            "QML self-check failures during smoke run:\n{stderr}"
        );

        // The exit contract (wired through Main.qml's smokeQuit timer
        // calling autosaveOnExit) must have written settings.json under
        // the isolated data dir, and it must parse as a settings document.
        let settings_path = data_dir
            .path()
            .join("classic-solitair")
            .join("settings.json");
        assert!(
            settings_path.is_file(),
            "expected settings written to {}",
            settings_path.display()
        );
        let bytes = std::fs::read(&settings_path).expect("reading settings.json");
        let settings = sol_session::Settings::from_bytes(&bytes).expect("parsing settings.json");

        // The seeded card back has to come back out unchanged. The smoke
        // run opens the Options dialog and cancels it, so this covers the
        // whole round trip: the dialog neither rewrote the selection on
        // the way in, nor captured a rewritten one as the point Cancel
        // returns to.
        assert_eq!(
            settings.back_index, SEEDED_BACK,
            "the card back the session booted on did not survive opening Options"
        );

        // The smoke run's resize timers (200 ms, then 400 ms) each restart
        // the QML settle timer (500 ms), landing a settled geometry well
        // before the 700 + 700 ms dialog/quit sequence exits — and the
        // quit timer records the window's live geometry itself right
        // before the exit persist, so the document carries a geometry
        // even if the settle timer never fired.
        let window = settings
            .window
            .expect("a geometry should have settled and persisted during the smoke run");
        assert!(
            window.width >= 400 && window.height >= 300,
            "persisted geometry below the clamp floor: {window:?}"
        );
    }
}
