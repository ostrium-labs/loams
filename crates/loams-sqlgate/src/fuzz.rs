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
            let again = decode_client_hello(&r.encode(), &limits);
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
