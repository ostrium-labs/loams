//! The fuzz invariants (fuzz/fuzz_targets) under proptest, so every
//! `cargo test` runs them; CI also runs the cargo-fuzz targets for 60 s each.
use loams_sqlgate::fuzz::{handshake_response, packet_framing};
use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig { cases: 4096, ..ProptestConfig::default() })]

    #[test]
    fn fuzz_handshake_response(data in prop::collection::vec(any::<u8>(), 0..512)) {
        handshake_response(&data);
    }

    #[test]
    fn fuzz_handshake_response_near_valid(cut in 0usize..200, flip in any::<(usize, u8)>()) {
        // Mutations of a valid response reach deeper than random bytes.
        let mut v = valid();
        let i = flip.0 % v.len();
        v[i] ^= flip.1;
        v.truncate(v.len().saturating_sub(cut % 8));
        handshake_response(&v);
    }

    #[test]
    fn fuzz_packet_framing(data in prop::collection::vec(any::<u8>(), 0..2048)) {
        packet_framing(&data);
    }
}

/// The mysql 8.4 client's HandshakeResponse41 from tests/fixtures/clients.
fn valid() -> Vec<u8> {
    static VALID: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    VALID.get_or_init(load_valid).clone()
}

fn load_valid() -> Vec<u8> {
    let path = format!(
        "{}/tests/fixtures/clients/mysql84.jsonl",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(path).expect("fixture");
    let line = text.lines().nth(1).expect("response line");
    let v: serde_json::Value = serde_json::from_str(line).expect("json");
    hex::decode(v["hex"].as_str().expect("hex")).expect("hex")
}
