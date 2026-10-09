//! Invariants checked by the fuzz targets (`fuzz/fuzz_targets`) and by
//! `tests/it/fuzz.rs`. Each panics only when an invariant breaks.

use crate::codec::handshake::{ClientHello, Limits, decode_client_hello};
use crate::codec::packet::{Assembler, encode};

/// Decoding arbitrary bytes as the client's first packet never panics, and
/// whatever decodes re-encodes to a value that decodes the same.
pub fn handshake_response(data: &[u8]) {
    let limits = Limits::default();
    match decode_client_hello(data, &limits) {
        Ok(ClientHello::Response(r)) => {
            let bytes = r.encode().expect("a decoded response is encodable");
            let again = decode_client_hello(&bytes, &limits);
            assert_eq!(again, Ok(ClientHello::Response(r)), "response re-encodes");
        }
        Ok(ClientHello::Ssl(s)) => {
            assert_eq!(
                decode_client_hello(&s.encode(), &limits),
                Ok(ClientHello::Ssl(s)),
                "ssl re-encodes"
            );
        }
        Err(_) => {}
    }
}

/// Framing arbitrary bytes never panics or over-consumes, in any chunking;
/// and any payload framed by [`encode`] reassembles to itself.
pub fn packet_framing(data: &[u8]) {
    let max = 4096;
    let split = data.first().map_or(1, |&b| usize::from(b % 7) + 1);
    let mut a = Assembler::new(max);
    let mut buf: Vec<u8> = Vec::new();
    for chunk in data.chunks(split) {
        buf.extend_from_slice(chunk);
        loop {
            match a.push(&buf) {
                Ok((used, msg)) => {
                    assert!(used <= buf.len());
                    buf.drain(..used);
                    match msg {
                        Some(m) => assert!(m.payload.len() <= max),
                        None => break,
                    }
                }
                Err(_) => return,
            }
        }
    }

    let mut seq = data.first().copied().unwrap_or(0);
    let mut framed = Vec::new();
    encode(data, &mut seq, &mut framed);
    let mut a = Assembler::new(data.len());
    a.expect_seq(data.first().copied().unwrap_or(0));
    let (used, msg) = a.push(&framed).expect("framed by encode");
    assert_eq!(used, framed.len());
    assert_eq!(msg.map(|m| m.payload).as_deref(), Some(data));
    assert_eq!(a.next_seq(), seq);
}

use crate::codec::auth::{
    AuthMoreData, AuthSwitchRequest, CACHING_SHA2, CachingSha2Server, NATIVE,
};
use crate::codec::command::{ErrPacket, OkPacket};
use crate::codec::connection::{ConnectionPhase, Step};
use crate::codec::handshake::{Capabilities, HandshakeV10, Nonce, TIDB_V8_5_8, advertise};

fn fuzz_greeting() -> HandshakeV10 {
    HandshakeV10 {
        server_version: "8.0.11-TiDB-v8.5.8-Loams".into(),
        connection_id: 1,
        nonce: Nonce::from_random([42; 20]),
        capabilities: advertise(TIDB_V8_5_8),
        charset: 0x2e,
        status: 2,
        auth_plugin: CACHING_SHA2.into(),
    }
}

