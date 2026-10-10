use loams_sqlgate::codec::auth::Password;
use loams_sqlgate::codec::handshake::{
    Capabilities as C, ClientHello, GATE_SUPPORTED, HandshakeResponse41, HandshakeV10, Limits,
    Nonce, RELAY_SENSITIVE, SslRequest, TIDB_V8_5_8, advertise, decode_client_hello, negotiate,
    upstream_capabilities,
};
use proptest::prelude::*;

fn nonce() -> Nonce {
    Nonce::from_random([7u8; 20])
}

fn greeting() -> HandshakeV10 {
    HandshakeV10 {
        server_version: "8.0.11-TiDB-v8.5.8-Loams".into(),
        connection_id: 42,
        nonce: nonce(),
        capabilities: advertise(
            C::PROTOCOL_41 | C::SSL | C::PLUGIN_AUTH | C::SECURE_CONNECTION | C::DEPRECATE_EOF,
        ),
        charset: 0x2e,
        status: 0x0002,
        auth_plugin: "caching_sha2_password".into(),
    }
}

#[test]
fn codec_roundtrips_handshake_v10() {
    let g = greeting();
    let bytes = g.encode();
    assert_eq!(bytes[0], 10, "protocol version");
    assert_eq!(HandshakeV10::decode(&bytes).expect("decodes"), g);
    // The nonce is 20 bytes, split 8 + 12, and never contains NUL.
    assert!(g.nonce.as_bytes().iter().all(|&b| b != 0));
    // Every prefix is refused, never panics.
    for cut in 0..bytes.len() {
        assert!(HandshakeV10::decode(&bytes[..cut]).is_err(), "cut {cut}");
    }
    // Trailing bytes are refused.
    let mut long = bytes.clone();
    long.push(0);
    assert!(HandshakeV10::decode(&long).is_err());
}

#[test]
fn nonces_never_contain_nul_or_high_bytes() {
    for b in 0..=255u8 {
        let n = Nonce::from_random([b; 20]);
        assert!(n.as_bytes().iter().all(|&x| (1..=127).contains(&x)), "{b}");
    }
}

fn response() -> HandshakeResponse41 {
    HandshakeResponse41 {
        capabilities: C::PROTOCOL_41
            | C::SECURE_CONNECTION
            | C::PLUGIN_AUTH
            | C::PLUGIN_AUTH_LENENC_CLIENT_DATA
            | C::CONNECT_WITH_DB
            | C::CONNECT_ATTRS,
        max_packet: 16 << 20,
        charset: 0xff,
        username: "u_abc".into(),
        auth_response: Password::new(vec![9; 32]),
        database: Some("app".into()),
        auth_plugin: Some("caching_sha2_password".into()),
        attributes: vec![(b"_client_name".to_vec(), b"test".to_vec())],
        zstd_level: None,
    }
}

#[test]
fn handshake_response_roundtrips_and_is_bounded() {
    let r = response();
    let bytes = r.encode().expect("encodable");
    assert_eq!(
        decode_client_hello(&bytes, &Limits::default()),
        Ok(ClientHello::Response(r.clone()))
    );
    for cut in 0..bytes.len() {
        assert!(
            decode_client_hello(&bytes[..cut], &Limits::default()).is_err(),
            "cut {cut}"
        );
    }
    let mut long = bytes.clone();
    long.push(1);
    assert!(
        decode_client_hello(&long, &Limits::default()).is_err(),
        "trailing bytes"
    );

    let tight = Limits {
        max_username: 4,
        ..Limits::default()
    };
    assert!(
        decode_client_hello(&bytes, &tight).is_err(),
        "username over the limit"
    );
    let tight = Limits {
        max_auth_response: 31,
        ..Limits::default()
    };
    assert!(
        decode_client_hello(&bytes, &tight).is_err(),
        "auth response over the limit"
    );
    let tight = Limits {
        max_attributes_bytes: 4,
        ..Limits::default()
    };
    assert!(
        decode_client_hello(&bytes, &tight).is_err(),
        "attributes over the limit"
    );

    // A pre-4.1 client is refused.
    let mut old = bytes.clone();
    old[1] &= !0x02; // PROTOCOL_41 is bit 9: byte 1, bit 1
    assert!(decode_client_hello(&old, &Limits::default()).is_err());

    // Invalid UTF-8 in the user name is refused.
    let mut bad = response();
    bad.username = "ab".into();
    let mut b = bad.encode().expect("encodable");
    let at = 32;
    b[at] = 0xff;
    assert!(decode_client_hello(&b, &Limits::default()).is_err());
}

