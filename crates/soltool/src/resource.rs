//! The shared shape of what a resource container yields to `extract`.
//!
//! Both container readers — the hand-rolled NE reader ([`crate::ne`]) and the
//! `pelite`-backed PE reader ([`crate::pe`]) — surface the same thing: the
//! raw, still-encoded `RT_BITMAP` bytes of every *integer-id* resource, plus a
//! count of the *string-named* ones deliberately skipped (only integer ids map
//! to card faces and backs). Keeping this vocabulary in one neutral module
//! lets the classifier in [`crate::extract`] treat NE and PE identically.

/// One integer-id `RT_BITMAP` resource: its id and its raw bytes (a
/// header-less DIB, still to be decoded by [`crate::dib`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceBitmap {
    /// The resource's integer id (NE ids are `u16`, PE ids `u32`; widened to
    /// `u32` so one classifier serves both).
    pub id: u32,
    /// The resource's raw bytes — a `BITMAPINFOHEADER`-form DIB.
    pub data: Vec<u8>,
}

/// Everything a container reader extracted: every integer-id `RT_BITMAP`, and
/// how many string-named bitmap resources were skipped (surfaced in the
/// `extract` summary so nothing silently disappears).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ContainerBitmaps {
    /// The integer-id bitmaps, in the order the container stored them.
    pub bitmaps: Vec<ResourceBitmap>,
    /// The number of string-named bitmap resources skipped.
    pub string_named_skipped: usize,
}