/// Drives the connection phase with arbitrary client bytes. The first byte
/// picks plaintext-allowed (bit 0), the verdicts (bits 1-3) and the chunk
/// size (bits 4-7). Never panics; reaching `Done` implies the rules held.
/// Returns whether the handshake completed.
pub fn connection_phase(data: &[u8]) -> bool {
    let Some((&ctl, rest)) = data.split_first() else {
        return false;
    };
    let plaintext_allowed = ctl & 1 == 1;
    let mut verdicts = (ctl >> 1) & 0x7;
    let mut next_verdict = move || {
        let v = verdicts & 1 == 1;
        verdicts = verdicts.rotate_right(1);
        v
    };
    let greeting = fuzz_greeting();
    let offered = greeting.capabilities;
    let (mut p, _) = ConnectionPhase::new(
        greeting,
        plaintext_allowed,
        crate::codec::handshake::Limits::default(),
    );
    let chunk = usize::from(ctl >> 4) + 1;
    let mut buf: Vec<u8> = Vec::new();
    let check_done = |p: &ConnectionPhase| {
        assert!(p.is_done());
        let agreed = p.agreed().expect("agreed");
        assert!(agreed.is_subset_of(offered));
        assert!(
            plaintext_allowed || p.is_tls(),
            "plaintext without permission"
        );
        let r = p.response().expect("response");
        assert_eq!(
            r.capabilities.contains(Capabilities::SSL),
            p.is_tls(),
            "SSL flag matches transport"
        );
    };
    for c in rest.chunks(chunk) {
        buf.extend_from_slice(c);
        loop {
            let (used, step) = match p.on_bytes(&buf) {
                Ok(r) => r,
                Err(e) => {
                    let _ = p.error_packet(&e);
                    return false;
                }
            };
            assert!(used <= buf.len());
            buf.drain(..used);
            let after = match step {
                Step::NeedMore => break,
                Step::StartTls => p.tls_established().map(|()| Step::NeedMore),
                Step::Write(_) => Ok(Step::NeedMore),
                Step::CheckFast { .. } => p.fast_result(next_verdict()),
                Step::CheckFull { .. } => p.full_result(next_verdict()),
                Step::Done(_) => unreachable!("on_bytes never finishes the handshake"),
            };
            match after {
                Ok(Step::Done(_)) => {
                    check_done(&p);
                    return true;
                }
                Ok(_) => {}
                Err(_) => return false,
            }
        }
    }
    false
}

/// Drives `CachingSha2Server` with arbitrary packets: the first byte picks
/// TLS, plugin and verdicts; the rest is length-prefixed packets. Never
/// panics; once done, nothing more is accepted.
pub fn auth_server(data: &[u8]) {
    let Some((&ctl, mut rest)) = data.split_first() else {
        return;
    };
    let mut s = CachingSha2Server::new(Nonce::from_random([9; 20]), ctl & 1 == 1);
    let next = |r: &mut &[u8]| -> Option<Vec<u8>> {
        let (&n, tail) = r.split_first()?;
        let n = usize::from(n).min(tail.len());
        let (p, t) = tail.split_at(n);
        *r = t;
        Some(p.to_vec())
    };
    let first = next(&mut rest).unwrap_or_default();
    let plugin = [Some(CACHING_SHA2), Some(NATIVE), None][usize::from(ctl >> 1) % 3];
    let mut action = s.start(plugin, &first);
    let mut verdict = ctl >> 3;
    loop {
        use crate::codec::auth::Action;
        match action {
            Action::CheckFast { .. } => {
                action = s.fast_result(verdict & 1 == 1);
                verdict >>= 1;
                continue;
            }
            Action::Fail(_) | Action::CheckFull { .. } => {
                assert!(s.is_done());
                assert!(s.on_packet(b"x").is_err());
                return;
            }
            Action::Send(_) => {}
        }
        let Some(p) = next(&mut rest) else { return };
        match s.on_packet(&p) {
            Ok(a) => action = a,
            Err(_) => return,
        }
    }
}

/// The remaining decoders on arbitrary bytes: never panic, and what decodes
/// re-encodes to the same value.
pub fn decoders(data: &[u8]) {
    let Some((&sel, rest)) = data.split_first() else {
        return;
    };
    match sel % 5 {
        0 => {
            if let Ok(g) = HandshakeV10::decode(rest) {
                assert_eq!(HandshakeV10::decode(&g.encode()), Ok(g));
            }
        }
        1 => {
            if let Ok(s) = AuthSwitchRequest::decode(rest) {
                assert_eq!(AuthSwitchRequest::decode(&s.encode()), Ok(s));
            }
        }
        2 => {
            if let Ok(m) = AuthMoreData::decode(rest) {
                assert_eq!(AuthMoreData::decode(&m.encode()), Ok(m));
            }
        }
        3 => {
            let caps = if sel & 0x80 != 0 {
                Capabilities::PROTOCOL_41 | Capabilities::SESSION_TRACK
            } else {
                Capabilities::PROTOCOL_41
            };
            let _ = OkPacket::decode(rest, caps);
        }
        _ => {
            if let Ok(e) = ErrPacket::decode(rest) {
                assert_eq!(ErrPacket::decode(&e.encode()).map(|x| x.code), Ok(e.code));
            }
        }
    }
}
