//! `loams knowledge publish` and `loams knowledge verify` (MS1 Task 9): the
//! knowledge cache's commands for CI and for people. The `loams` binary
//! dispatches them, as it does `loams upgrade` (ruling 3 of Task 9).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use clap::{Args, Subcommand, ValueEnum};
use loams_agentd_marketplace::verify::{SignatureVerifier, SigstoreVerifier};
use loams_store::Store;
use loams_walrus::{StoreOpts, Walrus, WalrusConfig};
use serde::Deserialize;
use serde_json::json;
use url::Url;

use crate::config::{KnowledgeConfig, ReaderConfig, SignerRule};
use crate::error::KnowledgeError;
use crate::manifest::Layout;
use crate::publish::{PublishRequest, Publisher, Visibility};
use crate::sign::{CosignSigner, ManifestSigner};
use crate::source::GitCheckout;

/// `loams knowledge …`.
#[derive(Debug, Args)]
pub struct KnowledgeArgs {
    /// What to do.
    #[command(subcommand)]
    pub command: KnowledgeCommand,
}

/// The knowledge cache's commands.
#[derive(Debug, Subcommand)]
pub enum KnowledgeCommand {
    /// Publish a public repository's knowledge set at one commit: a Walrus
    /// quilt and a Sigstore-signed manifest in the bucket. Prints the
    /// manifest's location and the quilt id.
    Publish(PublishArgs),
    /// Verify a published manifest's signature against the config's pinned
    /// signer, then every entry's SHA-256.
    Verify(VerifyArgs),
}

/// `loams knowledge publish`.
#[derive(Debug, Args)]
pub struct PublishArgs {
    /// The repository's checkout.
    #[arg(long, default_value = ".")]
    pub repo_path: PathBuf,
    /// The repository's name, as readers ask for it (`owner/name`).
    #[arg(long)]
    pub repo: String,
    /// The commit to publish [default: the checkout's HEAD].
    #[arg(long)]
    pub commit: Option<String>,
    /// What the forge says about the repository. Only `public` publishes;
    /// there is no default (in GitHub Actions:
    /// `${{ github.event.repository.visibility }}`).
    #[arg(long, value_enum)]
    pub visibility: VisibilityArg,
    /// The knowledge config file (`[walrus]` and `[knowledge]`, as agentd's
    /// reader takes it).
    #[arg(long)]
    pub config: PathBuf,
    /// The bucket's URL [default: the config's `knowledge.bucket`].
    #[arg(long)]
    pub bucket: Option<String>,
    /// The Walrus publisher [default: the config's `walrus.publisher`;
    /// there is no built-in one].
    #[arg(long)]
    pub publisher: Option<Url>,
    /// Epochs ahead to store the quilt for [default: the config's
    /// `walrus.epochs`].
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..=53))]
    pub epochs: Option<u32>,
    /// Store a permanent quilt instead of a deletable one.
    #[arg(long)]
    pub permanent: bool,
    /// The `cosign` binary that signs the manifest (keyless, with the
    /// ambient OIDC identity).
    #[arg(long, default_value = "cosign")]
    pub cosign: PathBuf,
    /// How to print the result.
    #[arg(long, value_enum, default_value_t = Output::Text)]
    pub output: Output,
}

/// `loams knowledge verify`.
#[derive(Debug, Args)]
pub struct VerifyArgs {
    /// The repository (`owner/name`).
    #[arg(long)]
    pub repo: String,
    /// The commit [default: the newest published].
    #[arg(long)]
    pub commit: Option<String>,
    /// The knowledge config file; its `[[knowledge.signers]]` pin who may
    /// sign the repository's manifests.
    #[arg(long)]
    pub config: PathBuf,
    /// The bucket's URL [default: the config's `knowledge.bucket`].
    #[arg(long)]
    pub bucket: Option<String>,
    /// How to print the result.
    #[arg(long, value_enum, default_value_t = Output::Text)]
    pub output: Output,
}

/// `--visibility`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum VisibilityArg {
    /// Public on its forge.
    Public,
    /// Private on its forge.
    Private,
    /// GitHub's "internal": refused like private.
    Internal,
    /// Not known: refused.
    Unknown,
}

impl From<VisibilityArg> for Visibility {
    fn from(v: VisibilityArg) -> Self {
        match v {
            VisibilityArg::Public => Visibility::Public,
            VisibilityArg::Private | VisibilityArg::Internal => Visibility::Private,
            VisibilityArg::Unknown => Visibility::Unknown,
        }
    }
}

