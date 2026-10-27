//! Static checks on `conformance/clickhouse/apps/` (plan HE1, Task 1; Review
//! Focus 4): every mined application records its licence before it runs, and
//! every image, in the manifest and in each compose file, is pinned by digest.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema: u32,
    clickhouse: Oracle,
    #[serde(default)]
    app: Vec<App>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Oracle {
    oracle: String,
    floor: String,
    licence: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct App {
    name: String,
    image: String,
    #[serde(default)]
    images: Vec<String>,
    #[serde(default)]
    licence: Option<String>,
    clickhouse_protocol: String,
    #[serde(default)]
    migrations_protocol: Option<String>,
    #[serde(default)]
    migrations: Option<String>,
    #[serde(default)]
    clickhouse_min: Option<String>,
    driver: String,
    #[serde(default)]
    suite: String,
    runtime_minutes: u32,
    #[serde(default)]
    gate: bool,
}

#[derive(Debug, Deserialize)]
struct Compose {
    services: BTreeMap<String, Service>,
}

#[derive(Debug, Deserialize)]
struct Service {
    image: Option<String>,
    #[serde(default)]
    labels: BTreeMap<String, String>,
}

fn apps_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../conformance/clickhouse/apps")
}

fn load_manifest(text: &str) -> Result<Manifest, String> {
    toml::from_str(text).map_err(|e| e.to_string())
}

