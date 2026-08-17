//! Hand-rolled NE (Win16 "New Executable") resource-table reader.
//!
//! The user's real `SOL.EXE`/`CARDS.DLL` are Win16 NE binaries, which
//! `pelite` (PE-only) cannot parse — so this reader walks the NE resource
//! table by hand to pull out every `RT_BITMAP` resource, bounds-checking
//! every field into a typed [`NeError`] rather than ever indexing a slice.
//!
//! # Normative layout
//!
//! The caller ([`crate::extract`]) has already confirmed the `MZ` signature
//! and located the NE header (`e_lfanew`, the `u32` at file offset `0x3C`),
//! whose first two bytes are `NE`. This reader takes that NE-header offset and
//! reads, all little-endian:
//!
//! - `u16` at `ne + 0x24` — the resource table's offset **relative to the NE
//!   header**. The resource table therefore starts at `ne + that`.
//! - At the resource table: `align_shift: u16`, then a sequence of `TYPEINFO`
//!   records terminated by a `type_id` of 0. Each `TYPEINFO` is
//!   `{ type_id: u16, count: u16, reserved: u32, count × NAMEINFO }`.
//! - `NAMEINFO` (12 bytes):
//!   `{ offset: u16, length: u16, flags: u16, id: u16, handle: u16, usage: u16 }`.
//!   `offset` and `length` are in *alignment units*: the resource's byte
//!   position is `offset << align_shift` and its byte length is
//!   `length << align_shift`.
//!
//! Integer ids (both `type_id` and a resource's `id`) have bit 15 set, with
//! the real value in the low 15 bits; `RT_BITMAP` is thus `type_id == 0x8002`
//! (`0x8000 | 2`). A resource `id` with bit 15 clear is instead an offset to a
//! string name — those are counted and skipped (only integer ids map to
//! cards). Every extracted resource is the same header-less DIB the PE path
//! produces, decoded downstream by [`crate::dib`].

use crate::bytes::read_u16_le;
use crate::resource::{ContainerBitmaps, ResourceBitmap};

/// NE integer-resource-type id for `RT_BITMAP` (`0x8000` integer flag | `2`).
const RT_BITMAP: u16 = 0x8002;
/// Bit 15: set on an integer id, clear on an offset-to-string name.
const INTEGER_ID_FLAG: u16 = 0x8000;
/// Offset of the resource-table pointer within the NE header.
const RESOURCE_TABLE_POINTER: usize = 0x24;
/// Bytes of a `NAMEINFO` record.
const NAMEINFO_LEN: usize = 12;
/// Ceiling on the total bytes copied out of one resource table. A real
/// `CARDS.DLL` is well under a megabyte of bitmap data; this leaves ample
/// room while refusing a table whose records each claim the whole file. Every
/// range is already bounds-checked individually, but a record may legally
/// span the entire input and a table may hold thousands of them, so only a
/// running total bounds the copying a small file can drive.
const TOTAL_RESOURCE_BUDGET_BYTES: usize = 64 * 1024 * 1024;

/// Reads every integer-id `RT_BITMAP` resource from the NE binary in `data`
/// whose NE header begins at `ne_offset` (already validated by the caller).
///
/// # Errors
///
/// Returns [`NeError::ResourceTableTruncated`] if the resource-table pointer,
/// its start, or the alignment shift lie past the end of `data`;
/// [`NeError::TypeInfoTruncated`] or [`NeError::NameInfoTruncated`] if a
/// `TYPEINFO`/`NAMEINFO` record runs past the end;
/// [`NeError::AlignShiftOverflow`] if `align_shift` is too large to shift a
/// resource's offset or length into a valid byte position;
/// [`NeError::ResourceOutOfBounds`] if a resource's computed byte range lies
/// outside `data`; or [`NeError::ResourcesTooLarge`] if the table's resources
/// together exceed [`TOTAL_RESOURCE_BUDGET_BYTES`].
pub fn extract(data: &[u8], ne_offset: usize) -> Result<ContainerBitmaps, NeError> {
    extract_within(data, ne_offset, TOTAL_RESOURCE_BUDGET_BYTES)
}

