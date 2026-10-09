//! The fuzz invariants (fuzz/fuzz_targets) under proptest, so every
//! `cargo test` runs them; CI also runs the cargo-fuzz targets for 60 s each.
use loams_sqlgate::fuzz::{
    auth_server, connection_phase, decoders, handshake_response, packet_framing,
};
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
    fn fuzz_connection_phase(data in prop::collection::vec(any::<u8>(), 0..1024)) {
        let _ = connection_phase(&data);
    }

    #[test]
    fn fuzz_connection_phase_near_valid(ctl in any::<u8>(), flip in any::<(usize, u8)>(), cut in 0usize..16) {
        let mut v = conversation(ctl);
        let i = flip.0 % v.len();
        v[i] ^= flip.1;
        v.truncate(v.len() - cut.min(v.len() - 1));
        let _ = connection_phase(&v);
    }

    #[test]
    fn fuzz_auth_server(data in prop::collection::vec(any::<u8>(), 0..256)) {
        auth_server(&data);
    }

    #[test]
    fn fuzz_decoders(data in prop::collection::vec(any::<u8>(), 0..256)) {
        decoders(&data);
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

/// A valid TLS conversation for `connection_phase`: control byte, then
/// SSLRequest (seq 1), response (seq 2) and a full-auth password (seq 4).
pub fn conversation(ctl: u8) -> Vec<u8> {
    use loams_sqlgate::codec::handshake::{Capabilities as C, HandshakeResponse41, SslRequest};
    use loams_sqlgate::codec::packet::encode;
    let caps = C::PROTOCOL_41 | C::SECURE_CONNECTION | C::PLUGIN_AUTH | C::SSL;
    let mut out = vec![ctl & !1];
    let mut seq = 1;
    encode(
        &SslRequest {
            capabilities: caps,
            max_packet: 1 << 24,
            charset: 0xff,
        }
        .encode(),
        &mut seq,
        &mut out,
    );
    let r = HandshakeResponse41 {
        capabilities: caps,
        max_packet: 1 << 24,
        charset: 0xff,
        username: "u".into(),
        auth_response: loams_sqlgate::codec::auth::Password::new(vec![5; 32]),
        database: None,
        auth_plugin: Some("caching_sha2_password".into()),
        attributes: vec![],
        zstd_level: None,
    };
    encode(&r.encode().expect("enc"), &mut seq, &mut out);
    let mut seq = 4;
    encode(b"pw\0", &mut seq, &mut out);
    out
}

#[test]
fn a_valid_conversation_completes() {
    // ctl: TLS required, fast miss then full ok (bits 1-3 = 0b010), chunk 1.
    assert!(
        connection_phase(&conversation(0b0000_0100)),
        "miss, then full auth ok"
    );
    assert!(
        !connection_phase(&conversation(0b0000_0000)),
        "miss, then full auth denied"
    );
}

/// `UPDATE_FUZZ_SEEDS=1` writes the connection_phase seed corpus.
#[test]
fn fuzz_seed_corpus() {
    let dir = format!(
        "{}/fuzz/corpus/connection_phase",
        env!("CARGO_MANIFEST_DIR")
    );
    let seeds = [
        ("tls-full-ok", 0b0000_0100u8),
        ("tls-fast-hit", 0b0000_0010),
        ("tls-full-denied", 0),
    ];
    for (name, ctl) in seeds {
        let path = format!("{dir}/{name}");
        if std::env::var("UPDATE_FUZZ_SEEDS").as_deref() == Ok("1") {
            std::fs::create_dir_all(&dir).expect("dir");
            std::fs::write(&path, conversation(ctl)).expect("seed");
        }
        assert_eq!(
            std::fs::read(&path).expect("seed (UPDATE_FUZZ_SEEDS=1 writes it)"),
            conversation(ctl)
        );
    }
}
