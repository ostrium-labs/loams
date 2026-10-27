//! Fixed-point amounts and money (design §63 §9.2, D374, D2161).
//!
//! [`Amount`] is a quantity in a feature's unit with nine decimal places,
//! held as an `i128` count of 10⁻⁹ units. Nine places and 38 digits match
//! `_meter.windows.value Decimal(38, 9)`, so a window read back from the
//! House converts without rounding. [`Money`] is integer micros plus an
//! ISO 4217 code. Neither is ever a float.

use std::fmt;
use std::ops::{Add, AddAssign, Neg, Sub, SubAssign};
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Why an amount, currency or money value was refused.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum AmountError {
    #[error("not a decimal amount: '{0}'")]
    Syntax(String),
    #[error("amount '{0}' has more than 9 decimal places")]
    Precision(String),
    #[error("amount '{0}' is out of range")]
    Range(String),
    #[error("not an ISO 4217 currency code: '{0}'")]
    Currency(String),
    #[error("currencies differ: {0} and {1}")]
    CurrencyMismatch(Currency, Currency),
    #[error("money overflow")]
    Overflow,
}

/// A fixed-point decimal quantity with nine decimal places.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Amount(i128);

/// The largest magnitude a `Decimal(38, 9)` holds, in raw units.
const MAX_RAW: i128 = 99_999_999_999_999_999_999_999_999_999_999_999_999;

impl Amount {
    /// Raw units per whole unit.
    pub const SCALE: i128 = 1_000_000_000;
    pub const ZERO: Amount = Amount(0);
    pub const MAX: Amount = Amount(MAX_RAW);
    pub const MIN: Amount = Amount(-MAX_RAW);

    /// `units` whole units.
    pub const fn from_units(units: i64) -> Amount {
        Amount(units as i128 * Self::SCALE)
    }

    /// An amount from its raw count of 10⁻⁹ units.
    pub const fn from_raw(raw: i128) -> Amount {
        Amount(raw)
    }

    /// The raw count of 10⁻⁹ units.
    pub const fn raw(self) -> i128 {
        self.0
    }

    /// The whole units, rounded toward zero.
    pub const fn trunc_units(self) -> i128 {
        self.0 / Self::SCALE
    }

    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }

    pub const fn is_positive(self) -> bool {
        self.0 > 0
    }

    pub const fn is_negative(self) -> bool {
        self.0 < 0
    }

    /// Parses `12`, `-3`, `0.5`, `1.000000001`. Up to nine decimal places;
    /// no exponent, no grouping, no leading `+`.
    pub fn parse(text: &str) -> Result<Amount, AmountError> {
        let syntax = || AmountError::Syntax(text.to_owned());
        let (negative, body) = match text.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, text),
        };
        let (whole, frac) = match body.split_once('.') {
            Some((whole, frac)) => (whole, Some(frac)),
            None => (body, None),
        };
        if whole.is_empty() || !whole.bytes().all(|b| b.is_ascii_digit()) {
            return Err(syntax());
        }
        let mut raw: i128 = 0;
        for b in whole.bytes() {
            raw = raw
                .checked_mul(10)
                .and_then(|r| r.checked_add(i128::from(b - b'0')))
                .ok_or_else(|| AmountError::Range(text.to_owned()))?;
        }
        raw = raw
            .checked_mul(Self::SCALE)
            .ok_or_else(|| AmountError::Range(text.to_owned()))?;
        if let Some(frac) = frac {
            if frac.is_empty() || !frac.bytes().all(|b| b.is_ascii_digit()) {
                return Err(syntax());
            }
            if frac.len() > 9 {
                return Err(AmountError::Precision(text.to_owned()));
            }
            let mut f: i128 = 0;
            for b in frac.bytes() {
                f = f * 10 + i128::from(b - b'0');
            }
            for _ in frac.len()..9 {
                f *= 10;
            }
            raw += f;
        }
        if raw > MAX_RAW {
            return Err(AmountError::Range(text.to_owned()));
        }
        Ok(Amount(if negative { -raw } else { raw }))
    }

    /// Reads a JSON number exactly when it is an integer or a decimal with at
    /// most nine places; `None` for anything else (strings, floats with an
    /// exponent, too many places).
    pub fn from_json(value: &serde_json::Value) -> Option<Amount> {
        match value {
            serde_json::Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    return Some(Amount::from_units(i));
                }
                if let Some(u) = n.as_u64() {
                    let raw = i128::from(u).checked_mul(Self::SCALE)?;
                    return (raw <= MAX_RAW).then_some(Amount(raw));
                }
                Amount::parse(&n.to_string()).ok()
            }
            _ => None,
        }
    }

    pub fn checked_add(self, other: Amount) -> Option<Amount> {
        self.0
            .checked_add(other.0)
            .filter(|r| r.abs() <= MAX_RAW)
            .map(Amount)
    }

    pub fn checked_sub(self, other: Amount) -> Option<Amount> {
        self.0
            .checked_sub(other.0)
            .filter(|r| r.abs() <= MAX_RAW)
            .map(Amount)
    }

    pub fn saturating_add(self, other: Amount) -> Amount {
        Amount(self.0.saturating_add(other.0).clamp(-MAX_RAW, MAX_RAW))
    }

    pub fn saturating_sub(self, other: Amount) -> Amount {
        Amount(self.0.saturating_sub(other.0).clamp(-MAX_RAW, MAX_RAW))
    }

    /// `self × num / den`, rounded toward negative infinity. `den` must be
    /// positive; a zero `den` gives zero.
    pub fn mul_ratio(self, num: i128, den: i128) -> Amount {
        if den <= 0 {
            return Amount::ZERO;
        }
        match self.0.checked_mul(num) {
            Some(p) => Amount(p.div_euclid(den).clamp(-MAX_RAW, MAX_RAW)),
            // Divide first when the product overflows; loses at most the
            // remainder's share, still rounded down.
            None => Amount(
                (self.0.div_euclid(den))
                    .saturating_mul(num)
                    .clamp(-MAX_RAW, MAX_RAW),
            ),
        }
    }

    /// `self × count`, saturating.
    pub fn mul_int(self, count: i128) -> Amount {
        Amount(self.0.saturating_mul(count).clamp(-MAX_RAW, MAX_RAW))
    }

    /// Zero when negative.
    pub fn clamp_non_negative(self) -> Amount {
        if self.0 < 0 { Amount::ZERO } else { self }
    }
}

