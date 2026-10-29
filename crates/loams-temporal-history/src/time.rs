//! Conversions between protobuf times and milliseconds since the Unix epoch.

use prost_types::{Duration, Timestamp};

/// A timestamp `ms` milliseconds after the epoch.
pub fn ts(ms: i64) -> Timestamp {
    Timestamp {
        seconds: ms.div_euclid(1000),
        nanos: (ms.rem_euclid(1000) * 1_000_000) as i32,
    }
}

/// A timestamp's milliseconds since the epoch (sub-millisecond parts dropped).
pub fn ms(t: &Timestamp) -> i64 {
    t.seconds * 1000 + i64::from(t.nanos) / 1_000_000
}

/// A duration of `ms` milliseconds.
pub fn dur(ms: i64) -> Duration {
    Duration {
        seconds: ms.div_euclid(1000),
        nanos: (ms.rem_euclid(1000) * 1_000_000) as i32,
    }
}

/// A duration's milliseconds; `None` or zero is 0 ("no timeout" in Temporal).
pub fn dur_ms(d: Option<&Duration>) -> i64 {
    d.map_or(0, |d| d.seconds * 1000 + i64::from(d.nanos) / 1_000_000)
}