/// [`extract`] against an explicit byte budget.
///
/// Split out so the budget can be exercised with a small fixture: the shipped
/// ceiling is 64 MB, and a test that had to reach it would copy that much on
/// every run.
fn extract_within(
    data: &[u8],
    ne_offset: usize,
    budget: usize,
) -> Result<ContainerBitmaps, NeError> {
    let pointer_at = ne_offset
        .checked_add(RESOURCE_TABLE_POINTER)
        .ok_or(NeError::ResourceTableTruncated)?;
    let table_relative = read_u16_le(data, pointer_at).ok_or(NeError::ResourceTableTruncated)?;
    let table_start = ne_offset
        .checked_add(usize::from(table_relative))
        .ok_or(NeError::ResourceTableTruncated)?;
    let align_shift = read_u16_le(data, table_start).ok_or(NeError::ResourceTableTruncated)?;
    let align_shift = u32::from(align_shift);

    let mut cursor = table_start
        .checked_add(2)
        .ok_or(NeError::ResourceTableTruncated)?;
    let mut result = ContainerBitmaps::default();
    let mut copied_total: usize = 0;

    loop {
        let type_id = read_u16_le(data, cursor).ok_or(NeError::TypeInfoTruncated)?;
        if type_id == 0 {
            break;
        }
        let count = read_u16_le(
            data,
            cursor.checked_add(2).ok_or(NeError::TypeInfoTruncated)?,
        )
        .ok_or(NeError::TypeInfoTruncated)?;
        // Skip type_id (2) + count (2) + reserved (4) to reach the NAMEINFO array.
        let names_start = cursor.checked_add(8).ok_or(NeError::TypeInfoTruncated)?;

        for index in 0..usize::from(count) {
            let entry = names_start
                .checked_add(
                    index
                        .checked_mul(NAMEINFO_LEN)
                        .ok_or(NeError::NameInfoTruncated)?,
                )
                .ok_or(NeError::NameInfoTruncated)?;
            if type_id == RT_BITMAP {
                collect_bitmap(
                    data,
                    entry,
                    align_shift,
                    budget,
                    &mut copied_total,
                    &mut result,
                )?;
            } else {
                // Not RT_BITMAP: still bounds-check the record so the table
                // walk cannot stride past truncated data undetected.
                read_u16_le(
                    data,
                    entry
                        .checked_add(NAMEINFO_LEN - 2)
                        .ok_or(NeError::NameInfoTruncated)?,
                )
                .ok_or(NeError::NameInfoTruncated)?;
            }
        }

        cursor = names_start
            .checked_add(
                usize::from(count)
                    .checked_mul(NAMEINFO_LEN)
                    .ok_or(NeError::TypeInfoTruncated)?,
            )
            .ok_or(NeError::TypeInfoTruncated)?;
    }

    Ok(result)
}

/// Reads one `RT_BITMAP` `NAMEINFO` at `entry`, appending an integer-id
/// resource's bytes to `result` or counting a string-named one as skipped.
fn collect_bitmap(
    data: &[u8],
    entry: usize,
    align_shift: u32,
    budget: usize,
    copied_total: &mut usize,
    result: &mut ContainerBitmaps,
) -> Result<(), NeError> {
    let offset_units = read_u16_le(data, entry).ok_or(NeError::NameInfoTruncated)?;
    let length_units = read_u16_le(
        data,
        entry.checked_add(2).ok_or(NeError::NameInfoTruncated)?,
    )
    .ok_or(NeError::NameInfoTruncated)?;
    let id = read_u16_le(
        data,
        entry.checked_add(6).ok_or(NeError::NameInfoTruncated)?,
    )
    .ok_or(NeError::NameInfoTruncated)?;

    if id & INTEGER_ID_FLAG == 0 {
        result.string_named_skipped += 1;
        return Ok(());
    }

    let byte_offset = shift_by_align(offset_units, align_shift)?;
    let byte_length = shift_by_align(length_units, align_shift)?;
    let end = byte_offset
        .checked_add(byte_length)
        .ok_or(NeError::ResourceOutOfBounds {
            id: u32::from(id & !INTEGER_ID_FLAG),
            offset: byte_offset,
            length: byte_length,
        })?;
    let bytes = data
        .get(byte_offset..end)
        .ok_or(NeError::ResourceOutOfBounds {
            id: u32::from(id & !INTEGER_ID_FLAG),
            offset: byte_offset,
            length: byte_length,
        })?;

    // Checked before the copy, not after: the budget exists to bound the
    // allocation, so it has to refuse the byte range rather than measure it.
    *copied_total = copied_total.saturating_add(bytes.len());
    if *copied_total > budget {
        return Err(NeError::ResourcesTooLarge { limit: budget });
    }

    result.bitmaps.push(ResourceBitmap {
        id: u32::from(id & !INTEGER_ID_FLAG),
        data: bytes.to_vec(),
    });
    Ok(())
}

