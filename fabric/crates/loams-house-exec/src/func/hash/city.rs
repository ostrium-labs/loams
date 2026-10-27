//! CityHash64 as of CityHash **v1.0.2**, the version ClickHouse keeps for
//! `cityHash64` (`contrib/cityhash102`, `CityHash_v1_0_2::CityHash64`) so
//! that hashes stay stable across releases. Later CityHash releases changed
//! the function; the `cityhash-rs` crate the repository pins is 128-bit
//! only, so this is a port (R31.4).
//!
//! Ported from Google's CityHash v1.0.2 (`city.cc`), Copyright (c) 2011
//! Google, Inc., MIT licence, as vendored by ClickHouse (Apache-2.0). See
//! `NOTICE`.

const K0: u64 = 0xc3a5_c85c_97cb_3127;
const K1: u64 = 0xb492_b66f_be98_f273;
const K2: u64 = 0x9ae1_6a3b_2f90_404f;
const K3: u64 = 0xc949_d7c7_509e_6557;

fn fetch64(s: &[u8], i: usize) -> u64 {
    let mut b = [0u8; 8];
    b.copy_from_slice(&s[i..i + 8]);
    u64::from_le_bytes(b)
}

fn fetch32(s: &[u8], i: usize) -> u64 {
    let mut b = [0u8; 4];
    b.copy_from_slice(&s[i..i + 4]);
    u64::from(u32::from_le_bytes(b))
}

fn rotate(v: u64, shift: u32) -> u64 {
    if shift == 0 { v } else { v.rotate_right(shift) }
}

fn rotate_by_at_least_1(v: u64, shift: u32) -> u64 {
    v.rotate_right(shift)
}

fn shift_mix(v: u64) -> u64 {
    v ^ (v >> 47)
}

/// CityHash's `Hash128to64`: 128 bits (`low`, `high`) to 64. ClickHouse
/// combines the hashes of several arguments with it.
pub fn hash128_to_64(low: u64, high: u64) -> u64 {
    const MUL: u64 = 0x9ddf_ea08_eb38_2d69;
    let mut a = (low ^ high).wrapping_mul(MUL);
    a ^= a >> 47;
    let mut b = (high ^ a).wrapping_mul(MUL);
    b ^= b >> 47;
    b.wrapping_mul(MUL)
}

fn hash_len16(u: u64, v: u64) -> u64 {
    hash128_to_64(u, v)
}

fn hash_len0to16(s: &[u8]) -> u64 {
    let len = s.len();
    if len > 8 {
        let a = fetch64(s, 0);
        let b = fetch64(s, len - 8);
        return hash_len16(
            a,
            rotate_by_at_least_1(b.wrapping_add(len as u64), len as u32),
        ) ^ b;
    }
    if len >= 4 {
        let a = fetch32(s, 0);
        return hash_len16((len as u64).wrapping_add(a << 3), fetch32(s, len - 4));
    }
    if len > 0 {
        let a = u64::from(s[0]);
        let b = u64::from(s[len >> 1]);
        let c = u64::from(s[len - 1]);
        let y = a.wrapping_add(b << 8);
        let z = (len as u64).wrapping_add(c << 2);
        return shift_mix(y.wrapping_mul(K2) ^ z.wrapping_mul(K3)).wrapping_mul(K2);
    }
    K2
}

fn hash_len17to32(s: &[u8]) -> u64 {
    let len = s.len();
    let a = fetch64(s, 0).wrapping_mul(K1);
    let b = fetch64(s, 8);
    let c = fetch64(s, len - 8).wrapping_mul(K2);
    let d = fetch64(s, len - 16).wrapping_mul(K0);
    hash_len16(
        rotate(a.wrapping_sub(b), 43)
            .wrapping_add(rotate(c, 30))
            .wrapping_add(d),
        a.wrapping_add(rotate(b ^ K3, 20))
            .wrapping_sub(c)
            .wrapping_add(len as u64),
    )
}

fn weak_hash_len32_with_seeds_raw(
    w: u64,
    x: u64,
    y: u64,
    z: u64,
    mut a: u64,
    mut b: u64,
) -> (u64, u64) {
    a = a.wrapping_add(w);
    b = rotate(b.wrapping_add(a).wrapping_add(z), 21);
    let c = a;
    a = a.wrapping_add(x);
    a = a.wrapping_add(y);
    b = b.wrapping_add(rotate(a, 44));
    (a.wrapping_add(z), b.wrapping_add(c))
}

fn weak_hash_len32_with_seeds(s: &[u8], i: usize, a: u64, b: u64) -> (u64, u64) {
    weak_hash_len32_with_seeds_raw(
        fetch64(s, i),
        fetch64(s, i + 8),
        fetch64(s, i + 16),
        fetch64(s, i + 24),
        a,
        b,
    )
}

