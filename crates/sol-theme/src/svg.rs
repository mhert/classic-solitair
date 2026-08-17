//! SVG dimension probing: a thin presence guard (via `roxmltree`) in front
//! of `usvg`, the same renderer-grade parser the game's renderer will
//! use. After this module, "sol-theme accepts this SVG" and "the renderer
//! can draw this SVG" are the same statement.
//!
//! ## Division of labor
//!
//! [`probe`] needs a declared pixel size for validation, not a rendered
//! image. It checks three independent things, in order:
//!
//! 1. **Container** (this module): is `bytes` gzip-compressed (svgz)? A
//!    two-byte magic-number check, mirroring the one `usvg::Tree::from_data`
//!    itself uses to decide whether to auto-decompress. Without this check
//!    running first, gzip bytes would reach the presence guard below, fail
//!    its UTF-8 read, and defer straight to `usvg` — which *does*
//!    decompress and parse them, an unchecked guard bypass (see "Accepted
//!    semantic deltas" below). This crate does not support svgz, so the
//!    container is rejected outright, before either the guard or `usvg`
//!    ever runs.
//! 2. **Presence** (this module, via `roxmltree`): does the root `<svg>`
//!    element carry at least one of `width`, `height`, `viewBox`? Only the
//!    attribute *names* are inspected — never their values. This step
//!    exists because `usvg` itself happily accepts a genuinely dimensionless
//!    SVG: it resolves to a hard-coded 100×100, or — once child content is
//!    present — to a content-derived bounding box (auto-detection). Neither
//!    is distinguishable from an authored size once only the resolved size
//!    is available, so without this guard a dimensionless SVG could
//!    silently probe as a plausible card size. (An earlier design tried to
//!    prevent this via `usvg::Options::default_size` instead; that field
//!    turned out to be dead for this purpose in usvg 0.47.0 — written into
//!    locals `resolve_svg_size` never reads back — so it is deliberately
//!    left unset here rather than implying a behavior this crate doesn't
//!    get.) The guard parses with `allow_dtd: true`, the same option `usvg`
//!    itself always uses, so a DTD prologue can't push a document into the
//!    "guard can't parse it, so defer to usvg" fallback below either — see
//!    [`has_a_dimension_attr`].
//! 3. **Everything else** (`usvg`): units, percentages, resolution order,
//!    and the actual size math are wholly `usvg`'s job. This module never
//!    re-implements any of it.
//!
//! Because the guard only checks presence, not validity, any *present but
//! unparseable* dimension value slips past it: a malformed `viewBox` (e.g.
//! the wrong number of fields), or a `width`/`height` `usvg` cannot parse as
//! a length (e.g. `width="abc"`, `width=""`). `usvg` treats a value it
//! cannot parse exactly like an absent attribute (it logs a warning and
//! moves on), so every case in this class falls through identically: to
//! `viewBox` if one is present and does parse, or otherwise to the same
//! hard-coded 100×100 default true dimensionlessness would hit without this
//! guard. This is an accepted, narrow consequence of keeping the guard
//! presence-only rather than growing a second value parser alongside
//! `usvg` — see this module's
//! `a_malformed_view_box_with_no_width_or_height_falls_through_to_usvgs_default_size`
//! test, which pins the observed behavior deliberately so it stays a visible
//! test, not a silent gap.
//!
//! ## Accepted semantic deltas vs. the previous hand-rolled parser
//!
//! - Full SVG-spec dimension resolution: units (mm, pt, in, %, …) now
//!   resolve; an integer-*valued* result passes regardless of how it was
//!   written (e.g. `width="1in"` at the default 96 DPI resolves to `96`).
//! - A present `width`/`height` no longer defers wholesale to `viewBox`:
//!   `usvg`'s own resolution order governs which value wins per axis, so a
//!   present-but-non-integral `width` is now honored (and can reject the
//!   whole probe) rather than being ignored in favor of `viewBox`.
//! - `xmlns` remains OPTIONAL: a root `<svg>` with no namespace at all is
//!   still accepted (unchanged from the previous parser); only a *wrong*
//!   namespace is rejected (surfaces as [`SvgProbeError::Parse`]).
//! - Size flows through `f32` (`usvg::Size`) rather than `f64` — immaterial
//!   at card scale.
//! - A `<!DOCTYPE …>` prologue is now accepted, given a usable dimension.
//!   The previous hand-rolled parser called plain
//!   `roxmltree::Document::parse` (DTD disallowed by that function's own
//!   default), so any DTD was an unconditional hard rejection. `usvg` itself
//!   has always parsed DTDs; the guard now matches it (`allow_dtd: true`,
//!   point 2 above) specifically so a *dimensionless* DTD-carrying SVG can't
//!   slip past the guard — and that same opt-in is what makes a
//!   *dimensioned* one newly pass end to end.
//! - `.svgz` (gzip-compressed SVG) stays rejected — the same outcome as the
//!   previous parser, which rejected it incidentally (gzip's binary payload
//!   is essentially never valid UTF-8). The rejection is now the explicit,
//!   documented container check at point 1 above, firing before the UTF-8
//!   read rather than because of it.

