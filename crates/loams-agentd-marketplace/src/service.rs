//! The marketplace as agentd runs it (MK1 Task 4b): one [`MarketplaceHost`]
//! per daemon, owning the verified sources, the installs under
//! `<agentd data>/agents`, the secrets in the vault, the 6-hour refresh and
//! revocation check, and the registrations agentd's harness registry and
//! runs read. `MarketplaceService`'s handlers (`loams-agentd-sessions`) call
//! it; it knows nothing of either wire.
//!
//! Every byte comes through the [`Fetcher`] trait. agentd's is
//! [`crate::fetch::HttpsFetcher`]; MS1 Task 5 adds a Walrus location behind
//! the same trait.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::index::Kind;
use crate::install::{
    self, Context, HarnessRegistration, InstallError, Installed, Layout, McpRegistration, Npm,
};
use crate::market::{Fetcher, ListingView, MarketError, Marketplace, Query, SourceInfo, Summary};
use crate::plan::{self, InstallPlan, InstalledVersion};
use crate::revocations::{self, Finding, Revocation};
use crate::sources::{Source, SourceError};
use crate::verify::{GITHUB_ACTIONS_ISSUER, SignatureVerifier};

/// How often sources are refreshed and installs checked against their
/// revocation lists (Q1135): every 6 hours, and once at start.
pub const REFRESH_INTERVAL: Duration = Duration::from_secs(revocations::CHECK_INTERVAL_SECS);

/// The npm registry agentd fetches tarballs and attestations from.
pub const NPM_REGISTRY: &str = "https://registry.npmjs.org";

#[async_trait::async_trait]
impl<T: Fetcher + ?Sized> Fetcher for Arc<T> {
    async fn get(&self, url: &str) -> Result<Vec<u8>, String> {
        (**self).get(url).await
    }
}

impl<T: SignatureVerifier + ?Sized> SignatureVerifier for Arc<T> {
    fn verify_blob(
        &self,
        artifact: &[u8],
        bundle: &[u8],
        signer: &crate::verify::Identity,
    ) -> Result<(), crate::verify::SignatureError> {
        (**self).verify_blob(artifact, bundle, signer)
    }
}

/// Where an install's secrets live (D1453: never in a file the agent or its
/// package can read). agentd's is the OS keyring under its own service; tests
/// keep them in memory.
pub trait Vault: Send + Sync {
    /// # Errors
    ///
    /// When the store refuses the write.
    fn set(&self, key: &str, value: &str) -> Result<(), String>;
    fn get(&self, key: &str) -> Option<String>;
    fn delete(&self, key: &str);
}

/// A [`Vault`] in memory (tests, and a daemon without a keyring).
#[derive(Debug, Default)]
pub struct MemoryVault(Mutex<HashMap<String, String>>);

impl Vault for MemoryVault {
    fn set(&self, key: &str, value: &str) -> Result<(), String> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key.into(), value.into());
        Ok(())
    }
    fn get(&self, key: &str) -> Option<String> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(key)
            .cloned()
    }
    fn delete(&self, key: &str) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(key);
    }
}

/// The vault key of one secret of one install.
pub fn secret_key(listing_id: &str, name: &str) -> String {
    format!("marketplace/{listing_id}/{name}")
}

/// What the host needs.
#[derive(Clone)]
pub struct Deps {
    /// agentd's data directory: `marketplace/` and `agents/` go under it.
    pub data_dir: PathBuf,
    pub fetcher: Arc<dyn Fetcher>,
    pub verifier: Arc<dyn SignatureVerifier>,
    pub npm: Arc<dyn Npm>,
    pub vault: Arc<dyn Vault>,
    /// The `node` that runs `node` listings, when one was found.
    pub node: Option<PathBuf>,
    /// Default [`NPM_REGISTRY`].
    pub registry: String,
    /// Where installed agents' machine keys are traded for tokens (D1618, D1629); `None`: no
    /// token files.
    pub identity: Option<crate::identity::AgentIdentity>,
    /// Zitadel's user API, to deactivate an uninstalled agent's machine user (D1632); `None`:
    /// the key is forgotten and the user left for the console.
    pub machine_users: Option<Arc<dyn crate::identity::MachineUsers>>,
}

