//! CN1 plan Task 2: the 200-connector registry and the drift checks around it.
//!
//! The registry is loaded from the repository's own `connectors/` tree, found through
//! `env!("CARGO_MANIFEST_DIR")` rather than the current directory, because the crate
//! reads the repository at run time (D352, D359) and a test that depended on the
//! working directory would pass only where it happened to be launched.
//!
//! Two of these tests re-implement a little of `scripts/connectors/gen_registry.py`
//! on purpose — the hand-rolled CSV reader and the Appendix A renderer — because the
//! point of each is that the Rust side and the Python side agree without one calling
//! the other. Where the two conventions differ, the comment says which is which.

#![allow(clippy::unwrap_used)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use buffa::{EnumValue, Message};
use loams_flow::manifest::{Category, ConnectorSpec, Priority, RuntimeKind, Status, pb};
use loams_flow::registry::{Filter, Registry};
use loams_flow::validate::LicenceGate;
use serde_json::Value;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn connectors_root() -> PathBuf {
    repo_root().join("connectors")
}

fn registry_dir() -> PathBuf {
    connectors_root().join("registry")
}

/// The repository's whole registry, or a panic naming every manifest that did not load.
fn registry() -> Registry {
    Registry::load(&registry_dir())
        .unwrap_or_else(|error| panic!("the registry must load; it did not:\n{error}"))
}

// -------------------------------------------------------------------------------------
// The registry
// -------------------------------------------------------------------------------------

/// CN1 Task 2's gate: 204 manifests, 21 of them ★.
///
/// **The two numbers differ on purpose, twice over.** CN1's "Rulings made during
/// execution" row 11 (with D628, 2026-10-04) raised the catalog from Appendix A's
/// original 200 rows to 203 and renamed this gate `registry_has_200_entries_and_21_starred`
/// → `registry_has_203_entries_and_21_starred`: §33 A.18's Zulip, ItsPlane and Forgejo
/// are **P1 and unstarred**, because they ship in CN1 (CN1 Task 15) and Loams owns the
/// applications, but they are not part of the précis' 21-connector hot path. D634
/// (2026-10-04) raised it once more to 204 by adding §33 A.3's Grafeo row, and renamed
/// the gate a second time — the same way, because Grafeo is likewise **P2 and unstarred**:
/// it is the engine D634 embeds in the Fabric, and adding it must not read as adding a
/// star. D358's ★ count is untouched at 21 across both rulings, and a reader must not read
/// 204 as "25 ★".
#[test]
fn registry_has_204_entries_and_21_starred() {
    let registry = registry();
    assert_eq!(
        registry.len(),
        CATALOG_ROWS,
        "§33 Appendix A's totals line, CN1 Ruling 11 (D628) and D634 say 204 rows — 203 \
         from D628 plus Grafeo; a shortfall names the manifests that have not been written"
    );
    assert_eq!(
        registry.starred().len(),
        STARRED_ROWS,
        "§33 §8 counts 21 ★ (D358), which neither D628 nor D634 changed: A.18's three and \
         Grafeo are unstarred"
    );

    // D634's own row, asserted field by field, because "it is in the registry" is not the
    // claim being made. A.3's row is P2 and unstarred, and the runtime is **native** at
    // `loams_flow::connectors::graph` — the engine is embedded in the Fabric (D634(b)),
    // not run beside it, and there is no Bolt anywhere in the path (D634(c)). Before the
    // generator's `LOAMS_OWNED_NATIVE` table named `grafeo`, A.3's empty Camel and Kestra
    // cells fell through its non-★ runtime rule to `openapi:grafeo` — an OpenAPI-generated
    // connector for a database Loams links into its own process, which is the wrong runtime
    // outright. These three assertions are what stop that drift going unnoticed.
    let grafeo = registry
        .get("grafeo")
        .expect("grafeo is in the registry: §33 A.3's second graph row (D634)");
    assert!(!grafeo.starred, "grafeo is P2, not one of D358's 21 (D634)");
    assert_eq!(grafeo.priority, Priority::P2);
    assert_eq!(grafeo.runtime.kind, RuntimeKind::Native);
    assert_eq!(grafeo.runtime.reference, "loams_flow::connectors::graph");

    for spec in registry.starred() {
        assert!(spec.starred, "{} is in the starred list", spec.id);
        assert!(
            spec.id.len() <= 48,
            "{} is not a legal registry key",
            spec.id
        );
    }

    // `handwritten.txt` records the hand-written manifests, and D628 made it the larger
    // set: 21 ★ plus A.18's three. CN1 Ruling 1 leans on it to keep the generator from
    // overwriting a hand-written row, so "hand-written" must never be read as "★".
    let handwritten = std::fs::read_to_string(registry_dir().join("handwritten.txt"))
        .expect("handwritten.txt is readable");
    let recorded: BTreeSet<&str> = handwritten
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let starred: BTreeSet<&str> = registry
        .starred()
        .iter()
        .map(|spec| spec.id.as_str())
        .collect();
    assert!(
        starred.is_subset(&recorded),
        "every ★ manifest must be one of the hand-written ones; {starred:?} is not a subset \
         of {recorded:?}"
    );
    let hand_written_but_unstarred: BTreeSet<&str> = recorded
        .iter()
        .filter(|id| !starred.contains(*id))
        .copied()
        .collect();
    assert!(
        hand_written_but_unstarred.is_subset(
            &A18.iter()
                .chain(HAND_WRITTEN_UNSTARRED_P2.iter())
                .copied()
                .collect()
        ),
        "a hand-written manifest that is neither ★, nor one of A.18's three, nor A.3's Grafeo \
         (D634) would be a generated stub's id (CN1 Ruling 1); found {hand_written_but_unstarred:?}"
    );

    // The index and the filters agree with each other: every category the registry
    // reports holds the specs it says it does, and nothing is listed twice.
    let mut counted = 0usize;
    for (category, specs) in registry.by_category() {
        counted += specs.len();
        for spec in specs {
            assert_eq!(spec.category, *category, "{} is filed wrongly", spec.id);
        }
    }
    assert_eq!(
        counted,
        registry.len(),
        "the category index covers every manifest"
    );

    // A P1 row that is not ★ is legal in the other direction too: the Camel runtime row
    // is §33 §5's runtime, not a Loams-owned connector (D354, D358).
    let camel = registry
        .get("camel-runtime")
        .expect("the Camel runtime row is in the registry");
    assert!(!camel.starred && camel.priority == Priority::P1);
    assert_eq!(camel.runtime.kind, RuntimeKind::Camel);

    let kafka = registry.get("kafka").expect("kafka is in the registry");
    assert_eq!(kafka.category, Category::Messaging);
    assert_eq!(
        registry
            .list(&Filter {
                category: Some(Category::Messaging),
                ..Filter::default()
            })
            .len(),
        20,
        "§33 A.1 counts 20 messaging connectors"
    );
    assert_eq!(
        registry
            .list(&Filter {
                category: Some(Category::Cdc),
                ..Filter::default()
            })
            .len(),
        6,
        "§33 A.7 counts 6 CDC connectors"
    );
    assert_eq!(
        registry
            .list(&Filter {
                runtime_kind: Some(RuntimeKind::Debezium),
                ..Filter::default()
            })
            .len(),
        2,
        "§33 A.7's six CDC connectors, but only CN1's two run on the Debezium Server; the \
         other four are Camel components CN2 would ship"
    );
    assert!(
        registry
            .list(&Filter {
                query: Some("kafka".to_string()),
                ..Filter::default()
            })
            .iter()
            .any(|spec| spec.id == "kafka"),
        "the free-text query reaches the id, the name and the docs path"
    );
}

