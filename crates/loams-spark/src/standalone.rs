//! Running the proxy from a configuration file: what the standalone
//! `loams-spark-proxy` binary and `loams-gateway --module spark` share
//! (SP-a D1992, the `temporal` module pattern). The example is
//! `deploy/release/spark/proxy.toml.example`.

use std::future::Future;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use loams_authz::{
    AllowAllAuthorizer, Authorizer, CachedAuthorizer, CheckRequest, Decision, ObjectRef,
    OpenFgaAuthorizer, Subject, SubjectKind,
};
use serde::Deserialize;

use crate::auth::{Authenticator, DevAuthenticator, KeyEntry, KeyTable, Principal};
use crate::proxy::{
    AccessCheck, DEFAULT_MAX_MESSAGE_BYTES, NoLifecycle, ProxySettings, SparkConnectProxy,
};
use crate::route::{HostPattern, RouteConfig, StaticRouter};
use crate::sessions::Limits;

/// The proxy's configuration file.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Where to listen (h2c; TLS is the gateway's or the ingress's).
    #[serde(default = "default_listen")]
    pub listen: SocketAddr,
    /// Allows the dev authenticator; only on a loopback listener.
    #[serde(default)]
    pub insecure_dev: bool,
    /// With `insecure_dev`: the namespace every caller gets.
    #[serde(default)]
    pub dev_namespace: Option<String>,
    /// `spark.<base-domain>`; empty for a loopback/compose listener.
    #[serde(default)]
    pub host_suffix: String,
    /// The largest gRPC message either way.
    #[serde(default = "default_max_message_bytes")]
    pub max_message_bytes: usize,
    /// How often idle sessions are swept, in seconds.
    #[serde(default = "default_sweep_secs")]
    pub sweep_interval_secs: u64,
    /// Session and operation limits.
    #[serde(default)]
    pub limits: Limits,
    /// The access check.
    #[serde(default)]
    pub access: AccessCheck,
    /// Who decides access.
    #[serde(default)]
    pub authz: AuthzConfig,
    /// API keys inline.
    #[serde(default)]
    pub keys: Vec<KeyEntry>,
    /// API keys from a JSON file `{"keys": [...]}` (a mounted secret).
    #[serde(default)]
    pub keys_file: Option<PathBuf>,
    /// Backends.
    #[serde(default)]
    pub routes: Vec<RouteConfig>,
}

/// Who decides access after authentication.
#[derive(Debug, Default, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AuthzConfig {
    /// A key's namespace is its grant: holding a key of namespace `n` allows
    /// Spark in `n`. The compose preview mode (design 81 R9: single tenant,
    /// internal network).
    #[default]
    KeyGrants,
    /// Everything, behind `insecure_dev` on loopback only.
    AllowAll,
    /// OpenFGA (design 76), cached briefly.
    Openfga {
        endpoint: String,
        store_id: String,
        model_id: String,
        #[serde(default)]
        preshared_key_env: Option<String>,
        #[serde(default = "default_cache_secs")]
        cache_ttl_secs: u64,
    },
}

#[derive(Debug, Deserialize)]
struct KeysFile {
    keys: Vec<KeyEntry>,
}

fn default_listen() -> SocketAddr {
    "127.0.0.1:15002".parse().expect("valid")
}
fn default_max_message_bytes() -> usize {
    DEFAULT_MAX_MESSAGE_BYTES
}
fn default_sweep_secs() -> u64 {
    30
}
fn default_cache_secs() -> u64 {
    10
}

/// The key's namespace is the grant (see [`AuthzConfig::KeyGrants`]). The
/// proxy has already bound the call to the key's namespace, so allowing
/// here never widens access beyond that namespace.
#[derive(Debug, Clone, Copy, Default)]
pub struct KeyGrants;

#[async_trait]
impl Authorizer for KeyGrants {
    async fn check(&self, _req: CheckRequest) -> loams_authz::Result<Decision> {
        Ok(Decision::allow(None))
    }