impl fmt::Display for Amount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sign = if self.0 < 0 { "-" } else { "" };
        let abs = self.0.unsigned_abs();
        let scale = Self::SCALE.unsigned_abs();
        let whole = abs / scale;
        let frac = abs % scale;
        if frac == 0 {
            return write!(f, "{sign}{whole}");
        }
        let digits = format!("{frac:09}");
        write!(f, "{sign}{whole}.{}", digits.trim_end_matches('0'))
    }
}

impl FromStr for Amount {
    type Err = AmountError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Amount::parse(s)
    }
}

impl Serialize for Amount {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Amount {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        match &value {
            serde_json::Value::String(s) => Amount::parse(s).map_err(serde::de::Error::custom),
            other => Amount::from_json(other)
                .ok_or_else(|| serde::de::Error::custom(format!("not an amount: {other}"))),
        }
    }
}

impl Add for Amount {
    type Output = Amount;
    fn add(self, rhs: Amount) -> Amount {
        self.saturating_add(rhs)
    }
}

impl AddAssign for Amount {
    fn add_assign(&mut self, rhs: Amount) {
        *self = *self + rhs;
    }
}

impl Sub for Amount {
    type Output = Amount;
    fn sub(self, rhs: Amount) -> Amount {
        self.saturating_sub(rhs)
    }
}

impl SubAssign for Amount {
    fn sub_assign(&mut self, rhs: Amount) {
        *self = *self - rhs;
    }
}

impl Neg for Amount {
    type Output = Amount;
    fn neg(self) -> Amount {
        Amount(-self.0)
    }
}

impl std::iter::Sum for Amount {
    fn sum<I: Iterator<Item = Amount>>(iter: I) -> Amount {
        iter.fold(Amount::ZERO, |a, b| a + b)
    }
}

/// An ISO 4217 alphabetic code: three upper-case ASCII letters.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Currency([u8; 3]);

impl Currency {
    pub const USD: Currency = Currency(*b"USD");
    pub const EUR: Currency = Currency(*b"EUR");

    /// Accepts three upper-case ASCII letters. The code list itself is not
    /// checked: providers add codes, and an unknown code fails at the
    /// provider, not here.
    pub fn parse(code: &str) -> Result<Currency, AmountError> {
        let bytes = code.as_bytes();
        if bytes.len() == 3 && bytes.iter().all(u8::is_ascii_uppercase) {
            Ok(Currency([bytes[0], bytes[1], bytes[2]]))
        } else {
            Err(AmountError::Currency(code.to_owned()))
        }
    }

    pub fn as_str(&self) -> &str {
        // Only ASCII letters are ever stored.
        std::str::from_utf8(&self.0).unwrap_or("XXX")
    }
}

impl fmt::Debug for Currency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Currency({})", self.as_str())
    }
}

impl fmt::Display for Currency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for Currency {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Currency {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Currency::parse(&s).map_err(serde::de::Error::custom)
    }
}

/// Money: integer micros of a currency (D374). 1 USD = 1,000,000 micros.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Money {
    pub micros: i64,
    pub currency: Currency,
}

impl Money {
    pub const fn new(micros: i64, currency: Currency) -> Money {
        Money { micros, currency }
    }

    pub const fn zero(currency: Currency) -> Money {
        Money {
            micros: 0,
            currency,
        }
    }

    pub fn checked_add(self, other: Money) -> Result<Money, AmountError> {
        self.same(other)?;
        let micros = self
            .micros
            .checked_add(other.micros)
            .ok_or(AmountError::Overflow)?;
        Ok(Money::new(micros, self.currency))
    }

    pub fn checked_sub(self, other: Money) -> Result<Money, AmountError> {
        self.same(other)?;
        let micros = self
            .micros
            .checked_sub(other.micros)
            .ok_or(AmountError::Overflow)?;
        Ok(Money::new(micros, self.currency))
    }

