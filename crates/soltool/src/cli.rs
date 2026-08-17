//! [`Cli`]: soltool's command-line surface — parsed by `main.rs`
//! and handed to [`crate::run`]. All three subcommands are declared:
//! `extract`, `validate`, and `pack-strip`.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

/// `soltool` — asset extraction and theme authoring CLI.
///
/// Exit codes: `0` success, `1` the requested operation failed (see each
/// subcommand's help), `2` a usage error (missing/unknown arguments, no
/// subcommand, an unrecognized subcommand, or `extract --animate` given a
/// loose-directory input).
#[derive(Debug, Parser)]
#[command(name = "soltool", version, about)]
pub struct Cli {
    /// The subcommand to run.
    #[command(subcommand)]
    pub command: Command,
}

/// One `soltool` subcommand.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Extracts card bitmaps into a complete `render_mode = "png"` theme.
    ///
    /// `<input>` is sniffed by content: an NE (Win16) or PE (Win32)
    /// executable/DLL whose card bitmaps live in resources, or a directory of
    /// already-extracted loose `.bmp`/`.png` bitmaps. Resource backs are
    /// static by default; `--animate` reconstructs the original's four
    /// animated backs from the file's own overlay-sprite resources instead.
    /// For a loose directory, frame-numbered files (`<stem>_0`, `<stem>_1`,
    /// …, contiguous from 0, same size) are already packed into one animated
    /// strip back at 2 fps, so `--animate` on a loose directory is a usage
    /// error (exit 2) rather than a silent no-op.
    ///
    /// Writes `theme.toml`, `cards/*.png`, and `backs/*.png` under
    /// `<theme-dir>` (which must be empty or absent — never overwritten),
    /// prints a summary, and exits 0. The output is for your local use only:
    /// the original artwork must never be redistributed or committed.
    Extract(ExtractArgs),
    /// Validates a theme package and reports every problem found.
    ///
    /// Exit 0 and a one-line summary on stdout when the theme is valid;
    /// exit 1 and the full validation error (naming which asset failed and
    /// why) on stderr otherwise.
    Validate(ValidateArgs),
    /// Packs loose frame images into one horizontal strip PNG.
    ///
    /// The strip file itself carries no fps metadata — theme.toml's
    /// `[backs]` entry is where that number lives, so this command prints a
    /// ready-to-paste `[backs]` snippet (including the fps you gave it) to
    /// stdout alongside writing the strip.
    ///
    /// The snippet's back name comes from the output file's stem, sanitized
    /// into a valid back name: uppercase letters are lowercased and any
    /// character outside `a-z`, `0-9`, `_`, `-` becomes `_` (e.g. `card
    /// back.png` -> `card_back`), falling back to `strip` if the stem is
    /// empty. The snippet's `image` value is TOML-escaped, so it is always
    /// syntactically valid — but it is `--output` exactly as given
    /// otherwise, so an absolute or theme-package-external path still needs
    /// relocating (and its `image` value editing to match) before the
    /// snippet belongs in a `theme.toml`.
    PackStrip(PackStripArgs),
}

/// Arguments for `soltool extract`.
#[derive(Debug, Args)]
pub struct ExtractArgs {
    /// The asset source: an NE/PE executable or DLL, or a directory of loose
    /// `.bmp`/`.png` bitmaps. Sniffed by content, not by extension.
    pub input: PathBuf,
    /// Directory to write the generated theme into — created if absent,
    /// refused if it already exists and is not empty.
    #[arg(short = 'o', long = "output")]
    pub output: PathBuf,
    /// Reconstructs the original's animated card backs from the file's own
    /// overlay-sprite resources (resource inputs only; frames derived from
    /// the original's animation code; the existing local-use notice covers
    /// the strips).
    #[arg(long)]
    pub animate: bool,
}

/// Arguments for `soltool validate`.
#[derive(Debug, Args)]
pub struct ValidateArgs {
    /// Theme package to validate: a directory, or a file treated as a zip
    /// archive.
    pub theme: PathBuf,
}

/// Arguments for `soltool pack-strip`.
#[derive(Debug, Args)]
pub struct PackStripArgs {
    /// Frame image files, left to right in the strip (at least 2).
    #[arg(required = true)]
    pub frames: Vec<PathBuf>,
    /// Where to write the packed strip PNG.
    #[arg(short = 'o', long = "output")]
    pub output: PathBuf,
    /// Playback rate in frames per second, echoed into the printed
    /// `[backs]` snippet (theme.toml's only record of it — see this
    /// subcommand's help above).
    #[arg(long, value_parser = clap::value_parser!(u8).range(1..=255))]
    pub fps: u8,
}