#[test]
fn ssl_request_is_recognised() {
    let s = SslRequest {
        capabilities: C::PROTOCOL_41 | C::SSL | C::SECURE_CONNECTION,
        max_packet: 1 << 24,
        charset: 33,
    };
    let bytes = s.encode();
    assert_eq!(bytes.len(), 32);
    assert_eq!(
        decode_client_hello(&bytes, &Limits::default()),
        Ok(ClientHello::Ssl(s))
    );
    // 32 bytes without CLIENT_SSL is not an SSLRequest (and too short for a response).
    let mut no_ssl = bytes.clone();
    no_ssl[1] &= !0x08;
    assert!(decode_client_hello(&no_ssl, &Limits::default()).is_err());
}

#[test]
fn negotiation_requires_41_secure_and_plugin_auth() {
    let offered = advertise(C(u32::MAX));
    let ok = negotiate(
        C::PROTOCOL_41 | C::SECURE_CONNECTION | C::PLUGIN_AUTH | C::COMPRESS,
        offered,
    )
    .expect("ok");
    assert!(!ok.contains(C::COMPRESS), "compression is never negotiated");
    for missing in [C::PROTOCOL_41, C::SECURE_CONNECTION, C::PLUGIN_AUTH] {
        let client =
            C(C::PROTOCOL_41.0 | C::SECURE_CONNECTION.0 | C::PLUGIN_AUTH.0).without(missing);
        assert!(negotiate(client, offered).is_err(), "{missing:?}");
    }
}

proptest! {
    /// The gate never offers or agrees to a capability the upstream TiDB
    /// lacks, nor one the gate does not implement; and the flags that shape
    /// relayed results are the same on both legs.
    #[test]
    fn capabilities_never_exceed_upstream(upstream in any::<u32>(), client in any::<u32>()) {
        let upstream = C(upstream);
        let offered = advertise(upstream);
        // SSL is the gate's own (it terminates TLS); everything else is
        // what TiDB's profile has.
        prop_assert!(offered.contains(C::SSL));
        prop_assert!(offered.without(C::SSL).is_subset_of(upstream));
        prop_assert!(offered.is_subset_of(GATE_SUPPORTED));
        if let Ok(agreed) = negotiate(C(client), offered) {
            prop_assert!(agreed.without(C::SSL).is_subset_of(upstream));
            prop_assert!(agreed.is_subset_of(GATE_SUPPORTED));
            prop_assert!(agreed.is_subset_of(C(client)));
            let up = upstream_capabilities(agreed, upstream);
            prop_assert!(up.contains(C::SSL));
            prop_assert!(up.without(C::SSL).is_subset_of(upstream));
            prop_assert_eq!(up.intersect(RELAY_SENSITIVE), agreed.intersect(RELAY_SENSITIVE));
            // LOAD DATA LOCAL is never offered and never asked of TiDB.
            prop_assert!(!agreed.contains(C::LOCAL_FILES));
            prop_assert!(!up.contains(C::LOCAL_FILES));
        }
        prop_assert!(!offered.contains(C::LOCAL_FILES));
    }
}

#[test]
fn auth_response_never_prints() {
    let r = response();
    let shown = format!("{r:?}");
    assert!(shown.contains("[redacted]"), "{shown}");
    assert!(!shown.contains("9, 9, 9"), "{shown}");
}

/// R3.6: the offer never depends on TiDB's greeting: the profile is static
/// for the pinned v8.5.8, and SSL is the gate's own.
#[test]
fn ssl_is_offered_on_the_gates_terms() {
    assert!(advertise(TIDB_V8_5_8).contains(C::SSL));
    assert!(advertise(TIDB_V8_5_8.without(C::SSL)).contains(C::SSL));
    // The profile is the captured v8.5.8 greeting's capabilities plus SSL.
    let captured = C(0x051b_a6af);
    assert_eq!(TIDB_V8_5_8, captured | C::SSL);
}

