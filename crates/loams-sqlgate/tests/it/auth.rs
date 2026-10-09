use loams_sqlgate::codec::auth::{
    Action, AuthError, AuthMoreData, AuthSwitchRequest, CACHING_SHA2, CLEAR, CachingSha2Server,
    NATIVE, Password, client_auth_response, double_sha256, scramble_caching_sha2,
    verify_caching_sha2,
};
use loams_sqlgate::codec::handshake::Nonce;

fn nonce() -> Nonce {
    Nonce::from_random(*b"0123456789abcdefghij")
}

#[test]
fn caching_sha2_fast_auth_with_cache_hit() {
    let n = nonce();
    let mut s = CachingSha2Server::new(n, true);
    let scramble = scramble_caching_sha2(b"pw", n.as_bytes());
    let Action::CheckFast { scramble: got } = s.start(Some(CACHING_SHA2), &scramble) else {
        panic!("fast check")
    };
    assert!(verify_caching_sha2(
        &double_sha256(b"pw"),
        n.as_bytes(),
        &got
    ));
    // Cache hit: fast_auth_success, then the caller sends OK.
    assert_eq!(
        s.fast_result(true),
        Action::Send(AuthMoreData::FastAuthSuccess.encode())
    );
    assert!(s.is_done());
}

#[test]
fn caching_sha2_full_auth_over_tls() {
    let n = nonce();
    let mut s = CachingSha2Server::new(n, true);
    let scramble = scramble_caching_sha2(b"pw", n.as_bytes());
    assert!(matches!(
        s.start(Some(CACHING_SHA2), &scramble),
        Action::CheckFast { .. }
    ));
    // Cache miss: ask for the cleartext password (TLS is on).
    assert_eq!(
        s.fast_result(false),
        Action::Send(AuthMoreData::PerformFullAuthentication.encode())
    );
    let Ok(Action::CheckFull { password }) = s.on_packet(b"pw\0") else {
        panic!("full check")
    };
    assert_eq!(password.expose(), b"pw");
    assert_eq!(format!("{password:?}"), "[redacted]");
    assert!(s.is_done());
    // Nothing more is accepted.
    assert_eq!(s.on_packet(b"x"), Err(AuthError::Unexpected));
}

#[test]
fn full_auth_is_refused_without_tls() {
    let n = nonce();
    let mut s = CachingSha2Server::new(n, false);
    let scramble = scramble_caching_sha2(b"pw", n.as_bytes());
    s.start(Some(CACHING_SHA2), &scramble);
    assert_eq!(
        s.fast_result(false),
        Action::Fail(AuthError::SecureTransportRequired)
    );
    // And a public-key request is never served.
    let mut s = CachingSha2Server::new(n, true);
    s.start(Some(CACHING_SHA2), &scramble);
    s.fast_result(false);
    assert_eq!(s.on_packet(&[0x02]), Err(AuthError::PublicKeyRefused));
}

#[test]
fn other_plugins_are_switched_to_caching_sha2() {
    let n = nonce();
    let mut s = CachingSha2Server::new(n, true);
    let Action::Send(pkt) = s.start(Some(NATIVE), &[1; 20]) else {
        panic!("switch")
    };
    let switch = AuthSwitchRequest::decode(&pkt).expect("switch request");
    assert_eq!(
        (switch.plugin.as_str(), switch.nonce()),
        (CACHING_SHA2, Some(n.as_bytes()))
    );
    // The client answers the switch with a caching_sha2 scramble.
    let scramble = scramble_caching_sha2(b"pw", n.as_bytes());
    assert!(matches!(
        s.on_packet(&scramble),
        Ok(Action::CheckFast { .. })
    ));
    // No plugin at all also switches.
    let mut s = CachingSha2Server::new(n, true);
    assert!(matches!(s.start(None, &[]), Action::Send(_)));
}

