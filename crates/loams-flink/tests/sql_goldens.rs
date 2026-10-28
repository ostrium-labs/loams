//! Golden files for the generated Flink SQL (D1980 to D1985).
//!
//! Each `tests/goldens/<case>.toml` is a `SessionSpec`; its bootstrap script
//! is compared with `<case>.redacted.sql` (secrets as markers) and, where it
//! exists, `<case>.resolved.sql` (secrets from [`secrets`]). Set
//! `LOAMS_UPDATE_GOLDENS=1` to rewrite the files from the code.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use loams_flink::catalog::{RenderCtx, SessionSpec};

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/goldens")
}

fn secrets() -> BTreeMap<String, String> {
    [
        ("lakekeeper-client", "s3cr'et"),
        ("loams-api-key", "lk_\"quoted\"\\x"),
        ("pg-password", "pg-pass"),
        ("s3-secret", "s3-pass"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect()
}

fn spec(case: &str) -> SessionSpec {
    let path = dir().join(format!("{case}.toml"));
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    toml::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

fn check(case: &str, kind: &str, actual: &str) {
    let path = dir().join(format!("{case}.{kind}.sql"));
    if std::env::var_os("LOAMS_UPDATE_GOLDENS").is_some() {
        std::fs::write(&path, actual).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
        return;
    }
    let want =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    assert!(
        want == actual,
        "{} differs from the generated SQL (LOAMS_UPDATE_GOLDENS=1 rewrites it).\n--- generated ---\n{actual}--- end ---",
        path.display()
    );
}

fn run(case: &str, resolved: bool) {
    let script = spec(case)
        .script()
        .unwrap_or_else(|e| panic!("{case}: {e}"));
    let redacted = script
        .render_script(&RenderCtx::redacted())
        .unwrap_or_else(|e| panic!("{case}: {e}"));
    check(case, "redacted", &redacted);
    if resolved {
        let secrets = secrets();
        let sql = script
            .render_script(&RenderCtx::resolved(&secrets))
            .unwrap_or_else(|e| panic!("{case}: {e}"));
        check(case, "resolved", &sql);
    }
}

#[test]
fn local_stack() {
    run("local", false);
}

#[test]
fn cloud_shape() {
    run("cloud", true);
}

#[test]
fn every_secret_reference_is_listed() {
    let script = spec("local").script().unwrap_or_else(|e| panic!("{e}"));
    let refs: Vec<String> = script.secret_refs().into_iter().collect();
    assert_eq!(refs, vec!["loams-api-key", "pg-password", "s3-secret"]);
}

#[test]
fn redacted_output_never_holds_a_secret() {
    let secrets = secrets();
    for case in ["local", "cloud"] {
        let sql = spec(case)
            .script()
            .and_then(|s| s.render_script(&RenderCtx::redacted()))
            .unwrap_or_else(|e| panic!("{case}: {e}"));
        for value in secrets.values() {
            assert!(!sql.contains(value.as_str()), "{case} leaks {value:?}");
        }
    }
}