impl std::fmt::Debug for Deps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Deps")
            .field("data_dir", &self.data_dir)
            .field("node", &self.node)
            .field("registry", &self.registry)
            .field("identity", &self.identity)
            .field("machine_users", &self.machine_users.is_some())
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ServiceError {
    #[error(transparent)]
    Market(#[from] MarketError),
    #[error(transparent)]
    Install(#[from] InstallError),
    #[error(
        "plan_changed: the plan for {id} is now {current}, not the {confirmed} that was confirmed; plan again"
    )]
    PlanChanged {
        id: String,
        confirmed: String,
        current: String,
    },
    #[error("consent_required: updating {id} to {version} asks for more ({}); confirm its new plan", widens.join("; "))]
    ConsentRequired {
        id: String,
        version: String,
        widens: Vec<String>,
    },
    #[error("bad_params: the secret {0} is required")]
    MissingSecret(String),
    #[error("not_found: {0} is not installed")]
    NotInstalled(String),
    #[error("bad_params: {0}")]
    BadRequest(String),
    #[error("vault: {0}")]
    Vault(String),
}

impl ServiceError {
    /// The agentd error reason this answers with (`docs/api/reasons.md`),
    /// when it has one. The message of an error that has a reason starts
    /// with it, as every agentd error does.
    pub fn reason(&self) -> Option<&'static str> {
        match self {
            ServiceError::Market(error) => market_reason(error),
            ServiceError::Install(InstallError::NotInstalled(_)) => Some(NOT_FOUND),
            ServiceError::Install(InstallError::Provenance(_))
            | ServiceError::Install(InstallError::InstallScript { .. })
            | ServiceError::Install(InstallError::NoLockfile(_)) => Some(UNTRUSTED_LISTING),
            ServiceError::Install(_) | ServiceError::Vault(_) => None,
            ServiceError::PlanChanged { .. } => Some(PLAN_CHANGED),
            ServiceError::ConsentRequired { .. } => Some(CONSENT_REQUIRED),
            ServiceError::NotInstalled(_) => Some(NOT_FOUND),
            ServiceError::MissingSecret(_) | ServiceError::BadRequest(_) => Some(BAD_PARAMS),
        }
    }

    /// The message with its reason first (`reason: …`).
    pub fn with_reason(&self) -> String {
        let text = self.to_string();
        match self.reason() {
            Some(reason) if !text.starts_with(reason) => format!("{reason}: {text}"),
            _ => text,
        }
    }
}

/// Error reasons the marketplace adds (`docs/api/reasons.md`).
pub const PLAN_CHANGED: &str = "plan_changed";
pub const CONSENT_REQUIRED: &str = "consent_required";
pub const LISTING_REVOKED: &str = "listing_revoked";
pub const UNTRUSTED_LISTING: &str = "untrusted_listing";
const NOT_FOUND: &str = "not_found";
const BAD_PARAMS: &str = "bad_params";

fn market_reason(error: &MarketError) -> Option<&'static str> {
    match error {
        MarketError::Source(SourceError::Unknown(_))
        | MarketError::NoListing(_)
        | MarketError::NoVersion { .. }
        | MarketError::NotLoaded(_) => Some(NOT_FOUND),
        MarketError::Source(
            SourceError::BadName(_)
            | SourceError::BadUrl(_)
            | SourceError::NoIdentity
            | SourceError::Exists(_)
            | SourceError::Official,
        ) => Some(BAD_PARAMS),
        MarketError::Signature { .. } | MarketError::Index { .. } => Some(UNTRUSTED_LISTING),
        MarketError::Yanked { .. } | MarketError::Revoked { .. } => Some(LISTING_REVOKED),
        MarketError::Source(SourceError::Io { .. })
        | MarketError::Fetch { .. }
        | MarketError::Cache(..) => None,
    }
}

/// One installed listing as `ListInstalled` shows it.
#[derive(Debug, Clone)]
pub struct InstalledView {
    pub installed: Installed,
    pub revocation: Option<Revocation>,
    /// A newer version in its source, if any.
    pub update: Option<String>,
}

/// One update `CheckUpdates` found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvailableUpdate {
    pub id: String,
    pub from: String,
    pub to: String,
    /// What the new version asks for that the installed one did not.
    pub widens: Vec<String>,
}

/// An `Install`.
#[derive(Debug, Clone, Default)]
pub struct InstallRequest {
    pub id: String,
    pub version: Option<String>,
    pub source: Option<String>,
    pub plan_digest: String,
    pub config: BTreeMap<String, String>,
    pub secrets: BTreeMap<String, String>,
}

