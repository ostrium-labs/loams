//! Installers (design §73 §3.2, §3.7, §7.2, §10.4; D1464, D1473; MK1 Task 6):
//! `node` agents from npm, harness descriptors and MCP servers.
//!
//! Layout under `<agentd data>/agents/<namespace>/<name>/`:
//! - `<version>/`: the installed package, replaced whole on update;
//! - `data/`: the agent's own files (HOME, TMPDIR, LOAMS_DATA_DIR), never
//!   deleted by uninstall or revocation;
//! - `installed.json`: what is installed, from where, under which plan.
//!
//! No code runs at install: npm runs with scripts off, and a package (or any
//! dependency) that declares an install script, or needs a native build, is
//! refused and removed before anything is registered.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::artifact::{ArtifactFetch, ArtifactSources, FetchError, Origin};
use crate::engine::{self, Engine, EngineError, EngineProbe, ResolvedEngine};
use crate::provenance::{self, ProvenanceError};
use crate::verify::SignatureVerifier;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InstallError {
    #[error(transparent)]
    Provenance(#[from] ProvenanceError),
    #[error(transparent)]
    Fetch(#[from] FetchError),
    #[error(transparent)]
    Engine(#[from] EngineError),
    #[error("{package} runs code at install ({script}); Loams installs run no code (D1473)")]
    InstallScript { package: String, script: String },
    #[error(
        "{0} has dependencies but ships no npm-shrinkwrap.json, so its install is not reproducible"
    )]
    NoLockfile(String),
    #[error("npm failed: {0}")]
    Npm(String),
    #[error("the entrypoint {0} is not in the package")]
    NoEntrypoint(String),
    #[error("the listing is not installable here: {0}")]
    Unsupported(String),
    #[error("{0} is not installed")]
    NotInstalled(String),
    #[error("{path}: {error}")]
    Io { path: String, error: String },
}

fn io(path: &Path) -> impl Fn(std::io::Error) -> InstallError + '_ {
    move |e| InstallError::Io {
        path: path.display().to_string(),
        error: e.to_string(),
    }
}

/// Runs npm. agentd's implementation spawns
/// `npm install --ignore-scripts --omit=dev --no-audit --no-fund --prefix <prefix> <tarball>`
/// with the cleared environment of the contract; tests lay files out themselves.
pub trait Npm: Send + Sync {
    fn install(&self, tarball: &Path, prefix: &Path) -> Result<(), String>;
}