    async fn list_objects(
        &self,
        _: &Subject,
        _: &str,
        _: &str,
    ) -> loams_authz::Result<Vec<ObjectRef>> {
        Ok(Vec::new())
    }
}

/// Parses and checks a configuration.
///
/// # Errors
///
/// A message for a parse error or a failed [`check`].
pub fn load(text: &str) -> Result<Config, String> {
    let config: Config = toml::from_str(text).map_err(|e| e.to_string())?;
    check(&config)?;
    Ok(config)
}

/// Checks the dev-only settings are on a loopback listener with
/// `insecure_dev`, and that a production configuration has keys and routes.
///
/// # Errors
///
/// A message naming the problem.
pub fn check(c: &Config) -> Result<(), String> {
    let no_keys = c.keys.is_empty() && c.keys_file.is_none();
    let dev_parts = no_keys || matches!(c.authz, AuthzConfig::AllowAll);
    if dev_parts && !c.insecure_dev {
        return Err("no API keys or authz = allow_all need insecure_dev = true".into());
    }
    if c.insecure_dev && !c.listen.ip().is_loopback() {
        return Err(format!(
            "insecure_dev is only allowed on a loopback listener, not {}",
            c.listen
        ));
    }
    if c.insecure_dev && no_keys && c.dev_namespace.is_none() {
        return Err("insecure_dev without keys needs dev_namespace".into());
    }
    if c.routes.is_empty() {
        return Err("no routes: name at least one Spark Connect backend".into());
    }
    if c.sweep_interval_secs == 0 {
        return Err("sweep_interval_secs must be positive".into());
    }
    Ok(())
}

/// A loopback development configuration routing namespace `local` to `uri`.
pub fn dev_config(uri: &str) -> Config {
    Config {
        listen: default_listen(),
        insecure_dev: true,
        dev_namespace: Some("local".into()),
        host_suffix: String::new(),
        max_message_bytes: default_max_message_bytes(),
        sweep_interval_secs: default_sweep_secs(),
        limits: Limits::default(),
        access: AccessCheck::default(),
        authz: AuthzConfig::AllowAll,
        keys: Vec::new(),
        keys_file: None,
        routes: vec![RouteConfig {
            namespace: "local".into(),
            instance: "spark".into(),
            default: true,
            uri: uri.to_owned(),
            token_env: None,
        }],
    }
}

/// Builds the proxy a configuration describes (no listener yet).
///
/// # Errors
///
/// A message for unreadable keys, a bad route, or an authorizer that
/// cannot be built.
pub fn build(c: &Config) -> Result<SparkConnectProxy, String> {
    let mut entries = c.keys.clone();
    if let Some(path) = &c.keys_file {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("reading {}: {e}", path.display()))?;
        let file: KeysFile =
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        entries.extend(file.keys);
    }
    let auth: Arc<dyn Authenticator> = if entries.is_empty() {
        let ns = c
            .dev_namespace
            .clone()
            .ok_or("insecure_dev without keys needs dev_namespace")?;
        tracing::warn!(namespace = %ns, "no API keys: every caller is namespace {ns} (insecure_dev)");
        Arc::new(DevAuthenticator(Principal {
            namespace: ns,
            subject: Subject::new(SubjectKind::User, "dev"),
            credential_id: None,
        }))
    } else {
        Arc::new(KeyTable::new(entries)?)
    };
    let authz: Arc<dyn Authorizer> = match &c.authz {
        AuthzConfig::KeyGrants => Arc::new(KeyGrants),
        AuthzConfig::AllowAll => {
            Arc::new(AllowAllAuthorizer::loopback_only(&[c.listen]).map_err(|e| e.to_string())?)
        }
        AuthzConfig::Openfga {
            endpoint,
            store_id,
            model_id,
            preshared_key_env,
            cache_ttl_secs,
        } => {
            let key = match preshared_key_env {
                Some(var) => Some(
                    std::env::var(var)
                        .map_err(|_| format!("environment variable {var} is not set"))?,
                ),
                None => None,
            };
            let fga = OpenFgaAuthorizer::connect_lazy(endpoint, store_id, model_id, key.as_deref())
                .map_err(|e| e.to_string())?;
            Arc::new(CachedAuthorizer::new(
                fga,
                Duration::from_secs(*cache_ttl_secs),
                10_000,
            ))
        }
    };
    let router = Arc::new(StaticRouter::new(&c.routes)?);
    let settings = ProxySettings {
        hosts: HostPattern {
            suffix: c.host_suffix.clone(),
        },
        limits: c.limits,
        access: c.access.clone(),
        max_message_bytes: c.max_message_bytes,
        sweep_interval: Duration::from_secs(c.sweep_interval_secs),
    };
    Ok(SparkConnectProxy::new(
        auth,
        authz,
        router,
        Arc::new(NoLifecycle),
        settings,
    ))
}