/// Every manifest's `config.$ref` names a file that exists and is JSON.
///
/// CN1's "Rulings made during execution" row 5 killed the exemption that let a
/// `status: planned` row's `$ref` dangle, so this is a whole-registry invariant with
/// no exceptions: the 179 generated stubs resolve as well as the 21 hand-written ones.
#[test]
fn every_config_ref_resolves() {
    let registry = registry();
    let mut seen = 0usize;
    for spec in registry.all() {
        let reference = &spec.config_ref.reference;
        assert!(
            !reference.starts_with('/') && !reference.contains(".."),
            "{}: {reference:?} must be a path under connectors/",
            spec.id
        );
        let path = connectors_root().join(reference);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("{}: {reference} must resolve: {error}", spec.id));
        let document: Value = serde_json::from_str(&text)
            .unwrap_or_else(|error| panic!("{}: {reference} must be JSON: {error}", spec.id));
        assert_eq!(
            document, spec.config_schema,
            "{}: the resolved schema is the manifest's own",
            spec.id
        );
        assert_eq!(
            reference,
            &format!("schemas/{}.config.json", spec.id),
            "{}: the config schema is named by the manifest's id",
            spec.id
        );
        seen += 1;
    }
    assert_eq!(
        seen,
        registry.len(),
        "every manifest's config.$ref resolves, including A.18's three once Task 15 lands"
    );
}

/// CN1 Task 1's rule 4 over the whole registry, as a cheap invariant.
#[test]
fn starred_implies_p1() {
    for spec in registry().all() {
        if spec.starred {
            assert_eq!(
                spec.priority,
                Priority::P1,
                "{}: starred implies P1 (§33 §8, D358)",
                spec.id
            );
            assert!(
                spec.status != Status::Deprecated,
                "{}: a ★ connector is not deprecated",
                spec.id
            );
        }
    }
}

// -------------------------------------------------------------------------------------
// The CSV
// -------------------------------------------------------------------------------------

/// One row of `connectors/registry/catalog.csv`, read by a small hand-rolled reader.
///
/// No new dependency: the CSV `scripts/connectors/gen_registry.py` writes is RFC 4180,
/// and its quoting is not optional — a Camel cell such as
/// `"plugin-fs (ftp, ftps)"` holds a comma — so the reader below is a field state
/// machine over `,` and `"` rather than a `split(',')`. Everything else about the file
/// is trivial: no embedded newline, and a multi-value cell is `|`-joined with a note in
/// parentheses.
#[derive(Debug, Clone)]
struct CsvRow {
    cells: BTreeMap<String, String>,
}

impl CsvRow {
    /// One cell by column name.
    fn get(&self, column: &str) -> &str {
        self.cells
            .get(column)
            .unwrap_or_else(|| panic!("the CSV row has no {column} column"))
    }
}

