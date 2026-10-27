//! MS1 Task 6 (D1808): the `node` runtime's engine, `node` or `bun`. The
//! install path is the same scripts-off npm path for both; only the program
//! that runs the entrypoint changes, and a missing Bun is refused before
//! anything is written.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use loams_agentd_marketplace::SigstoreVerifier;
use loams_agentd_marketplace::contract::{self, Provisioned};
use loams_agentd_marketplace::engine::{
    self, BUN_PINNED, Engine, EngineError, EngineProbe, PathProbe, ResolvedEngine,
};
use loams_agentd_marketplace::install::{self, Context, InstallError, Installed, Layout, Npm};
use loams_agentd_marketplace::listing as schema;
use semver::Version;
use serde_json::{Value, json};

fn read(name: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("testdata/npm")
            .join(name),
    )
    .unwrap()
}

/// An agent listing for the real sigstore@3.1.0 tarball, on `engine`.
fn listing(engine: Option<&str>) -> Value {
    let integrity = String::from_utf8(read("sigstore-3.1.0.integrity"))
        .unwrap()
        .trim()
        .to_owned();
    let mut runtime = json!({ "kind": "node" });
    if let Some(engine) = engine {
        runtime["engine"] = engine.into();
        if engine == "bun" {
            runtime["version"] = ">=1.4.4".into();
        }
    }
    json!({
        "id": "ostrium/sigstore-agent",
        "kind": "agent",
        "version": "3.1.0",
        "title": "Sigstore agent",
        "source": { "repository": "https://github.com/sigstore/sigstore-js", "commit": "0".repeat(40) },
        "artifact": { "type": "npm", "name": "sigstore", "version": "3.1.0", "integrity": integrity },
        "provenance": { "type": "npm-slsa", "ref": "https://registry.npmjs.org/-/npm/v1/attestations/sigstore@3.1.0" },
        "runtime": runtime,
        "entrypoint": "dist/index.js",
        "pricing": "free"
    })
}

fn fixture(name: &str) -> Value {
    serde_json::from_slice(
        &std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../schemas/marketplace/testdata/valid")
                .join(name),
        )
        .unwrap(),
    )
    .unwrap()
}

/// Engines found at fixed paths and versions.
#[derive(Default)]
struct FakeProbe {
    found: BTreeMap<&'static str, (PathBuf, &'static str)>,
}

impl FakeProbe {
    fn with(mut self, program: &'static str, path: &str, version: &'static str) -> Self {
        self.found.insert(program, (PathBuf::from(path), version));
        self
    }
}

impl EngineProbe for FakeProbe {
    fn find(&self, program: &str) -> Option<PathBuf> {
        self.found.get(program).map(|(path, _)| path.clone())
    }
    fn version(&self, program: &Path) -> Option<Version> {
        self.found
            .values()
            .find(|(path, _)| path == program)
            .and_then(|(_, v)| Version::parse(v).ok())
    }
}

fn both() -> FakeProbe {
    FakeProbe::default()
        .with("node", "/opt/node-24/bin/node", "24.21.0")
        .with("bun", "/opt/bun-1.4.4/bin/bun", BUN_PINNED)
}

/// Lays out node_modules as npm would, recording that it ran.
struct FakeNpm {
    files: Vec<(&'static str, &'static str)>,
    calls: Mutex<u32>,
}

impl FakeNpm {
    fn new(files: Vec<(&'static str, &'static str)>) -> Self {
        Self {
            files,
            calls: Mutex::new(0),
        }
    }
    fn clean() -> Self {
        Self::new(vec![
            (
                "node_modules/sigstore/package.json",
                r#"{"name":"sigstore","version":"3.1.0"}"#,
            ),
            ("node_modules/sigstore/dist/index.js", "export {};"),
        ])
    }
}

