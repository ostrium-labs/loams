//! `statemachine_has_no_io_deps`: the pure crates depend only on the
//! protos, prost and thiserror (plan DX1 global constraints).

#[test]
fn statemachine_has_no_io_deps() {
    let root = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("set"));
    let allowed = [
        "loams-temporal-history",
        "loams-temporal-proto",
        "prost",
        "prost-types",
        "thiserror",
    ];
    for krate in ["loams-temporal-statemachine", "loams-temporal-history"] {
        let text = std::fs::read_to_string(root.join("..").join(krate).join("Cargo.toml"))
            .expect("Cargo.toml");
        let deps = text
            .split("[dependencies]")
            .nth(1)
            .expect("a dependencies table")
            .split("\n[")
            .next()
            .expect("its body");
        for line in deps
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
        {
            let name = line.split(['=', '.', ' ']).next().expect("a name");
            assert!(allowed.contains(&name), "{krate} depends on {name}");
        }
    }
}
