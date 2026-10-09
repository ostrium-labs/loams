//! The parts of an HTTP request the House still parses itself: the query string,
//! and the statement's shape (`INSERT … FORMAT` heads, trailing `FORMAT`, the
//! read-only keyword check), incremental scanning included.

#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    loams_house::request::fuzz_request(data);
});
