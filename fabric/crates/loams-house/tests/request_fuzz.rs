//! The `request_parse` fuzz target (`fuzz/`) on stable: the same body over random
//! bytes and over statement-shaped text, so CI exercises what libFuzzer does
//! (Task 3 review I8).

use loams_house::request::fuzz_request;
use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(3_000))]

    #[test]
    fn arbitrary_bytes(data in proptest::collection::vec(any::<u8>(), 0..512)) {
        fuzz_request(&data);
    }

    #[test]
    fn statement_shaped(
        parts in proptest::collection::vec(
            prop_oneof![
                Just("INSERT"), Just(" INTO t "), Just("FORMAT"), Just(" TSV"), Just("\n"),
                Just("'"), Just("''"), Just("\\\\"), Just("`x`"), Just("--"), Just("/*"), Just("*/"),
                Just("("), Just(")"), Just("SELECT"), Just(";"), Just("%"), Just("&q="), Just("+"),
                Just("VALUES"), Just("é"), Just("#"),
            ],
            0..40,
        )
    ) {
        fuzz_request(parts.concat().as_bytes());
    }
}