/// The CSV's 17 columns, in `scripts/connectors/gen_registry.py`'s order.
const CSV_COLUMNS: [&str; 17] = [
    "id",
    "name",
    "category",
    "priority",
    "starred",
    "status",
    "runtime",
    "ref",
    "source",
    "sink",
    "streaming",
    "batch",
    "cdc",
    "webhook",
    "auth",
    "camel",
    "kestra",
];

/// §33 A.18's three Loams applications (D628, CN1 Ruling 11): they have a CSV row and an
/// Appendix A.18 row, and CN1 Task 15 writes their manifests.
/// Appendix A.18's three Loams applications: P1, hand-written, and **not** ★ (D628).
///
/// This was named `A18_PENDING` while CN1 Task 15's three manifests did not exist and
/// `csv_and_manifests_agree` allowed the CSV three rows the registry could not serve.
/// Task 15 has written them, so the set is no longer pending: the CSV and the registry
/// are one-to-one, and the assertion below compares them without an offset.
const A18: [&str; 3] = ["forgejo", "itsplane", "zulip"];

/// D634's Grafeo row: **P2 and hand-written**, unlike A.18's three.
///
/// `handwritten.txt` is how CN1 Ruling 1 keeps the generator from overwriting a hand-written
/// manifest, and D634 added a hand-written row that is neither ★ nor an A.18 application:
/// A.3's Grafeo is P2 because it is a CN2 build, and hand-written because the engine is
/// Loams's own and there is nothing for the generator to derive a capability detail from.
/// So the rule below, which once read "a hand-written manifest that is neither ★ nor one of
/// A.18's three would be a generated stub's id", would now fail on Grafeo — correctly enough
/// to notice, which is why the id is named rather than the rule loosened.
const HAND_WRITTEN_UNSTARRED_P2: [&str; 1] = ["grafeo"];

/// Ruling 11's row count, after D628 added A.18's three to the précis' 200 and D634 added
/// §33 A.3's Grafeo row.
const CATALOG_ROWS: usize = 204;

/// §33 §8 and D358's ★ count, which D628 and D634 both deliberately did not change.
const STARRED_ROWS: usize = 21;

/// Every data row of `connectors/registry/catalog.csv`.
fn read_catalog_csv() -> Vec<CsvRow> {
    let text = std::fs::read_to_string(registry_dir().join("catalog.csv"))
        .expect("connectors/registry/catalog.csv is readable");
    let mut lines = text.lines().filter(|line| !line.trim().is_empty());
    let header: Vec<String> = split_csv_line(lines.next().expect("the CSV has a header"));
    assert_eq!(
        header, CSV_COLUMNS,
        "the CSV header changed; the reader must change too"
    );
    lines
        .map(|line| {
            let cells = split_csv_line(line);
            assert_eq!(
                cells.len(),
                CSV_COLUMNS.len(),
                "a CSV row must have {} cells: {line}",
                CSV_COLUMNS.len()
            );
            CsvRow {
                cells: CSV_COLUMNS
                    .iter()
                    .map(|column| ((*column).to_string(), cells[column_index(column)].clone()))
                    .collect(),
            }
        })
        .collect()
}

/// One CSV line's fields, honouring RFC 4180's `"` quoting and its `""` escape.
fn split_csv_line(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut characters = line.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '"' if quoted && characters.peek() == Some(&'"') => {
                field.push('"');
                characters.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => fields.push(std::mem::take(&mut field)),
            other => field.push(other),
        }
    }
    assert!(!quoted, "a CSV field's quotes are not closed: {line}");
    fields.push(field);
    fields
}

fn column_index(column: &str) -> usize {
    CSV_COLUMNS
        .iter()
        .position(|candidate| *candidate == column)
        .unwrap_or_else(|| panic!("{column} is not a CSV column"))
}

