//! A log-linear latency histogram: 32 sub-buckets per power of two of
//! nanoseconds (about 3 % precision), cheap to record and to merge.

const SUB_BITS: u32 = 5;
const SUB: u64 = 1 << SUB_BITS;
const BUCKETS: usize = (64 - SUB_BITS as usize + 1) << SUB_BITS;

#[derive(Debug, Clone)]
pub struct Hist {
    counts: Vec<u64>,
    total: u64,
}

impl Default for Hist {
    fn default() -> Self {
        Hist {
            counts: vec![0; BUCKETS],
            total: 0,
        }
    }
}

fn index(v: u64) -> usize {
    if v < SUB {
        return v as usize;
    }
    let bit = 63 - v.leading_zeros();
    let shift = bit - SUB_BITS;
    let sub = (v >> shift) & (SUB - 1);
    (((shift + 1) as u64) << SUB_BITS | sub) as usize
}

fn value(i: usize) -> u64 {
    let i = i as u64;
    if i < SUB {
        return i;
    }
    let shift = (i >> SUB_BITS) - 1;
    let sub = i & (SUB - 1);
    ((SUB + sub) << shift) + ((1u64 << shift) >> 1)
}

impl Hist {
    pub fn record(&mut self, ns: u64) {
        self.counts[index(ns)] += 1;
        self.total += 1;
    }

    pub fn merge(&mut self, other: &Hist) {
        for (a, b) in self.counts.iter_mut().zip(&other.counts) {
            *a += b;
        }
        self.total += other.total;
    }

    /// The `q` quantile in milliseconds, or `None` when empty.
    pub fn quantile_ms(&self, q: f64) -> Option<f64> {
        if self.total == 0 {
            return None;
        }
        let want = ((self.total as f64) * q).ceil().max(1.0) as u64;
        let mut seen = 0;
        for (i, c) in self.counts.iter().enumerate() {
            seen += c;
            if seen >= want {
                return Some((value(i) as f64 / 1e6 * 1000.0).round() / 1000.0);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_and_value_round_trip_within_precision() {
        for v in [
            0u64,
            1,
            31,
            32,
            33,
            64,
            1000,
            123_456,
            9_999_999,
            u64::MAX / 4,
        ] {
            let back = value(index(v));
            let err = (back as f64 - v as f64).abs() / (v.max(1) as f64);
            assert!(err < 0.04, "{v} -> {back}");
        }
    }

    #[test]
    fn quantiles() {
        let mut h = Hist::default();
        for v in 1..=1000u64 {
            h.record(v * 1_000_000);
        }
        let p50 = h.quantile_ms(0.5).unwrap();
        let p99 = h.quantile_ms(0.99).unwrap();
        assert!((480.0..520.0).contains(&p50), "{p50}");
        assert!((960.0..1020.0).contains(&p99), "{p99}");
    }
}
