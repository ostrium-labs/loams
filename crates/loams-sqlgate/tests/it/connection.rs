//! The connection-phase state machine (R3.13).
use loams_sqlgate::codec::auth::{
    AuthError, AuthSwitchRequest, CACHING_SHA2, NATIVE, scramble_caching_sha2,
};
use loams_sqlgate::codec::connection::{ConnectionPhase, HANDSHAKE_MAX_MESSAGE, PhaseError, Step};
use loams_sqlgate::codec::handshake::{
    Capabilities as C, HandshakeResponse41, HandshakeV10, Limits, Nonce, SslRequest, TIDB_V8_5_8,
    advertise,
};
use loams_sqlgate::codec::packet::{Assembler, FrameError, encode};

fn greeting() -> HandshakeV10 {
    HandshakeV10 {
        server_version: "8.0.11-TiDB-v8.5.8-Loams".into(),
        connection_id: 7,
        nonce: Nonce::from_random(*b"abcdefghijklmnopqrst"),
        capabilities: advertise(TIDB_V8_5_8),
        charset: 0x2e,
        status: 2,
        auth_plugin: CACHING_SHA2.into(),
    }
}

fn caps(ssl: bool) -> C {
    let c = C::PROTOCOL_41
        | C::SECURE_CONNECTION
        | C::PLUGIN_AUTH
        | C::CONNECT_ATTRS
        | C::DEPRECATE_EOF;
    if ssl { c | C::SSL } else { c }
}

fn response(ssl: bool, plugin: &str, auth: Vec<u8>) -> HandshakeResponse41 {
    HandshakeResponse41 {
        capabilities: caps(ssl),
        max_packet: 1 << 24,
        charset: 0xff,
        username: "u_test".into(),
        auth_response: loams_sqlgate::codec::auth::Password::new(auth),
        database: None,
        auth_plugin: Some(plugin.into()),
        attributes: vec![],
        zstd_level: None,
    }
}

fn frame(payload: &[u8], seq: u8) -> Vec<u8> {
    let mut s = seq;
    let mut out = Vec::new();
    encode(payload, &mut s, &mut out);
    out
}

/// Server output → (seq, payload) per packet.
fn packets(bytes: &[u8]) -> Vec<(u8, Vec<u8>)> {
    let mut out = Vec::new();
    let mut rest = bytes;
    while !rest.is_empty() {
        let mut a = Assembler::new(1 << 20);
        a.expect_seq(rest[3]);
        let (used, m) = a.push(rest).expect("frame");
        let m = m.expect("whole");
        out.push((m.seq, m.payload));
        rest = &rest[used..];
    }
    out
}

fn feed(p: &mut ConnectionPhase, bytes: &[u8]) -> Result<Step, PhaseError> {
    let (used, step) = p.on_bytes(bytes)?;
    assert_eq!(used, bytes.len());
    Ok(step)
}

fn tls_phase() -> (ConnectionPhase, u8) {
    let (mut p, g) = ConnectionPhase::new(greeting(), false, Limits::default());
    assert_eq!(packets(&g)[0].0, 0);
    let ssl = SslRequest {
        capabilities: caps(true),
        max_packet: 1 << 24,
        charset: 0xff,
    }
    .encode();
    assert!(matches!(feed(&mut p, &frame(&ssl, 1)), Ok(Step::StartTls)));
    p.tls_established().expect("tls");
    (p, 2)
}

#[test]
fn tls_fast_auth_hit() {
    let (mut p, seq) = tls_phase();
    let n = greeting().nonce;
    let r = response(
        true,
        CACHING_SHA2,
        scramble_caching_sha2(b"pw", n.as_bytes()),
    );
    let Ok(Step::CheckFast { user, scramble }) =
        feed(&mut p, &frame(&r.encode().expect("enc"), seq))
    else {
        panic!("fast check")
    };
    assert_eq!((user.as_str(), scramble.len()), ("u_test", 32));
    let Ok(Step::Done(out)) = p.fast_result(true) else {
        panic!("done")
    };
    let pk = packets(&out);
    assert_eq!(pk[0], (3, vec![0x01, 0x03]), "fast_auth_success");
    assert_eq!((pk[1].0, pk[1].1[0]), (4, 0x00), "OK");
    assert!(p.is_done() && p.is_tls());
}

