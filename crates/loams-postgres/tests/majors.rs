//! `majors.rs` is the pin file's `[majors]` (design §62 §9.1, NF2 Task 10).
//! `scripts/nf1/pins.py check` compares the whole file; this test keeps
//! the same promise for a Rust-only run.

use loams_postgres::majors;

const PINS: &str = include_str!("../../../deploy/pins/loams-postgres-images.toml");

fn majors_list(pins: &toml::Value, name: &str) -> Vec<u32> {
    pins["majors"][name]
        .as_array()
        .unwrap_or_else(|| panic!("[majors] {name}"))
        .iter()
        .map(|m| u32::try_from(m.as_integer().expect("an integer")).expect("a u32"))
        .collect()
}

#[test]
fn majors_rs_matches_pins() {
    let pins: toml::Value = toml::from_str(PINS).expect("the pin file parses");
    assert_eq!(majors::SUPPORTED, majors_list(&pins, "supported"));
    assert_eq!(majors::BETA, majors_list(&pins, "beta"));
    assert_eq!(majors::MAINTENANCE, majors_list(&pins, "maintenance"));
    assert_eq!(
        i64::from(majors::DEFAULT),
        pins["majors"]["default"].as_integer().expect("default")
    );
    assert!(majors::SUPPORTED.contains(&majors::DEFAULT));

    let mut all: Vec<u32> = [majors::SUPPORTED, majors::BETA, majors::MAINTENANCE].concat();
    all.sort_unstable();
    let minors = &pins["images"]["storage"]["pg_minors"];
    assert_eq!(majors::MINORS.iter().map(|m| m.0).collect::<Vec<_>>(), all);
    for (major, minor) in majors::MINORS {
        assert_eq!(minors[major.to_string()].as_str(), Some(*minor), "{major}");
    }
    assert_eq!(majors::EOL.iter().map(|m| m.0).collect::<Vec<_>>(), all);
    for (major, eol) in majors::EOL {
        assert!(
            eol.len() == 10 && eol.as_bytes()[4] == b'-' && eol.as_bytes()[7] == b'-',
            "{major}: {eol}"
        );
    }
}
