//! The function and engine matrix (HE0 Task 8): classification, and the CI
//! check that fails on a mined feature nobody classified.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use clap::Parser as _;
use loams_house_mine::cli::{Cli, Command};
use loams_house_mine::matrix::{Class, Classes, Item, Kind, Mined, classify, mined_not_in_chdb};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn function(name: &str, alias_to: Option<&str>, case_insensitive: bool) -> Item {
    Item {
        kind: Kind::Function,
        name: name.into(),
        aggregate: false,
        case_insensitive,
        alias_to: alias_to.map(str::to_string),
    }
}

fn mined(profile: &[&str], found: &[(&str, &str)]) -> Mined {
    let mut m = Mined::default();
    for p in profile {
        m.profile.insert((*p).to_string(), None);
        m.found
            .entry((*p).to_string())
            .or_default()
            .insert("langfuse".into());
    }
    for (f, app) in found {
        m.found
            .entry((*f).to_string())
            .or_default()
            .insert((*app).to_string());
    }
    m
}

const CLASSES: &str = r#"
[[rule]]
class = "disabled"
kind = "function"
names = ["file"]
reason = "security"

[[rule]]
class = "refused"
kind = "function"
prefixes = ["geo"]
reason = "Tier 2"

[[rule]]
class = "profile"
kind = "function"
names = ["lower"]
reason = "Tier 0"
"#;

#[test]
fn classes_follow_their_precedence_aliases_and_case() {
    let classes = Classes::parse(CLASSES).expect("classes");
    let items = vec![
        function("file", None, false),
        function("geoDistance", None, false),
        function("lower", None, true),
        function("lcase", Some("lower"), true),
        function("geoToH3", None, false),
        function("toYYYYMM", None, false),
        function("COUNT", None, true),
    ];
    // `file` is mined into the profile and still disabled; `geoToH3` is mined
    // into the profile and so beats its Tier 2 rule; `count` is mined in
    // another case and matched because chDB's `COUNT` is case-insensitive.
    let m = mined(
        &["function:file", "function:geoToH3", "function:count"],
        &[],
    );
    let entries = classify(&items, &classes, &m);
    let class_of = |name: &str| {
        entries
            .iter()
            .find(|e| e.item.name == name)
            .map(|e| e.class)
            .expect(name)
    };
    assert_eq!(class_of("file"), Class::Disabled);
    assert_eq!(class_of("geoDistance"), Class::Refused);
    assert_eq!(class_of("geoToH3"), Class::Profile);
    assert_eq!(class_of("lower"), Class::Profile);
    assert_eq!(class_of("lcase"), Class::Profile);
    assert_eq!(class_of("toYYYYMM"), Class::Unclassified);
    assert_eq!(class_of("COUNT"), Class::Profile);
    let lcase = entries
        .iter()
        .find(|e| e.item.name == "lcase")
        .expect("lcase");
    assert!(lcase.why.contains("alias of `lower`"), "{}", lcase.why);
}

#[test]
fn combinator_forms_are_not_reported_missing_from_chdb() {
    let items = vec![function("count", None, true), function("uniq", None, false)];
    let m = mined(
        &[],
        &[
            ("function:countIf", "a"),
            ("function:uniqMergeState", "a"),
            ("function:nope", "a"),
            ("type:UInt8", "a"),
        ],
    );
    assert_eq!(mined_not_in_chdb(&items, &m), ["function:nope"]);
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
    std::fs::write(path, text).expect("write");
}

fn matrix_args(dir: &Path, check: bool) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "loams-house-mine".into(),
        "matrix".into(),
        "--dumps".into(),
        dir.join("dumps").display().to_string(),
        "--classes".into(),
        dir.join("classes.toml").display().to_string(),
        "--profile".into(),
        dir.join("profile.toml").display().to_string(),
        "--inventories".into(),
        dir.join("inventory").display().to_string(),
        "--out".into(),
        dir.join("compat-matrix.md").display().to_string(),
    ];
    if check {
        args.push("--check".into());
    }
    args
}

fn run(args: Vec<String>) -> anyhow::Result<()> {
    let cli = Cli::try_parse_from(args).expect("args");
    assert!(matches!(cli.command, Command::Matrix(_)));
    cli.command.run()
}

