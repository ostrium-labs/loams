//! The route map (API1 plan Task 0): `docs/api/route-map.md` lists every
//! HTTP route the binary serves and every path of the console contract,
//! with the RPC that replaces it. This test reads the routers' source and
//! the contract, and fails when a route is missing from the map or the map
//! lists a route that no longer exists.
//!
//! It also guards the direction the route map never checked: that every
//! `loams.*.v1` **package** the code and the protos name is a package the
//! design enumerates. Checking methods against paths cannot see an invented
//! package, because a package name appears nowhere in a route's path. So
//! `route_map_covers_every_route` stayed green while Task 3 shipped
//! `loams.document.v1`, a package `docs/design/44-unified-api-and-sdks.md`
//! never names — the design puts `DocumentService` in `loams.collection.v1`
//! beside `CollectionService`, and the map's own document rows (lines 49–54)
//! name `loams.collection.v1.DocumentService/*` too. The three package checks
//! below read the design, the catalogue, the protos and the map, and fail
//! with the difference set, so a rename names the file to fix.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

/// A route: an upper-case HTTP method and an axum path.
type Route = (String, String);

const METHODS: [&str; 7] = ["get", "post", "put", "delete", "patch", "head", "options"];

/// How many `/v1` method+path pairs the as-built router serves (ruling 0.1 in
/// `docs/api/route-map.md`): 27 registered in `api::router` (25, plus the
/// stream and link listings of AP1e Task 19) plus `hot` and `warm`, which
/// `hot::routes()` registers separately and `api::router` merges. `/health`,
/// `/ready` and the `/internal/*` routes are not app routes, so they are not
/// counted. Changing a router without updating this
/// number and ruling 0.1 fails here.
const APP_ROUTE_COUNT: usize = 29;

/// Whether a path is an app route (under `/v1`, but not the console's
/// `/api/v1`).
fn is_app_path(path: &str) -> bool {
    matches!(path.strip_prefix("/v1"), Some(rest) if rest.is_empty() || rest.starts_with('/'))
}

