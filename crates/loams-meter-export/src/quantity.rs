//! [`Quantity`]: a metered amount in fixed point, value × 10⁹.
//!
//! The same representation as `_meter.windows.value` (`Decimal(38, 9)`) and
//! `loams-meter-core`'s `Amount`, so a window's value converts without
//! rounding. Never a float (§63 §9.2).

use std::fmt;

/// A metered amount: `raw` = value × 10⁹.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Quantity(i128);

/// A decimal string that is not a quantity.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0:?} is not a decimal with at most 9 fractional digits")]
pub struct QuantityError(pub String);

impl Quantity {
    pub const ZERO: Quantity = Quantity(0);
    /// 10⁹: one unit.
    pub const SCALE: i128 = 1_000_000_000;

    /// Whole units.
    pub const fn from_units(units: i64) -> Quantity {
        Quantity(units as i128 * Self::SCALE)
    }

    /// From value × 10⁹.
    pub const fn from_raw(raw: i128) -> Quantity {
        Quantity(raw)
    }

    /// Value × 10⁹.
    pub const fn raw(self) -> i128 {
        self.0
    }

    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }

    pub const fn is_negative(self) -> bool {
        self.0 < 0
    }

    pub fn saturating_add(self, other: Quantity) -> Quantity {
        Quantity(self.0.saturating_add(other.0))
    }

    pub fn saturating_sub(self, other: Quantity) -> Quantity {
        Quantity(self.0.saturating_sub(other.0))
    }

    /// The smallest whole number of units at least this large.
    pub fn ceil_units(self) -> Quantity {
        let rem = self.0.rem_euclid(Self::SCALE);
        if rem == 0 {
            self
        } else {
            Quantity(self.0 - rem + Self::SCALE)
        }
    }

    /// Whether this is a whole number of units.
    pub fn is_whole(self) -> bool {
        self.0 % Self::SCALE == 0
    }

    /// Parses `"12"`, `"0.5"`, `"-3.000000001"`.
    pub fn parse(s: &str) -> Result<Quantity, QuantityError> {
        let bad = || QuantityError(s.to_owned());
        let (negative, digits) = match s.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, s),
        };
        let (int, frac) = match digits.split_once('.') {
            Some((i, f)) => (i, f),
            None => (digits, ""),
        };
        if int.is_empty()
            || !int.bytes().all(|b| b.is_ascii_digit())
            || !frac.bytes().all(|b| b.is_ascii_digit())
            || frac.len() > 9
            || (digits.contains('.') && frac.is_empty())
        {
            return Err(bad());
        }
        let int: i128 = int.parse().map_err(|_| bad())?;
        let mut frac_raw: i128 = 0;
        for (i, b) in frac.bytes().enumerate() {
            frac_raw += i128::from(b - b'0') * 10_i128.pow(8 - i as u32);
        }
        let raw = int
            .checked_mul(Self::SCALE)
            .and_then(|v| v.checked_add(frac_raw))
            .ok_or_else(bad)?;
        Ok(Quantity(if negative { -raw } else { raw }))
    }
}

impl fmt::Display for Quantity {
    /// The canonical decimal: no trailing fractional zeros, no `.` for a
    /// whole number, `-` for a negative value.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sign = if self.0 < 0 { "-" } else { "" };
        let abs = self.0.unsigned_abs();
        let scale = Self::SCALE as u128;
        let (int, frac) = (abs / scale, abs % scale);
        if frac == 0 {
            write!(f, "{sign}{int}")
        } else {
            let frac = format!("{frac:09}");
            write!(f, "{sign}{int}.{}", frac.trim_end_matches('0'))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Quantity;

    #[test]
    fn parse_and_display_round_trip() {
        for (s, raw) in [
            ("0", 0),
            ("12", 12_000_000_000),
            ("0.5", 500_000_000),
            ("-3.000000001", -3_000_000_001),
            ("1.25", 1_250_000_000),
        ] {
            let q = Quantity::parse(s).expect(s);
            assert_eq!(q.raw(), raw, "{s}");
            assert_eq!(q.to_string(), s);
        }
        for bad in [
            "",
            "-",
            "1.",
            ".5",
            "1.0000000001",
            "1e3",
            "a",
            "1.2.3",
            " 1",
        ] {
            assert!(Quantity::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn ceil_units() {
        assert_eq!(
            Quantity::parse("1.2").expect("q").ceil_units(),
            Quantity::from_units(2)
        );
        assert_eq!(
            Quantity::from_units(3).ceil_units(),
            Quantity::from_units(3)
        );
        assert_eq!(
            Quantity::parse("-1.5").expect("q").ceil_units(),
            Quantity::from_units(-1)
        );
    }
}