/// The npm command line agentd uses, for its `Npm` implementation and docs.
pub fn npm_args(tarball: &Path, prefix: &Path) -> Vec<String> {
    vec![
        "install".into(),
        "--ignore-scripts".into(),
        "--omit=dev".into(),
        "--no-audit".into(),
        "--no-fund".into(),
        "--no-save".into(),
        "--prefix".into(),
        prefix.display().to_string(),
        tarball.display().to_string(),
    ]
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Installed {
    pub id: String,
    pub version: String,
    pub source: String,
    pub kind: String,
    pub installed_ms: i64,
    /// The plan the person confirmed.
    pub plan_digest: String,
    /// What to start: an absolute path, or empty for a remote MCP server.
    pub entry: String,
    /// The signing identity the provenance named, or empty.
    pub built_by: String,
    /// The `node` runtime's engine (`node` or `bun`, D1808); absent for other
    /// runtimes and in records written before it, which mean `node`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<String>,
}

impl Installed {
    /// The engine this install runs on, for a `node` runtime install.
    pub fn engine(&self) -> Engine {
        match self.engine.as_deref() {
            Some("bun") => Engine::Bun,
            _ => Engine::Node,
        }
    }
}

/// Where a listing lives on disk.
#[derive(Debug, Clone)]
pub struct Layout {
    root: PathBuf,
}

impl Layout {
    /// `root` is `<agentd data>/agents`.
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn listing_dir(&self, id: &str) -> PathBuf {
        // Ids are `<namespace>/<name>` with no `..` (schema pattern).
        self.root.join(id)
    }

    pub fn version_dir(&self, id: &str, version: &str) -> PathBuf {
        self.listing_dir(id).join(version)
    }

    pub fn data_dir(&self, id: &str) -> PathBuf {
        self.listing_dir(id).join("data")
    }

    fn record(&self, id: &str) -> PathBuf {
        self.listing_dir(id).join("installed.json")
    }

    pub fn installed(&self, id: &str) -> Option<Installed> {
        let bytes = std::fs::read(self.record(id)).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    /// Every installed listing, sorted by id.
    pub fn list(&self) -> Vec<Installed> {
        let mut out = Vec::new();
        let Ok(namespaces) = std::fs::read_dir(&self.root) else {
            return out;
        };
        for ns in namespaces.flatten() {
            let Ok(names) = std::fs::read_dir(ns.path()) else {
                continue;
            };
            for name in names.flatten() {
                if let Ok(bytes) = std::fs::read(name.path().join("installed.json"))
                    && let Ok(record) = serde_json::from_slice::<Installed>(&bytes)
                {
                    out.push(record);
                }
            }
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    fn write_record(&self, record: &Installed) -> Result<(), InstallError> {
        let path = self.record(&record.id);
        let text = serde_json::to_vec_pretty(record).map_err(|e| InstallError::Io {
            path: path.display().to_string(),
            error: e.to_string(),
        })?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text).map_err(io(&tmp))?;
        std::fs::rename(&tmp, &path).map_err(io(&path))
    }

    /// Removes the package and the record; `data/` stays (D1472).
    pub fn uninstall(&self, id: &str) -> Result<Installed, InstallError> {
        let record = self
            .installed(id)
            .ok_or_else(|| InstallError::NotInstalled(id.into()))?;
        let dir = self.listing_dir(id);
        for entry in std::fs::read_dir(&dir).map_err(io(&dir))?.flatten() {
            if entry.file_name() == "data" {
                continue;
            }
            let path = entry.path();
            if path.is_dir() {
                std::fs::remove_dir_all(&path).map_err(io(&path))?;
            } else {
                std::fs::remove_file(&path).map_err(io(&path))?;
            }
        }
        Ok(record)
    }
}

/// What an install needs besides the listing.
#[derive(Debug, Clone)]
pub struct Context {
    pub source: String,
    pub plan_digest: String,
    pub now_ms: i64,
}

// npm runs these when a registry package is installed (`prepare` only for git
// and local installs, which the marketplace never does).
const INSTALL_SCRIPTS: &[&str] = &["preinstall", "install", "postinstall"];

/// Every `package.json` under `node_modules`, checked for install-time code.
pub(crate) fn refuse_install_scripts(node_modules: &Path) -> Result<(), InstallError> {
    let mut stack = vec![node_modules.to_owned()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                stack.push(path);
                continue;
            }
            let name = entry.file_name();
            let package_dir = path.parent().unwrap_or(&dir);
            if name == "binding.gyp" {
                return Err(InstallError::InstallScript {
                    package: package_dir.display().to_string(),
                    script: "a native build (binding.gyp)".into(),
                });
            }
            if name != "package.json" {
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            let Ok(manifest) = serde_json::from_slice::<Value>(&bytes) else {
                continue;
            };
            let package = manifest["name"]
                .as_str()
                .unwrap_or("a dependency")
                .to_owned();
            for script in INSTALL_SCRIPTS {
                if manifest["scripts"][*script].is_string() {
                    return Err(InstallError::InstallScript {
                        package,
                        script: (*script).into(),
                    });
                }
            }
            if manifest["gypfile"].as_bool() == Some(true) {
                return Err(InstallError::InstallScript {
                    package,
                    script: "a native build (gypfile)".into(),
                });
            }
        }
    }
    Ok(())
}

/// `hasInstallScript` anywhere in the shipped lockfile.
pub(crate) fn shrinkwrap_declares_scripts(shrinkwrap: &Value) -> Option<String> {
    shrinkwrap["packages"].as_object().and_then(|packages| {
        packages
            .iter()
            .find(|(_, p)| p["hasInstallScript"].as_bool() == Some(true))
            .map(|(path, _)| path.clone())
    })
}

fn str_at<'a>(value: &'a Value, path: &[&str]) -> Option<&'a str> {
    path.iter().try_fold(value, |v, key| v.get(*key))?.as_str()
}

