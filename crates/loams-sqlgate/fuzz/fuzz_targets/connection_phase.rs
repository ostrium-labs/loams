//! See loams_sqlgate::fuzz::connection_phase.
#![no_main]
libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let _ = loams_sqlgate::fuzz::connection_phase(data);
});