    /// The price of `quantity` units at `self` per unit, rounded up to the
    /// next micro (a provider never bills a fraction of a micro).
    pub fn times(self, quantity: Amount) -> Result<Money, AmountError> {
        let product = i128::from(self.micros)
            .checked_mul(quantity.raw())
            .ok_or(AmountError::Overflow)?;
        let scale = Amount::SCALE;
        let micros = if product >= 0 {
            (product + scale - 1) / scale
        } else {
            product / scale
        };
        let micros = i64::try_from(micros).map_err(|_| AmountError::Overflow)?;
        Ok(Money::new(micros, self.currency))
    }

    fn same(self, other: Money) -> Result<(), AmountError> {
        if self.currency == other.currency {
            Ok(())
        } else {
            Err(AmountError::CurrencyMismatch(self.currency, other.currency))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn parses_and_prints_canonical_decimals() {
        for (text, raw, shown) in [
            ("0", 0, "0"),
            ("12", 12_000_000_000, "12"),
            ("-3", -3_000_000_000, "-3"),
            ("0.5", 500_000_000, "0.5"),
            ("1.000000001", 1_000_000_001, "1.000000001"),
            ("2.10", 2_100_000_000, "2.1"),
        ] {
            let a = Amount::parse(text).expect(text);
            assert_eq!(a.raw(), raw, "{text}");
            assert_eq!(a.to_string(), shown, "{text}");
        }
    }

    #[test]
    fn refuses_bad_amounts() {
        for text in ["", "-", "1.", ".5", "1e3", "+1", "1,000", "abc"] {
            assert!(
                matches!(Amount::parse(text), Err(AmountError::Syntax(_))),
                "{text}"
            );
        }
        assert!(matches!(
            Amount::parse("0.0000000001"),
            Err(AmountError::Precision(_))
        ));
        assert!(matches!(
            Amount::parse("1000000000000000000000000000000"),
            Err(AmountError::Range(_))
        ));
    }

    #[test]
    fn reads_json_numbers_exactly() {
        let v: serde_json::Value = serde_json::from_str("[1840, 0.25, -7, \"9\"]").expect("json");
        let items = v.as_array().expect("array");
        assert_eq!(Amount::from_json(&items[0]), Some(Amount::from_units(1840)));
        assert_eq!(
            Amount::from_json(&items[1]),
            Some(Amount::from_raw(250_000_000))
        );
        assert_eq!(Amount::from_json(&items[2]), Some(Amount::from_units(-7)));
        assert_eq!(Amount::from_json(&items[3]), None);
    }

    #[test]
    fn serde_is_a_decimal_string() {
        let a = Amount::parse("36000000.5").expect("amount");
        let json = serde_json::to_string(&a).expect("ser");
        assert_eq!(json, "\"36000000.5\"");
        let back: Amount = serde_json::from_str(&json).expect("de");
        assert_eq!(back, a);
        let from_number: Amount = serde_json::from_str("42").expect("de number");
        assert_eq!(from_number, Amount::from_units(42));
    }

    #[test]
    fn currency_is_three_upper_case_letters() {
        assert_eq!(Currency::parse("EUR").expect("eur"), Currency::EUR);
        for bad in ["eur", "EU", "EURO", "E1R", ""] {
            assert!(Currency::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn money_refuses_mixed_currencies_and_rounds_up() {
        let usd = Money::new(1_500_000, Currency::USD);
        let eur = Money::new(1, Currency::EUR);
        assert!(matches!(
            usd.checked_add(eur),
            Err(AmountError::CurrencyMismatch(_, _))
        ));
        // 0.000001 USD per unit × 0.5 units = half a micro → one micro.
        let price = Money::new(1, Currency::USD);
        assert_eq!(
            price
                .times(Amount::parse("0.5").expect("half"))
                .expect("times")
                .micros,
            1
        );
        assert_eq!(
            Money::new(2_000, Currency::USD)
                .times(Amount::from_units(3))
                .expect("times")
                .micros,
            6_000
        );
    }

    #[test]
    fn mul_ratio_floors() {
        assert_eq!(
            Amount::from_units(1000).mul_ratio(1, 100),
            Amount::from_units(10)
        );
        assert_eq!(Amount::from_raw(199).mul_ratio(1, 100), Amount::from_raw(1));
        assert_eq!(Amount::from_units(5).mul_ratio(1, 0), Amount::ZERO);
    }

    proptest! {
        #[test]
        fn display_parse_round_trip(raw in -MAX_RAW / 2..MAX_RAW / 2) {
            let a = Amount::from_raw(raw);
            prop_assert_eq!(Amount::parse(&a.to_string()), Ok(a));
        }

        #[test]
        fn add_sub_inverse(a in -1_000_000_000_000_000i128..1_000_000_000_000_000, b in -1_000_000_000_000_000i128..1_000_000_000_000_000) {
            let (a, b) = (Amount::from_raw(a), Amount::from_raw(b));
            prop_assert_eq!((a + b) - b, a);
        }
    }
}
