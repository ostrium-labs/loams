//! Packet framing from untrusted bytes in any chunking: never panics or
//! over-consumes; framed payloads reassemble to themselves.
#![no_main]
libfuzzer_sys::fuzz_target!(|data: &[u8]| loams_sqlgate::fuzz::packet_framing(data));