/// Shifts a `NAMEINFO` offset/length (`units`, in alignment units) left by
/// `align_shift` to compute a byte position, using a checked shift so a
/// hostile `align_shift` cannot panic by construction: `<<` on a `usize` is
/// only defined for a shift strictly less than `usize::BITS`, and this is the
/// one place that bound could ever be exceeded even if more shift call sites
/// are added later.
fn shift_by_align(units: u16, align_shift: u32) -> Result<usize, NeError> {
    usize::from(units)
        .checked_shl(align_shift)
        .ok_or(NeError::AlignShiftOverflow { align_shift })
}

/// Every way [`extract`] can fail to walk an NE resource table.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum NeError {
    /// The resource-table pointer, its start, or the alignment shift is past
    /// the end of the file.
    #[error("NE resource table is truncated or points past the end of the file")]
    ResourceTableTruncated,
    /// A `TYPEINFO` record's header runs past the end of the file.
    #[error("NE resource TYPEINFO record is truncated")]
    TypeInfoTruncated,
    /// A `NAMEINFO` record runs past the end of the file.
    #[error("NE resource NAMEINFO record is truncated")]
    NameInfoTruncated,
    /// A resource's computed byte range lies outside the file.
    #[error("NE resource id {id} at byte offset {offset} (length {length}) lies outside the file")]
    ResourceOutOfBounds {
        /// The resource's integer id (bit-15 flag stripped).
        id: u32,
        /// The resource's computed byte offset (`offset << align_shift`).
        offset: usize,
        /// The resource's computed byte length (`length << align_shift`).
        length: usize,
    },
    /// The table's resources together claim more bytes than a resource
    /// container is allowed to yield. Each range is in bounds on its own; it
    /// is their sum that is refused.
    #[error("NE resource table's resources exceed the {limit}-byte limit")]
    ResourcesTooLarge {
        /// The ceiling that was exceeded, in bytes.
        limit: usize,
    },
    /// The resource table's alignment shift is too large to shift a `u16`
    /// offset/length into a valid `usize` byte position (`align_shift` must
    /// be strictly less than `usize::BITS`, or `<<` is not a valid shift at
    /// all). A file whose declared shift cannot address any byte position is
    /// malformed, not merely huge.
    #[error(
        "NE resource table alignment shift {align_shift} is too large to compute a valid byte offset"
    )]
    AlignShiftOverflow {
        /// The alignment shift read from the resource table header.
        align_shift: u32,
    },
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::testkit::{build_ne, build_ne_repeated_full_span};

    /// An integer resource id (bit 15 set).
    fn int_id(id: u16) -> u16 {
        id | INTEGER_ID_FLAG
    }

    /// Each declared resource range is bounds-checked against the file, but a
    /// record may legally claim the whole file — and a table may hold
    /// thousands of them. Without an aggregate budget a small input drives
    /// unbounded copying before any decode is attempted.
    #[test]
    fn rejects_a_table_whose_resources_sum_past_the_budget() {
        // 512 records × 60 KB is ~30 MB claimed out of a 60 KB file.
        let (image, ne) = build_ne_repeated_full_span(512, 60 * 1024);
        let error = extract_within(&image, ne, 200_000).unwrap_err();
        assert!(matches!(error, NeError::ResourcesTooLarge { limit } if limit == 200_000));
    }

    /// The budget is a total across records, not a per-record cap: the same
    /// table is accepted when the ceiling covers its sum.
    #[test]
    fn a_table_within_the_budget_is_accepted_whole() {
        let (image, ne) = build_ne_repeated_full_span(4, 1024);
        assert!(extract_within(&image, ne, 4 * 1024).is_ok());
        assert!(matches!(
            extract_within(&image, ne, 4 * 1024 - 1).unwrap_err(),
            NeError::ResourcesTooLarge { .. }
        ));
    }

    /// A real container is far inside the shipped ceiling, so the public
    /// entry point loads an ordinary table without a budget in sight.
    ///
    /// The ceiling itself is spelled out as a plain number: written as
    /// `64 * 1024 * 1024` it is only as trustworthy as its arithmetic, and
    /// a budget that quietly became kilobytes would still pass a test that
    /// only asks for "an ordinary table".
    #[test]
    fn an_ordinary_table_is_within_the_shipped_budget() {
        assert_eq!(TOTAL_RESOURCE_BUDGET_BYTES, 67_108_864, "64 MiB");
        let (image, ne) = build_ne_repeated_full_span(64, 8 * 1024);
        assert_eq!(extract(&image, ne).unwrap().bitmaps.len(), 64);
    }

    #[test]
    fn extracts_integer_id_bitmaps_across_multiple_types() {
        // Two types: a non-bitmap type (skipped wholesale) and RT_BITMAP with
        // two integer-id resources.
        let (image, ne) = build_ne(
            0,
            &[
                (int_id(0x000C), vec![(int_id(1), vec![0xAA, 0xBB])]), // RT_MENU-ish
                (
                    RT_BITMAP,
                    vec![(int_id(5), vec![1, 2, 3]), (int_id(9), vec![4, 5])],
                ),
            ],
        );
        let result = extract(&image, ne).unwrap();
        assert_eq!(result.string_named_skipped, 0);
        assert_eq!(result.bitmaps.len(), 2);
        assert_eq!(result.bitmaps.first().unwrap().id, 5);
        assert_eq!(result.bitmaps.first().unwrap().data, vec![1, 2, 3]);
        assert_eq!(result.bitmaps.get(1).unwrap().id, 9);
        assert_eq!(result.bitmaps.get(1).unwrap().data, vec![4, 5]);
    }

    #[test]
    fn skips_and_counts_string_named_bitmap_resources() {
        // Within RT_BITMAP: one integer id, one string-named (bit 15 clear).
        let (image, ne) = build_ne(
            0,
            &[(
                RT_BITMAP,
                vec![(int_id(3), vec![7, 8]), (0x0040, vec![9, 9])],
            )],
        );
        let result = extract(&image, ne).unwrap();
        assert_eq!(result.bitmaps.len(), 1);
        assert_eq!(result.bitmaps.first().unwrap().id, 3);
        assert_eq!(result.string_named_skipped, 1);
    }

    #[test]
    fn applies_the_alignment_shift_to_locate_resource_bytes() {
        // A nonzero shift means offsets/lengths are in 16-byte units: the
        // resource's declared byte length (length_units << shift) rounds the
        // payload up to a whole unit, so the reader returns the payload plus
        // its alignment padding — downstream DIB decoding reads only what the
        // header needs, so the trailing padding is harmless.
        let payload = vec![0x11, 0x22, 0x33, 0x44, 0x55];
        let (image, ne) = build_ne(4, &[(RT_BITMAP, vec![(int_id(2), payload.clone())])]);
        let result = extract(&image, ne).unwrap();
        let extracted = &result.bitmaps.first().unwrap().data;
        assert_eq!(
            extracted.len(),
            16,
            "5-byte payload rounded up to one 16-byte unit"
        );
        assert_eq!(extracted.get(0..5).unwrap(), payload.as_slice());
    }

    #[test]
    fn strips_the_integer_flag_from_reported_ids() {
        let (image, ne) = build_ne(0, &[(RT_BITMAP, vec![(int_id(0x1234), vec![0])])]);
        let result = extract(&image, ne).unwrap();
        assert_eq!(result.bitmaps.first().unwrap().id, 0x1234);
    }

    #[test]
    fn a_pointer_past_the_end_is_resource_table_truncated() {
        // NE header exists but the file is too short to hold ne+0x24.
        let mut image = vec![0_u8; 0x50];
        image.get_mut(0..2).unwrap().copy_from_slice(b"MZ");
        image.get_mut(0x40..0x42).unwrap().copy_from_slice(b"NE");
        let error = extract(&image, 0x40).unwrap_err();
        assert!(matches!(error, NeError::ResourceTableTruncated));
    }

    #[test]
    fn a_table_start_past_the_end_is_resource_table_truncated() {
        let (mut image, ne) = build_ne(0, &[(RT_BITMAP, vec![(int_id(1), vec![0])])]);
        // Point the resource table far past the end of the file.
        image
            .get_mut(ne + 0x24..ne + 0x26)
            .unwrap()
            .copy_from_slice(&0xFFFF_u16.to_le_bytes());
        let error = extract(&image, ne).unwrap_err();
        assert!(matches!(error, NeError::ResourceTableTruncated));
    }

    #[test]
    fn a_truncated_typeinfo_is_reported() {
        // Build a valid image, then cut it off in the middle of the first
        // TYPEINFO's header (right after align_shift).
        let (image, ne) = build_ne(0, &[(RT_BITMAP, vec![(int_id(1), vec![0])])]);
        let table_start = ne + 0x100;
        let truncated = image.get(0..table_start + 3).unwrap().to_vec();
        let error = extract(&truncated, ne).unwrap_err();
        assert!(matches!(error, NeError::TypeInfoTruncated));
    }

    #[test]
    fn a_truncated_nameinfo_is_reported() {
        // Cut off partway through the (single) NAMEINFO record.
        let (image, ne) = build_ne(0, &[(RT_BITMAP, vec![(int_id(1), vec![0])])]);
        let table_start = ne + 0x100;
        // align_shift(2) + typeinfo header(8) + a few NAMEINFO bytes.
        let truncated = image.get(0..table_start + 2 + 8 + 4).unwrap().to_vec();
        let error = extract(&truncated, ne).unwrap_err();
        assert!(matches!(error, NeError::NameInfoTruncated));
    }

    #[test]
    fn a_truncated_non_bitmap_nameinfo_is_reported() {
        // Same truncation but the type is not RT_BITMAP, exercising the
        // bounds-check-only branch for skipped types.
        let (image, ne) = build_ne(0, &[(0x000C | INTEGER_ID_FLAG, vec![(int_id(1), vec![0])])]);
        let table_start = ne + 0x100;
        let truncated = image.get(0..table_start + 2 + 8 + 4).unwrap().to_vec();
        let error = extract(&truncated, ne).unwrap_err();
        assert!(matches!(error, NeError::NameInfoTruncated));
    }

    #[test]
    fn a_non_bitmap_nameinfo_with_exactly_its_12_bytes_present_passes_the_check() {
        // The bounds-check-only branch reads the record's LAST 2 bytes
        // (offset 10, `NAMEINFO_LEN - 2`) to confirm the whole 12-byte
        // record is present. Truncating to exactly 12 bytes means that read
        // succeeds, so the walk proceeds to look for the next TYPEINFO --
        // which is itself past EOF, so the overall walk still fails, but
        // with a *different* error than an in-record truncation. A
        // off-by-a-constant bug (e.g. `NAMEINFO_LEN + 2`, reading past the
        // 12 bytes) would instead see this record itself as truncated.
        let (image, ne) = build_ne(0, &[(0x000C | INTEGER_ID_FLAG, vec![(int_id(1), vec![0])])]);
        let table_start = ne + 0x100;
        let truncated = image.get(0..table_start + 2 + 8 + 12).unwrap().to_vec();
        let error = extract(&truncated, ne).unwrap_err();
        assert!(matches!(error, NeError::TypeInfoTruncated));
    }

    #[test]
    fn a_non_bitmap_nameinfo_missing_only_its_last_2_bytes_is_truncated() {
        // Exactly 8 of the record's 12 bytes are present: offset 10 (the
        // real check) is missing, but offset 6 (`NAMEINFO_LEN / 2`, what an
        // off-by-operator bug would read instead) is present -- so a bug
        // reading the wrong offset would wrongly accept this record.
        let (image, ne) = build_ne(0, &[(0x000C | INTEGER_ID_FLAG, vec![(int_id(1), vec![0])])]);
        let table_start = ne + 0x100;
        let truncated = image.get(0..table_start + 2 + 8 + 8).unwrap().to_vec();
        let error = extract(&truncated, ne).unwrap_err();
        assert!(matches!(error, NeError::NameInfoTruncated));
    }

    #[test]
    fn a_resource_pointing_past_the_end_is_out_of_bounds() {
        let (mut image, ne) = build_ne(0, &[(RT_BITMAP, vec![(int_id(7), vec![0, 0, 0, 0])])]);
        // Rewrite the resource's NAMEINFO offset to point past EOF.
        let table_start = ne + 0x100;
        let nameinfo = table_start + 2 + 8; // align_shift + typeinfo header
        image
            .get_mut(nameinfo..nameinfo + 2)
            .unwrap()
            .copy_from_slice(&0xFFFF_u16.to_le_bytes());
        let error = extract(&image, ne).unwrap_err();
        assert!(matches!(error, NeError::ResourceOutOfBounds { id: 7, .. }));
    }

    #[test]
    fn a_checked_add_overflow_still_reports_the_masked_id() {
        // Force `byte_offset + byte_length` to overflow `usize` (rather than
        // just point past EOF), and prove the reported id is still the
        // integer-flag-stripped value on THIS error path too -- the same
        // masking expression is duplicated at both `ResourceOutOfBounds`
        // call sites, and only the out-of-bounds one (not the overflow one)
        // was otherwise exercised.
        let (mut image, ne) = build_ne(0, &[(RT_BITMAP, vec![(int_id(0x1234), vec![0])])]);
        let table_start = ne + 0x100;
        // align_shift = 48: 0xFFFF << 48 is close to usize::MAX, so adding
        // it to itself (offset_units = length_units = 0xFFFF) overflows.
        image
            .get_mut(table_start..table_start + 2)
            .unwrap()
            .copy_from_slice(&48_u16.to_le_bytes());
        let nameinfo = table_start + 2 + 8; // align_shift + typeinfo header
        image
            .get_mut(nameinfo..nameinfo + 2)
            .unwrap()
            .copy_from_slice(&0xFFFF_u16.to_le_bytes()); // offset
        image
            .get_mut(nameinfo + 2..nameinfo + 4)
            .unwrap()
            .copy_from_slice(&0xFFFF_u16.to_le_bytes()); // length
        let error = extract(&image, ne).unwrap_err();
        assert!(matches!(
            error,
            NeError::ResourceOutOfBounds { id: 0x1234, .. }
        ));
    }

    #[test]
    fn an_empty_bitmap_type_yields_no_bitmaps() {
        let (image, ne) = build_ne(0, &[(RT_BITMAP, vec![])]);
        let result = extract(&image, ne).unwrap();
        assert!(result.bitmaps.is_empty());
    }

    #[test]
    fn an_alignment_shift_that_overflows_usize_is_rejected() {
        // A hostile align_shift >= usize::BITS is not a valid left-shift
        // amount at all; the reader must reject it with a typed error
        // instead of panicking ("attempt to shift left with overflow" in
        // debug/test builds) or silently wrapping in release builds.
        let (mut image, ne) = build_ne(0, &[(RT_BITMAP, vec![(int_id(1), vec![0xAA])])]);
        let table_start = ne + 0x100;
        image
            .get_mut(table_start..table_start + 2)
            .unwrap()
            .copy_from_slice(&0x40_u16.to_le_bytes());
        let error = extract(&image, ne).unwrap_err();
        assert!(matches!(
            error,
            NeError::AlignShiftOverflow { align_shift: 0x40 }
        ));
    }

    #[test]
    fn the_largest_shift_within_usize_bits_does_not_overflow() {
        // usize::BITS - 1 is the largest shift `checked_shl` still accepts.
        // A zero offset/length shifted by it is still exactly representable
        // (as 0), so the resource is found (empty) rather than the shift
        // itself being rejected — proving the fix doesn't reject legitimate
        // large shifts, only ones that overflow the shift amount itself.
        let (mut image, ne) = build_ne(0, &[(RT_BITMAP, vec![(int_id(4), vec![0xAA])])]);
        let table_start = ne + 0x100;
        let align_shift = u16::try_from(usize::BITS - 1).unwrap();
        image
            .get_mut(table_start..table_start + 2)
            .unwrap()
            .copy_from_slice(&align_shift.to_le_bytes());
        let nameinfo = table_start + 2 + 8; // align_shift + typeinfo header
        image
            .get_mut(nameinfo..nameinfo + 4)
            .unwrap()
            .copy_from_slice(&[0, 0, 0, 0]); // offset_units = 0, length_units = 0
        let result = extract(&image, ne).unwrap();
        assert_eq!(result.bitmaps.first().unwrap().data, Vec::<u8>::new());
    }

    #[test]
    fn every_error_variant_renders_a_non_empty_message() {
        for error in [
            NeError::ResourceTableTruncated,
            NeError::TypeInfoTruncated,
            NeError::NameInfoTruncated,
            NeError::ResourceOutOfBounds {
                id: 1,
                offset: 10,
                length: 4,
            },
            NeError::AlignShiftOverflow { align_shift: 64 },
        ] {
            assert!(!error.to_string().is_empty());
        }
    }
}
