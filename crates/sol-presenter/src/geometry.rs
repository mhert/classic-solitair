//! Plain integer geometry: [`Pt`], [`Size`], and [`Rect`] in logical
//! pixels.
//!
//! Logical pixels are the theme's `base_size` space — the space every
//! pile position, pointer event, and display-list quad lives in; the
//! renderer stretches them to the window by one continuous scale.
//! Rectangles are half-open (a point on the right or bottom edge is
//! outside), matching the Win32 rectangle semantics the original game's
//! hit-testing used.

/// A point in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Pt {
    /// Horizontal position, growing rightward.
    pub x: i32,
    /// Vertical position, growing downward.
    pub y: i32,
}

impl Pt {
    /// Creates a point.
    #[must_use]
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }

    /// This point moved by `(dx, dy)`, saturating on overflow.
    #[must_use]
    pub const fn translated(self, dx: i32, dy: i32) -> Self {
        Self {
            x: self.x.saturating_add(dx),
            y: self.y.saturating_add(dy),
        }
    }
}

/// A width/height pair in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Size {
    /// Width in pixels.
    pub w: i32,
    /// Height in pixels.
    pub h: i32,
}

impl Size {
    /// Creates a size.
    #[must_use]
    pub const fn new(w: i32, h: i32) -> Self {
        Self { w, h }
    }
}

/// An axis-aligned rectangle in logical pixels, half-open on the right
/// and bottom edges.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Rect {
    /// Left edge.
    pub x: i32,
    /// Top edge.
    pub y: i32,
    /// Width in pixels.
    pub w: i32,
    /// Height in pixels.
    pub h: i32,
}

impl Rect {
    /// Creates a rectangle from its top-left corner and size.
    #[must_use]
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    /// Creates a rectangle from a top-left point and a size.
    #[must_use]
    pub const fn at(origin: Pt, size: Size) -> Self {
        Self {
            x: origin.x,
            y: origin.y,
            w: size.w,
            h: size.h,
        }
    }

    /// The top-left corner.
    #[must_use]
    pub const fn origin(self) -> Pt {
        Pt::new(self.x, self.y)
    }

    /// The exclusive right edge, saturating on overflow.
    #[must_use]
    pub const fn right(self) -> i32 {
        self.x.saturating_add(self.w)
    }

    /// The exclusive bottom edge, saturating on overflow.
    #[must_use]
    pub const fn bottom(self) -> i32 {
        self.y.saturating_add(self.h)
    }

    /// Whether `pt` lies inside (half-open: the right and bottom edges are
    /// outside, as in Win32 `PtInRect`).
    #[must_use]
    pub const fn contains(self, pt: Pt) -> bool {
        pt.x >= self.x && pt.x < self.right() && pt.y >= self.y && pt.y < self.bottom()
    }

    /// Whether this rectangle and `other` share any area (an empty
    /// intersection — mere edge contact — does not count, as in Win32
    /// `IntersectRect`).
    #[must_use]
    pub const fn intersects(self, other: Self) -> bool {
        self.x < other.right()
            && other.x < self.right()
            && self.y < other.bottom()
            && other.y < self.bottom()
    }

    /// This rectangle moved by `(dx, dy)`, saturating on overflow.
    #[must_use]
    pub const fn translated(self, dx: i32, dy: i32) -> Self {
        Self {
            x: self.x.saturating_add(dx),
            y: self.y.saturating_add(dy),
            w: self.w,
            h: self.h,
        }
    }
}

/// Clamps an `i64` intermediate into `i32` coordinates.
///
/// Layout arithmetic runs in `i64` so that even absurd theme card sizes
/// (dimensions are arbitrary `u32`s) cannot overflow mid-formula; the final
/// coordinate saturates into the `i32` logical-pixel space.
pub(crate) fn saturate(value: i64) -> i32 {
    let clamp = if value.is_negative() {
        i32::MIN
    } else {
        i32::MAX
    };
    i32::try_from(value).unwrap_or(clamp)
}

/// Converts a card count or list index into coordinate space, saturating.
///
/// Counts in a solitaire game never exceed 52, so the saturation is a
/// totality guarantee, not an expected path.
pub(crate) fn index_to_i32(index: usize) -> i32 {
    i32::try_from(index).unwrap_or(i32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translated_moves_and_saturates() {
        assert_eq!(Pt::new(1, 2).translated(3, -4), Pt::new(4, -2));
        assert_eq!(Pt::new(i32::MAX, 0).translated(1, 0).x, i32::MAX);
        let rect = Rect::new(10, 20, 30, 40).translated(-10, 5);
        assert_eq!(rect, Rect::new(0, 25, 30, 40));
    }

    #[test]
    fn contains_is_half_open() {
        let rect = Rect::new(10, 10, 5, 5);
        assert!(rect.contains(Pt::new(10, 10)));
        assert!(rect.contains(Pt::new(14, 14)));
        assert!(!rect.contains(Pt::new(15, 10)));
        assert!(!rect.contains(Pt::new(10, 15)));
        assert!(!rect.contains(Pt::new(9, 10)));
    }

    #[test]
    fn intersects_requires_shared_area() {
        let rect = Rect::new(0, 0, 10, 10);
        assert!(rect.intersects(Rect::new(9, 9, 5, 5)));
        assert!(rect.intersects(Rect::new(-4, -4, 5, 5)));
        // Edge contact only: empty intersection — on every side.
        assert!(!rect.intersects(Rect::new(10, 0, 5, 5)));
        assert!(!rect.intersects(Rect::new(0, 10, 5, 5)));
        assert!(!rect.intersects(Rect::new(-5, 0, 5, 5)));
        assert!(!rect.intersects(Rect::new(0, -5, 5, 5)));
        assert!(!rect.intersects(Rect::new(20, 20, 5, 5)));
    }

    #[test]
    fn rect_accessors_cover_edges_and_origin() {
        let rect = Rect::at(Pt::new(3, 4), Size::new(5, 6));
        assert_eq!(rect.origin(), Pt::new(3, 4));
        assert_eq!(rect.right(), 8);
        assert_eq!(rect.bottom(), 10);
        assert_eq!(Rect::new(i32::MAX, i32::MAX, 1, 1).right(), i32::MAX);
        assert_eq!(Rect::new(i32::MAX, i32::MAX, 1, 1).bottom(), i32::MAX);
    }

    #[test]
    fn saturate_clamps_both_directions() {
        assert_eq!(saturate(7), 7);
        assert_eq!(saturate(i64::from(i32::MAX) + 1), i32::MAX);
        assert_eq!(saturate(i64::from(i32::MIN) - 1), i32::MIN);
    }

    #[test]
    fn index_to_i32_saturates() {
        assert_eq!(index_to_i32(51), 51);
        assert_eq!(index_to_i32(usize::MAX), i32::MAX);
    }
}