fn manifest() -> Manifest {
    let path = apps_dir().join("manifest.toml");
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    load_manifest(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// SPDX licence identifiers an application may carry here (a subset of the
/// SPDX list; add one when a new application needs it). AGPL applications run
/// only as unmodified containers (§75 §18).
const SPDX: &[&str] = &[
    "AGPL-3.0-only",
    "AGPL-3.0-or-later",
    "Apache-2.0",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "MIT",
    "MPL-2.0",
    "PostgreSQL",
    "curl",
    // Langfuse's ee/ directories (the image carries them; unused without a key).
    "LicenseRef-Langfuse-EE",
];

/// A licence is an SPDX expression of known identifiers joined by AND/OR.
fn licence_problem(licence: Option<&str>) -> Option<String> {
    let Some(licence) = licence.map(str::trim).filter(|l| !l.is_empty()) else {
        return Some("no licence recorded".into());
    };
    let unknown: Vec<&str> = licence
        .split([' ', '(', ')'])
        .filter(|t| !t.is_empty() && *t != "AND" && *t != "OR")
        .filter(|t| !SPDX.contains(t))
        .collect();
    (!unknown.is_empty()).then(|| format!("unknown SPDX identifiers {unknown:?} in {licence:?}"))
}

/// `<repo>:<tag>@sha256:<64 lowercase hex>`. A compose `${VAR:-default}`
/// reference is checked on its default, which is what runs unless overridden.
fn pin_problem(reference: &str) -> Option<String> {
    let reference = match reference.strip_prefix("${") {
        Some(rest) => match rest.strip_suffix('}').and_then(|r| r.split_once(":-")) {
            Some((_, default)) => default,
            None => return Some(format!("{reference}: a variable with no pinned default")),
        },
        None => reference,
    };
    let Some((name, digest)) = reference.split_once("@sha256:") else {
        return Some(format!("{reference}: not pinned by digest"));
    };
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    {
        return Some(format!("{reference}: the digest is not 64 lowercase hex"));
    }
    let tag = name.rsplit_once('/').map_or(name, |(_, last)| last);
    match tag.split_once(':') {
        Some((_, t)) if !t.is_empty() && t != "latest" => None,
        _ => Some(format!(
            "{reference}: needs a release tag beside the digest"
        )),
    }
}

fn compose(app: &str) -> Compose {
    let path = apps_dir().join(app).join("compose.yaml");
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_norway::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
fn apps_manifest_requires_licence() {
    let m = manifest();
    assert_eq!(m.schema, 1);
    assert!(!m.app.is_empty(), "the manifest lists no applications");
    let mut problems = Vec::new();
    if let Some(p) = licence_problem(Some(&m.clickhouse.licence)) {
        problems.push(format!("[clickhouse]: {p}"));
    }
    for app in &m.app {
        if let Some(p) = licence_problem(app.licence.as_deref()) {
            problems.push(format!("{}: {p}", app.name));
        }
        // Every service the app's compose file runs carries its licence too.
        for (service, s) in compose(&app.name).services {
            let label = s.labels.get("io.loams.licence").map(String::as_str);
            if let Some(p) = licence_problem(label) {
                problems.push(format!("{}/compose.yaml service {service}: {p}", app.name));
            }
        }
    }
    assert!(problems.is_empty(), "{problems:#?}");

    // The check refuses what it must refuse.
    let base = r#"
        schema = 1
        [clickhouse]
        oracle = "x"
        floor = "x"
        licence = "Apache-2.0"
        [[app]]
        name = "a"
        image = "x"
        clickhouse_protocol = "http"
        driver = "a/driver.sh"
        runtime_minutes = 1
    "#;
    let missing = load_manifest(base).expect("parses");
    assert!(licence_problem(missing.app[0].licence.as_deref()).is_some());
    let empty = load_manifest(&format!("{base}licence = \"  \"\n")).expect("parses");
    assert!(licence_problem(empty.app[0].licence.as_deref()).is_some());
    assert!(licence_problem(Some("Proprietary")).is_some());
    assert!(licence_problem(Some("MIT OR Apache-2.0")).is_none());
}

#[test]
fn apps_manifest_pins_by_digest() {
    let m = manifest();
    let mut problems: Vec<String> = [&m.clickhouse.oracle, &m.clickhouse.floor]
        .into_iter()
        .filter_map(|r| pin_problem(r))
        .collect();
    let mut names = std::collections::BTreeSet::new();
    for app in &m.app {
        assert!(names.insert(app.name.clone()), "{}: listed twice", app.name);
        assert!(
            matches!(app.clickhouse_protocol.as_str(), "http" | "native"),
            "{}: clickhouse_protocol {:?}",
            app.name,
            app.clickhouse_protocol
        );
        if let Some(p) = &app.migrations_protocol {
            assert!(
                matches!(p.as_str(), "http" | "native"),
                "{}: migrations_protocol {p:?}",
                app.name
            );
        }
        assert!(app.runtime_minutes > 0, "{}: runtime_minutes", app.name);
        assert!(
            apps_dir().join(&app.driver).is_file(),
            "{}: no driver {}",
            app.name,
            app.driver
        );
        let _ = (&app.migrations, &app.clickhouse_min, &app.suite, app.gate);

        let declared: Vec<&String> = std::iter::once(&app.image).chain(&app.images).collect();
        problems.extend(
            declared
                .iter()
                .filter_map(|r| pin_problem(r))
                .map(|p| format!("{}: {p}", app.name)),
        );

        let services = compose(&app.name).services;
        let used: Vec<String> = services.values().filter_map(|s| s.image.clone()).collect();
        for (service, s) in &services {
            match &s.image {
                Some(image) => problems.extend(
                    pin_problem(image)
                        .map(|p| format!("{}/compose.yaml service {service}: {p}", app.name)),
                ),
                None => problems.push(format!(
                    "{}/compose.yaml service {service}: builds an image",
                    app.name
                )),
            }
        }
        // What the manifest declares is what the compose file runs, and the
        // compose file's clickhouse-server is the oracle by default.
        for image in declared {
            if !used.contains(image) {
                problems.push(format!("{}: {image} is not in its compose.yaml", app.name));
            }
        }
        if !used.iter().any(|u| u.contains(&m.clickhouse.oracle)) {
            problems.push(format!(
                "{}: compose.yaml does not run the oracle by default",
                app.name
            ));
        }
    }
    assert!(problems.is_empty(), "{problems:#?}");

    // The check refuses what it must refuse.
    let digest = "0".repeat(64);
    assert!(pin_problem("docker.io/clickhouse/clickhouse-server:26.9").is_some());
    assert!(pin_problem(&format!("docker.io/x/y@sha256:{digest}")).is_some());
    assert!(pin_problem(&format!("docker.io/x/y:latest@sha256:{digest}")).is_some());
    assert!(pin_problem("docker.io/x/y:1@sha256:abc").is_some());
    assert!(pin_problem("${IMAGE}").is_some());
    assert!(pin_problem("${IMAGE:-docker.io/x/y:1}").is_some());
    assert!(pin_problem(&format!("localhost:5000/x/y:1@sha256:{digest}")).is_none());
    assert!(pin_problem(&format!("${{IMAGE:-docker.io/x/y:1@sha256:{digest}}}")).is_none());
}