/// `--output`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Output {
    /// `key value` lines.
    Text,
    /// One JSON object.
    Json,
}

/// The config file: the reader's format, with every part optional, so a
/// publisher's file needs no signers and `--bucket` may stand in for
/// `knowledge.bucket`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct CliConfig {
    #[serde(default)]
    walrus: WalrusConfig,
    #[serde(default)]
    knowledge: CliKnowledge,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CliKnowledge {
    bucket: Option<String>,
    #[serde(default = "default_prefix")]
    prefix: String,
    #[serde(default)]
    signers: Vec<SignerRule>,
}

impl Default for CliKnowledge {
    fn default() -> Self {
        Self {
            bucket: None,
            prefix: default_prefix(),
            signers: Vec::new(),
        }
    }
}

fn default_prefix() -> String {
    "knowledge".into()
}

async fn load(path: &Path) -> Result<CliConfig, KnowledgeError> {
    let text = tokio::fs::read_to_string(path).await.map_err(|e| {
        KnowledgeError::Invalid(format!("knowledge config {}: {e}", path.display()))
    })?;
    toml::from_str(&text)
        .map_err(|e| KnowledgeError::Invalid(format!("knowledge config {}: {e}", path.display())))
}

fn bucket(flag: Option<&String>, config: &CliConfig) -> Result<String, KnowledgeError> {
    flag.cloned()
        .or_else(|| config.knowledge.bucket.clone())
        .ok_or_else(|| {
            KnowledgeError::Invalid(
                "no bucket: give --bucket or set knowledge.bucket in the config".into(),
            )
        })
}

fn need_walrus(config: &CliConfig) -> Result<(), KnowledgeError> {
    if config.walrus.enabled {
        Ok(())
    } else {
        Err(KnowledgeError::Invalid(
            "the knowledge cache needs [walrus] enabled = true in the config".into(),
        ))
    }
}

/// What the commands use that tests replace: the manifest signer and the
/// Sigstore verifier. `None` uses `cosign` and Sigstore's public-good root.
#[derive(Clone, Default)]
pub struct Deps {
    /// Signs manifests.
    pub signer: Option<Arc<dyn ManifestSigner>>,
    /// Verifies manifests.
    pub verifier: Option<Arc<dyn SignatureVerifier>>,
}

impl std::fmt::Debug for Deps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Deps")
            .field("signer", &self.signer.is_some())
            .field("verifier", &self.verifier.is_some())
            .finish()
    }
}