/// The manifest and the CSV agree on the columns the CSV is authoritative for.
///
/// Per CN1's "Rulings made during execution" row 6, only these eight are compared:
/// `id`, `name`, `category`, `priority`, `starred`, `status`, `runtime.kind` and
/// `runtime.ref`. The CSV's remaining cells — the six capability flags, `auth`,
/// `camel` and `kestra` — are §33 Appendix A's planning shorthand: a `·` where the
/// manifest says `false`, `Y`/`N` instead of a boolean, `per driver` instead of the
/// `per-driver` slug, and for the Camel and Kestra columns the third-party component
/// and plugin names, which the manifest deliberately does not carry (§33 §4's manifest
/// declares capabilities, runtime, licence, config and conformance; it does not name
/// the Camel component or Kestra plugin that covers the system). §33 §4 rule 2 makes
/// the manifest authoritative, so the manifest wins wherever the two differ, and this
/// test asserts only the eight columns both forms state.
#[test]
fn csv_and_manifests_agree() {
    let rows = read_catalog_csv();
    let registry = registry();
    // One CSV row per catalog entry and one manifest per row, with no offset: CN1 Task
    // 15 has written A.18's three, so the earlier "the CSV has three rows the registry
    // cannot serve" allowance is gone. The pair of numbers being equal here is the
    // thing that makes the per-column comparison below total.
    assert_eq!(
        rows.len(),
        registry.len(),
        "the CSV has exactly one row per manifest, A.18's three included (D628)"
    );
    for id in A18 {
        assert!(
            rows.iter().any(|row| row.get("id") == id),
            "A.18's `{id}` is missing from the CSV: it is a hand-written manifest, so \
             the generator cannot be the source of its row (Ruling 1)"
        );
    }

    for row in &rows {
        let id = row.get("id");
        // A row the registry does not hold is a real failure, not a pending manifest.
        // While CN1 Task 15's three were unwritten this branch allowed exactly those
        // three through and skipped their column comparison; now that they exist a
        // missing manifest means the CSV and the registry have diverged, so there is
        // no allowance left to make.
        let spec = registry
            .get(id)
            .unwrap_or_else(|| panic!("the CSV names {id}, which the registry does not hold"));

        // A helper that names the row and both values, so a mismatch says which
        // column of which connector disagrees.
        let same = |column: &str, csv: String, manifest: String| {
            assert_eq!(
                csv, manifest,
                "{id}: the CSV's `{column}` and the manifest's disagree"
            );
        };
        // `name` is compared like every other identity column, with no exception
        // list: the registry's display name is Appendix A's, so `kafka` says
        // `Kafka` even though §33 §4's illustrative example says `Apache Kafka`
        // (CN1 "Rulings made during execution", row 6). An earlier draft allowed
        // that one divergence; naming the manifest after the catalog removes the
        // need, and a second divergence now fails here rather than needing a list.
        same("name", row.get("name").to_string(), spec.name.clone());
        same(
            "category",
            row.get("category").to_string(),
            spec.category.slug().to_string(),
        );
        same(
            "priority",
            row.get("priority").to_string(),
            spec.priority.slug().to_string(),
        );
        same(
            "starred",
            row.get("starred").to_string(),
            spec.starred.to_string(),
        );
        same(
            "status",
            row.get("status").to_string(),
            spec.status.slug().to_string(),
        );
        same(
            "runtime",
            row.get("runtime").to_string(),
            spec.runtime.kind.slug().to_string(),
        );
        same(
            "ref",
            row.get("ref").to_string(),
            spec.runtime.reference.clone(),
        );
    }

    // And the ids are unique, so no connector is described twice and another omitted.
    let ids: BTreeSet<&str> = rows.iter().map(|row| row.get("id")).collect();
    assert_eq!(ids.len(), rows.len(), "the CSV repeats an id");
}

// -------------------------------------------------------------------------------------
// §33 Appendix A
// -------------------------------------------------------------------------------------

/// §33 Appendix A's Markdown tables are rendered from the CSV, cell for cell.
///
/// The Appendix is §33's planning record of the same 200 rows (D352), and
/// `scripts/connectors/gen_registry.py --docs` regenerates it from `catalog.csv`. This
/// test performs that rendering in Rust and compares it against
/// `docs/design/33-connectors.md`, so the two implementations of the rendering — the
/// Python one and this one — are checked against each other by their output.
///
/// The conventions it reproduces, all of them `gen_registry.py`'s:
/// `·` where a cell says no; `★ ` in front of a starred connector's name; a
/// multi-value cell `|`-joined in the CSV and `, `-joined in the table; the auth
/// legend's `per-component`/`per-driver`/`per-spec` slugs written as the prose
/// `per component`/`per driver`/`per spec`; and a Camel or Kestra component in
/// backticks unless it is one of the two that name a runtime rather than a component
/// (`core`, `itself`), with a parenthesised sub-module in plain text.
#[test]
fn appendix_matches_csv() {
    let rows = read_catalog_csv();
    let appendix = read_appendix_a();
    assert_eq!(
        appendix.len(),
        rows.len(),
        "§33 Appendix A has one row per CSV row"
    );

    let by_id: BTreeMap<&str, &CsvRow> = rows.iter().map(|row| (row.get("id"), row)).collect();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for (appendix_name, cells) in &appendix {
        let id = connector_id(appendix_name);
        assert!(
            seen.insert(id.clone()),
            "§33 Appendix A names {appendix_name} twice ({id})"
        );
        let row = by_id.get(id.as_str()).unwrap_or_else(|| {
            panic!("§33 Appendix A's {appendix_name:?} derives the id {id:?}, which the CSV has no row for")
        });

        // Cell for cell, left to right, with the column named on a mismatch.
        let rendered = render_appendix_row(row);
        let columns = [
            "name",
            "source",
            "sink",
            "streaming",
            "batch",
            "cdc",
            "webhook",
            "auth",
            "camel",
            "kestra",
            "priority",
        ];
        for (index, column) in columns.iter().enumerate() {
            let expected = cells[index].trim();
            let found = rendered[index].trim();
            assert_eq!(
                found, expected,
                "{id}: §33 Appendix A's `{column}` cell cannot be reproduced from the \
                 CSV's `{column}` cell"
            );
        }
    }
    let missing: Vec<&str> = by_id
        .keys()
        .filter(|id| !seen.contains(**id))
        .copied()
        .collect();
    assert!(
        missing.is_empty(),
        "the CSV has rows Appendix A does not: {missing:?}"
    );
}

