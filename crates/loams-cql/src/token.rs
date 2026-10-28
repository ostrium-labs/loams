//! Cassandra's compatibility variant of MurmurHash3 x64 128.

const C1: u64 = 0x87c3_7b91_1142_53d5;
const C2: u64 = 0x4cf5_ad43_2745_937f;

fn mix(value: u64) -> u64 {
    let value = (value ^ (value >> 33)).wrapping_mul(0xff51_afd7_ed55_8ccd);
    let value = (value ^ (value >> 33)).wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    value ^ (value >> 33)
}

/// Returns the Cassandra Murmur3 partition token for an encoded partition key.
///
/// Cassandra sign-extends each byte of the final partial block. Empty keys
/// map to the reserved minimum token; a nonempty hash equal to that token is
/// normalized to the maximum token.
#[must_use]
pub fn cassandra_token(key: &[u8]) -> i64 {
    if key.is_empty() {
        return i64::MIN;
    }
    let mut h1 = 0_u64;
    let mut h2 = 0_u64;
    let mut chunks = key.chunks_exact(16);
    for chunk in &mut chunks {
        let k1 = u64::from_le_bytes(chunk[..8].try_into().expect("eight bytes"));
        let k2 = u64::from_le_bytes(chunk[8..].try_into().expect("eight bytes"));
        h1 ^= k1.wrapping_mul(C1).rotate_left(31).wrapping_mul(C2);
        h1 = h1
            .rotate_left(27)
            .wrapping_add(h2)
            .wrapping_mul(5)
            .wrapping_add(0x52dc_e729);
        h2 ^= k2.wrapping_mul(C2).rotate_left(33).wrapping_mul(C1);
        h2 = h2
            .rotate_left(31)
            .wrapping_add(h1)
            .wrapping_mul(5)
            .wrapping_add(0x3849_5ab5);
    }
    let tail = chunks.remainder();
    let mut k1 = 0_u64;
    let mut k2 = 0_u64;
    for (index, byte) in tail.iter().copied().enumerate() {
        // Java's signed byte is extended to a long before each shift.
        let signed = (i64::from(byte as i8)) as u64;
        if index < 8 {
            k1 ^= signed << (index * 8);
        } else {
            k2 ^= signed << ((index - 8) * 8);
        }
    }
    if tail.len() > 8 {
        h2 ^= k2.wrapping_mul(C2).rotate_left(33).wrapping_mul(C1);
    }
    if !tail.is_empty() {
        h1 ^= k1.wrapping_mul(C1).rotate_left(31).wrapping_mul(C2);
    }
    let len = key.len() as u64;
    h1 ^= len;
    h2 ^= len;
    h1 = h1.wrapping_add(h2);
    h2 = h2.wrapping_add(h1);
    h1 = mix(h1);
    h2 = mix(h2);
    let token = h1.wrapping_add(h2) as i64;
    if token == i64::MIN { i64::MAX } else { token }
}
