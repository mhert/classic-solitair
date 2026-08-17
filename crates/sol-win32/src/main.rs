//! `sol-win32` — the Windows frontend: native-windows-gui chrome with a
//! **real Win32 menu bar** around a wgpu-rendered playfield. All chrome
//! (menu, dialogs, status bar) calls the presenter's API; this binary
//! contains zero game logic.
//!
//! # Playfield embedding
//!
//! The wgpu surface targets a **native child window** (an
//! `ExternCanvas`) covering the client area between the menu bar and
//! the status bar. Unlike on Linux — where sol-qt had to render
//! offscreen and import pixels into the QML scene graph — the child
//! window is the robust choice here: a Win32 `HWND` is exactly what
//! every wgpu Windows backend consumes, there is no compositor
//! protocol underneath to double-attach buffers, and the airspace
//! problem does not arise because none of the chrome overlaps the
//! canvas (the menu bar lives in the non-client area above it, the
//! status bar below it, and every dialog is its own top-level window).
//!
//! # Win32 specifics worth knowing
//!
//! - **DPI.** The process opts into system-DPI awareness, so window
//!   pixels are real pixels: a png theme at Original scaling stays
//!   integer-scaled and nearest-sampled, matching the original's zero
//!   smoothing (xBRZ scaling is deliberately smoothed and linearly
//!   sampled instead). The nwg control geometry is DPI-logical; only
//!   the canvas deals in physical pixels (`physical_size`, and pointer
//!   coordinates taken raw from the window messages).
//! - **Accelerators.** native-windows-gui has no accelerator-table
//!   support, so F2 / Ctrl+Z / Ctrl+Y are dispatched from raw
//!   `WM_KEYDOWN` handlers on the window and canvas — behaviorally
//!   identical, and the menu items carry the standard `\t` shortcut
//!   labels.
//! - **Status bar.** nwg exposes `SB_SETTEXT` but not `SB_SETPARTS`;
//!   the four sections (transient message · seed · score · time) are
//!   created with one raw `SB_SETPARTS` message. Clicking the seed
//!   section copies the bare seed digits to the clipboard.

// GUI subsystem: launching the exe must not open a console window
// (the original sol.exe is a GUI app). stdout/stderr still reach
// pipes, so `--smoke` under the test harness and `--help` under a
// redirect keep working; only the auto-allocated console goes away.
#![cfg_attr(windows, windows_subsystem = "windows")]

// The chrome, its dialogs, its placement geometry and the render path
// are all Windows-only; what compiles everywhere — the app state, theme
// discovery, the status text — moved to `sol-frontend`, which carries its
// own tests on every platform. Only the CLI below is shared here, and the
// Linux build exists to keep it and this binary's argument parsing honest.
#[cfg(windows)]
mod dialogs;
#[cfg(windows)]
mod gfx;
#[cfg(windows)]
mod placement;
#[cfg(windows)]
mod ui;

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, anyhow};
use sol_engine::Seed;

const HELP: &str = "\
classic-solitair (Windows frontend)

USAGE: sol-win32 [--theme <dir-or-zip>] [--seed <0-32767>]

  --theme <path>   theme package to load (default: the \"default\" theme)
  --seed <0-32767> deal this exact game instead of a random one
  --smoke          build the chrome, render a frame, and exit (self-test)

Menus and dialogs cover everything else; the current game's seed is
always visible in the status bar (click it to copy).
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

#[cfg(windows)]
fn main() -> anyhow::Result<ExitCode> {
    let cli = Cli::parse(std::env::args().skip(1))?;
    if cli.help {
        print!("{HELP}");
        return Ok(ExitCode::SUCCESS);
    }
    ui::run(&cli)?;
    Ok(ExitCode::SUCCESS)
}

#[cfg(not(windows))]
fn main() -> ExitCode {
    // Parse anyway so `--help` works and typos are reported the same
    // way on every platform.
    match Cli::parse(std::env::args().skip(1)) {
        Ok(cli) if cli.help => {
            print!("{HELP}");
            ExitCode::SUCCESS
        }
        Ok(_) => {
            eprintln!("sol-win32 is the Windows frontend and only runs on Windows.");
            eprintln!("On this platform use sol-qt (Linux) or sol-shell (dev harness).");
            ExitCode::FAILURE
        }
        Err(error) => {
            eprintln!("sol-win32: {error:#}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    fn args(list: &[&str]) -> anyhow::Result<Cli> {
        Cli::parse(list.iter().map(ToString::to_string))
    }

    #[test]
    fn cli_parses_theme_seed_smoke_and_help() {
        assert_eq!(args(&[]).unwrap(), Cli::default());
        let parsed = args(&["--theme", "C:/t", "--seed", "42"]).unwrap();
        assert_eq!(parsed.theme, Some(PathBuf::from("C:/t")));
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
}
