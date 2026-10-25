//! Tenant placements and product roots (design §77 §6, D1645, D122; KV1
//! plan KV0 Task 6).
//!
//! A namespace lives in a **dedicated** keyspace, or in a **shared**
//! keyspace under a prefix of its own (D122's size classes). Under that,
//! each gateway keeps its keys below a one-byte product tag: `R` for Redis,
//! `C` for CQL (D1645). The directory that maps a namespace to its
//! placement is not here (KV2); callers pass placements.
//!
//! A shared tenant's prefix is appended to the base root **escaped and
//! terminated** ([`key::put_bytes`]), so no
//! tenant's root is a prefix of another's: tenants `t` and `t1` of one
//! shared keyspace get the roots `t 00 00` and `t 1 00 00`, and a scan of
//! one never reaches the other.

use crate::{StoreConfig, key};

/// Where a tenant's keys live (D122).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Placement {
    /// A keyspace of its own, under the base root.
    Dedicated { keyspace: String },
    /// A shared keyspace, under the base root and the tenant's prefix.
    Shared { keyspace: String, prefix: Vec<u8> },
}

impl Placement {
    /// The keyspace the tenant's keys are in.
    pub fn keyspace(&self) -> &str {
        match self {
            Placement::Dedicated { keyspace } | Placement::Shared { keyspace, .. } => keyspace,
        }
    }
}

/// The product tag of the Redis gateway's keys (D1645).
pub const PRODUCT_REDIS: u8 = b'R';
/// The product tag of the CQL gateway's keys (D1645).
pub const PRODUCT_CQL: u8 = b'C';

/// The store configuration of the tenant `placement` names, on the backend
/// (file or cluster) and under the root of `base`: its keyspace, and for a
/// shared keyspace the root `base.root ‖ key::bytes(prefix)`.
pub fn store_config(base: &StoreConfig, placement: &Placement) -> StoreConfig {
    match placement {
        Placement::Dedicated { keyspace } => base.with_keyspace(keyspace),
        Placement::Shared { keyspace, prefix } => {
            let mut root = base.root().to_vec();
            key::put_bytes(&mut root, prefix);
            base.with_keyspace(keyspace).with_root(root)
        }
    }
}

/// `config` under the root `config.root ‖ product`: one product's keys of a
/// tenant ([`PRODUCT_REDIS`], [`PRODUCT_CQL`]).
pub fn product_root(config: &StoreConfig, product: u8) -> StoreConfig {
    let mut root = config.root().to_vec();
    root.push(product);
    config.with_root(root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EmbeddedConfig;

    fn base() -> StoreConfig {
        StoreConfig::Embedded(EmbeddedConfig {
            root: vec![9],
            ..EmbeddedConfig::new("/data/kv.redb", "base")
        })
    }

    #[test]
    fn placements_set_keyspace_and_root() {
        let dedicated = store_config(
            &base(),
            &Placement::Dedicated {
                keyspace: "ns_a".into(),
            },
        );
        assert_eq!(dedicated.keyspace(), "ns_a");
        assert_eq!(dedicated.root(), &[9]);
        let shared = store_config(
            &base(),
            &Placement::Shared {
                keyspace: "pool_1".into(),
                prefix: b"t\x00".to_vec(),
            },
        );
        assert_eq!(shared.keyspace(), "pool_1");
        assert_eq!(shared.root(), &[9, b't', 0x00, 0xFF, 0x00, 0x00]);
        assert_eq!(
            product_root(&shared, PRODUCT_REDIS).root(),
            &[9, b't', 0, 0xFF, 0, 0, b'R']
        );
    }

    #[test]
    fn shared_roots_are_prefix_free() {
        let root = |p: &[u8]| {
            store_config(
                &base(),
                &Placement::Shared {
                    keyspace: "pool".into(),
                    prefix: p.to_vec(),
                },
            )
            .root()
            .to_vec()
        };
        let prefixes: [&[u8]; 5] = [b"", b"t", b"t1", b"t\x00", b"t\x00\x00"];
        for a in prefixes {
            for b in prefixes {
                if a != b {
                    assert!(!root(b).starts_with(&root(a)), "{a:?} covers {b:?}");
                }
            }
        }
    }
}
