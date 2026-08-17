//! `soltool` — asset extraction and theme authoring CLI.
//!
//! Turns a user's own local Windows solitaire assets into classic-solitair
//! theme packages, and hosts the authoring tools theme creators need. Three
//! subcommands are implemented: [`extract`] pulls card bitmaps out of
//! NE (Win16) / PE (Win32) resources — or ingests a directory of loose
//! bitmaps — and writes a complete `render_mode = "png"` theme;
//! [`validate`] lints a theme package by delegating straight
//! to [`sol_theme::Theme::load_path`]; and [`pack_strip`] packs loose frame
//! images into the horizontal strip PNGs the `[backs]` format expects.
//! `extract` is the only subcommand that writes a `theme.toml`, and it
//! does so through one private manifest writer rather than rendering TOML
//! inline; `pack-strip` borrows that writer's string escaping for the
//! `[backs]` snippet it prints.
//!
//! The hand-rolled byte parsers [`dib`] (header-less DIBs), [`ne`] (the Win16
//! resource table), and the `pelite`-backed [`pe`] reader feed [`extract`];
//! [`raster`] is this crate's one place that touches PNG pixels — decoding
//! any 8-bit PNG (grayscale, indexed, RGB, or RGBA, with or without alpha)
//! to a uniform RGBA8 buffer and back — so every subcommand that needs raw
//! pixels shares one normalization path.
//!
//! `src/main.rs` is a thin binary: parse [`Cli`], call [`run`], map the
//! outcome to an exit code, print errors to stderr.
//! - `0` — success.
//! - `1` — the requested operation failed (a [`ToolError`]): an invalid
//!   theme, a bad input frame, or another I/O failure. The full error
//!   chain — which asset or frame, and why — goes to stderr.
//! - `2` — a usage error: missing or unknown arguments, no subcommand, or
//!   an unrecognized subcommand (`clap`'s default); or `extract --animate`
//!   given a loose-directory input.

mod animate;
mod bytes;
pub mod cli;
pub mod dib;
pub mod error;
pub mod extract;
mod manifest_writer;
pub mod ne;
mod outdir;
pub mod pack_strip;
pub mod pe;
pub mod raster;
pub mod resource;
mod strip;
pub mod validate;

#[cfg(test)]
pub(crate) mod testkit;

pub use cli::{Cli, Command, ExtractArgs, PackStripArgs, ValidateArgs};
pub use dib::DibError;
pub use error::ToolError;
pub use extract::{ExtractError, FaceSizeMismatch};
pub use ne::NeError;
pub use outdir::OutDirError;
pub use pack_strip::PackStripError;
pub use pe::PeError;
pub use raster::{RasterDecodeError, RasterEncodeError, RasterImage};
pub use resource::{ContainerBitmaps, ResourceBitmap};

/// Runs a parsed [`Command`], dispatching to the requested subcommand and
/// returning its stdout message on success.
///
/// ```
/// use soltool::{Command, ValidateArgs, run};
///
/// let command = Command::Validate(ValidateArgs {
///     theme: "/no/such/theme/at/all".into(),
/// });
/// let error = run(command).unwrap_err();
/// assert!(error.to_string().contains("/no/such/theme/at/all"));
/// ```
///
/// # Errors
///
/// Returns [`ToolError::Extract`], [`ToolError::Validate`], or
/// [`ToolError::PackStrip`] under the same conditions as [`extract::run`],
/// [`validate::run`], and [`pack_strip::run`] respectively.
pub fn run(command: Command) -> Result<String, ToolError> {
    match command {
        Command::Extract(args) => Ok(extract::run(
            &args.input,
            args.output.as_deref(),
            args.name.as_deref(),
            args.animate,
        )?),
        Command::Validate(args) => Ok(validate::run(&args.theme)?),
        Command::PackStrip(args) => Ok(pack_strip::run(&args.frames, &args.output, args.fps)?),
    }
}
