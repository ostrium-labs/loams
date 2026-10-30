//! Exact arithmetic for allocation (design §84 §8.1): amounts are
//! [`Amount`]s, `i128` nano-units of the reporting currency, and driver
//! quantities are [`Amount`]s in their driver unit. A share is
//! `cost × weight / total`, where the product can exceed `i128` (a month of
//! petabyte byte-seconds times a large pool), so it is computed in 256 bits.
//! Shares are floored and the remainder is handed out by largest remainder,
//! so the parts always sum to the whole.

use loams_meter_core::Amount;

use crate::CostError;

const LOW: u128 = u64::MAX as u128;

/// `a × b` as a 256-bit `(high, low)` pair.
fn mul_wide(a: u128, b: u128) -> (u128, u128) {
    let (a1, a0) = (a >> 64, a & LOW);
    let (b1, b0) = (b >> 64, b & LOW);
    let p00 = a0 * b0;
    let p01 = a0 * b1;
    let p10 = a1 * b0;
    let p11 = a1 * b1;
    let mid = (p00 >> 64) + (p01 & LOW) + (p10 & LOW);
    let low = (p00 & LOW) | (mid << 64);
    let high = p11 + (p01 >> 64) + (p10 >> 64) + (mid >> 64);
    (high, low)
}

/// `(high:low) / d` and its remainder; `high < d` so the quotient fits.
fn div_wide(high: u128, low: u128, d: u128) -> (u128, u128) {
    let mut rem = high;
    let mut quot = 0u128;
    for bit in (0..128).rev() {
        let carry = rem >> 127;
        rem = (rem << 1) | ((low >> bit) & 1);
        quot <<= 1;
        if carry == 1 || rem >= d {
            rem = rem.wrapping_sub(d);
            quot |= 1;
        }
    }
    (quot, rem)
}

/// `⌊a × b / d⌋` and `(a × b) mod d`, or `None` when `d` is zero or the
/// quotient does not fit in 128 bits.
pub fn mul_div(a: u128, b: u128, d: u128) -> Option<(u128, u128)> {
    if d == 0 {
        return None;
    }
    let (high, low) = mul_wide(a, b);
    if high >= d {
        return None;
    }
    Some(div_wide(high, low, d))
}

/// Splits `total` over `weights` in proportion, exactly: the parts sum to
/// `total`. Each part is floored; the remainder goes one nano-unit at a time
/// to the largest fractional parts, ties to the lower index, so the result
/// depends only on the weights' order (callers pass them sorted by key).
/// A negative `total` is split as its magnitude and negated.
///
/// Weights must be non-negative and not all zero.
pub fn split(total: Amount, weights: &[Amount]) -> Result<Vec<Amount>, CostError> {
    let mut denominator: u128 = 0;
    let mut w = Vec::with_capacity(weights.len());
    for weight in weights {
        let raw = u128::try_from(weight.raw()).map_err(|_| CostError::NegativeQuantity)?;
        denominator = denominator.checked_add(raw).ok_or(CostError::Overflow)?;
        w.push(raw);
    }
    if denominator == 0 {
        return Err(CostError::NoWeight);
    }
    let negative = total.is_negative();
    let magnitude = total.raw().unsigned_abs();
    let mut parts = Vec::with_capacity(w.len());
    let mut remainders = Vec::with_capacity(w.len());
    let mut given: u128 = 0;
    for (i, raw) in w.iter().enumerate() {
        let (q, r) = mul_div(magnitude, *raw, denominator).ok_or(CostError::Overflow)?;
        given += q;
        parts.push(q);
        remainders.push((r, i));
    }
    // Σ⌊x_i⌋ ≥ Σx_i − n, so fewer than n nano-units are left.
    let left = magnitude - given;
    if left > 0 {
        // Largest remainder first; equal remainders keep index order.
        remainders.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        let left = usize::try_from(left).map_err(|_| CostError::Overflow)?;
        for (_, i) in remainders.iter().take(left) {
            parts[*i] += 1;
        }
    }
    parts
        .into_iter()
        .map(|p| {
            let p = i128::try_from(p).map_err(|_| CostError::Overflow)?;
            Ok(Amount::from_raw(if negative { -p } else { p }))
        })
        .collect()
}

/// `weight / total` as a nine-decimal [`Amount`], floored (for display; the
/// FOCUS export re-splits ratios with [`split`] so they sum to 1).
pub fn ratio(weight: Amount, total: Amount) -> Amount {
    let (Ok(w), Ok(t)) = (u128::try_from(weight.raw()), u128::try_from(total.raw())) else {
        return Amount::ZERO;
    };
    match mul_div(w, Amount::SCALE as u128, t) {
        Some((q, _)) => Amount::from_raw(i128::try_from(q).unwrap_or(i128::MAX)),
        None => Amount::ZERO,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(s: &str) -> Amount {
        Amount::parse(s).expect("amount")
    }

    #[test]
    fn mul_div_matches_small_numbers() {
        assert_eq!(mul_div(7, 9, 4), Some((15, 3)));
        assert_eq!(mul_div(0, 9, 4), Some((0, 0)));
        assert_eq!(mul_div(1, 1, 0), None);
    }

    #[test]
    fn mul_div_beyond_i128() {
        // 2^100 × 2^100 / 2^90 = 2^110.
        let x = 1u128 << 100;
        assert_eq!(mul_div(x, x, 1u128 << 90), Some((1u128 << 110, 0)));
        // u128::MAX × u128::MAX / u128::MAX = u128::MAX.
        assert_eq!(
            mul_div(u128::MAX, u128::MAX, u128::MAX),
            Some((u128::MAX, 0))
        );
        // Quotient too large.
        assert_eq!(mul_div(u128::MAX, 2, 1), None);
    }

    #[test]
    fn mul_div_remainder_exact() {
        let a_ = 0xdead_beef_cafe_babe_1234_5678_9abc_def0u128;
        let b_ = 0x0fed_cba9_8765_4321u128;
        let d = 0x1_0000_0000_0000_0001u128;
        let (q, r) = mul_div(a_, b_, d).expect("fits");
        // Check q × d + r = a × b in 256 bits.
        let (h1, l1) = mul_wide(q, d);
        let (l2, c) = l1.overflowing_add(r);
        let h2 = h1 + u128::from(c);
        assert_eq!((h2, l2), mul_wide(a_, b_));
        assert!(r < d);
    }

    #[test]
    fn split_conserves_and_rounds_by_largest_remainder() {
        let parts = split(a("0.000000010"), &[a("1"), a("1"), a("1")]).expect("split");
        // 10 nano-units over three: 4, 3, 3 (ties go to the lower index).
        assert_eq!(
            parts.iter().map(|p| p.raw()).collect::<Vec<_>>(),
            vec![4, 3, 3]
        );
        let parts = split(a("-1"), &[a("1"), a("2")]).expect("split");
        assert_eq!(parts.iter().copied().sum::<Amount>(), a("-1"));
        assert!(parts.iter().all(|p| !p.is_positive()));
    }

    #[test]
    fn split_refuses_bad_weights() {
        assert_eq!(split(a("1"), &[a("0")]), Err(CostError::NoWeight));
        assert_eq!(
            split(a("1"), &[a("-1"), a("2")]),
            Err(CostError::NegativeQuantity)
        );
    }

    #[test]
    fn ratio_floors() {
        assert_eq!(ratio(a("1"), a("3")).to_string(), "0.333333333");
        assert_eq!(ratio(a("3"), a("3")).to_string(), "1");
    }
}