/// §33 Appendix A's table rows, as `(first cell, all cells)`, in document order.
///
/// Every `### A.n` subsection's table has the same eleven columns; the header row and
/// the `|---|---|` separator under it are skipped, and so is every other line, which is
/// either a heading or prose.
fn read_appendix_a() -> Vec<(String, Vec<String>)> {
    let path = repo_root().join("docs/design/33-connectors.md");
    let text = std::fs::read_to_string(&path).expect("docs/design/33-connectors.md is readable");
    let start = text
        .find("## Appendix A.")
        .unwrap_or_else(|| panic!("{} has no Appendix A", path.display()));
    let appendix = &text[start..];

    let mut rows = Vec::new();
    for line in appendix.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with('|') || !trimmed.ends_with('|') {
            continue;
        }
        let cells: Vec<String> = trimmed
            .trim_matches('|')
            .split('|')
            .map(|cell| cell.trim().to_string())
            .collect();
        if cells.len() != 11 {
            continue;
        }
        // The header row and the separator carry no connector.
        if cells[0] == "Connector" || cells[1].starts_with('-') {
            continue;
        }
        rows.push((cells[0].clone(), cells));
    }
    rows
}

/// One CSV row rendered as §33 Appendix A's eleven cells.
fn render_appendix_row(row: &CsvRow) -> Vec<String> {
    let yes_no = |flag: bool| if flag { "Y" } else { "·" }.to_string();
    vec![
        format!(
            "{}{}",
            if row.get("starred") == "true" {
                "★ "
            } else {
                ""
            },
            row.get("name")
        ),
        yes_no(row.get("source") == "Y"),
        yes_no(row.get("sink") == "Y"),
        yes_no(row.get("streaming") == "Y"),
        yes_no(row.get("batch") == "Y"),
        yes_no(row.get("cdc") == "Y"),
        yes_no(row.get("webhook") == "Y"),
        render_auth_cell(row.get("auth")),
        render_items_cell(row.get("camel")),
        render_items_cell(row.get("kestra")),
        row.get("priority").to_string(),
    ]
}

