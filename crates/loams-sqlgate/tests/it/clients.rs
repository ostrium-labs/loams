//! Handshakes captured from real clients against keyspace-mode TiDB v8.5.8
//! (scripts/sqlgate/capture/capture.sh; tests/fixtures/clients/README.md).
use loams_sqlgate::codec::auth::{
    AuthMoreData, AuthSwitchRequest, CACHING_SHA2, NATIVE, double_sha256, scramble_caching_sha2,
    verify_caching_sha2,
};
use loams_sqlgate::codec::command::ErrPacket;
use loams_sqlgate::codec::handshake::{
    Capabilities as C, ClientHello, HandshakeV10, Limits, decode_client_hello,
};

struct Packet {
    dir: String,
    payload: Vec<u8>,
}

fn load(name: &str) -> Vec<Packet> {
    let path = format!(
        "{}/tests/fixtures/clients/{name}.jsonl",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{path}: {e}"))
        .lines()
        .map(|l| {
            let v: serde_json::Value = serde_json::from_str(l).expect("json");
            Packet {
                dir: v["dir"].as_str().expect("dir").to_owned(),
                payload: hex::decode(v["hex"].as_str().expect("hex")).expect("hex"),
            }
        })
        .collect()
}

const CLIENTS: [(&str, &str, &str); 4] = [
    ("mysql84", "libmysql", "8.4.10"),
    ("connector-j", "MySQL Connector/J", "9.7.0"),
    ("mysql2", "Node-MySQL-2", "3.15.3"),
    ("mariadb", "libmariadb", "3.4.10"),
];

#[test]
fn handshake_response_decodes_captured_clients() {
    for (name, client_name, version) in CLIENTS {
        let p = load(name);
        assert_eq!(p[0].dir, "s2c");
        let greeting =
            HandshakeV10::decode(&p[0].payload).unwrap_or_else(|e| panic!("{name} greeting: {e}"));
        assert_eq!(greeting.server_version, "8.0.11-TiDB-v8.5.8");
        assert_eq!(greeting.auth_plugin, NATIVE);
        // Our encoder reproduces TiDB's greeting byte for byte.
        assert_eq!(
            greeting.encode(),
            p[0].payload,
            "{name}: greeting re-encodes exactly"
        );

        let ClientHello::Response(r) = decode_client_hello(&p[1].payload, &Limits::default())
            .unwrap_or_else(|e| panic!("{name}: {e}"))
        else {
            panic!("{name}: expected a HandshakeResponse41");
        };
        assert_eq!(r.username, "loams_cap", "{name}");
        assert_eq!(r.auth_plugin.as_deref(), Some(NATIVE), "{name}");
        assert_eq!(r.auth_response.len(), 20, "{name}: a native scramble");
        assert!(
            r.capabilities
                .contains(C::PROTOCOL_41 | C::SECURE_CONNECTION | C::PLUGIN_AUTH),
            "{name}"
        );
        let attr = |k: &str| {
            r.attributes
                .iter()
                .find(|(n, _)| n == k)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(attr("_client_name"), Some(client_name), "{name}");
        assert_eq!(attr("_client_version"), Some(version), "{name}");
        // Re-encoding gives the client's bytes back.
        assert_eq!(
            r.encode(),
            p[1].payload,
            "{name}: response re-encodes exactly"
        );

        // TiDB switches the user to caching_sha2_password with a fresh nonce.
        let switch =
            AuthSwitchRequest::decode(&p[2].payload).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(switch.plugin, CACHING_SHA2);
        assert_eq!(switch.encode(), p[2].payload);
        let nonce = switch.nonce().expect("20-byte nonce");
        // The client's scramble is ours, for the capture password.
        assert_eq!(
            p[3].payload,
            scramble_caching_sha2(b"capture", nonce),
            "{name}"
        );
        assert!(verify_caching_sha2(
            &double_sha256(b"capture"),
            nonce,
            &p[3].payload
        ));
        assert!(!verify_caching_sha2(
            &double_sha256(b"capturf"),
            nonce,
            &p[3].payload
        ));
        // No cache entry: TiDB asks for full authentication; over plaintext
        // the client asks for the RSA key (0x02), which TiDB cannot serve.
        assert_eq!(
            AuthMoreData::decode(&p[4].payload),
            Ok(AuthMoreData::PerformFullAuthentication)
        );
        assert_eq!(p[5].payload, [0x02]);
        let err = ErrPacket::decode(&p[6].payload).expect("err");
        assert_eq!((err.code, &err.sql_state), (1045, b"28000"));
    }
}

#[test]
fn ssl_requests_decode_from_captured_clients() {
    for name in ["mysql84", "connector-j"] {
        let p = load(&format!("{name}-ssl"));
        match decode_client_hello(&p[1].payload, &Limits::default()) {
            Ok(ClientHello::Ssl(s)) => {
                assert!(s.capabilities.contains(C::SSL | C::PROTOCOL_41), "{name}");
                assert_eq!(s.encode(), p[1].payload, "{name}");
            }
            other => panic!("{name}: expected an SSLRequest, got {other:?}"),
        }
    }
    // mysql2 (no ssl option) and mariadb (--skip-ssl) answer the same
    // greeting in plaintext.
    for name in ["mysql2", "mariadb"] {
        let p = load(&format!("{name}-ssl"));
        assert!(matches!(
            decode_client_hello(&p[1].payload, &Limits::default()),
            Ok(ClientHello::Response(_))
        ));
    }
}
