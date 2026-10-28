//! The agent environment contract (design §73 §3.3, D1453; MK1 Task 5):
//! every installed agent, in every runtime, gets exactly these variables and
//! nothing else from the user's machine. The process is started with a
//! cleared environment and this map.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde_json::Value;
use sha2::{Digest, Sha256};

/// Tokens agentd writes to `LOAMS_TOKEN_FILE` live 15 minutes and are
/// replaced every 10, so a reader never holds an expired one for long.
pub const TOKEN_LIFETIME: Duration = Duration::from_secs(15 * 60);
pub const TOKEN_ROTATION: Duration = Duration::from_secs(10 * 60);

/// The dummy key handed to SDKs that insist on one; the model proxy injects
/// the real key, so the agent never sees it.
pub const PROXY_DUMMY_KEY: &str = "loams-model-proxy";

/// What agentd has prepared for one install. Each `Option` is present only
/// when the listing declares the capability and agentd provisioned it.
#[derive(Debug, Clone, Default)]
pub struct Provisioned {
    pub endpoint: String,
    pub namespace: String,
    /// `<agentd data>/agents/<id>/data`: HOME, TMPDIR and LOAMS_DATA_DIR live here.
    pub data_dir: PathBuf,
    pub token_file: PathBuf,
    /// The directory holding the runtime's own binaries (`node`, or `bun` for
    /// the bun engine: `ResolvedEngine::bin_dir`), the only PATH entry.
    pub runtime_bin: Option<PathBuf>,
    pub pg_url: Option<String>,
    pub qdrant: Option<(String, String)>,
    pub elasticsearch_url: Option<String>,
    pub durable_url: Option<String>,
    pub mcp_config: Option<PathBuf>,
    /// The model proxy's loopback base URL (`http://127.0.0.1:<port>`).
    pub model_proxy: Option<String>,
    /// The egress proxy that enforces the manifest's hosts.
    pub egress_proxy: Option<String>,
    /// Configuration inputs with an `env` name: values by input name.
    pub config: BTreeMap<String, String>,
    /// MK0's first-party agents get the user's provider keys from agentd's
    /// vault (§73 §3.4) until the model proxy is required in MK1.
    pub provider_keys: BTreeMap<String, String>,
}

/// The declared capabilities that change the contract.
fn declared(listing: &Value, capability: &str) -> bool {
    listing["loams"][capability].as_bool() == Some(true)
}

/// The environment for an agent: exactly the contract's variables for what
/// the listing declares and agentd provisioned.
pub fn environment(listing: &Value, p: &Provisioned) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    let path = |p: &Path| p.display().to_string();
    env.insert("LOAMS_ENDPOINT".into(), p.endpoint.clone());
    env.insert("LOAMS_NAMESPACE".into(), p.namespace.clone());
    env.insert("LOAMS_TOKEN_FILE".into(), path(&p.token_file));
    env.insert("LOAMS_DATA_DIR".into(), path(&p.data_dir));
    env.insert("HOME".into(), path(&p.data_dir.join("home")));
    env.insert("TMPDIR".into(), path(&p.data_dir.join("tmp")));
    env.insert(
        "PATH".into(),
        p.runtime_bin.as_deref().map(path).unwrap_or_default(),
    );
    if declared(listing, "postgres")
        && let Some(url) = &p.pg_url
    {
        env.insert("LOAMS_PG_URL".into(), url.clone());
    }
    if declared(listing, "qdrant")
        && let Some((url, key)) = &p.qdrant
    {
        env.insert("LOAMS_QDRANT_URL".into(), url.clone());
        env.insert("LOAMS_QDRANT_API_KEY".into(), key.clone());
    }
    if declared(listing, "elasticsearch")
        && let Some(url) = &p.elasticsearch_url
    {
        env.insert("LOAMS_ES_URL".into(), url.clone());
    }
    if declared(listing, "durable")
        && let Some(url) = &p.durable_url
    {
        env.insert("LOAMS_DURABLE_URL".into(), url.clone());
    }
    if let Some(config) = &p.mcp_config {
        env.insert("LOAMS_MCP_CONFIG".into(), path(config));
    }
    if let Some(proxy) = &p.model_proxy {
        env.insert("OPENAI_BASE_URL".into(), format!("{proxy}/openai/v1"));
        env.insert("ANTHROPIC_BASE_URL".into(), format!("{proxy}/anthropic"));
        env.insert("OPENAI_API_KEY".into(), PROXY_DUMMY_KEY.into());
        env.insert("ANTHROPIC_API_KEY".into(), PROXY_DUMMY_KEY.into());
    } else {
        for (name, value) in &p.provider_keys {
            if matches!(name.as_str(), "OPENAI_API_KEY" | "ANTHROPIC_API_KEY") {
                env.insert(name.clone(), value.clone());
            }
        }
    }
    if let Some(proxy) = &p.egress_proxy {
        env.insert("HTTPS_PROXY".into(), proxy.clone());
        env.insert("HTTP_PROXY".into(), proxy.clone());
    }
    // Configuration inputs that name a variable, never one of the contract's
    // own (loams-listing's `config-env-reserved` rule refuses those listings).
    if let Some(inputs) = listing["config"].as_array() {
        for input in inputs {
            let (Some(name), Some(var)) = (input["name"].as_str(), input["env"].as_str()) else {
                continue;
            };
            if env.contains_key(var) || var.starts_with("LOAMS_") {
                continue;
            }
            if let Some(value) = p.config.get(name) {
                env.insert(var.into(), value.clone());
            }
        }
    }
    env
}

