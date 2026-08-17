//! Theme package format for classic-solitair.
//!
//! This crate has two layers. The **manifest layer** parses and validates a
//! `theme.toml` document into a typed [`Manifest`] — see
//! [`Manifest::from_toml_bytes`] and [`Manifest::from_toml_str`] — doing no
//! I/O and knowing nothing about directories, zip archives, or the asset
//! bytes a manifest's paths point at.
//!
//! The **loading layer**, built on top of the manifest layer, turns a
//! theme package plus its manifest into a fully validated [`Theme`]: every
//! face, back, background, and sound asset is read, probed, and checked
//! against the theme format rules. It is byte-oriented at its core — see
//! [`AssetSource`], the read boundary every asset goes through — with a
//! thin filesystem/zip layer ([`DirSource`], [`ZipSource`]) on top so the
//! core itself never touches a filesystem or zip archive directly. See
//! [`Theme::from_source`] and its conveniences [`Theme::load_dir`],
//! [`Theme::load_zip_bytes`], and [`Theme::load_path`].
//!
//! [`Manifest`] and [`Theme`] are the crate's two entry points; every other
//! public type is a field of one of them or a step in producing one, down
//! to [`ManifestError`] and [`ThemeError`], which enumerate every way a
//! `theme.toml` document or a theme package can fail to validate.

pub mod asset;
pub mod back;
pub mod background;
pub mod card_scaling;
pub mod color;
mod dir_source;
pub mod error;
pub mod face;
pub mod faces;
mod load_background;
mod load_backs;
mod load_faces;
mod load_placeholders;
mod load_sounds;
pub mod manifest;
pub mod mem_source;
mod ordered_map;
mod path;
pub mod placeholders;
mod png;
pub mod render_mode;
pub mod size;
pub mod source;
mod svg;
#[cfg(test)]
pub(crate) mod testkit;
mod theme;
pub mod theme_error;
mod zip_source;

pub use asset::{Asset, AssetKind};
pub use back::{BackDef, BackLayout, BackName, BackNameError};
pub use background::Background;
pub use card_scaling::CardScaling;
pub use color::{Color, ColorError};
pub use dir_source::DirSource;
pub use error::ManifestError;
pub use face::{FaceRank, FaceRankError, FaceSuit, canonical_faces};
pub use faces::FacesSource;
pub use load_background::LoadedBackground;
pub use load_backs::LoadedBack;
pub use load_placeholders::LoadedPlaceholders;
pub use manifest::Manifest;
pub use mem_source::MemSource;
pub use path::RelativeAssetPath;
pub use placeholders::Placeholders;
pub use render_mode::RenderMode;
pub use size::{CardSize, CardSizeError};
pub use source::{AssetSource, SourceError};
pub use svg::hardened_options;
pub use theme::Theme;
pub use theme_error::ThemeError;
pub use zip_source::ZipSource;