#[test]
fn tls_full_auth_after_a_miss() {
    let (mut p, seq) = tls_phase();
    let n = greeting().nonce;
    let r = response(
        true,
        CACHING_SHA2,
        scramble_caching_sha2(b"pw", n.as_bytes()),
    );
    assert!(matches!(
        feed(&mut p, &frame(&r.encode().expect("enc"), seq)),
        Ok(Step::CheckFast { .. })
    ));
    let Ok(Step::Write(out)) = p.fast_result(false) else {
        panic!("perform full")
    };
    assert_eq!(packets(&out), vec![(3, vec![0x01, 0x04])]);
    let Ok(Step::CheckFull { password, .. }) = feed(&mut p, &frame(b"pw\0", 4)) else {
        panic!("full")
    };
    assert_eq!(password.expose(), b"pw");
    let Ok(Step::Done(out)) = p.full_result(true) else {
        panic!("ok")
    };
    assert_eq!(packets(&out)[0].0, 5);
}

#[test]
fn wrong_password_is_1045() {
    let (mut p, seq) = tls_phase();
    let r = response(true, CACHING_SHA2, vec![9; 32]);
    feed(&mut p, &frame(&r.encode().expect("enc"), seq)).expect("fast");
    p.fast_result(false).expect("full");
    feed(&mut p, &frame(b"nope\0", 4)).expect("check");
    let e = p.full_result(false).expect_err("denied");
    assert_eq!(e, PhaseError::Auth(AuthError::AccessDenied));
    let err = packets(&p.error_packet(&e));
    assert_eq!(
        (err[0].1[0], u16::from_le_bytes([err[0].1[1], err[0].1[2]])),
        (0xff, 1045)
    );
}

#[test]
fn plaintext_is_refused_unless_allowed() {
    let r = response(false, CACHING_SHA2, vec![9; 32])
        .encode()
        .expect("enc");
    let (mut p, _) = ConnectionPhase::new(greeting(), false, Limits::default());
    let e = feed(&mut p, &frame(&r, 1)).expect_err("tls required");
    assert_eq!(e, PhaseError::TlsRequired);
    let err = packets(&p.error_packet(&e));
    assert_eq!(u16::from_le_bytes([err[0].1[1], err[0].1[2]]), 3159);

    // Loopback-plaintext-allowed: fast auth works, full auth needs TLS.
    let (mut p, _) = ConnectionPhase::new(greeting(), true, Limits::default());
    assert!(matches!(
        feed(&mut p, &frame(&r, 1)),
        Ok(Step::CheckFast { .. })
    ));
    assert_eq!(
        p.fast_result(false).expect_err("no tls"),
        PhaseError::Auth(AuthError::SecureTransportRequired)
    );
}

#[test]
fn ssl_bit_without_ssl_request_is_refused() {
    let r = response(true, CACHING_SHA2, vec![9; 32])
        .encode()
        .expect("enc");
    for allowed in [false, true] {
        let (mut p, _) = ConnectionPhase::new(greeting(), allowed, Limits::default());
        assert_eq!(
            feed(&mut p, &frame(&r, 1)).expect_err("refused"),
            PhaseError::SslWithoutRequest
        );
    }
}

#[test]
fn second_ssl_request_is_refused() {
    let (mut p, seq) = tls_phase();
    let ssl = SslRequest {
        capabilities: caps(true),
        max_packet: 1 << 24,
        charset: 0xff,
    }
    .encode();
    assert_eq!(
        feed(&mut p, &frame(&ssl, seq)).expect_err("dup"),
        PhaseError::DuplicateSslRequest
    );
}

