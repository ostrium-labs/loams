//! The worker side of the agent globals that need Rust (SPIKE): the
//! entropy pool behind `Math.random` and `crypto`. The worker cannot call
//! `getrandom` (the seal forbids it), so the pool is a ChaCha20 stream
//! keyed by the host's 32 bytes (`AgentLoad.entropy`) and rekeyed with
//! every `Entropy` frame; after each [`REFILL_AFTER`] bytes drawn the
//! worker asks for more (`EntropyRequest`) and keeps serving meanwhile.

use std::cell::{Cell, RefCell};

use rand::{RngCore, SeedableRng};
use rand_chacha::ChaCha20Rng;

/// Bytes drawn between two `EntropyRequest`s.
pub(crate) const REFILL_AFTER: usize = 1 << 20;

pub(crate) struct Entropy {
    rng: RefCell<ChaCha20Rng>,
    drawn: Cell<usize>,
}

impl Entropy {
    pub(crate) fn new(seed: &[u8]) -> Self {
        let mut key = [0u8; 32];
        for (i, b) in seed.iter().enumerate() {
            key[i % 32] ^= b;
        }
        Entropy {
            rng: RefCell::new(ChaCha20Rng::from_seed(key)),
            drawn: Cell::new(0),
        }
    }

    /// Mixes fresh host entropy into the key.
    pub(crate) fn reseed(&self, data: &[u8]) {
        let mut key = [0u8; 32];
        self.rng.borrow_mut().fill_bytes(&mut key);
        for (i, b) in data.iter().enumerate() {
            key[i % 32] ^= b;
        }
        *self.rng.borrow_mut() = ChaCha20Rng::from_seed(key);
        self.drawn.set(0);
    }

    /// `Math.random`: 53 random bits in [0, 1).
    pub(crate) fn random(&self) -> f64 {
        let bits = self.rng.borrow_mut().next_u64() >> 11;
        bits as f64 / (1u64 << 53) as f64
    }

    /// `len` random bytes, and whether to ask the host for more entropy.
    pub(crate) fn bytes(&self, len: usize) -> (Vec<u8>, bool) {
        let mut out = vec![0u8; len];
        self.rng.borrow_mut().fill_bytes(&mut out);
        let before = self.drawn.get();
        let after = before.saturating_add(len);
        self.drawn.set(after);
        (out, before / REFILL_AFTER != after / REFILL_AFTER)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pool_is_keyed_by_the_host_and_asks_for_more() {
        let a = Entropy::new(&[1; 32]);
        let b = Entropy::new(&[1; 32]);
        let c = Entropy::new(&[2; 32]);
        assert_eq!(a.bytes(16).0, b.bytes(16).0);
        assert_ne!(a.bytes(16).0, c.bytes(16).0);
        let r = a.random();
        assert!((0.0..1.0).contains(&r));
        b.reseed(&[9; 32]);
        assert_ne!(a.bytes(16).0, b.bytes(16).0);
        let fresh = Entropy::new(&[3; 32]);
        assert!(!fresh.bytes(REFILL_AFTER - 1).1);
        assert!(fresh.bytes(1).1);
    }
}
