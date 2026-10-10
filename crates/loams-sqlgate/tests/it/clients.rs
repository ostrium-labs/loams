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

/// (fixture, `_client_name`, `_client_version`; go-sql-driver sends none).
const CLIENTS: [(&str, &str, Option<&str>); 5] = [
    ("mysql84", "libmysql", Some("8.4.10")),
    ("connector-j", "MySQL Connector/J", Some("9.7.0")),
    ("mysql2", "Node-MySQL-2", Some("3.15.3")),
    ("go-sql-driver", "Go-MySQL-Driver", None),
    ("mariadb", "libmariadb", Some("3.4.10")),
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
        assert_eq!(
            r.auth_response.expose().len(),
            20,
            "{name}: a native scramble"
        );
        assert!(
            r.capabilities
                .contains(C::PROTOCOL_41 | C::SECURE_CONNECTION | C::PLUGIN_AUTH),
            "{name}"
        );
        let attr = |k: &str| {
            r.attributes
                .iter()
                .find(|(n, _)| n.as_slice() == k.as_bytes())
                .map(|(_, v)| std::str::from_utf8(v).expect("ascii"))
        };
        assert_eq!(attr("_client_name"), Some(client_name), "{name}");
        assert_eq!(attr("_client_version"), version, "{name}");
        // Re-encoding gives the client's bytes back.
        assert_eq!(
            r.encode().expect("encodable"),
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
    // mysql2 and go-sql-driver (no TLS option) and mariadb (--skip-ssl)
    // answer the same
    // greeting in plaintext.
    for name in ["mysql2", "go-sql-driver", "mariadb"] {
        let p = load(&format!("{name}-ssl"));
        assert!(matches!(
            decode_client_hello(&p[1].payload, &Limits::default()),
            Ok(ClientHello::Response(_))
        ));
    }
}

/// R3.14: mysql 8.4 and Connector/J against a greeting that offers TLS and
/// `caching_sha2_password` (the proxy terminates TLS, see the README): their
/// first response is a SHA-2 scramble of the greeting nonce, and on full
/// authentication they send the password in clear over TLS. The captured
/// client bytes drive `ConnectionPhase` to `Done`.
#[test]
fn tls_sha2_captures_drive_the_connection_phase() {
    use loams_sqlgate::codec::auth::CachingSha2Server;
    use loams_sqlgate::codec::auth::{Action, double_sha256};
    use loams_sqlgate::codec::connection::{ConnectionPhase, Step};
    use loams_sqlgate::codec::packet::encode;

    for name in ["mysql84", "connector-j"] {
        let p = load(&format!("{name}-tls-sha2"));
        let greeting = HandshakeV10::decode(&p[0].payload).expect("greeting");
        assert_eq!(greeting.auth_plugin, CACHING_SHA2, "{name}");
        assert!(greeting.capabilities.contains(C::SSL), "{name}");
        let nonce = *greeting.nonce.as_bytes();

        let ClientHello::Ssl(ssl) =
            decode_client_hello(&p[1].payload, &Limits::default()).expect("ssl")
        else {
            panic!("{name}: SSLRequest first")
        };
        let ClientHello::Response(r) =
            decode_client_hello(&p[2].payload, &Limits::default()).expect("response")
        else {
            panic!("{name}: response after TLS")
        };
        assert_eq!(
            r.capabilities, ssl.capabilities,
            "{name}: same capabilities before and after TLS"
        );
        assert_eq!(r.auth_plugin.as_deref(), Some(CACHING_SHA2), "{name}");
        assert_eq!(
            r.auth_response.expose(),
            scramble_caching_sha2(b"capture", &nonce),
            "{name}: SHA-2 first response"
        );
        assert!(verify_caching_sha2(
            &double_sha256(b"capture"),
            &nonce,
            r.auth_response.expose()
        ));

        // TiDB asked for full authentication; the client sent its password
        // in clear (over TLS to the proxy).
        let clear = p
            .iter()
            .find(|x| x.dir == "c2s" && x.payload == b"capture\0")
            .expect("cleartext password");
        let mut s = CachingSha2Server::new(greeting.nonce, true);
        assert!(matches!(
            s.start(r.auth_plugin.as_deref(), r.auth_response.expose()),
            Action::CheckFast { .. }
        ));
        s.fast_result(false);
        let Ok(Action::CheckFull { password }) = s.on_packet(&clear.payload) else {
            panic!("{name}: full")
        };
        assert_eq!(password.expose(), b"capture");

        // The same client bytes through the connection phase (TLS required).
        let (mut phase, _) = ConnectionPhase::new(greeting.clone(), false, Limits::default());
        let framed = |payload: &[u8], seq: u8| {
            let mut s = seq;
            let mut out = Vec::new();
            encode(payload, &mut s, &mut out);
            out
        };
        assert!(matches!(
            phase.on_bytes(&framed(&p[1].payload, 1)),
            Ok((_, Step::StartTls))
        ));
        phase.tls_established().expect("tls");
        let Ok((_, Step::CheckFast { scramble, .. })) = phase.on_bytes(&framed(&p[2].payload, 2))
        else {
            panic!("{name}: fast check")
        };
        assert!(verify_caching_sha2(
            &double_sha256(b"capture"),
            &nonce,
            &scramble
        ));
        assert!(matches!(phase.fast_result(false), Ok(Step::Write(_))));
        let Ok((_, Step::CheckFull { password, .. })) = phase.on_bytes(&framed(&clear.payload, 4))
        else {
            panic!("{name}: full check")
        };
        assert_eq!(password.expose(), b"capture");
        assert!(matches!(phase.full_result(true), Ok(Step::Done(_))));
        assert!(phase.is_tls() && phase.is_done());
    }
}
