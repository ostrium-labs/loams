//! Property tests of the routed-name grammar (PG2 Task 4).

use loams_pg_control::names::{Routed, format_routed, parse_routed, validate_name};
use proptest::prelude::*;

fn name() -> impl Strategy<Value = String> {
    "[a-z0-9][a-z0-9-]{0,62}"
}

/// Postgres identifiers of 1..=63 bytes with no NUL, biased toward the
/// characters the grammar treats specially.
fn database() -> impl Strategy<Value = String> {
    prop_oneof![
        "[a-z0-9._-]{1,63}",
        "(_|\\.|__ro|[a-z]){1,20}",
        "[^\\x00]{1,30}",
    ]
    .prop_filter("1..=63 bytes", |d| !d.is_empty() && d.len() <= 63)
}

fn routed() -> impl Strategy<Value = Routed> {
    prop_oneof![
        name().prop_map(|project| Routed::Project { project }),
        (name(), name()).prop_map(|(project, branch)| Routed::Branch { project, branch }),
        (name(), name(), database()).prop_map(|(project, branch, database)| Routed::Database {
            project,
            branch,
            database
        }),
        (name(), name()).prop_map(|(project, branch)| Routed::ReadOnly { project, branch }),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(10_000))]

    /// format then parse is the identity on every valid routed name.
    #[test]
    fn routed_roundtrip_prop(r in routed()) {
        let text = format_routed(&r).unwrap();
        prop_assert_eq!(parse_routed(&text).unwrap(), r);
    }

    /// parse then format is the identity on whatever parses, so each
    /// routed name has exactly one meaning and one spelling.
    #[test]
    fn parsed_text_formats_back(s in "(([a-z0-9-]|_|\\.|__ro){0,12}|[^\\x00]{0,20})") {
        if let Ok(r) = parse_routed(&s) {
            prop_assert_eq!(format_routed(&r).unwrap(), s);
        }
    }

    #[test]
    fn generated_names_are_valid_and_have_no_double_underscore(n in name()) {
        prop_assert!(validate_name(&n).is_ok());
        prop_assert!(!n.contains("__"));
    }
}