/// A local MCP server ready for a run: its command, and its environment with
/// the secrets read from the vault.
#[derive(Clone, PartialEq, Eq)]
pub struct McpServerRun {
    pub listing_id: String,
    /// The server's name in a session (`marketplace-<namespace>-<name>`).
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
}

impl std::fmt::Debug for McpServerRun {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpServerRun")
            .field("listing_id", &self.listing_id)
            .field("command", &self.command)
            .field("args", &self.args)
            .field("env", &self.env.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// What agentd registers from the installs: ACP harnesses for its harness
/// registry, MCP servers for its runs. Installs a `disable` revocation covers
/// are left out.
#[derive(Debug, Clone, Default)]
pub struct Registrations {
    pub harnesses: Vec<HarnessRegistration>,
    pub mcp_servers: Vec<McpServerRun>,
    /// Remote MCP servers, which wait for agentd's MCP proxy (D1463).
    pub remote_mcp: Vec<String>,
}

type Market = Marketplace<Arc<dyn Fetcher>, Arc<dyn SignatureVerifier>>;

struct Inner {
    market: tokio::sync::Mutex<Market>,
    layout: Layout,
    deps: Deps,
    findings: Mutex<Vec<Finding>>,
    /// Bumped whenever the registrations may have changed.
    generation: tokio::sync::watch::Sender<u64>,
}

/// The daemon's marketplace. Cheap to clone.
#[derive(Clone)]
pub struct MarketplaceHost {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for MarketplaceHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MarketplaceHost")
            .field("deps", &self.inner.deps)
            .finish_non_exhaustive()
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

fn newer(candidate: &str, than: &str) -> bool {
    match (
        semver::Version::parse(candidate),
        semver::Version::parse(than),
    ) {
        (Ok(a), Ok(b)) => a > b,
        _ => false,
    }
}

fn str_at<'a>(value: &'a Value, path: &[&str]) -> Option<&'a str> {
    path.iter().try_fold(value, |v, key| v.get(*key))?.as_str()
}

impl MarketplaceHost {
    /// Opens the sources and their verified caches and checks the installs
    /// against the cached revocation lists (no network).
    ///
    /// # Errors
    ///
    /// When `sources.json` cannot be read.
    pub fn open(deps: Deps) -> Result<Self, ServiceError> {
        let market =
            Marketplace::open(&deps.data_dir, deps.fetcher.clone(), deps.verifier.clone())?;
        let layout = Layout::new(deps.data_dir.join("agents"));
        let (generation, _) = tokio::sync::watch::channel(0);
        let host = Self {
            inner: Arc::new(Inner {
                market: tokio::sync::Mutex::new(market),
                layout,
                deps,
                findings: Mutex::new(Vec::new()),
                generation,
            }),
        };
        let findings = {
            let market = host
                .inner
                .market
                .try_lock()
                .map_err(|e| ServiceError::BadRequest(e.to_string()))?;
            market.check_revocations(&host.revocation_inputs())
        };
        host.set_findings(findings);
        Ok(host)
    }

    pub fn layout(&self) -> &Layout {
        &self.inner.layout
    }

    /// Changes whenever installs, uninstalls, updates or revocations may have
    /// changed [`Self::registrations`].
    pub fn subscribe(&self) -> tokio::sync::watch::Receiver<u64> {
        self.inner.generation.subscribe()
    }

    fn bump(&self) {
        self.inner
            .generation
            .send_modify(|g| *g = g.wrapping_add(1));
    }

    fn revocation_inputs(&self) -> Vec<revocations::Installed> {
        self.inner
            .layout
            .list()
            .into_iter()
            .map(|i| revocations::Installed {
                source: i.source,
                id: i.id,
                version: i.version,
            })
            .collect()
    }

    fn set_findings(&self, findings: Vec<Finding>) {
        let mut current = self
            .inner
            .findings
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if *current != findings {
            *current = findings;
            drop(current);
            self.bump();
        }
    }

