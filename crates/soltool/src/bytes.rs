//! Bounds-checked little-endian scalar reads over a byte slice — the shared
//! primitive every hand-rolled parser in this crate ([`crate::ne`]) is built
//! on.
//!
//! Each reader returns `None` rather than panicking when the requested field
//! runs past the end of the slice (or its start offset overflows), so callers
//! translate a short buffer into their own typed error with `.ok_or(..)`
//! instead of risking an out-of-bounds index.

/// Reads a little-endian `u16` at `offset`, or `None` if `data` is too short.
pub(crate) fn read_u16_le(data: &[u8], offset: usize) -> Option<u16> {
    let end = offset.checked_add(2)?;
    let array: [u8; 2] = data.get(offset..end)?.try_into().ok()?;
    Some(u16::from_le_bytes(array))
}

/// Reads a little-endian `u32` at `offset`, or `None` if `data` is too short.
pub(crate) fn read_u32_le(data: &[u8], offset: usize) -> Option<u32> {
    let end = offset.checked_add(4)?;
    let array: [u8; 4] = data.get(offset..end)?.try_into().ok()?;
    Some(u32::from_le_bytes(array))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_little_endian_scalars() {
        let data = [0x01, 0x02, 0x03, 0x04, 0x05];
        assert_eq!(read_u16_le(&data, 0), Some(0x0201));
        assert_eq!(read_u32_le(&data, 0), Some(0x0403_0201));
    }

    #[test]
    fn returns_none_past_the_end() {
        let data = [0x01, 0x02];
        assert_eq!(read_u16_le(&data, 1), None);
        assert_eq!(read_u32_le(&data, 0), None);
    }

    #[test]
    fn returns_none_when_the_offset_overflows() {
        assert_eq!(read_u16_le(&[0; 4], usize::MAX), None);
        assert_eq!(read_u32_le(&[0; 4], usize::MAX), None);
    }
}
