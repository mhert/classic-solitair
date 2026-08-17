//! [`MsRand`]: the C runtime `rand()` the original game rolled cascade
//! velocities with.
//!
//! The linear congruential generator behind Microsoft's C `rand()`:
//! `state = state · 214013 + 2531011 (mod 2³²)`, returning bits 30..16 —
//! values in `0..=0x7FFF`. The win cascade's launch velocities come from
//! this exact sequence, so reproducing the generator reproduces the
//! original's trajectories bit for bit.

/// The MSVC `rand()` linear congruential generator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MsRand {
    state: u32,
}

impl MsRand {
    /// Seeds the generator (`srand`).
    pub(crate) const fn new(seed: u32) -> Self {
        Self { state: seed }
    }

    /// The next `rand()` value, `0..=0x7FFF`.
    pub(crate) fn next(&mut self) -> i32 {
        self.state = self.state.wrapping_mul(214_013).wrapping_add(2_531_011);
        // Bits 30..16 fit 15 bits, so the conversion cannot fail; the
        // fallback merely satisfies totality.
        i32::try_from((self.state >> 16) & 0x7FFF).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_zero_matches_the_msvc_sequence() {
        // First values of the MSVC C runtime's rand() after srand(0),
        // verifiable against any MSVC rand reference.
        let mut rng = MsRand::new(0);
        let first: Vec<i32> = (0..5).map(|_| rng.next()).collect();
        assert_eq!(first, vec![38, 7719, 21238, 2437, 8855]);
    }

    #[test]
    fn seed_one_matches_the_msvc_sequence() {
        let mut rng = MsRand::new(1);
        let first: Vec<i32> = (0..3).map(|_| rng.next()).collect();
        assert_eq!(first, vec![41, 18467, 6334]);
    }

    #[test]
    fn values_stay_in_the_15_bit_range() {
        let mut rng = MsRand::new(0xFFFF_FFFF);
        for _ in 0..1000 {
            let value = rng.next();
            assert!((0..=0x7FFF).contains(&value));
        }
    }
}
