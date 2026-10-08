//! [`Ts`], a timestamp in TSO layout (LV1 plan Ruling 3).

use std::fmt;

/// The logical bits of a TSO timestamp.
const LOGICAL_BITS: u32 = 18;
const LOGICAL_MASK: u64 = (1 << LOGICAL_BITS) - 1;

/// A store timestamp: `physical_ms << 18 | logical`, the layout of a TiKV
/// TSO version, on both backends (LV1 plan Ruling 3). Versions on the wire
/// (`StateVersion.ts`, `commit_ts`) are its `u64`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Ts(pub u64);

impl Ts {
    /// Milliseconds since the Unix epoch.
    pub fn physical_ms(self) -> u64 {
        self.0 >> LOGICAL_BITS
    }

    /// The logical counter within the millisecond (18 bits).
    pub fn logical(self) -> u32 {
        // Masked to 18 bits, so it fits.
        u32::try_from(self.0 & LOGICAL_MASK).unwrap_or(u32::MAX)
    }

    /// The largest physical part a timestamp holds (2^46 − 1 ms).
    pub const MAX_PHYSICAL_MS: u64 = u64::MAX >> LOGICAL_BITS;

    /// The timestamp of `ms` and `logical` (its low 18 bits). A physical
    /// part above [`MAX_PHYSICAL_MS`](Self::MAX_PHYSICAL_MS) saturates to
    /// the largest timestamp instead of wrapping.
    pub fn from_parts(ms: u64, logical: u32) -> Self {
        Self::checked_from_parts(ms, logical).unwrap_or(Ts(u64::MAX))
    }

    /// [`from_parts`](Self::from_parts), or `None` when `ms` does not fit.
    pub fn checked_from_parts(ms: u64, logical: u32) -> Option<Self> {
        (ms <= Self::MAX_PHYSICAL_MS)
            .then(|| Ts((ms << LOGICAL_BITS) | (u64::from(logical) & LOGICAL_MASK)))
    }
}

impl fmt::Display for Ts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_physical_part_too_large_saturates_instead_of_wrapping() {
        assert_eq!(Ts::checked_from_parts(Ts::MAX_PHYSICAL_MS + 1, 0), None);
        assert_eq!(Ts::from_parts(u64::MAX, 0), Ts(u64::MAX));
        let top = Ts::from_parts(Ts::MAX_PHYSICAL_MS, 5);
        assert_eq!(top.physical_ms(), Ts::MAX_PHYSICAL_MS);
        assert_eq!(top.logical(), 5);
    }

    #[test]
    fn logical_is_masked_to_its_bits() {
        assert_eq!(Ts::from_parts(1, 1 << 18), Ts::from_parts(1, 0));
        assert_eq!(
            Ts::from_parts(7, 3).to_string(),
            ((7 << 18) | 3).to_string()
        );
    }
}
