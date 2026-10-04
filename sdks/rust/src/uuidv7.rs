//! A UUIDv7 (RFC 9562), the identifier format R3 asks an SDK to mint.
//!
//! Millisecond timestamp in the high 48 bits, version 7 in the next nibble,
//! `rand_a`, then the variant bits and 62 random bits. Time-ordered, which is
//! what a database index wants, and it carries the time of the call, which is
//! what makes a key's age readable in a log.
//!
//! Written out rather than taken from the `uuid` crate: the SDK's whole
//! dependency argument is that it needs no more than the Connect stack, and one
//! 8-byte timestamp plus 10 bytes of `getrandom` is not worth a dependency.

use std::time::{SystemTime, UNIX_EPOCH};

/// The bytes of randomness the OS gives us per call.
///
/// `getrandom` reads the platform's CSPRNG. Its only failure mode is a platform
/// with no entropy source at all, and this function cannot do better about that
/// than the caller can, so it fills with zeros rather than panicking inside a
/// library: the millisecond stamp still orders the ids, and a panic in a token
/// path is worse than a weaker key.
fn random_bytes<const N: usize>() -> [u8; N] {
    let mut bytes = [0u8; N];
    // A short read is left as zeros rather than retried in a loop: `getrandom`
    // already retries internally, and a partial read of a CSPRNG is not an error
    // worth a retry loop on a call path.
    if getrandom::fill(&mut bytes).is_err() {
        return bytes;
    }
    bytes
}

/// A fresh UUIDv7, as `8-4-4-4-12` lowercase hex.
#[must_use]
pub fn uuidv7() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0u64, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        });
    format_uuidv7(millis, random_bytes::<10>())
}

/// A UUIDv7 for an explicit millisecond stamp, which is what the tests use to
/// assert the stamp is *now* rather than merely well-formed.
#[must_use]
pub fn format_uuidv7(unix_millis: u64, tail: [u8; 10]) -> String {
    let mut bytes = [0u8; 16];
    bytes[0] = (unix_millis >> 40) as u8;
    bytes[1] = (unix_millis >> 32) as u8;
    bytes[2] = (unix_millis >> 24) as u8;
    bytes[3] = (unix_millis >> 16) as u8;
    bytes[4] = (unix_millis >> 8) as u8;
    bytes[5] = unix_millis as u8;
    // Version 7 in the high nibble of byte 6, and 4 bits of `rand_a` below it.
    bytes[6] = 0x70 | (tail[0] & 0x0f);
    bytes[7] = tail[1];
    // Variant `0b10` in the top two bits of byte 8.
    bytes[8] = 0x80 | (tail[2] & 0x3f);
    bytes[9..16].copy_from_slice(&tail[3..10]);

    let mut hex = String::with_capacity(36);
    for (at, byte) in bytes.iter().enumerate() {
        if matches!(at, 4 | 6 | 8 | 10) {
            hex.push('-');
        }
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

/// A `f64` uniform over `[0, 1)`, which is what full jitter needs.
///
/// 53 random bits scaled into the mantissa: uniform without the modulo bias a
/// narrow integer draw would have, which matters because the whole point of full
/// jitter is that two clients do not pick the same delay.
#[must_use]
pub fn unit_jitter() -> f64 {
    let mut bytes = [0u8; 8];
    if getrandom::fill(&mut bytes).is_err() {
        return 0.0;
    }
    // 11 leading zero bits leave 53 random ones, and the exponent forces the
    // value into `[1, 2)`, so the subtraction lands in `[0, 1)`.
    let bits = u64::from_le_bytes(bytes) >> 11;
    f64::from_bits(bits | (1023u64 << 52)) - 1.0
}

/// The millisecond stamp a UUIDv7 carries, or `None` for one that is not a v7.
#[must_use]
pub fn uuidv7_time(id: &str) -> Option<u64> {
    let hex: String = id.chars().filter(|c| *c != '-').collect();
    if hex.len() != 32 {
        return None;
    }
    let mut millis = 0u64;
    for at in 0..12 {
        millis = (millis << 4) | u64::from_str_radix(hex.get(at..at + 2)?, 16).ok()?;
    }
    Some(millis)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_minted_id_is_a_v7_with_the_current_stamp() {
        let before = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |s| s.as_millis() as u64);
        let id = uuidv7();
        assert_eq!(id.len(), 36);
        assert_eq!(id.as_bytes()[14], b'7', "version nibble");
        assert!(
            matches!(id.as_bytes()[19], b'8' | b'9' | b'a' | b'b'),
            "variant bits"
        );
        let stamped = uuidv7_time(&id).expect("a v7");
        assert!(
            (before..=before + 1_000).contains(&stamped),
            "{stamped} not near {before}"
        );
    }

    #[test]
    fn ids_are_distinct_and_time_ordered() {
        let first = uuidv7_time(&uuidv7()).expect("stamped");
        let second = uuidv7_time(&uuidv7()).expect("stamped");
        assert!(
            second >= first,
            "the high bits are a clock, so ids do not go backwards"
        );
        assert_ne!(uuidv7(), uuidv7(), "the random tail differs per call");
    }

    #[test]
    fn a_non_uuid_reads_as_no_stamp_rather_than_a_wrong_one() {
        assert_eq!(uuidv7_time("not-a-uuid"), None);
        assert_eq!(uuidv7_time("00000000-0000-0000-0000-000000000000"), Some(0));
    }
}