impl Npm for FakeNpm {
    fn install(&self, tarball: &Path, prefix: &Path) -> Result<(), String> {
        assert!(tarball.is_file());
        *self.calls.lock().unwrap() += 1;
        for (path, text) in &self.files {
            let path = prefix.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        Ok(())
    }
}

fn context() -> Context {
    Context {
        source: "official".into(),
        plan_digest: "ab".repeat(32),
        now_ms: 1_800_000_000_000,
    }
}

fn install_with(
    layout: &Layout,
    listing: &Value,
    npm: &FakeNpm,
    probe: &dyn EngineProbe,
) -> Result<(Installed, ResolvedEngine), InstallError> {
    let verifier = SigstoreVerifier::production().unwrap();
    install::install_node_with_engine(
        layout,
        listing,
        &read("sigstore-3.1.0.tgz"),
        &[read("sigstore-3.1.0.slsa.sigstore.json")],
        &verifier,
        npm,
        probe,
        &context(),
    )
}

#[test]
fn engine_defaults_to_node() {
    assert_eq!(Engine::of(&listing(None)), Some(Engine::Node));
    assert_eq!(Engine::of(&listing(Some("node"))), Some(Engine::Node));
    assert_eq!(Engine::of(&listing(Some("bun"))), Some(Engine::Bun));
    assert_eq!(
        Engine::of(&fixture("agent-mastra-acp.json")),
        Some(Engine::Node)
    );
    assert_eq!(
        Engine::of(&fixture("agent-mastra-bun.json")),
        Some(Engine::Bun)
    );
    // Only the node runtime has an engine.
    assert_eq!(Engine::of(&fixture("agent-pydantic-ai-a2a.json")), None);

    // With no range, node needs 22.13 or later.
    let (engine, range, _) = engine::required(&listing(None)).unwrap();
    assert_eq!((engine, range.as_str()), (Engine::Node, ">=22.13.0"));
    let resolved = engine::resolve(&listing(None), &both()).unwrap();
    assert_eq!(resolved.program, Path::new("/opt/node-24/bin/node"));

    // A record written before D1808 has no engine and means node.
    let old: Installed = serde_json::from_value(json!({
        "id": "ostrium/x", "version": "1.0.0", "source": "official", "kind": "agent",
        "installed_ms": 1, "plan_digest": "", "entry": "/a/b.js", "built_by": ""
    }))
    .unwrap();
    assert_eq!(old.engine, None);
    assert_eq!(old.engine(), Engine::Node);
    assert!(
        !serde_json::to_string(&old).unwrap().contains("engine"),
        "a record without an engine is written as before"
    );
}

#[test]
fn bun_engine_launches_bun() {
    let installed = Installed {
        id: "ostrium/sigstore-agent".into(),
        version: "3.1.0".into(),
        source: "official".into(),
        kind: "agent".into(),
        installed_ms: 1,
        plan_digest: String::new(),
        entry: "/data/agents/ostrium/sigstore-agent/3.1.0/node_modules/sigstore/dist/index.js"
            .into(),
        built_by: String::new(),
        engine: Some("bun".into()),
    };
    let bun = engine::resolve(&listing(Some("bun")), &both()).unwrap();
    assert_eq!(bun.engine, Engine::Bun);
    assert_eq!(bun.version, Version::parse(BUN_PINNED).unwrap());
    let launch = engine::launch(&installed, &bun);
    assert_eq!(launch.program, "/opt/bun-1.4.4/bin/bun");
    assert_eq!(
        launch.args,
        ["--no-install", "--no-env-file", installed.entry.as_str()]
    );

    let node = engine::resolve(&listing(None), &both()).unwrap();
    let launch = engine::launch(&installed, &node);
    assert_eq!(launch.program, "/opt/node-24/bin/node");
    assert_eq!(launch.args, [installed.entry.as_str()]);

    // The environment contract is unchanged: the engine's directory is the
    // only PATH entry, and no Bun or Node variable is added.
    let provisioned = |runtime_bin| Provisioned {
        endpoint: "http://127.0.0.1:7000".into(),
        namespace: "ns".into(),
        data_dir: "/data/agents/ostrium/sigstore-agent/data".into(),
        token_file: "/run/token".into(),
        runtime_bin,
        ..Provisioned::default()
    };
    let on_bun = contract::environment(&listing(Some("bun")), &provisioned(bun.bin_dir()));
    let on_node = contract::environment(&listing(None), &provisioned(node.bin_dir()));
    assert_eq!(on_bun["PATH"], "/opt/bun-1.4.4/bin");
    assert_eq!(on_node["PATH"], "/opt/node-24/bin");
    assert_eq!(
        on_bun.keys().collect::<Vec<_>>(),
        on_node.keys().collect::<Vec<_>>()
    );

    // An ACP harness on the node runtime may declare bun too.
    let mut harness = listing(Some("bun"));
    harness["kind"] = "harness".into();
    harness["harness"] = json!({ "launch": { "protocol": "acp", "args": ["--acp"] } });
    let registration = install::harness_registration(&harness, &installed, &bun.program)
        .unwrap()
        .unwrap();
    assert_eq!(registration.program, "/opt/bun-1.4.4/bin/bun");
    assert_eq!(
        registration.args,
        [
            "--no-install",
            "--no-env-file",
            installed.entry.as_str(),
            "--acp"
        ]
    );
}

#[test]
fn bun_missing_is_refused() {
    let only_node = FakeProbe::default().with("node", "/opt/node-24/bin/node", "24.21.0");
    let error = engine::resolve(&listing(Some("bun")), &only_node).unwrap_err();
    assert_eq!(
        error,
        EngineError::NotInstalled {
            engine: Engine::Bun,
            required: ">=1.4.4".into()
        }
    );
    assert_eq!(
        error.to_string(),
        "this agent runs on Bun >=1.4.4, and Bun is not installed"
    );

    // A program that is there but does not answer `--version` is not Bun.
    let broken = FakeProbe::default().with("bun", "/opt/bun/bin/bun", "not a version");
    assert!(matches!(
        engine::resolve(&listing(Some("bun")), &broken),
        Err(EngineError::NotInstalled { .. })
    ));

    // The install is refused before anything is written or npm runs.
    let dir = tempfile::tempdir().unwrap();
    let layout = Layout::new(dir.path().join("agents"));
    let npm = FakeNpm::clean();
    let error = install_with(&layout, &listing(Some("bun")), &npm, &only_node).unwrap_err();
    assert!(
        matches!(
            error,
            InstallError::Engine(EngineError::NotInstalled {
                engine: Engine::Bun,
                ..
            })
        ),
        "{error}"
    );
    assert_eq!(*npm.calls.lock().unwrap(), 0);
    assert!(!dir.path().join("agents").exists());

    // The real probe finds nothing in an empty directory.
    let empty = tempfile::tempdir().unwrap();
    let probe = PathProbe {
        dirs: vec![empty.path().to_owned()],
    };
    assert!(matches!(
        engine::resolve(&listing(Some("bun")), &probe),
        Err(EngineError::NotInstalled { .. })
    ));
}

#[test]
fn bun_too_old_is_refused() {
    let old = FakeProbe::default().with("bun", "/opt/bun/bin/bun", "1.3.9");
    let error = engine::resolve(&listing(Some("bun")), &old).unwrap_err();
    assert_eq!(
        error,
        EngineError::TooOld {
            engine: Engine::Bun,
            found: "1.3.9".into(),
            required: ">=1.4.4".into()
        }
    );
    assert_eq!(
        error.to_string(),
        "this agent runs on Bun >=1.4.4, and the installed Bun is 1.3.9"
    );
    let old_node = FakeProbe::default().with("node", "/usr/bin/node", "20.18.0");
    assert!(matches!(
        engine::resolve(&listing(None), &old_node),
        Err(EngineError::TooOld {
            engine: Engine::Node,
            ..
        })
    ));
}

#[test]
fn node_range_applies_to_node_only() {
    // A Bun listing with no range needs the pinned Bun, whatever Node is.
    let mut no_range = listing(Some("bun"));
    no_range["runtime"]
        .as_object_mut()
        .unwrap()
        .remove("version");
    let (engine, range, _) = engine::required(&no_range).unwrap();
    assert_eq!((engine, range.as_str()), (Engine::Bun, ">=1.4.4"));
    let probe = FakeProbe::default()
        .with("node", "/usr/bin/node", "18.0.0")
        .with("bun", "/opt/bun/bin/bun", "1.4.4");
    assert_eq!(
        engine::resolve(&no_range, &probe).unwrap().engine,
        Engine::Bun
    );
    // A Python listing has no engine to resolve.
    assert_eq!(
        engine::resolve(&fixture("agent-pydantic-ai-a2a.json"), &probe),
        Err(EngineError::NotNode)
    );
}

#[test]
fn bun_install_runs_no_scripts() {
    // The npm command line is the one agentd runs for both engines.
    let args = install::npm_args(Path::new("/tmp/p.tgz"), Path::new("/tmp/prefix"));
    assert!(args.iter().any(|a| a == "--ignore-scripts"));

    // A Bun listing installs through the same verified, scripts-off path.
    let dir = tempfile::tempdir().unwrap();
    let layout = Layout::new(dir.path().join("agents"));
    let (record, resolved) =
        install_with(&layout, &listing(Some("bun")), &FakeNpm::clean(), &both()).unwrap();
    assert_eq!(resolved.engine, Engine::Bun);
    assert_eq!(record.engine.as_deref(), Some("bun"));
    assert_eq!(layout.installed("ostrium/sigstore-agent").unwrap(), record);
    assert!(
        record
            .built_by
            .starts_with("https://github.com/sigstore/sigstore-js/"),
        "Sigstore provenance was verified"
    );

    // And a dependency that runs code at install is refused, as on node.
    for (path, text) in [
        (
            "node_modules/evil-dep/package.json",
            r#"{"name":"evil-dep","scripts":{"postinstall":"bun steal.ts"}}"#,
        ),
        ("node_modules/native-dep/binding.gyp", "{}"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::new(dir.path().join("agents"));
        let mut files = FakeNpm::clean().files;
        files.push((path, text));
        let error = install_with(
            &layout,
            &listing(Some("bun")),
            &FakeNpm::new(files),
            &both(),
        )
        .unwrap_err();
        assert!(
            matches!(error, InstallError::InstallScript { .. }),
            "{error}"
        );
        assert!(layout.list().is_empty());
    }
}

#[test]
fn engine_schema_rules() {
    schema::validate(&fixture("agent-mastra-bun.json")).unwrap();
    let mut python = fixture("agent-pydantic-ai-a2a.json");
    python["runtime"]["engine"] = "bun".into();
    assert!(schema::validate(&python).is_err());
    let mut deno = fixture("agent-mastra-bun.json");
    deno["runtime"]["engine"] = "deno".into();
    assert!(schema::validate(&deno).is_err());
}

#[test]
fn versions_parse() {
    assert_eq!(
        engine::parse_version("v24.21.0\n"),
        Some(Version::new(24, 21, 0))
    );
    assert_eq!(
        engine::parse_version("1.4.4\n"),
        Some(Version::new(1, 4, 4))
    );
    assert_eq!(engine::parse_version(""), None);
}
