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

    /// The timestamp of `ms` and `logical` (its low 18 bits).
    pub fn from_parts(ms: u64, logical: u32) -> Self {
        Ts((ms << LOGICAL_BITS) | (u64::from(logical) & LOGICAL_MASK))
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
    fn logical_is_masked_to_its_bits() {
        assert_eq!(Ts::from_parts(1, 1 << 18), Ts::from_parts(1, 0));
        assert_eq!(
            Ts::from_parts(7, 3).to_string(),
            ((7 << 18) | 3).to_string()
        );
    }
}
