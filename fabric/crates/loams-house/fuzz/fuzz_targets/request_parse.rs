//! Everything the House parses out of a request itself: the query string, the
//! credentials, the session parameters, the content codings, the body decoder and
//! the statement scanner in arbitrary pieces. See `loams_house::fuzz`.

#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    loams_house::fuzz::fuzz_request(data);
});