#[test]
fn malformed_auth_data_is_refused() {
    let n = nonce();
    // A caching_sha2 response that is not 32 bytes.
    let mut s = CachingSha2Server::new(n, true);
    assert_eq!(
        s.start(Some(CACHING_SHA2), &[1; 31]),
        Action::Fail(AuthError::Malformed)
    );
    // A cleartext password without its NUL, with an inner NUL, or too long.
    for bad in [&b"pw"[..], b"p\0w\0", &[b'a'; 1025]] {
        let mut s = CachingSha2Server::new(n, true);
        s.start(
            Some(CACHING_SHA2),
            &scramble_caching_sha2(b"pw", n.as_bytes()),
        );
        s.fast_result(false);
        assert_eq!(
            s.on_packet(bad).err(),
            Some(AuthError::Malformed),
            "{bad:?}"
        );
    }
}

#[test]
fn empty_password_is_checked_in_full() {
    let n = nonce();
    let mut s = CachingSha2Server::new(n, true);
    let Action::CheckFull { password } = s.start(Some(CACHING_SHA2), &[]) else {
        panic!("full")
    };
    assert!(password.expose().is_empty());
}

#[test]
fn client_side_responses() {
    let n = nonce();
    assert_eq!(
        client_auth_response(CACHING_SHA2, &Password::new(b"pw".to_vec()), n.as_bytes())
            .expect("sha2")
            .expose(),
        scramble_caching_sha2(b"pw", n.as_bytes())
    );
    // mysql_clear_password (tidb_auth_token, R2.12) sends the secret with a
    // NUL. Responses are Passwords: redacted and zeroed on drop.
    let r =
        client_auth_response(CLEAR, &Password::new(b"jwt".to_vec()), n.as_bytes()).expect("clear");
    assert_eq!(format!("{r:?}"), "[redacted]");
    assert_eq!(
        client_auth_response(CLEAR, &Password::new(b"jwt".to_vec()), n.as_bytes())
            .expect("clear")
            .expose(),
        b"jwt\0"
    );
    assert!(
        client_auth_response(CACHING_SHA2, &Password::new(Vec::new()), n.as_bytes())
            .expect("empty")
            .expose()
            .is_empty()
    );
    assert!(
        client_auth_response(
            "mysql_old_password",
            &Password::new(b"x".to_vec()),
            n.as_bytes()
        )
        .is_err()
    );
}

#[test]
fn auth_more_data_and_switch_codecs() {
    for m in [
        AuthMoreData::FastAuthSuccess,
        AuthMoreData::PerformFullAuthentication,
    ] {
        assert_eq!(AuthMoreData::decode(&m.encode()), Ok(m));
    }
    assert!(AuthMoreData::decode(&[0x01]).is_err());
    assert!(AuthMoreData::decode(&[0x00, 0x03]).is_err());
    assert!(
        AuthSwitchRequest::decode(&[0xfe]).is_err(),
        "plugin name without NUL"
    );
    let r = AuthSwitchRequest {
        plugin: CACHING_SHA2.into(),
        data: [nonce().as_bytes().as_slice(), &[0]].concat(),
    };
    assert_eq!(AuthSwitchRequest::decode(&r.encode()), Ok(r));
}

/// R3.8: `tidb_auth_token` (R2.12) travels in the auth-switch response, a
/// raw packet: a JWT longer than 255 bytes goes through whole.
#[test]
fn tidb_auth_token_jwt_travels_in_the_switch_response() {
    use loams_sqlgate::codec::packet::{Assembler, encode};
    let switch = AuthSwitchRequest {
        plugin: CLEAR.into(),
        data: Vec::new(),
    };
    let decoded = AuthSwitchRequest::decode(&switch.encode()).expect("switch");
    assert_eq!(decoded.plugin, CLEAR);
    let jwt = vec![b'e'; 2048];
    let response = client_auth_response(CLEAR, &Password::new(jwt.clone()), &[]).expect("clear");
    assert_eq!(response.expose().len(), 2049);
    let mut seq = 3;
    let mut framed = Vec::new();
    encode(response.expose(), &mut seq, &mut framed);
    let mut a = Assembler::new(64 * 1024);
    a.expect_seq(3);
    let (_, m) = a.push(&framed).expect("frames");
    assert_eq!(m.expect("message").payload, [jwt.as_slice(), &[0]].concat());
}