/// Serves the proxy until `shutdown`.
///
/// # Errors
///
/// A message when the proxy cannot be built or the listener fails.
pub async fn run(config: Config, shutdown: impl Future<Output = ()> + Send) -> Result<(), String> {
    let proxy = build(&config)?;
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let sweeper = tokio::spawn(proxy.clone().run_sweeper({
        let mut rx = stop_rx.clone();
        async move {
            let _ = rx.wait_for(|stop| *stop).await;
        }
    }));
    tracing::info!(listen = %config.listen, routes = config.routes.len(), "Spark Connect proxy");
    let served = tonic::transport::Server::builder()
        .http2_keepalive_interval(Some(Duration::from_secs(30)))
        .add_service(proxy)
        .serve_with_shutdown(config.listen, shutdown)
        .await
        .map_err(|e| e.to_string());
    let _ = stop_tx.send(true);
    let _ = sweeper.await;
    served
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_settings_need_loopback() {
        let text = r#"
listen = "0.0.0.0:15002"
insecure_dev = true
dev_namespace = "local"
[[routes]]
namespace = "local"
instance = "spark"
uri = "http://sail:50051"
"#;
        assert!(load(text).unwrap_err().contains("loopback"));
    }

    #[test]
    fn production_needs_keys() {
        let text = r#"
listen = "0.0.0.0:15002"
[[routes]]
namespace = "local"
instance = "spark"
uri = "http://sail:50051"
"#;
        assert!(load(text).unwrap_err().contains("insecure_dev"));
    }

    #[test]
    fn full_config_parses() {
        let text = r#"
listen = "0.0.0.0:15002"
host_suffix = "spark.loams.dev"
sweep_interval_secs = 10
[limits]
max_sessions = 4
idle_timeout = 600
[access]
relation = "can_connect"
object_type = "engine"
[authz]
kind = "openfga"
endpoint = "http://openfga:8081"
store_id = "s"
model_id = "m"
[[keys]]
id = "01JABCDEFGHJKMNPQRSTVWXYZ0"
sha256 = "0000000000000000000000000000000000000000000000000000000000000000"
namespace = "ns-a"
subject = "ci"
[[routes]]
namespace = "ns-a"
instance = "etl"
uri = "http://spark-connect:15002"
token_env = "SPARK_CONNECT_AUTHENTICATE_TOKEN"
"#;
        let c = load(text).unwrap();
        assert_eq!(c.limits.max_sessions, 4);
        assert_eq!(c.access.object_type, "engine");
        assert!(matches!(c.authz, AuthzConfig::Openfga { .. }));
        assert_eq!(
            c.routes[0].token_env.as_deref(),
            Some("SPARK_CONNECT_AUTHENTICATE_TOKEN")
        );
    }

    #[test]
    fn dev_config_is_valid() {
        check(&dev_config("http://127.0.0.1:50051")).unwrap();
    }
}