/// The `usvg` options every parse of theme SVG uses — validation here and
/// rasterization in the renderer alike.
///
/// Both href resolvers return `None`, so an `<image href="...">` in a theme
/// cannot read a file or fetch a URL. Theme packages are untrusted input and
/// the two parses must agree: a theme accepted by validation and a theme the
/// renderer can draw need to be the same statement, under the same rules.
#[must_use]
pub fn hardened_options() -> usvg::Options<'static> {
    usvg::Options {
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_data: Box::new(|_, _, _| None),
            resolve_string: Box::new(|_, _| None),
        },
        ..usvg::Options::default()
    }
}

/// Reads `(width, height)` from `bytes`: a container check, then a cheap
/// presence guard (see the module doc's division of labor), then
/// `usvg::Tree::from_data`, then an integer-valued check on the resolved
/// size.
///
/// # Errors
///
/// Returns [`SvgProbeError::Dimensionless`] if the root `<svg>` element has
/// none of `width`, `height`, `viewBox`; [`SvgProbeError::Parse`] if `bytes`
/// is gzip-compressed (svgz, unsupported — see the module doc) or `usvg`
/// rejects `bytes` outright (malformed XML/UTF-8, the wrong root element or
/// namespace, a non-positive resolved size, …); or
/// [`SvgProbeError::NonIntegralSize`] if the resolved size has a fractional
/// part.
pub(crate) fn probe(bytes: &[u8]) -> Result<(u32, u32), SvgProbeError> {
    if bytes.starts_with(&GZIP_MAGIC) {
        return Err(SvgProbeError::Parse {
            message: "svgz (gzip-compressed SVG) is not supported".to_owned(),
        });
    }

    if !has_a_dimension_attr(bytes) {
        return Err(SvgProbeError::Dimensionless);
    }

    let opt = hardened_options();
    let tree = usvg::Tree::from_data(bytes, &opt).map_err(|error| SvgProbeError::Parse {
        message: error.to_string(),
    })?;

    let size = tree.size();
    let (width, height) = (size.width(), size.height());
    match (integral_dim(width), integral_dim(height)) {
        (Some(w), Some(h)) => Ok((w, h)),
        _ => Err(SvgProbeError::NonIntegralSize { width, height }),
    }
}

/// Presence-only guard: does the root element carry at least one of
/// `width`, `height`, `viewBox`? Never reads what they contain — see the
/// module doc's division of labor. Parses with `allow_dtd: true`, matching
/// `usvg`'s own parsing options exactly, so a DTD prologue lands here as a
/// real parse rather than falling into the fallback below. If `bytes` still
/// cannot be parsed as XML (not UTF-8, not well-formed even with DTD
/// allowed), this returns `true`: presence is moot, and `usvg`'s own parse
/// error is the more informative rejection.
fn has_a_dimension_attr(bytes: &[u8]) -> bool {
    let Ok(text) = core::str::from_utf8(bytes) else {
        return true;
    };
    let options = roxmltree::ParsingOptions {
        allow_dtd: true,
        ..roxmltree::ParsingOptions::default()
    };
    let Ok(document) = roxmltree::Document::parse_with_options(text, options) else {
        return true;
    };
    let root = document.root_element();
    ["width", "height", "viewBox"]
        .into_iter()
        .any(|attr| root.attribute(attr).is_some())
}