/// The `/v1` method+path pairs of a route set.
fn app_routes(routes: &BTreeSet<Route>) -> BTreeSet<Route> {
    routes
        .iter()
        .filter(|(_, path)| is_app_path(path))
        .cloned()
        .collect()
}

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The source files whose `.route(...)` calls make up the HTTP surface.
fn router_sources() -> Vec<PathBuf> {
    let root = workspace();
    let mut files: Vec<PathBuf> = fs::read_dir(root.join("crates/loams/src/api"))
        .expect("read crates/loams/src/api")
        .map(|entry| entry.expect("dir entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "rs"))
        .collect();
    files.sort();
    files.push(root.join("crates/loams/src/server.rs"));
    files.push(root.join("crates/loams-meta/src/rpc.rs"));
    files
}

/// Files that only define path constants the routers use.
fn constant_sources() -> Vec<PathBuf> {
    vec![workspace().join("crates/loams-hot/src/remote.rs")]
}

/// The non-test, non-comment part of a source file.
fn code(path: &Path) -> String {
    let text = fs::read_to_string(path).unwrap_or_else(|err| panic!("read {path:?}: {err}"));
    let text = match text.find("#[cfg(test)]") {
        Some(at) => &text[..at],
        None => &text[..],
    };
    text.lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `name = "value"` pairs from `const NAME: &str = "…";` and
/// `let name = "…";`.
fn string_bindings(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        let rest = if let Some(rest) = line.strip_prefix("pub const ") {
            rest
        } else if let Some(rest) = line.strip_prefix("const ") {
            rest
        } else if let Some(rest) = line.strip_prefix("let ") {
            rest
        } else {
            continue;
        };
        let name: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        let Some(eq) = rest.find("= \"") else {
            continue;
        };
        let value = &rest[eq + 3..];
        let Some(end) = value.find('"') else { continue };
        if !name.is_empty() {
            out.push((name, value[..end].to_owned()));
        }
    }
    out
}

/// The text between the `(` at `open` and its matching `)`.
fn balanced(text: &str, open: usize) -> &str {
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut in_str = false;
    for (i, &b) in bytes.iter().enumerate().skip(open) {
        match b {
            b'"' if i == 0 || bytes[i - 1] != b'\\' => in_str = !in_str,
            b'(' if !in_str => depth += 1,
            b')' if !in_str => {
                depth -= 1;
                if depth == 0 {
                    return &text[open + 1..i];
                }
            }
            _ => {}
        }
    }
    panic!("unbalanced parentheses after offset {open}");
}

/// Splits a call's arguments at the first top-level comma.
fn first_arg(args: &str) -> (&str, &str) {
    let mut depth = 0i32;
    let mut in_str = false;
    for (i, c) in args.char_indices() {
        match c {
            '"' => in_str = !in_str,
            '(' | '[' | '{' if !in_str => depth += 1,
            ')' | ']' | '}' if !in_str => depth -= 1,
            ',' if !in_str && depth == 0 => return (args[..i].trim(), args[i + 1..].trim()),
            _ => {}
        }
    }
    panic!("no handler argument in .route({args})");
}

fn lookup(bindings: &[(String, String)], name: &str) -> String {
    bindings
        .iter()
        .rev()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.clone())
        .unwrap_or_else(|| panic!("route path uses `{name}`, which is not a string constant"))
}

/// Renders a `format!` string: `{{`/`}}` are braces, `{name}` a binding.
fn render(format: &str, bindings: &[(String, String)]) -> String {
    let mut out = String::new();
    let mut rest = format;
    while let Some(c) = rest.chars().next() {
        if let Some(tail) = rest.strip_prefix("{{") {
            out.push('{');
            rest = tail;
        } else if let Some(tail) = rest.strip_prefix("}}") {
            out.push('}');
            rest = tail;
        } else if c == '{' {
            let end = rest.find('}').expect("closing brace in format string");
            out.push_str(&lookup(bindings, &rest[1..end]));
            rest = &rest[end + 1..];
        } else {
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    out
}

fn route_path(expr: &str, bindings: &[(String, String)]) -> String {
    let expr = expr.trim();
    if let Some(literal) = expr.strip_prefix('"') {
        return literal.trim_end_matches('"').to_owned();
    }
    if let Some(inner) = expr.strip_prefix("&format!(") {
        let inner = inner.trim_end_matches(')').trim();
        let literal = inner.trim_start_matches('"').trim_end_matches('"');
        return render(literal, bindings);
    }
    lookup(bindings, expr.trim_start_matches('&'))
}

/// The HTTP methods a handler expression such as `post(a).get(b)` serves.
fn route_methods(handler: &str) -> Vec<String> {
    let bytes = handler.as_bytes();
    let mut out = Vec::new();
    for method in METHODS {
        let mut from = 0;
        while let Some(at) = handler[from..].find(&format!("{method}(")) {
            let at = from + at;
            let boundary =
                at == 0 || !(bytes[at - 1].is_ascii_alphanumeric() || bytes[at - 1] == b'_');
            if boundary {
                out.push(method.to_ascii_uppercase());
            }
            from = at + method.len();
        }
    }
    out
}

/// Every route in the routers' source.
fn served_routes() -> BTreeSet<Route> {
    let sources = router_sources();
    let mut constants = Vec::new();
    for path in sources.iter().chain(constant_sources().iter()) {
        constants.extend(
            string_bindings(&code(path))
                .into_iter()
                .filter(|(name, _)| name.chars().all(|c| c.is_ascii_uppercase() || c == '_')),
        );
    }
    let mut routes = BTreeSet::new();
    for path in &sources {
        let text = code(path);
        let mut bindings = constants.clone();
        bindings.extend(string_bindings(&text));
        let mut from = 0;
        while let Some(at) = text[from..].find(".route(") {
            let open = from + at + ".route".len();
            let args = balanced(&text, open);
            let (path_expr, handler) = first_arg(args);
            let route = route_path(path_expr, &bindings);
            let methods = route_methods(handler);
            assert!(
                !methods.is_empty(),
                "no method in .route({args}) in {path:?}"
            );
            for method in methods {
                routes.insert((method, route.clone()));
            }
            from = open;
        }
    }
    routes
}

/// Every operation of the console contract.
fn contract_routes() -> BTreeSet<Route> {
    let path = workspace().join("api/console/openapi.json");
    let text = fs::read_to_string(&path).expect("read the console contract");
    let contract: serde_json::Value = serde_json::from_str(&text).expect("contract is JSON");
    let mut routes = BTreeSet::new();
    for (route, item) in contract["paths"].as_object().expect("paths") {
        for method in METHODS {
            if item.get(method).is_some() {
                routes.insert((method.to_ascii_uppercase(), route.clone()));
            }
        }
    }
    routes
}

/// The `| METHOD | `path` | …` rows of the route map.
fn mapped_routes() -> BTreeSet<Route> {
    let path = workspace().join("docs/api/route-map.md");
    let text = fs::read_to_string(&path).expect("read docs/api/route-map.md");
    let mut routes = BTreeSet::new();
    for line in text.lines() {
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        if cells.len() < 4 || !cells[0].is_empty() {
            continue;
        }
        let method = cells[1];
        if !METHODS.iter().any(|m| m.eq_ignore_ascii_case(method))
            || method != method.to_ascii_uppercase()
        {
            continue;
        }
        let Some(route) = cells[2].strip_prefix('`').and_then(|r| r.strip_suffix('`')) else {
            panic!("route-map row without a backticked path: {line}");
        };
        routes.insert((method.to_owned(), route.to_owned()));
    }
    routes
}

#[test]
fn route_map_covers_every_route() {
    let mut actual = served_routes();
    // Sanity: the parser found the routers, not an empty set.
    assert!(actual.contains(&("POST".into(), "/v1/namespaces/{ns}/query".into())));
    assert!(actual.contains(&("POST".into(), "/internal/v1/reads/{op}".into())));
    assert!(actual.contains(&(
        "PUT".into(),
        "/v1/namespaces/{ns}/collections/{c}/hot".into()
    )));
    // The exact count of the app routes, so the coverage check above cannot
    // stay green while ruling 0.1 claims a different number.
    assert_eq!(
        app_routes(&actual).len(),
        APP_ROUTE_COUNT,
        "the routers serve {APP_ROUTE_COUNT} `/v1` method+path pairs; a route was added or removed, so update this number, ruling 0.1 of docs/api/route-map.md and the execution ruling in docs/plans/2026-10-02-api1-unified-connect.md"
    );
    actual.extend(contract_routes());
    let mapped = mapped_routes();
    assert_eq!(
        app_routes(&mapped).len(),
        APP_ROUTE_COUNT,
        "docs/api/route-map.md must list exactly the {APP_ROUTE_COUNT} `/v1` method+path pairs the routers serve, the count ruling 0.1 records"
    );
    let missing: Vec<_> = actual.difference(&mapped).collect();
    let stale: Vec<_> = mapped.difference(&actual).collect();
    assert!(
        missing.is_empty() && stale.is_empty(),
        "docs/api/route-map.md is out of date.\nmissing from the map: {missing:#?}\nmapped but not served: {stale:#?}"
    );
}

#[test]
fn chained_head_and_options_are_inventoried() {
    let methods = route_methods("get(read).head(head_only).options(preflight)");
    assert_eq!(methods, vec!["GET", "HEAD", "OPTIONS"]);
}

// --- The package guard -------------------------------------------------------
//
// A `loams.<concern>.v1` package is the API's unit of compatibility: SDKs and
// `buf breaking` key on it, and §44 §4 advertises it in
// `GetInstance.services[]`. Inventing one is therefore not a cosmetic change,
// so the design is the only thing allowed to enumerate it. The three tests
// below read the same authorisation set from three places that can name a
// package, and one of them (the map) is a negative control that must pass.

/// The design that owns the API's package list: §44 §7.2 is the module
/// catalogue, §44 §8 is the summary of what API1 adds.
const DESIGN: &str = "docs/design/44-unified-api-and-sdks.md";

/// The design document's text.
fn design_text() -> String {
    let path = workspace().join(DESIGN);
    fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {path:?}: {err}"))
}

/// Every `loams.<concern>.v1` token in `text`.
///
/// Deliberately greedy over `[a-z0-9_.]` and then a shape check, rather than
/// a `Service/Method` split: the tokens appear in the design inside
/// backticks, bare in prose, before a `Service` name and before a `/Method`,
/// and the only thing every one of them has in common is the `.v1` suffix.
/// A token that runs into a capitalised `Service` (`loams.collection.v1`
/// followed by `CollectionService`) stops there because the scan does not
/// take upper-case letters, which is why this sees the package and not the
/// whole RPC path.
fn package_tokens(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for (at, _) in text.match_indices("loams.") {
        let name: String = text[at + "loams.".len()..]
            .chars()
            .take_while(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '.' || *c == '_')
            .collect();
        let name = name.trim_end_matches('.');
        let segments: Vec<&str> = name.split('.').collect();
        // `loams.` is already stripped, so a well-formed name is
        // `<concern>.<…>.v1`: at least two segments, the last of which is the
        // version. `loams.collections` (a module name from §7.2's first
        // column) and `loams.v1` are both rejected by this.
        let shaped = segments.len() >= 2
            && segments[segments.len() - 1] == "v1"
            && segments[..segments.len() - 1].iter().all(|s| !s.is_empty());
        if shaped {
            out.insert(format!("loams.{name}"));
        }
    }
    out
}

/// The `loams.instance/devices/…/errors.v1` shorthand of §3's "what exists
/// today" row into the six package names it abbreviates.
///
/// §7.2's row for `loams.instance`, `loams.devices` and `loams.notifications`
/// writes "AP0 packages" in its proto-service column instead of naming them,
/// so the catalogue is not a place a package can be *found*; §3 is, and it
/// writes them as one slash-joined token. Without expanding this the guard
/// would reject the packages AP0 has shipped since the design was written,
/// and a test that fails on six correct names trains its readers to ignore
/// it. The rule that keeps this from becoming a wildcard is that only a cell
/// naming `proto/loams/*` is expanded, and there is exactly one such cell.
fn compressed_packages(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for line in text.lines() {
        if !line.contains("`proto/loams/*`") {
            continue;
        }
        for cell in line.split('|') {
            if cell.contains("proto/loams/*") {
                continue;
            }
            let mut from = 0;
            while let Some(at) = cell[from..].find("loams.") {
                let at = from + at;
                let joined: String = cell[at..]
                    .chars()
                    .take_while(|c| {
                        c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '/')
                    })
                    .collect();
                from = at + joined.len().max("loams.".len());
                if !joined.ends_with(".v1") {
                    continue;
                }
                for part in joined.split('/') {
                    let part = part.strip_prefix("loams.").unwrap_or(part);
                    let part = part.strip_suffix(".v1").unwrap_or(part);
                    if !part.is_empty() {
                        out.insert(format!("loams.{part}.v1"));
                    }
                }
            }
        }
    }
    out
}

/// The packages the design enumerates, from the places it enumerates them.
///
/// Three sources, each located by a marker string rather than a line number
/// so that editing the design cannot silently shrink the set:
///
/// - **§7.2's module catalogue**, the `| Module | Proto service(s) | Status |`
///   table: the line that starts `### 7.2` up to the next heading, keeping the
///   table rows. Only rows, so the paragraph after the table ("Not in the
///   public protos or SDKs: `loams.internal.v1`") cannot authorise by
///   appearing in a negation.
/// - **§8's `New protos:` sentence**, the summary of what API1 introduces.
/// - **§3's "what exists today" table row** naming `proto/loams/*`, the AP0
///   packages §7.2 delegates to by name. See [`compressed_packages`].
fn authorised_packages() -> BTreeSet<String> {
    let text = design_text();
    let lines: Vec<&str> = text.lines().collect();
    let mut out = BTreeSet::new();

    let Some(start) = lines.iter().position(|line| line.starts_with("### 7.2")) else {
        panic!("{DESIGN} has no `### 7.2` module-catalogue heading to read packages from");
    };
    let end = lines[start + 1..]
        .iter()
        .position(|line| line.starts_with("### "))
        .map_or(lines.len(), |at| start + 1 + at);
    for line in &lines[start..end] {
        if line.starts_with('|') {
            out.extend(package_tokens(line));
        }
    }

    let summaries = lines
        .iter()
        .filter(|line| line.contains("New protos:"))
        .collect::<Vec<_>>();
    assert_eq!(
        summaries.len(),
        1,
        "expected exactly one `New protos:` sentence in {DESIGN}, found {} — \
         the design's package list moved and the guard needs re-reading, not loosening",
        summaries.len()
    );
    out.extend(package_tokens(summaries[0]));

    let existing = lines
        .iter()
        .filter(|line| line.contains("`proto/loams/*`"))
        .count();
    assert_eq!(
        existing, 1,
        "expected exactly one `proto/loams/*` row in {DESIGN} to expand the AP0 \
         package shorthand, found {existing}"
    );
    out.extend(compressed_packages(&text));
    out
}

/// The `package: "…"` entries of the `CATALOGUE` const in
/// `crates/loams/src/api/connect.rs`, read as text rather than by importing
/// the generated types, so the check is about what the source says.
///
/// `code()` drops the comment lines, so the `package: "…"` example in the
/// `Package` struct's doc comment cannot be counted as a catalogue entry, and
/// `package: entry.package.to_owned()` in `statuses()` is not a string literal
/// so it cannot be counted either.
fn catalogue_packages() -> BTreeSet<String> {
    let path = workspace().join("crates/loams/src/api/connect.rs");
    let text = code(&path);
    let start = text
        .find("const CATALOGUE")
        .unwrap_or_else(|| panic!("no CATALOGUE const in {path:?}"));
    let block = &text[start..];
    let end = block
        .find("\n];")
        .expect("CATALOGUE is a `];`-terminated slice");
    let mut out = BTreeSet::new();
    for line in block[..end].lines() {
        let Some(rest) = line.trim().strip_prefix("package: \"") else {
            continue;
        };
        let value = rest.split('"').next().expect("a package string ends");
        out.insert(value.to_owned());
    }
    assert!(
        !out.is_empty(),
        "no `package: \"…\"` entry parsed out of CATALOGUE in {path:?}"
    );
    out
}

/// Every `.proto` under `proto/`, recursively.
///
/// Hand-rolled rather than a glob crate: the crate has no walkdir dependency,
/// and the guard should not add one to read a tree of forty files.
fn proto_sources(dir: &Path, into: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(dir).unwrap_or_else(|err| panic!("read {dir:?}: {err}"));
    let mut paths: Vec<PathBuf> = entries
        .map(|entry| entry.expect("dir entry").path())
        .filter(|path| path.is_file() && path.extension().is_some_and(|ext| ext == "proto"))
        .collect();
    paths.sort();
    into.append(&mut paths);
    for entry in fs::read_dir(dir).expect("read proto dir").flatten() {
        let path = entry.path();
        if path.is_dir() {
            proto_sources(&path, into);
        }
    }
}

/// Every `package loams.<concern>.v1;` declaration in `proto/`, with the file
/// that declares it.
///
/// Matched loosely, because this repo writes the declaration at the **bottom**
/// of the file and indents it (`document.proto:116`, `collection.proto:85`),
/// which is the reverse of what a `^package` anchor expects.
fn declared_packages() -> BTreeMap<String, PathBuf> {
    let root = workspace().join("proto");
    let mut files = Vec::new();
    proto_sources(&root, &mut files);
    assert!(!files.is_empty(), "no .proto files found under {root:?}");
    let mut out = BTreeMap::new();
    for path in &files {
        let text = fs::read_to_string(path).expect("read a .proto file");
        for line in text.lines() {
            let Some(rest) = line.trim().strip_prefix("package ") else {
                continue;
            };
            let declared = rest.trim().trim_end_matches(';').split_whitespace().next();
            let Some(name) = declared else { continue };
            out.insert(
                name.to_owned(),
                path.strip_prefix(workspace()).unwrap_or(path).to_owned(),
            );
        }
    }
    out
}

/// The map's column-3 RPC for every mapped route, keyed by `(METHOD, path)`.
///
/// `mapped_routes()` reads columns 1–2, which is all the method/path coverage
/// check needs. The RPC is a different column and the column's index is taken
/// from the table's own `| Method | Route | Kind | Target RPC | …` header
/// rather than hard-coded, because the map repeats that header once per
/// section and its column count is not a constant of the file.
fn mapped_rpcs() -> BTreeMap<(String, String), String> {
    let path = workspace().join("docs/api/route-map.md");
    let text = fs::read_to_string(&path).expect("read docs/api/route-map.md");
    let mut rpc_column = None;
    let mut out = BTreeMap::new();
    for line in text.lines() {
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        if cells.len() < 4 || !cells[0].is_empty() {
            continue;
        }
        if cells[1] == "Method" {
            rpc_column = cells
                .iter()
                .position(|cell| *cell == "Target RPC")
                .filter(|at| *at > 0);
            continue;
        }
        let method = cells[1];
        if !METHODS.iter().any(|m| m.eq_ignore_ascii_case(method))
            || method != method.to_ascii_uppercase()
        {
            continue;
        }
        let (Some(column), Some(route)) = (rpc_column, cells.get(2)) else {
            continue;
        };
        let route = route
            .trim_start_matches('`')
            .trim_end_matches('`')
            .to_owned();
        out.insert(
            (method.to_owned(), route),
            cells.get(column).copied().unwrap_or_default().to_owned(),
        );
    }
    out
}

/// The assertion every package check makes, spelled once so the three tests
/// report the same way.
///
/// The message names the offending package *and* the file that has to change,
/// because the fix is a rename in exactly one of them and the difference set
/// on its own sends the reader hunting.
fn assert_enumerated(found: &BTreeSet<String>, authorised: &BTreeSet<String>, fix_in: &str) {
    let unauthorised: Vec<&String> = found.difference(authorised).collect();
    assert!(
        unauthorised.is_empty(),
        "{unauthorised:#?} — these packages are not enumerated by the design \
         (docs/design/44-unified-api-and-sdks.md §7.2 module catalogue or §8 \
         `New protos:`). Fix {fix_in}: use the package the design names for \
         this service, or add the package to §7.2 in the same change that \
         introduces it. Do not invent a package the design does not list; \
         §7.2 puts several services in one package, so a new concern is \
         usually a new service in an existing one.\nenumerated: {authorised:#?}"
    );
}

/// The catalogue may only advertise packages the design enumerates.
///
/// `CATALOGUE` is what `GetInstance.services[]` and the health service report,
/// so an unlisted package here is advertised to every SDK on first call. This
/// is the check that was missing when Task 3 shipped `loams.document.v1`.
#[test]
fn the_catalogue_declares_only_packages_the_design_enumerates() {
    let catalogue = catalogue_packages();
    let authorised = authorised_packages();
    assert_enumerated(&catalogue, &authorised, "crates/loams/src/api/connect.rs");
}

/// No proto file may declare a package the design does not enumerate.
///
/// Same rule as the catalogue, one level down: the protos are what `buf`
/// generates and what `buf breaking` protects, so an undeclared package there
/// is a second, wire-visible surface the design never agreed to.
/// The packages of `declared` that the design does not enumerate, with the
/// `.proto` that declares each. Separate from [`assert_enumerated`] because
/// the proto fix has to name a file per package, not one directory.
#[test]
fn no_proto_declares_a_package_the_design_does_not_enumerate() {
    let declared = declared_packages();
    let authorised = authorised_packages();
    let unauthorised: Vec<(&String, &PathBuf)> = declared
        .iter()
        .filter(|(package, _)| !authorised.contains(*package))
        .collect();
    assert!(
        unauthorised.is_empty(),
        "{:#?} — these packages are not enumerated by the design \
         (docs/design/44-unified-api-and-sdks.md §7.2 module catalogue or §8 \
         `New protos:`). Fix the named .proto, and the matching CATALOGUE entry \
         in crates/loams/src/api/connect.rs, to use the package the design \
         names for this service; or add the package to §7.2 in the same change \
         that introduces it. Do not invent a package the design does not list — \
         §7.2 puts several services in one package, so a new concern is \
         usually a new service in an existing one.\nenumerated: {authorised:#?}",
        unauthorised
            .iter()
            .map(|(package, file)| (package, file))
            .collect::<Vec<_>>()
    );
}

/// The route map's RPCs may only name packages the design enumerates, and the
/// query route's RPC is pinned.
///
/// This one passes today, on purpose: it is the negative control. A guard that
/// failed here too would be indistinguishable from a parser that finds
/// nothing, and a check that finds nothing passes vacuously — which is how the
/// original defect survived a green suite. The pins below keep that risk
/// visible: the authorisation set must be non-empty and must contain the
/// package these rows are about, enough RPC cells must have parsed, and the
/// query route's column-3 RPC must be exactly
/// `loams.collection.v1.QueryService/Search` (route-map line 57).
#[test]
fn the_route_maps_rpc_paths_name_enumerated_packages() {
    let authorised = authorised_packages();
    assert!(
        authorised.len() > 10,
        "the design's package set parsed as {} entries; the §7.2/§8 markers \
         moved and this guard is reading nothing",
        authorised.len()
    );
    for expected in [
        "loams.collection.v1",
        "loams.instance.v1",
        "loams.internal.v1",
        "loams.live.v1",
        "loams.sql.v1",
    ] {
        assert!(
            authorised.contains(expected),
            "{expected} is absent from the design's enumerated packages {authorised:#?}; \
             a marker in {DESIGN} moved"
        );
    }

    let rpcs = mapped_rpcs();
    assert!(
        rpcs.len() > 20,
        "only {} RPC cells parsed out of docs/api/route-map.md; the \
         `Target RPC` header was not found, so the check below would pass \
         vacuously",
        rpcs.len()
    );
    let mut named: BTreeSet<String> = BTreeSet::new();
    for rpc in rpcs.values() {
        named.extend(package_tokens(rpc));
    }

    assert!(
        named.len() > 4,
        "the map's RPC cells name only {named:#?}; the column index moved and \
         the check below would pass vacuously"
    );
    assert_enumerated(
        &named,
        &authorised,
        "docs/api/route-map.md, then the proto package it names",
    );

    assert_eq!(
        rpcs.get(&("POST".to_owned(), "/v1/namespaces/{ns}/query".to_owned()))
            .map(String::as_str),
        Some("`loams.collection.v1.QueryService/Search`"),
        "the query route's Target RPC cell is not the one docs/api/route-map.md \
         line 57 records; §7.2 puts `QueryService/Search` in `loams.collection.v1`, \
         so a query RPC in any other package is the Task 3 mistake again"
    );
}