#[test]
fn post_tls_capabilities_must_match_the_ssl_request() {
    // Without CLIENT_SSL after TLS.
    let (mut p, seq) = tls_phase();
    let r = response(false, CACHING_SHA2, vec![9; 32])
        .encode()
        .expect("enc");
    assert_eq!(
        feed(&mut p, &frame(&r, seq)).expect_err("no ssl"),
        PhaseError::SslMismatch
    );
    // With CLIENT_SSL but other capabilities.
    let (mut p, seq) = tls_phase();
    let mut r = response(true, CACHING_SHA2, vec![9; 32]);
    r.capabilities = r.capabilities | C::MULTI_STATEMENTS;
    let r = r.encode().expect("enc");
    assert_eq!(
        feed(&mut p, &frame(&r, seq)).expect_err("mismatch"),
        PhaseError::SslMismatch
    );
}

#[test]
fn handshake_messages_are_capped() {
    let (mut p, _) = ConnectionPhase::new(greeting(), false, Limits::default());
    let len = HANDSHAKE_MAX_MESSAGE + 1;
    let header = [len as u8, (len >> 8) as u8, (len >> 16) as u8, 1];
    let e = p.on_bytes(&header).expect_err("too large");
    assert_eq!(
        e,
        PhaseError::Frame(FrameError::TooLarge {
            limit: HANDSHAKE_MAX_MESSAGE
        })
    );
    // Nothing more is accepted.
    assert_eq!(p.on_bytes(&[]).expect_err("failed"), PhaseError::Unexpected);
}

#[test]
fn other_plugins_are_switched() {
    let (mut p, seq) = tls_phase();
    let r = response(true, NATIVE, vec![1; 20]).encode().expect("enc");
    let Ok(Step::Write(out)) = feed(&mut p, &frame(&r, seq)) else {
        panic!("switch")
    };
    let pk = packets(&out);
    let switch = AuthSwitchRequest::decode(&pk[0].1).expect("switch");
    assert_eq!((pk[0].0, switch.plugin.as_str()), (3, CACHING_SHA2));
    let n = greeting().nonce;
    let scramble = scramble_caching_sha2(b"pw", n.as_bytes());
    assert!(matches!(
        feed(&mut p, &frame(&scramble, 4)),
        Ok(Step::CheckFast { .. })
    ));
}

#[test]
fn partial_input_waits_and_steps_do_not_print_secrets() {
    let (mut p, seq) = tls_phase();
    let r = response(true, CACHING_SHA2, vec![7; 32])
        .encode()
        .expect("enc");
    let bytes = frame(&r, seq);
    assert!(matches!(p.on_bytes(&bytes[..10]), Ok((0, Step::NeedMore))));
    let (_, step) = p.on_bytes(&bytes).expect("whole");
    let shown = format!("{step:?}");
    assert!(
        shown.contains("[redacted]") && !shown.contains("7, 7"),
        "{shown}"
    );
}

/// Fix round 2, B: a first packet's auth response (here a cleartext
/// `mysql_clear_password` secret) is not kept: the stored response holds an
/// empty, zeroing `Password`, on success and on refusal.
#[test]
fn first_packet_secret_is_not_retained() {
    use loams_sqlgate::codec::auth::{CLEAR, Password};
    let (mut p, seq) = tls_phase();
    let mut r = response(true, CLEAR, Vec::new());
    r.auth_response = Password::new(b"s3cret-token\0".to_vec());
    let Ok(Step::Write(_)) = feed(&mut p, &frame(&r.encode().expect("enc"), seq)) else {
        panic!("switch")
    };
    let kept = p.response().expect("response");
    assert!(
        kept.auth_response.expose().is_empty(),
        "the first packet's secret is dropped"
    );
    assert!(!format!("{kept:?}").contains("s3cret"));

    // Refused (plaintext with TLS required): nothing is kept at all.
    let (mut p, _) = ConnectionPhase::new(greeting(), false, Limits::default());
    let mut r = response(false, CLEAR, Vec::new());
    r.auth_response = Password::new(b"s3cret-token\0".to_vec());
    assert!(feed(&mut p, &frame(&r.encode().expect("enc"), 1)).is_err());
    assert!(p.response().is_none());
}