#[test]
fn an_unclassified_mined_feature_fails_the_check() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path();
    write(
        &dir.join("dumps/functions.tsv"),
        "name\tis_aggregate\tcase_insensitive\talias_to\nfile\t0\t0\t\nlower\t0\t1\t\ntoYYYYMM\t0\t0\t\n",
    );
    write(
        &dir.join("dumps/table_engines.tsv"),
        "name\nMergeTree\nKafka\n",
    );
    write(&dir.join("dumps/table_functions.tsv"), "name\nnumbers\n");
    write(&dir.join("dumps/source.txt"), "chDB test, ClickHouse 0.0\n");
    write(&dir.join("classes.toml"), CLASSES);
    write(
        &dir.join("profile.toml"),
        "schema = \"loams-house-mine/profile/1\"\nmin_apps = 3\ngate = []\napps = [\"a\"]\n\n\
         [[feature]]\nname = \"function:lower\"\napps = [\"a\"]\nfrequency = 4\nreason = \"apps>=3\"\n",
    );

    // Nothing mined is unclassified: generate, then the check passes.
    run(matrix_args(dir, false)).expect("generate");
    let md = std::fs::read_to_string(dir.join("compat-matrix.md")).expect("matrix");
    assert!(
        md.contains("| `lower` | profile | mined profile | a |"),
        "{md}"
    );
    run(matrix_args(dir, true)).expect("the check passes");

    // An application's inventory uses `toYYYYMM`, which no rule classifies.
    write(
        &dir.join("inventory/umami.json"),
        r#"{"schema": "x", "app": "umami", "features": {"function:toYYYYMM": {"count": 2}}}"#,
    );
    let err = run(matrix_args(dir, true)).expect_err("an unclassified mined feature fails");
    let text = format!("{err:#}");
    assert!(text.contains("1 mined features are unclassified"), "{text}");

    // Classified, regenerated: the check passes again.
    let mut classes = CLASSES.to_string();
    classes.push_str("\n[[rule]]\nclass = \"profile\"\nkind = \"function\"\nnames = [\"toYYYYMM\"]\nreason = \"test\"\n");
    write(&dir.join("classes.toml"), &classes);
    // The committed matrix is now stale, which the check also catches.
    let err = run(matrix_args(dir, true)).expect_err("stale");
    assert!(format!("{err:#}").contains("is stale"), "{err:#}");
    run(matrix_args(dir, false)).expect("regenerate");
    run(matrix_args(dir, true)).expect("the check passes");
}

/// Collects the string literals of a `pub const NAME: &[&str] = &[…];` in a
/// Rust source, or of `host("name", …, Host::Disabled)` calls.
fn rust_list(source: &str, constant: &str) -> BTreeSet<String> {
    let start = source
        .find(&format!("pub const {constant}:"))
        .unwrap_or_else(|| panic!("{constant} in deny.rs"));
    let body = &source[start..];
    let body = &body[..body.find("];").expect("end of the list")];
    body.lines()
        .map(str::trim)
        .filter(|l| !l.starts_with("//"))
        .flat_map(|l| {
            l.split('"')
                .skip(1)
                .step_by(2)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Every `disabled` name in the committed classes is refused with `344` by
/// today's House, so the matrix never claims a security refusal the House
/// does not make.
#[test]
fn disabled_names_are_on_the_house_deny_list() {
    let deny = std::fs::read_to_string(repo().join("fabric/crates/loams-house/src/deny.rs"))
        .expect("deny.rs");
    let mut functions = rust_list(&deny, "DENIED_FUNCTIONS");
    functions.extend(
        deny.lines()
            .filter(|l| l.contains("Host::Disabled)"))
            .filter_map(|l| l.split('"').nth(1).map(str::to_string)),
    );
    let engines = rust_list(&deny, "DENIED_ENGINES");
    let table_functions = rust_list(&deny, "DENIED_TABLE_FUNCTIONS");
    let classes = Classes::parse(
        &std::fs::read_to_string(repo().join("conformance/clickhouse/matrix/classes.toml"))
            .expect("classes.toml"),
    )
    .expect("classes");
    for rule in classes.rules.iter().filter(|r| r.class == Class::Disabled) {
        let list = match rule.kind {
            Kind::Function => &functions,
            Kind::Engine => &engines,
            Kind::TableFunction => &table_functions,
        };
        for name in &rule.names {
            assert!(
                list.contains(name),
                "{name} ({:?}) is not on the House's deny list",
                rule.kind
            );
        }
        for prefix in &rule.prefixes {
            assert!(
                list.iter().any(|n| n.starts_with(prefix.as_str())),
                "no deny-list entry starts with {prefix}"
            );
        }
    }
}

/// The committed matrix is what the committed inputs generate (the CI check).
#[test]
fn the_committed_matrix_is_up_to_date() {
    let root = repo();
    let args: Vec<String> = [
        "loams-house-mine",
        "matrix",
        "--check",
        "--dumps",
        &root
            .join("conformance/clickhouse/matrix/chdb")
            .display()
            .to_string(),
        "--classes",
        &root
            .join("conformance/clickhouse/matrix/classes.toml")
            .display()
            .to_string(),
        "--profile",
        &root
            .join("conformance/clickhouse/profile/profile.toml")
            .display()
            .to_string(),
        "--inventories",
        &root
            .join("conformance/clickhouse/inventory")
            .display()
            .to_string(),
        "--out",
        &root
            .join("docs/house/compat-matrix.md")
            .display()
            .to_string(),
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect();
    run(args)
        .expect("run `loams-house-mine matrix` from the repository root and commit the result");
}