    /// The revocations covering installs, from the last check.
    pub fn findings(&self) -> Vec<Finding> {
        self.inner
            .findings
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn disabled(&self, id: &str) -> bool {
        self.findings()
            .iter()
            .any(|f| f.installed.id == id && f.disables())
    }

    // --- sources --------------------------------------------------------

    pub async fn list_sources(&self) -> Vec<SourceInfo> {
        self.inner.market.lock().await.list_sources()
    }

    /// Adds a source once its index verifies. An empty issuer is GitHub
    /// Actions'.
    ///
    /// # Errors
    ///
    /// A bad source, a fetch failure or an index that does not verify.
    pub async fn add_source(&self, mut source: Source) -> Result<SourceInfo, ServiceError> {
        if source.issuer.trim().is_empty() {
            source.issuer = GITHUB_ACTIONS_ISSUER.into();
        }
        source.official = false;
        Ok(self.inner.market.lock().await.add_source(source).await?)
    }

    /// # Errors
    ///
    /// An unknown source, or the official one.
    pub async fn remove_source(&self, name: &str) -> Result<(), ServiceError> {
        Ok(self.inner.market.lock().await.remove_source(name)?)
    }

    /// Refreshes every source, then checks the installs against the new
    /// revocation lists. Returns the sources that failed.
    pub async fn refresh(&self) -> Vec<(String, MarketError)> {
        let mut market = self.inner.market.lock().await;
        let failed = market.refresh_all().await;
        let findings = market.check_revocations(&self.revocation_inputs());
        drop(market);
        for (source, error) in &failed {
            tracing::warn!(%source, %error, "marketplace source refresh failed");
        }
        for finding in &findings {
            tracing::warn!(
                id = %finding.installed.id,
                version = %finding.installed.version,
                severity = ?finding.revocation.severity,
                reason = %finding.revocation.reason,
                "an installed listing is revoked"
            );
        }
        self.set_findings(findings);
        failed
    }

    /// Refreshes now and then every `every` (agentd passes
    /// [`REFRESH_INTERVAL`]), until the task is aborted.
    pub fn spawn_refresh(&self, every: Duration) -> tokio::task::JoinHandle<()> {
        let host = self.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(every);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tick.tick().await;
                host.refresh().await;
            }
        })
    }

    // --- listings -------------------------------------------------------

    pub async fn search(&self, query: &Query) -> Vec<Summary> {
        self.inner.market.lock().await.search(query)
    }

    /// # Errors
    ///
    /// No such listing or version.
    pub async fn get_listing(
        &self,
        id: &str,
        version: Option<&str>,
        source: Option<&str>,
    ) -> Result<ListingView, ServiceError> {
        Ok(self
            .inner
            .market
            .lock()
            .await
            .get_listing(id, version, source)?)
    }

    fn installed_versions(&self) -> Vec<InstalledVersion> {
        self.inner
            .layout
            .list()
            .into_iter()
            .map(|i| InstalledVersion {
                id: i.id,
                version: i.version,
            })
            .collect()
    }

    /// The consent plan and its digest. Creates nothing.
    ///
    /// # Errors
    ///
    /// No such listing, or a yanked or disabled version.
    pub async fn plan_install(
        &self,
        id: &str,
        version: Option<&str>,
        source: Option<&str>,
    ) -> Result<(InstallPlan, String, ListingView), ServiceError> {
        let installed = self.installed_versions();
        let market = self.inner.market.lock().await;
        let (plan, digest) = market.plan_install(id, version, source, &installed)?;
        let view = market.get_listing(id, Some(&plan.version), Some(&plan.source))?;
        Ok((plan, digest, view))
    }

    // --- installs -------------------------------------------------------

    /// Installs exactly the confirmed plan: the plan is made again and its
    /// digest must equal `plan_digest`.
    ///
    /// # Errors
    ///
    /// A changed plan, a missing required secret, a failed fetch, integrity
    /// or provenance check, or a listing this host cannot install.
    pub async fn install(&self, request: InstallRequest) -> Result<Installed, ServiceError> {
        let (plan, digest, view) = self
            .plan_install(
                &request.id,
                request.version.as_deref(),
                request.source.as_deref(),
            )
            .await?;
        if digest != request.plan_digest {
            return Err(ServiceError::PlanChanged {
                id: request.id,
                confirmed: request.plan_digest,
                current: digest,
            });
        }
        let secrets = request.secrets;
        for secret in &plan.secrets {
            if secret.required
                && !secrets.contains_key(&secret.name)
                && self
                    .inner
                    .deps
                    .vault
                    .get(&secret_key(&plan.id, &secret.name))
                    .is_none()
            {
                return Err(ServiceError::MissingSecret(secret.name.clone()));
            }
        }
        for name in secrets.keys() {
            if !plan.secrets.iter().any(|s| &s.name == name) {
                return Err(ServiceError::BadRequest(format!(
                    "{name} is not a secret {} asks for",
                    plan.id
                )));
            }
        }
        let installed = self
            .install_listing(&view, &plan.source, &digest, &request.config)
            .await?;
        for (name, value) in &secrets {
            self.inner
                .deps
                .vault
                .set(&secret_key(&installed.id, name), value)
                .map_err(ServiceError::Vault)?;
        }
        self.bump();
        Ok(installed)
    }

    async fn install_listing(
        &self,
        view: &ListingView,
        source: &str,
        digest: &str,
        config: &BTreeMap<String, String>,
    ) -> Result<Installed, ServiceError> {
        let listing = view.version.listing.clone();
        let context = Context {
            source: source.into(),
            plan_digest: digest.into(),
            now_ms: now_ms(),
        };
        let layout = self.inner.layout.clone();
        let runtime = str_at(&listing, &["runtime", "kind"]);
        let artifact = str_at(&listing, &["artifact", "type"]);
        let installed = match (listing.get("artifact"), runtime, artifact) {
            (None, ..) => install::install_without_artifact(&layout, &listing, &context)?,
            (Some(_), Some("node"), Some("npm")) => {
                let name = str_at(&listing, &["artifact", "name"]).unwrap_or_default();
                let version = str_at(&listing, &["artifact", "version"])
                    .or_else(|| str_at(&listing, &["version"]))
                    .unwrap_or_default();
                let tarball = self
                    .fetch(&tarball_url(&self.inner.deps.registry, name, version))
                    .await?;
                let bundles = self
                    .attestations(&attestations_url(&self.inner.deps.registry, name, version))
                    .await?;
                let verifier = self.inner.deps.verifier.clone();
                let npm = self.inner.deps.npm.clone();
                let listing = listing.clone();
                tokio::task::spawn_blocking(move || {
                    install::install_node(
                        &layout,
                        &listing,
                        &tarball,
                        &bundles,
                        verifier.as_ref(),
                        npm.as_ref(),
                        &context,
                    )
                })
                .await
                .map_err(|e| ServiceError::BadRequest(format!("install task failed: {e}")))??
            }
            (Some(_), runtime, artifact) => {
                return Err(InstallError::Unsupported(format!(
                    "a {} runtime with a {} artifact is not installable in MK0",
                    runtime.unwrap_or("missing"),
                    artifact.unwrap_or("missing")
                ))
                .into());
            }
        };
        let dir = self.inner.layout.listing_dir(&installed.id);
        write_json(&dir.join(LISTING_FILE), &listing)?;
        write_json(
            &dir.join(CONFIG_FILE),
            &serde_json::to_value(config).unwrap_or(Value::Null),
        )?;
        Ok(installed)
    }

    async fn fetch(&self, url: &str) -> Result<Vec<u8>, ServiceError> {
        self.inner.deps.fetcher.get(url).await.map_err(|error| {
            MarketError::Fetch {
                url: url.into(),
                error,
            }
            .into()
        })
    }

    /// npm's attestations document: each attestation's Sigstore bundle.
    async fn attestations(&self, url: &str) -> Result<Vec<Vec<u8>>, ServiceError> {
        let bytes = self.fetch(url).await?;
        let doc: Value = serde_json::from_slice(&bytes).map_err(|e| {
            InstallError::Provenance(crate::provenance::ProvenanceError::Malformed(e.to_string()))
        })?;
        Ok(doc["attestations"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|a| serde_json::to_vec(&a["bundle"]).ok())
            .collect())
    }

    /// Removes the package, its registration and its secrets; `data/` stays.
    ///
    /// # Errors
    ///
    /// When it is not installed.
    pub fn uninstall(&self, id: &str) -> Result<(), ServiceError> {
        let removed = self.uninstall_files(id)?;
        self.inner
            .deps
            .vault
            .delete(&crate::identity::machine_key_vault_key(&removed));
        Ok(())
    }

    /// [`MarketplaceHost::uninstall`], and retires the agent's identity (D1632): its machine
    /// user is deactivated and its key removed at Zitadel when agentd holds a management
    /// credential, and the key leaves the vault in any case.
    ///
    /// # Errors
    ///
    /// When it is not installed; Zitadel's refusal, after the uninstall (the key is gone from
    /// the vault anyway).
    pub async fn uninstall_retiring(
        &self,
        id: &str,
    ) -> Result<crate::identity::Retired, ServiceError> {
        let removed = self.uninstall_files(id)?;
        crate::identity::retire(
            self.inner.deps.vault.as_ref(),
            self.inner.deps.machine_users.as_deref(),
            &removed,
        )
        .await
        .map_err(|e| ServiceError::Vault(format!("{removed} is uninstalled, but {e}")))
    }

    /// Removes the package, its registration and its config secrets; answers the canonical id.
    fn uninstall_files(&self, id: &str) -> Result<String, ServiceError> {
        let listing = self.installed_listing(id);
        let removed = self.inner.layout.uninstall(id)?;
        if let Some(listing) = listing {
            for name in secret_names(&listing) {
                self.inner
                    .deps
                    .vault
                    .delete(&secret_key(&removed.id, &name));
            }
        }
        self.bump();
        Ok(removed.id)
    }

    /// Keeps `id`'s machine key (the JSON Zitadel handed out when its machine user was made,
    /// IA4 Task 1) in the vault; answers the machine user's id.
    ///
    /// # Errors
    ///
    /// When it is not installed, the file is not a machine key, or the vault refuses it.
    pub fn store_machine_key(&self, id: &str, key_json: &str) -> Result<String, ServiceError> {
        if self.installed_listing(id).is_none() {
            return Err(ServiceError::Vault(format!("{id} is not installed")));
        }
        crate::identity::store_machine_key(self.inner.deps.vault.as_ref(), id, key_json)
            .map_err(ServiceError::Vault)
    }

    /// The token file of a run of `id` (`LOAMS_TOKEN_FILE`, D1453), vended from its machine key
    /// (D1618, D1629). Drive it with [`crate::identity::run_token_file`].
    ///
    /// # Errors
    ///
    /// When agentd has no issuer configured or `id` has no machine key.
    pub fn token_file(
        &self,
        id: &str,
        path: PathBuf,
        namespace: String,
    ) -> Result<crate::contract::TokenFile<crate::identity::ZitadelIssuer>, ServiceError> {
        let identity = self.inner.deps.identity.clone().ok_or_else(|| {
            ServiceError::Vault(format!(
                "no agent identity: set {} and {}",
                crate::identity::ISSUER_ENV,
                crate::identity::PROJECT_ENV
            ))
        })?;
        let issuer = crate::identity::ZitadelIssuer::from_vault(
            identity,
            self.inner.deps.vault.as_ref(),
            id,
        )
        .map_err(ServiceError::Vault)?;
        Ok(crate::contract::TokenFile::new_async(
            path, namespace, issuer,
        ))
    }

    fn installed_listing(&self, id: &str) -> Option<Value> {
        let bytes = std::fs::read(self.inner.layout.listing_dir(id).join(LISTING_FILE)).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    fn installed_config(&self, id: &str) -> BTreeMap<String, String> {
        std::fs::read(self.inner.layout.listing_dir(id).join(CONFIG_FILE))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    /// Every install, with its revocation and newer version if any.
    pub async fn list_installed(&self) -> Vec<InstalledView> {
        let findings = self.findings();
        let market = self.inner.market.lock().await;
        self.inner
            .layout
            .list()
            .into_iter()
            .map(|installed| {
                let revocation = findings
                    .iter()
                    .find(|f| f.installed.id == installed.id)
                    .map(|f| f.revocation.clone());
                let update = market
                    .get_listing(&installed.id, None, Some(&installed.source))
                    .ok()
                    .map(|view| view.version.version)
                    .filter(|latest| newer(latest, &installed.version));
                InstalledView {
                    installed,
                    revocation,
                    update,
                }
            })
            .collect()
    }

    /// The installed listing's plan and the plan of `to`, from its source.
    fn plans(
        market: &Market,
        installed: &Installed,
        to: Option<&str>,
    ) -> Result<(Option<InstallPlan>, InstallPlan, String), ServiceError> {
        let old = market
            .get_listing(
                &installed.id,
                Some(&installed.version),
                Some(&installed.source),
            )
            .ok()
            .map(|view| plan::plan(&view, &[]));
        let (new, digest) = market.plan_install(&installed.id, to, Some(&installed.source), &[])?;
        Ok((old, new, digest))
    }

    fn widening(old: Option<&InstallPlan>, new: &InstallPlan) -> Vec<String> {
        match old {
            Some(old) => plan::widens(old, new),
            // The installed version left the index: consent again.
            None => vec!["the installed version is no longer in its source".into()],
        }
    }

    /// Newer versions of installed listings, and what each would widen.
    pub async fn check_updates(&self) -> Vec<AvailableUpdate> {
        let market = self.inner.market.lock().await;
        let mut out = Vec::new();
        for installed in self.inner.layout.list() {
            let Ok((old, new, _)) = Self::plans(&market, &installed, None) else {
                continue;
            };
            if !newer(&new.version, &installed.version) {
                continue;
            }
            out.push(AvailableUpdate {
                widens: Self::widening(old.as_ref(), &new),
                id: installed.id,
                from: installed.version,
                to: new.version,
            });
        }
        out
    }

    /// Updates to `version` (default: the latest). An update that widens a
    /// permission needs its new plan's digest; one that does not needs none.
    ///
    /// # Errors
    ///
    /// Not installed, consent required, or the install's own errors.
    pub async fn update(
        &self,
        id: &str,
        version: Option<&str>,
        plan_digest: &str,
    ) -> Result<Installed, ServiceError> {
        let installed = self
            .inner
            .layout
            .installed(id)
            .ok_or_else(|| ServiceError::NotInstalled(id.into()))?;
        let (new, digest, view) = {
            let market = self.inner.market.lock().await;
            let (old, new, digest) = Self::plans(&market, &installed, version)?;
            let widens = Self::widening(old.as_ref(), &new);
            if !widens.is_empty() && plan_digest != digest {
                return Err(ServiceError::ConsentRequired {
                    id: id.into(),
                    version: new.version,
                    widens,
                });
            }
            let view = market.get_listing(id, Some(&new.version), Some(&installed.source))?;
            (new, digest, view)
        };
        for secret in &new.secrets {
            if secret.required
                && self
                    .inner
                    .deps
                    .vault
                    .get(&secret_key(id, &secret.name))
                    .is_none()
            {
                return Err(ServiceError::MissingSecret(secret.name.clone()));
            }
        }
        let config = self.installed_config(id);
        let updated = self
            .install_listing(&view, &installed.source, &digest, &config)
            .await?;
        self.bump();
        Ok(updated)
    }

    // --- registrations --------------------------------------------------

    /// The harnesses and MCP servers the installs give agentd. A listing a
    /// `disable` revocation covers gives nothing.
    pub fn registrations(&self) -> Registrations {
        let mut out = Registrations::default();
        let node = self
            .inner
            .deps
            .node
            .clone()
            .unwrap_or_else(|| PathBuf::from("node"));
        for installed in self.inner.layout.list() {
            if self.disabled(&installed.id) {
                continue;
            }
            let Some(listing) = self.installed_listing(&installed.id) else {
                continue;
            };
            match installed.kind.as_str() {
                k if k == Kind::Harness.as_str() => {
                    match install::harness_registration(&listing, &installed, &node) {
                        Ok(Some(registration)) => out.harnesses.push(registration),
                        Ok(None) => {}
                        Err(error) => {
                            tracing::warn!(id = %installed.id, %error, "harness not registered");
                        }
                    }
                }
                k if k == Kind::McpServer.as_str() => {
                    match install::mcp_registration(&listing, &installed) {
                        Ok(McpRegistration::Local {
                            listing_id,
                            command,
                            args,
                            secret_env,
                        }) => {
                            let env = secret_env
                                .into_iter()
                                .filter_map(|(var, input)| {
                                    self.inner
                                        .deps
                                        .vault
                                        .get(&secret_key(&listing_id, &input))
                                        .map(|value| (var, value))
                                })
                                .collect();
                            // A `node` server runs as `node <entry> <args…>`.
                            let (command, args) =
                                if str_at(&listing, &["runtime", "kind"]) == Some("node") {
                                    let mut all = vec![command];
                                    all.extend(args);
                                    (node.display().to_string(), all)
                                } else {
                                    (command, args)
                                };
                            out.mcp_servers.push(McpServerRun {
                                name: mcp_name(&listing_id),
                                listing_id,
                                command,
                                args,
                                env,
                            });
                        }
                        Ok(McpRegistration::Remote { listing_id, .. }) => {
                            out.remote_mcp.push(listing_id);
                        }
                        Err(error) => {
                            tracing::warn!(id = %installed.id, %error, "MCP server not registered");
                        }
                    }
                }
                _ => {}
            }
        }
        out
    }
}

/// agentd's [`Npm`]: the `npm` it found, with the arguments of
/// [`install::npm_args`] (scripts off), a cleared environment, and a cache
/// beside the install.
#[derive(Debug, Clone)]
pub struct ProcessNpm {
    pub npm: PathBuf,
    /// Runs `npm` when it is a script (`npm-cli.js` on Windows), and is put
    /// first on the child's `PATH`.
    pub node: Option<PathBuf>,
}

impl Npm for ProcessNpm {
    fn install(&self, tarball: &Path, prefix: &Path) -> Result<(), String> {
        let mut cmd = match (&self.node, self.npm.extension()) {
            (Some(node), Some(ext)) if ext == "js" => {
                let mut cmd = std::process::Command::new(node);
                cmd.arg(&self.npm);
                cmd
            }
            _ => std::process::Command::new(&self.npm),
        };
        let cache = prefix.parent().unwrap_or(prefix).join(".npm-cache");
        cmd.args(install::npm_args(tarball, prefix))
            .env_clear()
            .env("HOME", &cache)
            .env("npm_config_cache", &cache)
            .env("npm_config_update_notifier", "false")
            .current_dir(prefix)
            .stdin(std::process::Stdio::null());
        let mut path: Vec<PathBuf> = Vec::new();
        for program in [self.node.as_deref(), Some(self.npm.as_path())]
            .into_iter()
            .flatten()
        {
            if let Some(dir) = program.parent() {
                path.push(dir.to_owned());
            }
        }
        if let Ok(joined) = std::env::join_paths(path) {
            cmd.env("PATH", joined);
        }
        if let Some(root) = std::env::var_os("SystemRoot") {
            cmd.env("SystemRoot", root);
        }
        let output = cmd
            .output()
            .map_err(|e| format!("{}: {e}", self.npm.display()))?;
        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let tail: Vec<&str> = stderr.lines().rev().take(20).collect();
            Err(format!(
                "{} ({})",
                output.status,
                tail.into_iter().rev().collect::<Vec<_>>().join("\n")
            ))
        }
    }
}