/// The two-byte gzip magic number (RFC 1952 §2.3.1). `usvg::Tree::from_data`
/// auto-decompresses any input starting with these bytes, treating it as
/// svgz; this crate does not support svgz (see the module doc's "Accepted
/// semantic deltas"), so [`probe`] rejects the container outright before
/// either the presence guard or `usvg` itself ever sees the bytes.
const GZIP_MAGIC: [u8; 2] = [0x1F, 0x8B];

/// The largest `f32` value that does not exceed `u32::MAX`.
///
/// `u32::MAX` itself (`4_294_967_295`) has no exact `f32` representation —
/// `f32`'s precision at this magnitude steps by 256, and `u32::MAX` falls
/// strictly between two representable values, `4_294_967_040.0` and
/// `4_294_967_296.0`. Anchoring [`integral_dim`]'s upper bound to this exact
/// constant (rather than comparing a widened `f64` against `f64::from(u32::MAX)`)
/// keeps the whole check in `f32`, without changing which values pass: no
/// `f32` value exists in `(MAX_INTEGRAL_DIM, u32::MAX]` for the two
/// approaches to disagree on.
const MAX_INTEGRAL_DIM: f32 = 4_294_967_040.0;

/// Accepts `value` only if it is integer-valued (`fract() == 0`) and in
/// `1.0..=u32::MAX`-equivalent range, converting it to `u32` exactly.
fn integral_dim(value: f32) -> Option<u32> {
    if value.fract() != 0.0 || !(1.0..=MAX_INTEGRAL_DIM).contains(&value) {
        return None;
    }
    // `value` is confirmed integer-valued and within `1.0..=MAX_INTEGRAL_DIM`
    // just above, so truncation and sign loss cannot occur.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let exact = value as u32;
    Some(exact)
}

/// [`probe`] could not determine SVG dimensions from `bytes`.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub(crate) enum SvgProbeError {
    /// The root `<svg>` element has none of `width`, `height`, `viewBox` —
    /// cheaply rejected before `usvg` ever sees `bytes` (see the module
    /// doc's division of labor).
    #[error("<svg> has none of width, height, or viewBox — cannot determine a size")]
    Dimensionless,
    /// `usvg` rejected `bytes` outright: malformed XML/UTF-8, the wrong root
    /// element or namespace, a non-positive resolved size, and so on.
    #[error("not a usable SVG: {message}")]
    Parse {
        /// `usvg`'s own error, stringified.
        message: String,
    },
    /// `usvg` resolved a size, but at least one dimension has a fractional
    /// part.
    #[error("resolved SVG size {width}x{height} is not integer-valued")]
    NonIntegralSize {
        /// The resolved width, fractional or not.
        width: f32,
        /// The resolved height, fractional or not.
        height: f32,
    },
}

#[cfg(test)]
mod tests {
    // float_cmp: several tests below pin exact `usvg`-resolved f32 values
    // (deterministic conversions, not approximate computations) — an
    // epsilon comparison would be the wrong tool and would mask a real
    // upstream drift instead of catching it.
    #![allow(clippy::unwrap_used, clippy::float_cmp)]

    use super::*;