fn hash_len33to64(s: &[u8]) -> u64 {
    let len = s.len();
    let mut z = fetch64(s, 24);
    let mut a = fetch64(s, 0).wrapping_add(
        (len as u64)
            .wrapping_add(fetch64(s, len - 16))
            .wrapping_mul(K0),
    );
    let mut b = rotate(a.wrapping_add(z), 52);
    let mut c = rotate(a, 37);
    a = a.wrapping_add(fetch64(s, 8));
    c = c.wrapping_add(rotate(a, 7));
    a = a.wrapping_add(fetch64(s, 16));
    let vf = a.wrapping_add(z);
    let vs = b.wrapping_add(rotate(a, 31)).wrapping_add(c);
    a = fetch64(s, 16).wrapping_add(fetch64(s, len - 32));
    z = fetch64(s, len - 8);
    b = rotate(a.wrapping_add(z), 52);
    c = rotate(a, 37);
    a = a.wrapping_add(fetch64(s, len - 24));
    c = c.wrapping_add(rotate(a, 7));
    a = a.wrapping_add(fetch64(s, len - 16));
    let wf = a.wrapping_add(z);
    let ws = b.wrapping_add(rotate(a, 31)).wrapping_add(c);
    let r = shift_mix(
        vf.wrapping_add(ws)
            .wrapping_mul(K2)
            .wrapping_add(wf.wrapping_add(vs).wrapping_mul(K0)),
    );
    shift_mix(r.wrapping_mul(K0).wrapping_add(vs)).wrapping_mul(K2)
}

/// CityHash64 v1.0.2 of `s`.
pub fn city_hash64(s: &[u8]) -> u64 {
    let len = s.len();
    if len <= 32 {
        return if len <= 16 {
            hash_len0to16(s)
        } else {
            hash_len17to32(s)
        };
    }
    if len <= 64 {
        return hash_len33to64(s);
    }
    // For strings over 64 bytes: the end first, then 64-byte chunks with 56
    // bytes of state (v, w, x, y, z), as in v1.0.2.
    let mut x = fetch64(s, 0);
    let mut y = fetch64(s, len - 16) ^ K1;
    let mut z = fetch64(s, len - 56) ^ K0;
    let mut v = weak_hash_len32_with_seeds(s, len - 64, len as u64, y);
    let mut w = weak_hash_len32_with_seeds(s, len - 32, (len as u64).wrapping_mul(K1), K0);
    z = z.wrapping_add(shift_mix(v.1).wrapping_mul(K1));
    x = rotate(z.wrapping_add(x), 39).wrapping_mul(K1);
    y = rotate(y, 33).wrapping_mul(K1);
    let mut remaining = (len - 1) & !63;
    let mut i = 0;
    loop {
        x = rotate(
            x.wrapping_add(y)
                .wrapping_add(v.0)
                .wrapping_add(fetch64(s, i + 16)),
            37,
        )
        .wrapping_mul(K1);
        y = rotate(y.wrapping_add(v.1).wrapping_add(fetch64(s, i + 48)), 42).wrapping_mul(K1);
        x ^= w.1;
        y ^= v.0;
        z = rotate(z ^ w.0, 33);
        v = weak_hash_len32_with_seeds(s, i, v.1.wrapping_mul(K1), x.wrapping_add(w.0));
        w = weak_hash_len32_with_seeds(s, i + 32, z.wrapping_add(w.1), y);
        std::mem::swap(&mut z, &mut x);
        i += 64;
        remaining -= 64;
        if remaining == 0 {
            break;
        }
    }
    hash_len16(
        hash_len16(v.0, w.0)
            .wrapping_add(shift_mix(y).wrapping_mul(K1))
            .wrapping_add(z),
        hash_len16(v.1, w.1).wrapping_add(x),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn city_hash64_matches_the_oracle() {
        // `SELECT cityHash64('…')` on clickhouse-server 26.9.2.8.
        assert_eq!(city_hash64(b""), 11_160_318_154_034_397_263);
        assert_eq!(city_hash64(b"a"), 2_603_192_927_274_642_682);
        assert_eq!(city_hash64(b"abc"), 4_220_206_313_085_259_313);
        assert_eq!(
            city_hash64(b"hello world, this is 33 bytes ok!"),
            912_851_255_445_140_898
        );
        assert_eq!(city_hash64(&[b'x'; 100]), 8_273_723_555_210_364_035);
        assert_eq!(
            city_hash64("ab".repeat(40).as_bytes()),
            16_250_926_667_562_127_564
        );
    }
}
