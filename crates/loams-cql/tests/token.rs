//! Cassandra partitioner regressions. Empty keys have a reserved token;
//! high-bit bytes exercise Cassandra's signed tail-byte behavior.
use loams_cql::cassandra_token;

#[test]
fn empty_partition_key_uses_minimum_token() {
    assert_eq!(cassandra_token(b""), i64::MIN);
}

#[test]
fn cassandra_murmur3_client_vectors() {
    // Published by the Apache-2.0 Scylla Rust driver's Murmur3 partitioner
    // tests; Cassandra 5.0.9 must independently verify these before KV3a exit.
    for (key, token) in [
        ("test", -6_017_608_668_500_074_083_i64),
        ("xd", 4_507_812_186_440_344_727_i64),
        ("primary_key", -1_632_642_444_691_073_360_i64),
        ("kremówki", 4_354_931_215_268_080_151_i64),
    ] {
        assert_eq!(cassandra_token(key.as_bytes()), token, "{key}");
    }
}

#[test]
fn distinct_binary_keys_do_not_share_a_token_for_representative_tails() {
    for len in 1..=15 {
        let low = vec![0x7f; len];
        let high = vec![0x80; len];
        assert_ne!(cassandra_token(&low), cassandra_token(&high));
    }
}