/// The Postgres role for an install: stable per listing id, short enough for
/// Postgres's 63-byte names, and never a name a person would pick.
pub fn pg_role(listing_id: &str) -> String {
    let digest = hex::encode(Sha256::digest(listing_id.as_bytes()));
    format!("loams_agent_{}", &digest[..16])
}

/// One MCP server as the agent sees it: a loopback endpoint agentd proxies,
/// so its credentials stay in agentd.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxiedMcp {
    pub name: String,
    pub url: String,
}

/// `LOAMS_MCP_CONFIG`'s content: the `mcpServers` shape MCP clients read,
/// with URLs only. There is nowhere to put a credential.
pub fn mcp_config(servers: &[ProxiedMcp]) -> String {
    let mut map = serde_json::Map::new();
    for server in servers {
        map.insert(
            server.name.clone(),
            serde_json::json!({ "type": "http", "url": server.url }),
        );
    }
    let mut text =
        serde_json::to_string_pretty(&serde_json::json!({ "mcpServers": map })).unwrap_or_default();
    text.push('\n');
    text
}

/// Issues short-lived Loams tokens scoped to a Place.
pub trait TokenIssuer {
    fn issue(&self, namespace: &str, lifetime: Duration) -> Result<String, String>;
}

/// [`TokenIssuer`] for an issuer that calls a token endpoint: agentd's
/// [`crate::identity::ZitadelIssuer`] (D1618, D1629).
#[async_trait::async_trait]
pub trait AsyncTokenIssuer: Send + Sync {
    async fn issue(&self, namespace: &str, lifetime: Duration) -> Result<String, String>;
}

/// Keeps `LOAMS_TOKEN_FILE` fresh: a new 15-minute token every 10 minutes,
/// written atomically with mode 0600.
pub struct TokenFile<I> {
    path: PathBuf,
    namespace: String,
    issuer: I,
    next: Option<SystemTime>,
}

impl<I> std::fmt::Debug for TokenFile<I> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenFile")
            .field("path", &self.path)
            .field("namespace", &self.namespace)
            .field("next", &self.next)
            .finish_non_exhaustive()
    }
}

impl<I: TokenIssuer> TokenFile<I> {
    pub fn new(path: PathBuf, namespace: String, issuer: I) -> Self {
        Self {
            path,
            namespace,
            issuer,
            next: None,
        }
    }

    /// When the next rotation is due.
    pub fn next_rotation(&self) -> Option<SystemTime> {
        self.next
    }

    /// Rotates when due (or on first call); returns whether it wrote a token.
    pub fn tick(&mut self, now: SystemTime) -> Result<bool, String> {
        if self.next.is_some_and(|next| now < next) {
            return Ok(false);
        }
        let token = self.issuer.issue(&self.namespace, TOKEN_LIFETIME)?;
        write_private(&self.path, token.as_bytes()).map_err(|e| e.to_string())?;
        self.next = Some(now + TOKEN_ROTATION);
        Ok(true)
    }
}

impl<I: AsyncTokenIssuer> TokenFile<I> {
    /// A token file over an [`AsyncTokenIssuer`].
    pub fn new_async(path: PathBuf, namespace: String, issuer: I) -> Self {
        Self {
            path,
            namespace,
            issuer,
            next: None,
        }
    }

    /// [`TokenFile::tick`] for an [`AsyncTokenIssuer`].
    ///
    /// # Errors
    ///
    /// The issuer's refusal, or the write's.
    pub async fn tick_async(&mut self, now: SystemTime) -> Result<bool, String> {
        if self.next.is_some_and(|next| now < next) {
            return Ok(false);
        }
        let token = self.issuer.issue(&self.namespace, TOKEN_LIFETIME).await?;
        write_private(&self.path, token.as_bytes()).map_err(|e| e.to_string())?;
        self.next = Some(now + TOKEN_ROTATION);
        Ok(true)
    }

    /// When the next rotation is due ([`TokenFile::next_rotation`]).
    pub fn next_async_rotation(&self) -> Option<SystemTime> {
        self.next
    }
}

/// Writes a file only its owner can read, replacing it atomically.
pub fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        use std::io::Write;
        let mut file = options.open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}
