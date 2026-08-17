//! [`CardSize`]: the `[cards] base_size` logical card dimensions.

/// A card's logical width and height in pixels (`[cards] base_size`,
/// e.g. `[71, 96]` — the Win98 original).
///
/// Both dimensions are at least 1 and fit in a [`u32`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CardSize {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

/// [`CardSize`] could not be built from `[cards] base_size`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum CardSizeError {
    /// `base_size` did not have exactly two elements.
    #[error("base_size must have exactly 2 elements [width, height], got {count}")]
    WrongArity {
        /// The number of elements `base_size` actually had.
        count: usize,
    },
    /// `base_size` had two elements, but at least one was not `1..=u32::MAX`.
    #[error(
        "base_size = [{width}, {height}] is invalid: each dimension must be at least 1 and fit a u32"
    )]
    OutOfRange {
        /// The raw width value.
        width: i64,
        /// The raw height value.
        height: i64,
    },
}

impl TryFrom<Vec<i64>> for CardSize {
    type Error = CardSizeError;

    /// # Errors
    ///
    /// Returns [`CardSizeError::WrongArity`] unless `raw` has exactly two
    /// elements, or [`CardSizeError::OutOfRange`] unless both (width,
    /// height) are at least 1 and fit in a [`u32`].
    fn try_from(raw: Vec<i64>) -> Result<Self, Self::Error> {
        let count = raw.len();
        let [width, height]: [i64; 2] = raw
            .try_into()
            .map_err(|_| CardSizeError::WrongArity { count })?;
        let valid = |value: i64| u32::try_from(value).ok().filter(|value| *value >= 1);
        match (valid(width), valid(height)) {
            (Some(width), Some(height)) => Ok(Self { width, height }),
            _ => Err(CardSizeError::OutOfRange { width, height }),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn builds_from_a_valid_pair() {
        let size = CardSize::try_from(vec![71, 96]).unwrap();
        assert_eq!(size.width, 71);
        assert_eq!(size.height, 96);
    }

    #[test]
    fn accepts_the_minimum_size_of_one_by_one() {
        let size = CardSize::try_from(vec![1, 1]).unwrap();
        assert_eq!(size.width, 1);
        assert_eq!(size.height, 1);
    }

    #[test]
    fn rejects_zero_width_or_height() {
        assert!(CardSize::try_from(vec![0, 96]).is_err());
        assert!(CardSize::try_from(vec![71, 0]).is_err());
    }

    #[test]
    fn rejects_negative_width_or_height() {
        assert!(CardSize::try_from(vec![-1, 96]).is_err());
        assert!(CardSize::try_from(vec![71, -1]).is_err());
    }

    #[test]
    fn rejects_values_that_do_not_fit_u32() {
        let huge = i64::from(u32::MAX) + 1;
        assert!(CardSize::try_from(vec![huge, 96]).is_err());
        assert!(CardSize::try_from(vec![71, huge]).is_err());
    }

    #[test]
    fn error_message_names_both_raw_values_and_is_accurate_for_single_dimension_errors() {
        let error = CardSize::try_from(vec![0, -5]).unwrap_err();
        let message = error.to_string();
        assert!(message.contains('0'), "{message}");
        assert!(message.contains("-5"), "{message}");
        assert!(message.contains("each dimension"), "{message}");
    }

    #[test]
    fn rejects_too_few_elements() {
        let error = CardSize::try_from(vec![71]).unwrap_err();
        assert_eq!(error, CardSizeError::WrongArity { count: 1 });
    }

    #[test]
    fn rejects_too_many_elements() {
        let error = CardSize::try_from(vec![71, 96, 3]).unwrap_err();
        assert_eq!(error, CardSizeError::WrongArity { count: 3 });
    }

    #[test]
    fn rejects_zero_elements() {
        let error = CardSize::try_from(vec![]).unwrap_err();
        assert_eq!(error, CardSizeError::WrongArity { count: 0 });
    }

    #[test]
    fn wrong_arity_message_names_the_count() {
        let error = CardSize::try_from(vec![1, 2, 3, 4]).unwrap_err();
        assert!(error.to_string().contains('4'));
    }
}