/// The auth legend's `per-*` slugs are prose in the table and `|`-joined in the CSV.
fn render_auth_cell(cell: &str) -> String {
    let items = split_multi(cell);
    if items.is_empty() {
        return "·".to_string();
    }
    items
        .iter()
        .map(|item| match item.as_str() {
            "per-component" => "per component".to_string(),
            "per-driver" => "per driver".to_string(),
            "per-spec" => "per spec".to_string(),
            other => other.to_string(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// A Camel or Kestra column, or `·` when it is empty.
fn render_items_cell(cell: &str) -> String {
    let items = split_multi(cell);
    if items.is_empty() {
        return "·".to_string();
    }
    items
        .iter()
        .map(|item| render_item(item))
        .collect::<Vec<_>>()
        .join(", ")
}

/// One component or plugin, in backticks unless it names a runtime, with a
/// parenthesised sub-module in plain text.
fn render_item(item: &str) -> String {
    if item.starts_with('(') {
        return item.to_string();
    }
    if let Some((name, note)) = split_note(item) {
        if name.is_empty() || matches!(name.to_lowercase().as_str(), "core" | "itself") {
            return format!("{name} {note}").trim().to_string();
        }
        return format!("`{name}` {note}");
    }
    if matches!(item.to_lowercase().as_str(), "core" | "itself") {
        return item.to_string();
    }
    format!("`{item}`")
}

/// `plugin-aws (sqs)` → (`plugin-aws`, `(sqs)`). Nothing else in the Appendix's cells
/// carries a parenthesis outside such a note.
fn split_note(item: &str) -> Option<(String, String)> {
    if !item.ends_with(')') {
        return None;
    }
    let open = item.find('(')?;
    let (name, note) = item.split_at(open);
    if name.contains(['(', ')']) || note[1..note.len() - 1].contains(['(', ')']) {
        return None;
    }
    Some((name.trim().to_string(), note.to_string()))
}

/// A `|`-joined multi-value cell, split and whitespace-collapsed.
fn split_multi(cell: &str) -> Vec<String> {
    cell.split('|')
        .map(|item| collapse(item.trim()))
        .filter(|item| !item.is_empty())
        .collect()
}

/// Every run of whitespace folded to one space, which is what the Python side does
/// before it writes a cell.
fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A connector's registry id, as `scripts/connectors/gen_registry.py`'s `connector_id`
/// derives it from §33 Appendix A's display name.
///
/// The four hand-assigned ids are the ones whose slug would not be the id anyone
/// writes: `arrow`, `http`, `adbc` and `airbyte`. Everything else is the name
/// lower-cased with runs of non-alphanumerics folded to `-`, trimmed, and cut to 48
/// characters, which is `connector.schema.json`'s `[a-z0-9-]{1,48}`.
fn connector_id(name: &str) -> String {
    let name = strip_star(name);
    match name {
        "Arrow IPC / Flight" => "arrow".to_string(),
        "HTTP / REST" => "http".to_string(),
        "ADBC (generic)" => "adbc".to_string(),
        "Airbyte (licence flag, D359)" => "airbyte".to_string(),
        other => {
            let mut slug = String::with_capacity(other.len());
            let mut in_run = false;
            for character in other.to_lowercase().chars() {
                if character.is_ascii_alphanumeric() {
                    slug.push(character);
                    in_run = false;
                } else if !in_run {
                    slug.push('-');
                    in_run = true;
                }
            }
            let slug = slug.trim_matches('-').to_string();
            let cut = slug.len().min(48);
            slug[..cut].trim_end_matches('-').to_string()
        }
    }
}

/// `★ Apache Kafka` → `Apache Kafka`.
fn strip_star(name: &str) -> &str {
    name.trim().strip_prefix('★').map_or(name.trim(), str::trim)
}

// -------------------------------------------------------------------------------------
// The licence gate
// -------------------------------------------------------------------------------------

/// D359's gate refuses each of the four flagged ids, and the refused set is read from
/// `connectors/licences.toml` rather than written here.
#[test]
fn licence_gate_refuses_flagged() {
    let gate = LicenceGate::repository()
        .unwrap_or_else(|error| panic!("connectors/licences.toml must be readable: {error}"));

    // The four flagged ids the gate exists for, plus the two D359 names besides them.
    for id in [
        "AGPL-3.0-only",
        "BUSL-1.1",
        "Elastic-2.0",
        "NOASSERTION",
        "SSPL-1.0",
        "unknown",
    ] {
        assert!(gate.refuses(id), "[deny] must refuse {id} (D359)");
    }
    // A permissive id, which must survive: the gate refuses specific ids, not
    // everything.
    for id in ["Apache-2.0", "BSD-2-Clause", "MIT", "Elastic-2.0-only"] {
        assert!(!gate.refuses(id), "[deny] must not refuse {id} (D359)");
    }

    // A fixture per flagged id: kafka's manifest with that id as its component, and as
    // one of its loaded dependencies. Both must be refused, named with the connector,
    // the id and the file that says so.
    for id in ["AGPL-3.0-only", "BUSL-1.1", "Elastic-2.0", "NOASSERTION"] {
        let mut as_component = kafka();
        as_component.licence.component = id.to_string();
        let error = gate
            .check_manifest(&as_component, "connectors/licences.toml")
            .err()
            .unwrap_or_else(|| panic!("{id} as licence.component must be refused (D359)"));
        assert_eq!(
            error.to_string(),
            format!(
                "kafka: licence.component is {id}, which the gate refuses (connectors/licences.toml)"
            )
        );

        let mut as_dependency = kafka();
        as_dependency
            .licence
            .dependencies
            .insert("librdkafka".to_string(), id.to_string());
        let error = gate
            .check_manifest(&as_dependency, "connectors/licences.toml")
            .err()
            .unwrap_or_else(|| panic!("{id} as a loaded dependency must be refused (D359)"));
        assert_eq!(
            error.to_string(),
            format!(
                "kafka: licence.dependencies.librdkafka is {id}, which the gate refuses \
             (connectors/licences.toml)"
            )
        );

        // The same manifest with a permissive id passes, so the case above is the gate
        // and not a manifest the gate refuses wholesale.
        let mut allowed = kafka();
        allowed.licence.component = "MIT".to_string();
        assert!(
            gate.check_manifest(&allowed, "connectors/licences.toml")
                .is_ok()
        );
    }

    // The real gate, applied to all 200 real manifests: none is refused.
    for spec in registry().all() {
        gate.check_manifest(spec, "connectors/licences.toml")
            .unwrap_or_else(|error| panic!("{error}"));
    }

    // And no `[components.*]` stanza of the gate input carries a refused id, which is
    // what `gen_registry.py --check` asserts.
    let path = connectors_root().join("licences.toml");
    let text = std::fs::read_to_string(&path).expect("connectors/licences.toml is readable");
    let components = LicenceGate::components(&text, "connectors/licences.toml")
        .unwrap_or_else(|error| panic!("the component table must read: {error}"));
    // 36 at the start of CN1 Task 15, plus the three A.18 upstreams Task 15 added: Zulip
    // (Apache-2.0), ItsPlane (AGPL-3.0) and Forgejo (MIT). The AGPL one is the point of
    // the assertion below: `check_components` refuses a refused id on a `kind = "library"`
    // row, and must accept it on the `kind = "service"` row that follows, because Loams
    // neither ships nor links ItsPlane and reaches it only over its HTTP API (D359).
    // D634 then added five more, all `kind = "library"` and all Apache-2.0: `grafeo` 0.5.43
    // with `grafeo-core`, `grafeo-engine`, `grafeo-adapters` and `grafeo-common`, read from
    // crates.io on 2026-10-04. They are on this side of the gate rather than in D359's
    // carve-out precisely because D634(b) **links** the engine into the Fabric, so a
    // `library` row is the honest kind and `check_components` really does licence-check them.
    assert_eq!(
        components.len(),
        47,
        "D359 records 42 components after CN1 Task 15, plus D634's five Grafeo crates"
    );
    gate.check_components(&text, "connectors/licences.toml")
        .unwrap_or_else(|error| panic!("no component may be refused: {error}"));

    // A gate with no `[deny]` list, or an empty one, is refused rather than silently
    // passing everything: a gate that refuses nothing is worse than no gate.
    assert!(LicenceGate::from_toml("[notes]\nsaas = \"x\"\n", "fixture.toml").is_err());
    assert!(LicenceGate::from_toml("[deny]\nids = []\n", "fixture.toml").is_err());
}

fn kafka() -> ConnectorSpec {
    loams_flow::manifest::load_manifest(&registry_dir().join("kafka.yaml"), &connectors_root())
        .unwrap_or_else(|errors| panic!("kafka must load; it did not: {errors:?}"))
}

// -------------------------------------------------------------------------------------
// The protobuf form
// -------------------------------------------------------------------------------------

/// §33 §4's last paragraph, for all 21 ★ manifests: the YAML is the source and the
/// protobuf form mirrors it one to one, so every field and every enum survives the
/// round trip in both directions.
///
/// The round trip is the long one — YAML to typed, typed to protobuf, protobuf over the
/// wire as protobuf bytes, back out of the wire, and back to typed — and it is compared
/// field by field rather than as one `PartialEq`, so a failure names the field that
/// drifted. Every enum is checked in both directions: the YAML slug becomes the proto
/// name `connector.proto` gives it, and that name becomes the slug again.
#[test]
fn proto_and_yaml_agree() {
    let registry = registry();
    // §33 §8's 21 ★ only. A.18's Zulip, ItsPlane and Forgejo are P1 and unstarred
    // (CN1 Ruling 11, D628) and are CN1 Task 15's to write, so they are outside this
    // test's scope by design, not by omission.
    let starred = registry.starred();
    assert_eq!(starred.len(), 21, "§33 §8 counts 21 ★ (D358)");

    for &spec in &starred {
        let id = spec.id.as_str();

        // --- YAML → protobuf, with every enum slug translated to its proto name. ---
        let proto = spec
            .to_proto()
            .unwrap_or_else(|error| panic!("{id}: the protobuf form must be buildable: {error}"));

        // proto3's JSON mapping uses the proto name, never the YAML slug. This is the
        // drift this test exists to catch, so it is asserted rather than assumed.
        let json: Value = serde_json::to_value(&proto)
            .unwrap_or_else(|error| panic!("{id}: the protobuf form must render: {error}"));
        assert_eq!(
            json["category"],
            proto_enum_name(spec.category.slug(), "CATEGORY")
        );
        assert_eq!(
            json["priority"],
            proto_enum_name(spec.priority.slug(), "PRIORITY")
        );
        assert_eq!(
            json["status"],
            proto_enum_name(spec.status.slug(), "STATUS")
        );
        assert_eq!(
            json["runtime"]["kind"],
            proto_enum_name(spec.runtime.kind.slug(), "RUNTIME_KIND")
        );
        assert_eq!(
            json["capabilities"]["ordering"],
            proto_enum_name(spec.capabilities.ordering.slug(), "ORDERING")
        );
        assert_eq!(
            json["capabilities"]["delivery"]["source"],
            proto_enum_name(spec.capabilities.delivery.source.slug(), "DELIVERY")
        );
        assert_eq!(
            json["capabilities"]["delivery"]["sink"],
            proto_enum_name(spec.capabilities.delivery.sink.slug(), "DELIVERY")
        );
        assert_eq!(
            json["capabilities"]["schema"]["registry"],
            proto_enum_name(
                spec.capabilities.schema.registry.slug(),
                "REGISTRY_REQUIREMENT"
            )
        );
        assert_eq!(
            json["capabilities"]["schema"]["evolution"],
            proto_enum_name(spec.capabilities.schema.evolution.slug(), "EVOLUTION")
        );
        assert_eq!(
            json["capabilities"]["backpressure"],
            proto_enum_name(spec.capabilities.backpressure.slug(), "BACKPRESSURE")
        );
        for (index, format) in spec.capabilities.formats.iter().enumerate() {
            assert_eq!(
                json["capabilities"]["formats"][index],
                proto_enum_name(format.slug(), "FORMAT"),
                "{id}: formats[{index}] must carry its proto name"
            );
        }
        for (index, method) in spec.auth.iter().enumerate() {
            assert_eq!(
                json["auth"][index],
                proto_enum_name(method.slug(), "AUTH_METHOD"),
                "{id}: auth[{index}] must carry its proto name"
            );
        }
        for (index, suite) in spec.conformance.iter().enumerate() {
            assert_eq!(
                json["conformance"][index],
                proto_enum_name(suite.slug(), "SUITE"),
                "{id}: conformance[{index}] must carry its proto name"
            );
        }

        // Every enum value the manifest used is a *known* protobuf value, so nothing
        // became the `_UNSPECIFIED` zero or an off-the-wire integer.
        assert!(matches!(proto.category, EnumValue::Known(_)));
        assert!(matches!(proto.runtime.kind, EnumValue::Known(_)));
        assert!(proto.auth.iter().all(|method| method.is_known()));
        assert!(
            proto
                .capabilities
                .formats
                .iter()
                .all(|format| format.is_known())
        );
        assert!(proto.conformance.iter().all(|suite| suite.is_known()));
        assert_eq!(
            proto.category.to_string(),
            proto_enum_name(spec.category.slug(), "CATEGORY"),
            "{id}: EnumValue renders its proto name, not its number"
        );

        // --- over the wire, so the two forms agree as protobuf bytes too. ---
        let bytes = proto.encode_to_vec();
        let decoded = pb::ConnectorSpec::decode_from_slice(&bytes)
            .unwrap_or_else(|error| panic!("{id}: the protobuf bytes must decode: {error}"));
        assert_eq!(decoded.id, proto.id);
        assert_eq!(decoded.spec_version, proto.spec_version);

        // --- protobuf → typed, with every proto name translated back to its slug. ---
        let back = ConnectorSpec::from_proto(&decoded, spec.config_schema.clone())
            .unwrap_or_else(|error| panic!("{id}: the protobuf form must convert back: {error}"));

        // Field by field, so a failure names the field.
        assert_eq!(back.id, spec.id, "{id}: id");
        assert_eq!(back.name, spec.name, "{id}: name");
        assert_eq!(
            back.spec_version, spec.spec_version,
            "{id}: specVersion (proto3 string → semver)"
        );
        assert_eq!(
            back.category, spec.category,
            "{id}: category (proto name → slug)"
        );
        assert_eq!(
            back.priority, spec.priority,
            "{id}: priority (proto name → slug)"
        );
        assert_eq!(back.starred, spec.starred, "{id}: starred");
        assert_eq!(back.status, spec.status, "{id}: status (proto name → slug)");
        assert_eq!(
            back.runtime, spec.runtime,
            "{id}: runtime (kind, ref, version)"
        );
        assert_eq!(
            back.licence, spec.licence,
            "{id}: licence (component, dependencies)"
        );
        assert_eq!(
            back.capabilities.source, spec.capabilities.source,
            "{id}: capabilities.source (a message the wire may omit)"
        );
        assert_eq!(
            back.capabilities.sink, spec.capabilities.sink,
            "{id}: capabilities.sink (a message the wire may omit)"
        );
        assert_eq!(
            back.capabilities.delivery, spec.capabilities.delivery,
            "{id}: capabilities.delivery (two proto names → slugs)"
        );
        assert_eq!(
            back.capabilities.ordering, spec.capabilities.ordering,
            "{id}: capabilities.ordering"
        );
        assert_eq!(
            back.capabilities.formats, spec.capabilities.formats,
            "{id}: capabilities.formats (every repeated enum, in order)"
        );
        assert_eq!(
            back.capabilities.schema, spec.capabilities.schema,
            "{id}: capabilities.schema (registry, evolution)"
        );
        assert_eq!(
            back.capabilities.bulk, spec.capabilities.bulk,
            "{id}: capabilities.bulk (arrow, max_batch_rows)"
        );
        assert_eq!(
            back.capabilities.backpressure, spec.capabilities.backpressure,
            "{id}: capabilities.backpressure"
        );
        assert_eq!(back.auth, spec.auth, "{id}: auth (every slug, in order)");
        assert_eq!(
            back.config_ref, spec.config_ref,
            "{id}: config.$ref (the protobuf form's config_ref)"
        );
        assert_eq!(
            back.config_schema, spec.config_schema,
            "{id}: the resolved config schema, which the protobuf form does not carry"
        );
        assert_eq!(back.secrets, spec.secrets, "{id}: secrets");
        assert_eq!(
            back.envelope, spec.envelope,
            "{id}: envelope (emits, consumes, passthrough)"
        );
        assert_eq!(
            back.limits, spec.limits,
            "{id}: limits (an absent bound is the wire's 0)"
        );
        assert_eq!(
            back.conformance, spec.conformance,
            "{id}: conformance (every suite, in order)"
        );
        assert_eq!(
            back.docs, spec.docs,
            "{id}: docs (an absent page is the wire's empty string)"
        );
        assert_eq!(back, *spec, "{id}: the whole manifest, as one value");

        // And the document form round-trips too: `to_yaml` writes `apiVersion` and
        // `kind`, which `connector.proto` has no field for, and the loader takes it
        // back to the same spec (D352).
        let yaml = spec
            .to_yaml()
            .unwrap_or_else(|error| panic!("{id}: the manifest must render as YAML: {error}"));
        assert!(
            yaml.contains("apiVersion: loams.flow/v1") && yaml.contains("kind: Connector"),
            "{id}: the rendered manifest must carry the two const keys"
        );
        let reloaded = loams_flow::manifest::load_manifest_value(
            &format!("{id}.yaml"),
            &serde_norway::from_str::<Value>(&yaml)
                .unwrap_or_else(|error| panic!("{id}: the rendered YAML must parse: {error}")),
            &connectors_root(),
            &loams_flow::validate::ManifestSchema::embedded()
                .expect("the manifest schema compiles"),
        )
        .unwrap_or_else(|errors| panic!("{id}: the rendered manifest must load: {errors:?}"));
        assert_eq!(
            reloaded, *spec,
            "{id}: YAML → proto → YAML must be the same manifest"
        );
    }
}

/// The proto name `connector.proto` gives a YAML slug: the slug upper-cased with its
/// separators folded to `_`, under the enum's own prefix. `per_key` is
/// `ORDERING_PER_KEY`; `object-storage` is `CATEGORY_OBJECT_STORAGE`; `P1` is
/// `PRIORITY_P1`, because the prefix carries the `P`.
fn proto_enum_name(slug: &str, prefix: &str) -> String {
    let body = match slug {
        "P1" | "P2" | "P3" => slug.to_string(),
        other => other.to_uppercase().replace(['-', '_'], "_"),
    };
    format!("{prefix}_{body}")
}
