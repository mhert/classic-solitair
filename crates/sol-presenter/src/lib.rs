//! Platform-neutral presentation core: layout, hit-testing, drag logic,
//! and animation timing, turned into sprite display lists, plus laying
//! out the card-back picker's contact sheet.
//!
//! Sits between the pure rules engine ([`sol_engine`], via the owned
//! [`sol_session::Session`]) and whatever renders: every frontend feeds
//! this crate pointer/key events and `advance(dt)` time, drives its
//! command/query API from menus, and draws the [`DisplayList`] it emits.
//! The playfield replicates the original Win98 layout in logical card
//! units derived from the theme's `base_size` — the board fills the
//! host's window through one continuous scale ([`Fit`]) applied by the
//! renderer, with the original's proportional column spread live; the
//! win cascade reproduces the original's physics bit for bit. No
//! rendering-API types appear anywhere here, and nothing reads a clock or
//! the OS — time and entropy are injected by the host, which keeps the
//! crate portable to wasm and Android.
//!
//! ```
//! use sol_engine::Seed;
//! use sol_presenter::Presenter;
//! use sol_session::{Options, Session};
//!
//! # fn theme() -> sol_theme::Theme {
//! #     let mut source = sol_theme::MemSource::new().with_file(
//! #         "theme.toml",
//! #         &br##"
//! # [theme]
//! # name = "Doc"
//! # render_mode = "vector"
//! # [cards]
//! # faces = "cards/"
//! # base_size = [71, 96]
//! # [backs]
//! # plain = { image = "backs/plain.svg" }
//! # [table]
//! # background = { color = "#008000" }
//! # [drag]
//! # outline_color = "#000000"
//! # "##[..],
//! #     );
//! #     let svg = |w: u32, h: u32| format!(r#"<svg width="{w}" height="{h}"></svg>"#).into_bytes();
//! #     source = source.with_file("backs/plain.svg", svg(71, 96));
//! #     for (suit, rank) in sol_theme::canonical_faces() {
//! #         source = source.with_file(format!("cards/{}.svg", suit.stem(rank)), svg(71, 96));
//! #     }
//! #     sol_theme::Theme::from_source(&source).unwrap()
//! # }
//! let theme = theme();
//! let session = Session::new(Options::default(), Seed::new(1).unwrap());
//! let mut presenter = Presenter::new(session, &theme);
//!
//! presenter.advance(16); // one host frame tick
//! let frame = presenter.frame();
//! assert!(frame.clear.is_some(), "a normal frame clears to the felt");
//! assert!(!frame.sprites.is_empty());
//! assert_eq!(presenter.seed().get(), 1);
//! ```

pub mod back_sheet;
mod backs;
mod cascade;
mod deal_anim;
pub mod display;
mod drag;
pub mod fit;
pub mod geometry;
mod hit;
pub mod layout;
mod msrand;
pub mod presenter;
mod profile;
#[cfg(test)]
pub(crate) mod testkit;
#[cfg(test)]
pub(crate) mod testkit_engine;
mod waste;

pub use back_sheet::{BackSheet, SheetCell};
pub use display::{DisplayList, PlaceholderSlot, Rgba, Sprite, TextureId};
pub use fit::Fit;
pub use geometry::{Pt, Rect, Size};
pub use layout::Layout;
pub use presenter::{Presenter, PresenterError};