/// An [`Npm`] for a machine without one: every npm install fails, with why.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoNpm;

impl Npm for NoNpm {
    fn install(&self, _tarball: &Path, _prefix: &Path) -> Result<(), String> {
        Err("npm was not found; install Node.js 22.13 or newer".into())
    }
}

/// The name an installed MCP server has in a session.
pub fn mcp_name(listing_id: &str) -> String {
    format!("marketplace-{}", listing_id.replace('/', "-"))
}

const LISTING_FILE: &str = "listing.json";
const CONFIG_FILE: &str = "config.json";

fn write_json(path: &Path, value: &Value) -> Result<(), ServiceError> {
    let io = |e: std::io::Error| {
        ServiceError::Install(InstallError::Io {
            path: path.display().to_string(),
            error: e.to_string(),
        })
    };
    let text =
        serde_json::to_vec_pretty(value).map_err(|e| ServiceError::BadRequest(e.to_string()))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(io)?;
    std::fs::rename(&tmp, path).map_err(io)
}

fn secret_names(listing: &Value) -> Vec<String> {
    listing["config"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| c["secret"].as_bool() == Some(true))
        .filter_map(|c| c["name"].as_str().map(str::to_owned))
        .collect()
}

/// `<registry>/<name>/-/<basename>-<version>.tgz`, npm's tarball URL.
pub fn tarball_url(registry: &str, name: &str, version: &str) -> String {
    let base = name.rsplit('/').next().unwrap_or(name);
    format!("{registry}/{name}/-/{base}-{version}.tgz")
}

/// `<registry>/-/npm/v1/attestations/<name>@<version>` (MK1 Task 0 ruling 5).
pub fn attestations_url(registry: &str, name: &str, version: &str) -> String {
    format!("{registry}/-/npm/v1/attestations/{name}@{version}")
}
