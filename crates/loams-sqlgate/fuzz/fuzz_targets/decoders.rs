//! See loams_sqlgate::fuzz::decoders.
#![no_main]
libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let _ = loams_sqlgate::fuzz::decoders(data);
});