/// Runs `loams knowledge …` on its own runtime; the result goes to stdout
/// and errors to stderr. Exit 0 on success, 1 on a refusal or failure.
pub fn run(args: &KnowledgeArgs) -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("loams knowledge: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut stdout = std::io::stdout().lock();
    match runtime.block_on(execute(args, &Deps::default(), &mut stdout)) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("loams knowledge: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Runs a command, writing its result to `out`. `Ok(false)` is a
/// verification that found problems (they are in `out`).
pub async fn execute(
    args: &KnowledgeArgs,
    deps: &Deps,
    out: &mut dyn Write,
) -> Result<bool, KnowledgeError> {
    match &args.command {
        KnowledgeCommand::Publish(a) => publish(a, deps, out).await.map(|()| true),
        KnowledgeCommand::Verify(a) => verify(a, deps, out).await,
    }
}

fn io(e: std::io::Error) -> KnowledgeError {
    KnowledgeError::Invalid(format!("writing the output: {e}"))
}

async fn head(repo_path: &Path) -> Result<String, KnowledgeError> {
    let out = tokio::process::Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .args(["rev-parse", "--verify", "HEAD^{commit}"])
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .map_err(|e| KnowledgeError::Source(format!("running git: {e}")))?;
    if !out.status.success() {
        return Err(KnowledgeError::Source(format!(
            "git rev-parse HEAD in {}: {}",
            repo_path.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

async fn publish(a: &PublishArgs, deps: &Deps, out: &mut dyn Write) -> Result<(), KnowledgeError> {
    let mut config = load(&a.config).await?;
    let bucket = bucket(a.bucket.as_ref(), &config)?;
    need_walrus(&config)?;
    // No built-in publisher (ruling 1 of Task 9): loams-walrus would fall
    // back to Testnet's public one.
    if let Some(p) = &a.publisher {
        config.walrus.publisher = Some(p.clone());
    }
    if config.walrus.publisher.is_none() {
        return Err(KnowledgeError::Invalid(
            "no Walrus publisher: give --publisher or set walrus.publisher in the config (there is no default)".into(),
        ));
    }
    let commit = match &a.commit {
        Some(c) => c.clone(),
        None => head(&a.repo_path).await?,
    };
    let layout = Layout::new(&config.knowledge.prefix)?;
    let store = Store::from_url(&bucket, Vec::<(String, String)>::new())?;
    let walrus = Walrus::new(&config.walrus).await?;
    let signer = deps
        .signer
        .clone()
        .unwrap_or_else(|| Arc::new(CosignSigner::with_binary(&a.cosign)));
    let publisher = Publisher::new(store, walrus, signer, layout);
    let request = PublishRequest {
        repo: a.repo.clone(),
        commit,
        visibility: a.visibility.into(),
        store: StoreOpts {
            epochs: a.epochs,
            permanent: a.permanent,
        },
    };
    let published = publisher
        .publish(&GitCheckout::new(&a.repo_path), &request)
        .await?;
    let base = bucket.trim_end_matches('/');
    let manifest = format!("{base}/{}", published.manifest_key);
    let bundle = format!("{base}/{}", published.bundle_key);
    let m = &published.manifest;
    match a.output {
        Output::Text => {
            writeln!(out, "repo {}", m.repo).map_err(io)?;
            writeln!(out, "commit {}", m.commit).map_err(io)?;
            writeln!(out, "quilt_id {}", m.quilt_id).map_err(io)?;
            writeln!(out, "manifest {manifest}").map_err(io)?;
            writeln!(out, "bundle {bundle}").map_err(io)?;
            writeln!(out, "entries {}", m.entries.len()).map_err(io)?;
            writeln!(out, "end_epoch {}", m.end_epoch).map_err(io)?;
        }
        Output::Json => {
            let value = json!({
                "repo": m.repo,
                "commit": m.commit,
                "quilt_id": m.quilt_id.to_string(),
                "manifest": manifest,
                "bundle": bundle,
                "entries": m.entries.len(),
                "end_epoch": m.end_epoch,
            });
            writeln!(out, "{value}").map_err(io)?;
        }
    }
    Ok(())
}

async fn verify(a: &VerifyArgs, deps: &Deps, out: &mut dyn Write) -> Result<bool, KnowledgeError> {
    let config = load(&a.config).await?;
    let bucket = bucket(a.bucket.as_ref(), &config)?;
    need_walrus(&config)?;
    if !config.knowledge.signers.iter().any(|s| s.repo == a.repo) {
        return Err(KnowledgeError::UnknownRepo(a.repo.clone()));
    }
    let reader_config = ReaderConfig {
        walrus: config.walrus,
        knowledge: KnowledgeConfig {
            bucket,
            prefix: config.knowledge.prefix,
            signers: config.knowledge.signers,
        },
    };
    let verifier = match &deps.verifier {
        Some(v) => v.clone(),
        None => Arc::new(SigstoreVerifier::production().map_err(|error| {
            KnowledgeError::Invalid(format!("the Sigstore trusted root: {error}"))
        })?),
    };
    let reader = reader_config.reader_with(verifier).await?;
    let manifest = reader.manifest(&a.repo, a.commit.as_deref()).await?;
    let mut failures = Vec::new();
    for entry in &manifest.entries {
        if let Err(e) = reader
            .get(&manifest.repo, Some(&manifest.commit), &entry.path)
            .await
        {
            failures.push((entry.path.clone(), e.to_string()));
        }
    }
    let ok = failures.is_empty();
    match a.output {
        Output::Text => {
            writeln!(out, "repo {}", manifest.repo).map_err(io)?;
            writeln!(out, "commit {}", manifest.commit).map_err(io)?;
            writeln!(out, "quilt_id {}", manifest.quilt_id).map_err(io)?;
            writeln!(out, "signature ok").map_err(io)?;
            writeln!(
                out,
                "entries {} verified, {} failed",
                manifest.entries.len() - failures.len(),
                failures.len()
            )
            .map_err(io)?;
            for (path, error) in &failures {
                writeln!(out, "FAILED {path}: {error}").map_err(io)?;
            }
        }
        Output::Json => {
            let value = json!({
                "repo": manifest.repo,
                "commit": manifest.commit,
                "quilt_id": manifest.quilt_id.to_string(),
                "signature": "ok",
                "entries": manifest.entries.len(),
                "ok": ok,
                "failures": failures
                    .iter()
                    .map(|(path, error)| json!({ "path": path, "error": error }))
                    .collect::<Vec<_>>(),
            });
            writeln!(out, "{value}").map_err(io)?;
        }
    }
    Ok(ok)
}
