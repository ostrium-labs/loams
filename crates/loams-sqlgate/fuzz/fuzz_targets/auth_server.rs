//! See loams_sqlgate::fuzz::auth_server.
#![no_main]
libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let _ = loams_sqlgate::fuzz::auth_server(data);
});