/// R3.7: the upstream leg is the client's relay-sensitive flags plus the
/// gate's own connection flags, limited to TiDB's profile. A plaintext
/// loopback client still gets TLS (and plugin auth, attributes) upstream.
#[test]
fn upstream_leg_is_the_gates_own_connection() {
    let client = C::PROTOCOL_41
        | C::SECURE_CONNECTION
        | C::PLUGIN_AUTH
        | C::DEPRECATE_EOF
        | C::MULTI_RESULTS;
    let agreed = negotiate(client, advertise(TIDB_V8_5_8)).expect("agreed");
    assert!(!agreed.contains(C::SSL), "a plaintext loopback client");
    let up = upstream_capabilities(agreed, TIDB_V8_5_8);
    for own in [
        C::SSL,
        C::PLUGIN_AUTH,
        C::CONNECT_ATTRS,
        C::PROTOCOL_41,
        C::SECURE_CONNECTION,
    ] {
        assert!(up.contains(own), "{own:?} missing upstream: {up:?}");
    }
    assert!(up.contains(C::DEPRECATE_EOF | C::MULTI_RESULTS));
    assert!(
        !up.contains(C::MULTI_STATEMENTS),
        "not the client's: not upstream"
    );
    assert!(up.is_subset_of(TIDB_V8_5_8));
}

/// R3.8: without CLIENT_PLUGIN_AUTH_LENENC_CLIENT_DATA (TiDB v8.5.8 does not
/// offer it) the auth response has a one-byte length: longer ones are an
/// error, never silently truncated.
#[test]
fn long_auth_response_needs_lenenc() {
    let mut r = response();
    r.auth_response = Password::new(vec![b'j'; 300]);
    assert!(r.encode().is_ok(), "length-encoded: any length");
    r.capabilities = r.capabilities.without(C::PLUGIN_AUTH_LENENC_CLIENT_DATA);
    assert!(r.encode().is_err(), "one-byte length: at most 255");
    r.auth_response = Password::new(vec![b'j'; 255]);
    let bytes = r.encode().expect("255 fits");
    assert!(
        matches!(decode_client_hello(&bytes, &Limits::default()), Ok(ClientHello::Response(d)) if d.auth_response.expose().len() == 255)
    );
    assert!(!TIDB_V8_5_8.contains(C::PLUGIN_AUTH_LENENC_CLIENT_DATA));
}

/// R3.9: connection attributes are kept as bytes (any encoding a client
/// uses); user and database names stay strict UTF-8.
#[test]
fn attributes_are_bytes_names_are_utf8() {
    let mut r = response();
    r.attributes = vec![(b"os_user".to_vec(), vec![0xff, 0xfe, b'x'])];
    let bytes = r.encode().expect("encodable");
    match decode_client_hello(&bytes, &Limits::default()) {
        Ok(ClientHello::Response(d)) => assert_eq!(d.attributes, r.attributes),
        other => panic!("{other:?}"),
    }
    let mut r = response();
    r.database = Some("db".into());
    let mut bytes = r.encode().expect("encodable");
    let at = bytes.windows(3).position(|w| w == b"db\0").expect("db");
    bytes[at] = 0xc3; // a lone UTF-8 lead byte
    assert!(decode_client_hello(&bytes, &Limits::default()).is_err());
}

/// R3.16: SSL is always on the upstream leg, even against a profile that
/// lacks it (the gate then fails at TLS, never logs in in clear).
#[test]
fn upstream_always_asks_for_ssl() {
    let profile = TIDB_V8_5_8.without(C::SSL);
    let client = C::PROTOCOL_41 | C::SECURE_CONNECTION | C::PLUGIN_AUTH;
    let agreed = negotiate(client, advertise(profile)).expect("agreed");
    assert!(upstream_capabilities(agreed, profile).contains(C::SSL));
}
