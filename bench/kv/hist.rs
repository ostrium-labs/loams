//! The log-linear latency histogram shared by kv-loadgen and kv-native.

/// A log-linear latency histogram in nanoseconds: 32 sub-buckets per power
/// of two (about 3% relative precision).
#[derive(Clone)]
pub struct Hist {
    pub counts: Vec<u64>,
    pub total: u64,
    pub max: u64,
}

const SUB: u32 = 5;

impl Hist {
    pub fn new() -> Self {
        Self {
            counts: vec![0; 64 << SUB],
            total: 0,
            max: 0,
        }
    }

    fn index(v: u64) -> usize {
        let v = v.max(1);
        let msb = 63 - v.leading_zeros();
        if msb < SUB {
            return v as usize;
        }
        let sub = (v >> (msb - SUB)) & ((1 << SUB) - 1);
        (((msb - SUB + 1) << SUB) as u64 + sub) as usize
    }

    fn lower(i: usize) -> u64 {
        let i = i as u64;
        let group = i >> SUB;
        if group == 0 {
            return i;
        }
        let msb = group + u64::from(SUB) - 1;
        (1 << msb) | ((i & ((1 << SUB) - 1)) << (msb - u64::from(SUB)))
    }

    pub fn record(&mut self, ns: u64, n: u64) {
        self.counts[Self::index(ns)] += n;
        self.total += n;
        self.max = self.max.max(ns);
    }

    pub fn merge(&mut self, o: &Hist) {
        for (a, b) in self.counts.iter_mut().zip(&o.counts) {
            *a += b;
        }
        self.total += o.total;
        self.max = self.max.max(o.max);
    }

    pub fn quantile_us(&self, q: f64) -> f64 {
        if self.total == 0 {
            return 0.0;
        }
        let want = ((self.total as f64) * q).ceil().max(1.0) as u64;
        let mut seen = 0;
        for (i, c) in self.counts.iter().enumerate() {
            seen += c;
            if seen >= want {
                return Self::lower(i) as f64 / 1000.0;
            }
        }
        self.max as f64 / 1000.0
    }
}


impl Default for Hist {
    fn default() -> Self {
        Self::new()
    }
}