    fn svg(inner: &str) -> Vec<u8> {
        format!(r#"<svg xmlns="http://www.w3.org/2000/svg" {inner}></svg>"#).into_bytes()
    }

    /// The renderer disables both href resolvers so a theme can never read
    /// the filesystem. Validation must apply the same options, or a theme is
    /// probed under weaker rules than it is drawn under.
    #[test]
    fn probing_never_resolves_an_image_href() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("href-probe-target.png");
        std::fs::write(&target, b"\x89PNG\r\n\x1a\nnot really a png").unwrap();

        let document = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="71" height="96">
                 <image href="{}" width="71" height="96"/>
               </svg>"#,
            target.display()
        );
        let tree = usvg::Tree::from_data(document.as_bytes(), &hardened_options()).unwrap();
        assert!(
            tree.root().children().is_empty(),
            "an image href must not resolve into the tree"
        );
    }

    #[test]
    fn plain_width_and_height_attrs_are_read_directly() {
        let bytes = svg(r#"width="71" height="96""#);
        assert_eq!(probe(&bytes).unwrap(), (71, 96));
    }

    #[test]
    fn a_px_suffix_on_width_and_height_is_accepted() {
        let bytes = svg(r#"width="71px" height="96px""#);
        assert_eq!(probe(&bytes).unwrap(), (71, 96));
    }

    #[test]
    fn view_box_only_with_integral_dims_passes() {
        // Mandated "viewBox-only-integral-pass": no width/height at all;
        // usvg resolves the size from viewBox alone. Same observable result
        // as the old hand-rolled viewBox fallback, now via usvg's own
        // resolution instead.
        let bytes = svg(r#"viewBox="0 0 923 384""#);
        assert_eq!(probe(&bytes).unwrap(), (923, 384));
    }

    #[test]
    fn view_box_accepts_comma_separated_numbers() {
        let bytes = svg(r#"viewBox="0,0,923,384""#);
        assert_eq!(probe(&bytes).unwrap(), (923, 384));
    }

    #[test]
    fn view_box_accepts_integer_valued_decimals() {
        let bytes = svg(r#"viewBox="0 0 923.0 384.0""#);
        assert_eq!(probe(&bytes).unwrap(), (923, 384));
    }

    #[test]
    fn a_non_integer_view_box_width_is_rejected() {
        let bytes = svg(r#"viewBox="0 0 923.5 384""#);
        let error = probe(&bytes).unwrap_err();
        assert!(matches!(error, SvgProbeError::NonIntegralSize { .. }));
    }

    #[test]
    fn a_malformed_view_box_with_no_width_or_height_falls_through_to_usvgs_default_size() {
        // Re-pinned (was `a_view_box_with_the_wrong_number_of_fields_is_rejected`,
        // asserting rejection): a wrong-arity viewBox (3 numbers, not 4)
        // with no width/height at all. The presence guard only checks that
        // the `viewBox` *attribute name* exists (see the module doc's
        // division of labor) — it never validates the value, so this passes
        // the guard untouched. usvg then cannot parse the malformed value,
        // treats it the same as an absent viewBox, and falls through to its
        // own hard-coded 100x100 default, which is integer-valued and so
        // passes the post-check too. This is a known, narrow consequence of
        // keeping the guard presence-only (documented in the module doc);
        // pinned here deliberately so an upstream usvg change that starts
        // hard-erroring on this instead surfaces as a test failure, not a
        // silent behavior change.
        let bytes = svg(r#"viewBox="0 0 923""#);
        assert_eq!(probe(&bytes).unwrap(), (100, 100));
    }

    #[test]
    fn dimensionless_empty_root_is_rejected() {
        // Mandated dimensionless-rejected (a): re-pinned from
        // `no_dimensions_at_all_is_rejected` onto the new `Dimensionless`
        // variant — no width/height/viewBox, no children either.
        let bytes = svg("");
        let error = probe(&bytes).unwrap_err();
        assert!(matches!(error, SvgProbeError::Dimensionless));
    }

    #[test]
    fn dimensionless_root_with_content_is_rejected() {
        // Mandated dimensionless-rejected (b): kills the guard-deletion
        // mutant. Observed directly: without the presence guard, usvg's
        // content-derived auto-detection resolves this to the rect's own
        // bounding box (40x60) and accepts it as a plausible card size — the
        // guard must reject it before usvg ever runs, regardless of content.
        let bytes =
            br#"<svg xmlns="http://www.w3.org/2000/svg"><rect width="40" height="60"/></svg>"#;
        let error = probe(bytes).unwrap_err();
        assert!(matches!(error, SvgProbeError::Dimensionless));
    }

    #[test]
    fn a_dtd_prologued_dimensionless_svg_is_rejected() {
        // Finding 1a (presence guard fails open on DTD): the guard's own
        // `roxmltree::Document::parse` used to default to `allow_dtd: false`
        // and error on this `<!DOCTYPE` prologue — which made the guard
        // "can't parse it, defer to usvg" (see the module doc), even though
        // `usvg` itself has always allowed DTDs. usvg then saw a genuinely
        // dimensionless `<svg>` and silently resolved it to its 100x100
        // default — an accepted "probe" that bypassed this guard entirely.
        // Observed directly against usvg 0.47.0 / roxmltree 0.21.1 before
        // this fix: `probe` returned `Ok((100, 100))` here, not an error.
        let bytes = br#"<?xml version="1.0"?>
<!DOCTYPE svg PUBLIC "-//W3C//DTD SVG 1.1//EN" "http://www.w3.org/Graphics/SVG/1.1/DTD/svg11.dtd">
<svg xmlns="http://www.w3.org/2000/svg"></svg>"#;
        let error = probe(bytes).unwrap_err();
        assert!(matches!(error, SvgProbeError::Dimensionless));
    }

    #[test]
    fn a_dtd_prologued_svg_with_real_dimensions_still_passes() {
        // The other half of the DTD delta (module doc): only the
        // dimensionless-guard-bypass is a bug. A DTD-carrying SVG that
        // genuinely declares a size must keep working once the guard
        // allows DTDs too, not just get rejected outright the way the old
        // hand-rolled parser rejected every DTD unconditionally.
        let bytes = br#"<?xml version="1.0"?>
<!DOCTYPE svg PUBLIC "-//W3C//DTD SVG 1.1//EN" "http://www.w3.org/Graphics/SVG/1.1/DTD/svg11.dtd">
<svg xmlns="http://www.w3.org/2000/svg" width="71" height="96"></svg>"#;
        assert_eq!(probe(bytes).unwrap(), (71, 96));
    }

    #[test]
    fn a_gzipped_svgz_is_rejected_before_it_reaches_usvg_or_the_guard() {
        // Finding 1b (presence guard fails open on svgz): gzip bytes fail
        // the guard's UTF-8 read, so the guard defers (see the module doc)
        // — and `usvg::Tree::from_data` auto-decompresses anything starting
        // with the gzip magic number and parses the result. A dimensionless
        // SVG smuggled inside a real gzip stream therefore used to reach
        // usvg's own dimensionless default/auto-detect fallback, bypassing
        // this crate's presence guard entirely: observed directly before
        // this fix, `probe` returned `Ok((100, 100))` for exactly these
        // bytes, not an error. The fix rejects the gzip container itself,
        // up front, before either the guard or usvg ever sees the payload.
        let bytes = gzip_wrapped(br#"<svg xmlns="http://www.w3.org/2000/svg"></svg>"#);
        let error = probe(&bytes).unwrap_err();
        assert!(matches!(error, SvgProbeError::Parse { .. }));
    }

    /// Wraps `payload` in a minimal, honest gzip stream: a real signature,
    /// header, CRC-32, and ISIZE trailer around a single uncompressed
    /// ("stored") DEFLATE block (RFC 1951 §3.2.4) — no compression library
    /// needed, since a stored block is valid, spec-mandated DEFLATE that any
    /// conformant decoder (including `usvg`'s own `flate2`-backed one)
    /// decodes identically to a Huffman-compressed block. Verified directly
    /// against `flate2::read::GzDecoder` (the exact decompressor
    /// `usvg::Tree::from_data` uses) before relying on it here: it decodes
    /// this construction back to `payload` byte-for-byte.
    fn gzip_wrapped(payload: &[u8]) -> Vec<u8> {
        let mut bytes = vec![
            0x1F, 0x8B, // ID1, ID2: gzip magic number
            0x08, // CM: deflate
            0x00, // FLG: no optional fields
            0x00, 0x00, 0x00, 0x00, // MTIME: unset
            0x00, // XFL
            0xFF, // OS: unknown
        ];

        // A single DEFLATE "stored" block: BFINAL=1, BTYPE=00 (3 bits,
        // LSB-first), then zero-padded to the next byte boundary.
        bytes.push(0b0000_0001);
        let len = u16::try_from(payload.len()).unwrap();
        bytes.extend_from_slice(&len.to_le_bytes());
        bytes.extend_from_slice(&(!len).to_le_bytes()); // NLEN: one's complement of LEN
        bytes.extend_from_slice(payload);

        bytes.extend_from_slice(&gzip_crc32(payload).to_le_bytes());
        let isize_field = u32::try_from(payload.len()).unwrap();
        bytes.extend_from_slice(&isize_field.to_le_bytes());
        bytes
    }

    /// The standard CRC-32 (ISO-HDLC / zlib / gzip) checksum, computed
    /// bit-by-bit — same algorithm as `png.rs`'s test-only `crc32`,
    /// replicated locally rather than shared across modules (same
    /// precedent as that function's own doc comment). gzip's trailer CRC-32
    /// is of the *uncompressed* payload, not the compressed bytes.
    fn gzip_crc32(bytes: &[u8]) -> u32 {
        let mut crc: u32 = 0xFFFF_FFFF;
        for &byte in bytes {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                let masked_polynomial = 0_u32.wrapping_sub(crc & 1) & 0xEDB8_8320;
                crc = (crc >> 1) ^ masked_polynomial;
            }
        }
        crc ^ 0xFFFF_FFFF
    }

    #[test]
    fn a_unit_specified_size_resolving_to_an_integer_passes() {
        // Mandated unit-resolution: units now resolve (an accepted delta —
        // see the module doc). The old parser rejected "1in" outright (not
        // a plain integer with an optional "px" suffix). 1in at the default
        // 96 DPI is exactly 96px — observed directly against usvg 0.47.0
        // rather than assumed.
        let bytes = svg(r#"width="1in" height="1in""#);
        assert_eq!(probe(&bytes).unwrap(), (96, 96));
    }

    #[test]
    fn a_namespaceless_root_is_accepted() {
        // Namespaceless roots are accepted by design: usvg 0.47.0
        // accepts a root <svg> with no xmlns at all — only a WRONG
        // namespace is rejected (see the test just below). Pinned so an
        // upstream tightening surfaces as a test failure, not a silent
        // theme break.
        let bytes = b"<svg width=\"71\" height=\"96\"></svg>";
        assert_eq!(probe(bytes).unwrap(), (71, 96));
    }

    #[test]
    fn a_wrong_namespace_root_is_rejected() {
        // The other half of the module doc's namespace delta: only a WRONG
        // namespace (not a missing one) is rejected.
        let bytes = br#"<svg xmlns="http://example.com/nope" width="71" height="96"></svg>"#;
        let error = probe(bytes).unwrap_err();
        assert!(matches!(error, SvgProbeError::Parse { .. }));
    }

    #[test]
    fn a_present_decimal_width_is_honored_over_view_box_and_rejected_if_non_integral() {
        // Re-pinned (was `a_malformed_width_with_no_height_falls_back_to_view_box_rather_than_erroring`):
        // its PURPOSE was old-semantics-specific ("a present-but-malformed
        // width, with height absent, is ignored wholesale in favor of
        // viewBox"). Under usvg, a present width/height no longer defers
        // wholesale to viewBox (an accepted delta — see the module doc):
        // "71.5" is a syntactically valid SVG length, so it is honored
        // directly for width, while the absent height still resolves via
        // viewBox — pinned against the real observed values, not guessed.
        let bytes = svg(r#"width="71.5" viewBox="0 0 923 384""#);
        let error = probe(&bytes).unwrap_err();
        assert!(matches!(
            error,
            SvgProbeError::NonIntegralSize { width, height }
            if width == 71.5 && height == 384.0
        ));
    }

    #[test]
    fn a_decimal_width_attribute_is_rejected_not_fallen_back_from() {
        // Re-pinned onto `NonIntegralSize` (was `InvalidDimensionAttr`):
        // same verdict (rejected), new mechanism — usvg resolves width from
        // the present attribute directly, and 71.5 fails the integral
        // post-check.
        let bytes = svg(r#"width="71.5" height="96" viewBox="0 0 71 96""#);
        let error = probe(&bytes).unwrap_err();
        assert!(matches!(
            error,
            SvgProbeError::NonIntegralSize { width, height }
            if width == 71.5 && height == 96.0
        ));
    }

    #[test]
    fn a_decimal_height_attribute_is_rejected_not_fallen_back_from() {
        // Mirror of the above onto `height`.
        let bytes = svg(r#"width="71" height="96.5" viewBox="0 0 71 96""#);
        let error = probe(&bytes).unwrap_err();
        assert!(matches!(
            error,
            SvgProbeError::NonIntegralSize { width, height }
            if width == 71.0 && height == 96.5
        ));
    }

    #[test]
    fn malformed_xml_is_rejected() {
        // Re-pinned onto `Parse` (was `MalformedXml`): the presence guard's
        // own roxmltree parse also fails here, so it defers silently (see
        // the module doc) and usvg's own parse is what actually rejects it.
        let error = probe(b"<svg><unclosed>").unwrap_err();
        assert!(matches!(error, SvgProbeError::Parse { .. }));
    }

    #[test]
    fn invalid_utf8_is_rejected() {
        // Re-pinned onto `Parse` (was `InvalidUtf8`): same deferral as
        // above; usvg has its own dedicated not-UTF-8 rejection.
        let error = probe(&[0xFF, 0xFE, 0xFD]).unwrap_err();
        assert!(matches!(error, SvgProbeError::Parse { .. }));
    }

    #[test]
    fn a_non_svg_root_is_rejected() {
        // Re-pinned onto `Parse` (was `WrongRoot`): the guard only checks
        // attribute presence on whatever the root element is (this `<rect>`
        // has width/height, so it passes the guard) — usvg is what actually
        // rejects a non-`<svg>` document root.
        let error = probe(b"<rect width=\"71\" height=\"96\"/>").unwrap_err();
        assert!(matches!(error, SvgProbeError::Parse { .. }));
    }

    #[test]
    fn error_messages_are_human_readable() {
        assert!(!SvgProbeError::Dimensionless.to_string().is_empty());
        assert!(
            !SvgProbeError::Parse {
                message: "x".to_owned()
            }
            .to_string()
            .is_empty()
        );
        assert!(
            !SvgProbeError::NonIntegralSize {
                width: 1.5,
                height: 2.0
            }
            .to_string()
            .is_empty()
        );
    }

    // -- integral_dim boundaries --

    #[test]
    fn integral_dim_rejects_zero() {
        // The range tightened from the old parser's `0.0..=u32::MAX` to
        // `1.0..=u32::MAX`: usvg's own
        // `Size` type cannot itself be zero (a positive-only type), so this
        // floor is belt-and-suspenders for this directly-testable helper,
        // not reachable from real usvg output.
        assert_eq!(integral_dim(0.0), None);
    }

    #[test]
    fn integral_dim_accepts_one() {
        // The lower boundary itself: `1.0 < 1.0` is false, so 1 must be
        // accepted, not just values strictly above it.
        assert_eq!(integral_dim(1.0), Some(1));
    }

    #[test]
    fn integral_dim_rejects_a_negative_number() {
        assert_eq!(integral_dim(-1.0), None);
    }

    #[test]
    fn integral_dim_accepts_max_integral_dim() {
        // The upper boundary itself: `MAX_INTEGRAL_DIM > MAX_INTEGRAL_DIM`
        // is false, so it must be accepted, not just values strictly below
        // it (see `MAX_INTEGRAL_DIM`'s doc comment for why this exact value,
        // rather than `u32::MAX`, is the right constant to test against).
        assert_eq!(integral_dim(MAX_INTEGRAL_DIM), Some(4_294_967_040));
    }

    // The precision loss below is the exact behavior under test (see the
    // comment inside), not an oversight — allowed locally rather than
    // avoided.
    #[allow(clippy::cast_precision_loss)]
    #[test]
    fn integral_dim_rejects_the_nearest_f32_to_u32_max() {
        // `u32::MAX as f32` rounds UP to `2^32`, one past `MAX_INTEGRAL_DIM`
        // — the next representable `f32` above it.
        assert_eq!(integral_dim(u32::MAX as f32), None);
    }
}
