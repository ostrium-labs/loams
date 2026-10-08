//! The client's first packet (SSLRequest or HandshakeResponse41) from
//! untrusted bytes: never panics; what decodes re-encodes the same.
#![no_main]
libfuzzer_sys::fuzz_target!(|data: &[u8]| loams_sqlgate::fuzz::handshake_response(data));