/// Installs a `node` agent or ACP harness from its npm tarball: integrity
/// and provenance first, then npm with scripts off into a temporary
/// directory, then the install-script and lockfile checks, then the rename.
pub fn install_node(
    layout: &Layout,
    listing: &Value,
    tarball: &[u8],
    provenance_bundles: &[Vec<u8>],
    verifier: &dyn SignatureVerifier,
    npm: &dyn Npm,
    context: &Context,
) -> Result<Installed, InstallError> {
    let id = str_at(listing, &["id"]).unwrap_or_default();
    let version = str_at(listing, &["version"]).unwrap_or_default();
    if str_at(listing, &["runtime", "kind"]) != Some("node")
        || str_at(listing, &["artifact", "type"]) != Some("npm")
    {
        return Err(InstallError::Unsupported(
            "only node listings with npm artifacts install in MK0".into(),
        ));
    }
    let name = str_at(listing, &["artifact", "name"]).unwrap_or_default();
    let integrity = str_at(listing, &["artifact", "integrity"]).unwrap_or_default();
    let repository = str_at(listing, &["source", "repository"]).unwrap_or_default();
    let entrypoint = str_at(listing, &["entrypoint"]).unwrap_or_default();

    let built =
        provenance::verify_npm(tarball, integrity, provenance_bundles, repository, verifier)?;

    let listing_dir = layout.listing_dir(id);
    std::fs::create_dir_all(&listing_dir).map_err(io(&listing_dir))?;
    let staging = listing_dir.join(format!(".tmp-{version}"));
    if staging.exists() {
        std::fs::remove_dir_all(&staging).map_err(io(&staging))?;
    }
    std::fs::create_dir_all(&staging).map_err(io(&staging))?;
    let result = (|| {
        let tgz = staging.join("package.tgz");
        std::fs::write(&tgz, tarball).map_err(io(&tgz))?;
        npm.install(&tgz, &staging).map_err(InstallError::Npm)?;
        std::fs::remove_file(&tgz).map_err(io(&tgz))?;
        let package = staging.join("node_modules").join(name);
        let manifest: Value = std::fs::read(package.join("package.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .ok_or_else(|| InstallError::NoEntrypoint(format!("{name}/package.json")))?;
        refuse_install_scripts(&staging.join("node_modules"))?;
        let has_deps = manifest["dependencies"]
            .as_object()
            .is_some_and(|d| !d.is_empty());
        let shrinkwrap: Option<Value> = std::fs::read(package.join("npm-shrinkwrap.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok());
        match &shrinkwrap {
            None if has_deps => return Err(InstallError::NoLockfile(name.into())),
            Some(lock) => {
                if let Some(path) = shrinkwrap_declares_scripts(lock) {
                    return Err(InstallError::InstallScript {
                        package: path,
                        script: "hasInstallScript in npm-shrinkwrap.json".into(),
                    });
                }
            }
            None => {}
        }
        let entry = package.join(entrypoint);
        if entrypoint.is_empty() || entrypoint.contains("..") || !entry.is_file() {
            return Err(InstallError::NoEntrypoint(entrypoint.into()));
        }
        Ok(())
    })();
    if let Err(error) = result {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(error);
    }

    let target = layout.version_dir(id, version);
    if target.exists() {
        std::fs::remove_dir_all(&target).map_err(io(&target))?;
    }
    std::fs::rename(&staging, &target).map_err(io(&target))?;
    let data = layout.data_dir(id);
    for sub in ["home", "tmp"] {
        std::fs::create_dir_all(data.join(sub)).map_err(io(&data))?;
    }
    let record = Installed {
        id: id.into(),
        version: version.into(),
        source: context.source.clone(),
        kind: str_at(listing, &["kind"]).unwrap_or_default().into(),
        installed_ms: context.now_ms,
        plan_digest: context.plan_digest.clone(),
        entry: target
            .join("node_modules")
            .join(name)
            .join(entrypoint)
            .display()
            .to_string(),
        built_by: built.identity,
        engine: Engine::of(listing).map(|e| e.as_str().to_owned()),
    };
    // Only one version is kept: the old one goes after the new one is in place.
    if let Some(previous) = layout.installed(id)
        && previous.version != version
    {
        let old = layout.version_dir(id, &previous.version);
        if old.exists() {
            std::fs::remove_dir_all(&old).map_err(io(&old))?;
        }
    }
    layout.write_record(&record)?;
    Ok(record)
}

/// Resolves the listing's engine (D1808), then installs with
/// [`install_node`]. A Bun listing on a machine without Bun at a version in
/// its range is refused here, before anything is written: the "Bun not
/// installed" state is [`EngineError::NotInstalled`]. The install itself is
/// the same scripts-off npm path for both engines (MS1 Task 6 ruling 2).
#[allow(clippy::too_many_arguments)]
pub fn install_node_with_engine(
    layout: &Layout,
    listing: &Value,
    tarball: &[u8],
    provenance_bundles: &[Vec<u8>],
    verifier: &dyn SignatureVerifier,
    npm: &dyn Npm,
    probe: &dyn EngineProbe,
    context: &Context,
) -> Result<(Installed, ResolvedEngine), InstallError> {
    let resolved = engine::resolve(listing, probe)?;
    let installed = install_node(
        layout,
        listing,
        tarball,
        provenance_bundles,
        verifier,
        npm,
        context,
    )?;
    Ok((installed, resolved))
}

/// Fetches a `node` listing's npm tarball, from its Walrus mirror when
/// Walrus is enabled and from the registry otherwise or on any Walrus
/// failure (D1812), then installs it with [`install_node`], unchanged:
/// integrity and Sigstore provenance are checked on the bytes whichever way
/// they came (D1459).
pub async fn install_node_fetched(
    layout: &Layout,
    listing: &Value,
    fetch: &ArtifactFetch<'_>,
    provenance_bundles: &[Vec<u8>],
    verifier: &dyn SignatureVerifier,
    npm: &dyn Npm,
    context: &Context,
) -> Result<(Installed, Origin), InstallError> {
    if str_at(listing, &["runtime", "kind"]) != Some("node")
        || str_at(listing, &["artifact", "type"]) != Some("npm")
    {
        return Err(InstallError::Unsupported(
            "only node listings with npm artifacts install in MK0".into(),
        ));
    }
    let sources = ArtifactSources::package(listing, fetch.npm_registry)?;
    let (tarball, origin) = fetch.fetch(&sources).await?;
    let installed = install_node(
        layout,
        listing,
        &tarball,
        provenance_bundles,
        verifier,
        npm,
        context,
    )?;
    Ok((installed, origin))
}

/// A marketplace ACP harness as agentd's harness registry takes it (D1464):
/// the listing id and the launch line for its installed entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessRegistration {
    pub listing_id: String,
    pub display_name: String,
    /// The engine (`node` or `bun`) run on `entry` (the node runtime), or the
    /// entry itself (binary).
    pub program: String,
    pub args: Vec<String>,
    pub models: Vec<String>,
}

/// Builds the registration for an installed ACP harness; native harnesses
/// are agentd's own adapters and are only enabled, never registered.
/// `engine` is the resolved engine's program for the node runtime
/// ([`engine::resolve`]): `node`, or `bun` with its flags (D1808).
pub fn harness_registration(
    listing: &Value,
    installed: &Installed,
    engine_program: &Path,
) -> Result<Option<HarnessRegistration>, InstallError> {
    let harness = &listing["harness"];
    match str_at(harness, &["launch", "protocol"]) {
        Some("native") => Ok(None),
        Some("acp") => {
            let mut args: Vec<String> = Vec::new();
            let program = if let Some(engine) = Engine::of(listing) {
                args.extend(engine::engine_args(engine, &installed.entry));
                engine_program.display().to_string()
            } else {
                installed.entry.clone()
            };
            args.extend(
                harness["launch"]["args"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|a| a.as_str().map(str::to_owned)),
            );
            Ok(Some(HarnessRegistration {
                listing_id: installed.id.clone(),
                display_name: str_at(listing, &["title"]).unwrap_or(&installed.id).into(),
                program,
                args,
                models: harness["models"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|m| m.as_str().map(str::to_owned))
                    .collect(),
            }))
        }
        _ => Err(InstallError::Unsupported("not a harness listing".into())),
    }
}

/// An MCP server for harness runs and agents (D1463): a local command, or a
/// remote URL. Secrets are named, never stored here; agentd fills them from
/// its vault when it starts or proxies the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpRegistration {
    Local {
        listing_id: String,
        command: String,
        args: Vec<String>,
        /// Environment variable → config input name, filled from the vault.
        secret_env: Vec<(String, String)>,
    },
    Remote {
        listing_id: String,
        url: String,
        secret_inputs: Vec<String>,
    },
}

pub fn mcp_registration(
    listing: &Value,
    installed: &Installed,
) -> Result<McpRegistration, InstallError> {
    let server = &listing["mcp_server"];
    let secrets: Vec<&Value> = listing["config"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| c["secret"].as_bool() == Some(true))
        .collect();
    match str_at(server, &["mode"]) {
        Some("local") => Ok(McpRegistration::Local {
            listing_id: installed.id.clone(),
            command: installed.entry.clone(),
            args: server["args"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|a| a.as_str().map(str::to_owned))
                .collect(),
            secret_env: secrets
                .iter()
                .filter_map(|c| {
                    Some((
                        c["env"].as_str()?.to_owned(),
                        c["name"].as_str()?.to_owned(),
                    ))
                })
                .collect(),
        }),
        Some("remote") => Ok(McpRegistration::Remote {
            listing_id: installed.id.clone(),
            url: str_at(server, &["remote_url"]).unwrap_or_default().into(),
            secret_inputs: secrets
                .iter()
                .filter_map(|c| c["name"].as_str().map(str::to_owned))
                .collect(),
        }),
        _ => Err(InstallError::Unsupported(
            "not an MCP server listing".into(),
        )),
    }
}

/// Records a remote MCP server or a native harness, which install no files.
pub fn install_without_artifact(
    layout: &Layout,
    listing: &Value,
    context: &Context,
) -> Result<Installed, InstallError> {
    let id = str_at(listing, &["id"]).unwrap_or_default();
    let remote = str_at(listing, &["mcp_server", "mode"]) == Some("remote");
    let native = str_at(listing, &["harness", "launch", "protocol"]) == Some("native");
    if !(remote || native) || listing.get("artifact").is_some() {
        return Err(InstallError::Unsupported(
            "this listing installs an artifact".into(),
        ));
    }
    let dir = layout.listing_dir(id);
    std::fs::create_dir_all(layout.data_dir(id)).map_err(io(&dir))?;
    let record = Installed {
        id: id.into(),
        version: str_at(listing, &["version"]).unwrap_or_default().into(),
        source: context.source.clone(),
        kind: str_at(listing, &["kind"]).unwrap_or_default().into(),
        installed_ms: context.now_ms,
        plan_digest: context.plan_digest.clone(),
        entry: String::new(),
        built_by: String::new(),
        engine: None,
    };
    layout.write_record(&record)?;
    Ok(record)
}
