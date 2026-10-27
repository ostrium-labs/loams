//! Where a call goes (SP-a D1992; design 81 R2, R8).
//!
//! The tenant is the Loams namespace of the credential, never something the
//! client names. In cloud the hostname is
//! `<instance-name>.<ns>.<region>.spark.<base-domain>` (R8): the proxy takes
//! the instance name from it and refuses a hostname whose namespace is not
//! the credential's. On a loopback compose listener the hostname carries
//! nothing and the namespace's default instance is used.
//!
//! [`Router`] resolves `(namespace, instance)` to a [`Backend`]: the gRPC
//! endpoint of that instance's Spark Connect server and the pre-shared token
//! the proxy presents to it. [`StaticRouter`] is the configuration-file
//! router (compose, tests); the lifecycle router of [`crate::managed`] wakes
//! a suspended instance first (start-on-first-connect, D1995).

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use tonic::Status;
use tonic::transport::{Channel, Endpoint};

/// What the caller asked for, after authentication.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Target {
    /// The credential's namespace.
    pub namespace: String,
    /// The instance name from the hostname, if it names one.
    pub instance: Option<String>,
}

/// The hostname form the proxy understands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostPattern {
    /// `spark.<base-domain>`, e.g. `spark.loams.dev`. Empty: hostnames are
    /// ignored (a loopback listener).
    pub suffix: String,
}

impl HostPattern {
    /// Reads `(instance, namespace)` from an authority
    /// `<instance>.<ns>.<region>.<suffix>[:port]`.
    ///
    /// `Ok(None)` when the pattern is off, or the host is an IP address or
    /// `localhost` (compose). `Err` when the host ends with the suffix but is
    /// not of that shape, so typos never fall through to a default.
    pub fn parse(&self, authority: Option<&str>) -> Result<Option<(String, String)>, Status> {
        let Some(authority) = authority else {
            return Ok(None);
        };
        let host = strip_port(authority).to_ascii_lowercase();
        if self.suffix.is_empty() || host == "localhost" || host.parse::<std::net::IpAddr>().is_ok()
        {
            return Ok(None);
        }
        let suffix = format!(".{}", self.suffix.to_ascii_lowercase());
        let Some(prefix) = host.strip_suffix(&suffix) else {
            return Err(Status::not_found(format!(
                "{host} is not a Loams Spark endpoint (expected <instance>.<namespace>.<region>.{})",
                self.suffix
            )));
        };
        let labels: Vec<&str> = prefix.split('.').collect();
        match labels.as_slice() {
            [instance, ns, _region] if is_label(instance) && is_label(ns) => {
                Ok(Some(((*instance).to_owned(), (*ns).to_owned())))
            }
            _ => Err(Status::not_found(format!(
                "{host}: expected <instance>.<namespace>.<region>.{}",
                self.suffix
            ))),
        }
    }

    /// The target for a principal's namespace and the request's authority.
    ///
    /// # Errors
    ///
    /// `PERMISSION_DENIED` when the hostname names another namespace;
    /// `NOT_FOUND` when it is malformed.
    pub fn target(&self, namespace: &str, authority: Option<&str>) -> Result<Target, Status> {
        match self.parse(authority)? {
            None => Ok(Target {
                namespace: namespace.to_owned(),
                instance: None,
            }),
            Some((instance, ns)) if ns == namespace => Ok(Target {
                namespace: namespace.to_owned(),
                instance: Some(instance),
            }),
            Some(_) => Err(Status::permission_denied(
                "the credential does not belong to this endpoint's namespace",
            )),
        }
    }
}

fn strip_port(authority: &str) -> &str {
    if authority.starts_with('[') {
        // [v6]:port
        return authority
            .split_once(']')
            .map_or(authority, |(h, _)| h.trim_start_matches('['));
    }
    match authority.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') && port.bytes().all(|b| b.is_ascii_digit()) => {
            host
        }
        _ => authority,
    }
}

fn is_label(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 63
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

/// A resolved backend: a Spark Connect server the proxy may call.
#[derive(Clone)]
pub struct Backend {
    /// The instance, for session bookkeeping and logs (`eng_<26>` or a name).
    pub instance_id: String,
    /// The gRPC channel (lazy; reused across calls).
    pub channel: Channel,
    /// The pre-shared token the backend expects as `authorization: Bearer`
    /// (Apache Spark's `SPARK_CONNECT_AUTHENTICATE_TOKEN`); `None` for Sail.
    pub token: Option<Arc<str>>,
}

impl fmt::Debug for Backend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Backend")
            .field("instance_id", &self.instance_id)
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .finish_non_exhaustive()
    }
}

/// Resolves a target to a backend.
#[async_trait]
pub trait Router: Send + Sync + 'static {
    /// The backend for `target`. `NOT_FOUND` when the namespace has no such
    /// instance; `UNAVAILABLE` when it cannot be started in time.
    async fn resolve(&self, target: &Target) -> Result<Backend, Status>;

    /// The backend of an instance id, for the sweeper's `ReleaseSession`
    /// calls. `None` when the instance is gone.
    async fn by_instance(&self, instance_id: &str) -> Option<Backend>;
}

/// One route of a [`StaticRouter`].
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteConfig {
    /// The Loams namespace.
    pub namespace: String,
    /// The instance name (the hostname's first label).
    pub instance: String,
    /// Whether this is the namespace's default instance (used when the
    /// hostname names none). The first route of a namespace is the default
    /// when none says so.
    #[serde(default)]
    pub default: bool,
    /// The backend's gRPC URI, e.g. `http://sail:50051`.
    pub uri: String,
    /// The environment variable holding the backend's pre-shared token.
    #[serde(default)]
    pub token_env: Option<String>,
}

impl fmt::Debug for RouteConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RouteConfig")
            .field("namespace", &self.namespace)
            .field("instance", &self.instance)
            .field("default", &self.default)
            .field("uri", &self.uri)
            .field("token_env", &self.token_env)
            .finish()
    }
}

/// A fixed route table.
#[derive(Debug, Clone, Default)]
pub struct StaticRouter {
    routes: BTreeMap<(String, String), Backend>,
    defaults: BTreeMap<String, String>,
}

/// Builds a lazy channel to a backend.
///
/// # Errors
///
/// A message when the URI is malformed.
pub fn channel(uri: &str, connect_timeout: Duration) -> Result<Channel, String> {
    Endpoint::from_shared(uri.to_owned())
        .map_err(|e| format!("backend uri {uri:?}: {e}"))
        .map(|e| {
            e.connect_timeout(connect_timeout)
                .tcp_nodelay(true)
                .http2_keep_alive_interval(Duration::from_secs(30))
                .keep_alive_while_idle(true)
                .connect_lazy()
        })
}

impl StaticRouter {
    /// A router over `routes`; tokens are read from the environment now.
    ///
    /// # Errors
    ///
    /// A message naming a repeated route, a bad URI, a missing token
    /// variable, or a namespace with two defaults.
    pub fn new(routes: &[RouteConfig]) -> Result<Self, String> {
        Self::with_env(routes, |name| std::env::var(name).ok())
    }

    /// [`Self::new`] with the environment given (tests).
    ///
    /// # Errors
    ///
    /// As [`Self::new`].
    pub fn with_env(
        routes: &[RouteConfig],
        env: impl Fn(&str) -> Option<String>,
    ) -> Result<Self, String> {
        let mut out = Self::default();
        for r in routes {
            let token = match &r.token_env {
                Some(var) => {
                    Some(Arc::<str>::from(env(var).ok_or_else(|| {
                        format!("environment variable {var} is not set")
                    })?))
                }
                None => None,
            };
            let backend = Backend {
                instance_id: format!("{}/{}", r.namespace, r.instance),
                channel: channel(&r.uri, Duration::from_secs(5))?,
                token,
            };
            let key = (r.namespace.clone(), r.instance.clone());
            if out.routes.insert(key, backend).is_some() {
                return Err(format!("route {}/{} is repeated", r.namespace, r.instance));
            }
            if r.default
                && let Some(prev) = out.defaults.insert(r.namespace.clone(), r.instance.clone())
                && prev != r.instance
            {
                return Err(format!(
                    "namespace {} has two default instances",
                    r.namespace
                ));
            }
        }
        for r in routes {
            out.defaults
                .entry(r.namespace.clone())
                .or_insert_with(|| r.instance.clone());
        }
        Ok(out)
    }

    /// A router with one prepared backend (tests and embedding).
    pub fn single(namespace: &str, instance: &str, backend: Backend) -> Self {
        let mut out = Self::default();
        out.routes
            .insert((namespace.to_owned(), instance.to_owned()), backend);
        out.defaults
            .insert(namespace.to_owned(), instance.to_owned());
        out
    }
}

#[async_trait]
impl Router for StaticRouter {
    async fn resolve(&self, target: &Target) -> Result<Backend, Status> {
        let instance = match &target.instance {
            Some(i) => i.clone(),
            None => self
                .defaults
                .get(&target.namespace)
                .cloned()
                .ok_or_else(|| Status::not_found("this namespace has no Spark instance"))?,
        };
        self.routes
            .get(&(target.namespace.clone(), instance))
            .cloned()
            .ok_or_else(|| Status::not_found("no such Spark instance in this namespace"))
    }

    async fn by_instance(&self, instance_id: &str) -> Option<Backend> {
        self.routes
            .values()
            .find(|b| b.instance_id == instance_id)
            .cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pattern() -> HostPattern {
        HostPattern {
            suffix: "spark.loams.dev".into(),
        }
    }

    #[test]
    fn parses_cloud_hostnames() {
        let p = pattern();
        assert_eq!(
            p.parse(Some("etl.ns-a.eu1.spark.loams.dev:443")).unwrap(),
            Some(("etl".into(), "ns-a".into()))
        );
        assert_eq!(p.parse(Some("127.0.0.1:15002")).unwrap(), None);
        assert_eq!(p.parse(Some("[::1]:15002")).unwrap(), None);
        assert_eq!(p.parse(Some("localhost:15002")).unwrap(), None);
        assert_eq!(p.parse(None).unwrap(), None);
        assert!(p.parse(Some("ns-a.eu1.spark.loams.dev")).is_err());
        assert!(p.parse(Some("evil.example.com")).is_err());
    }

    #[test]
    fn hostname_namespace_must_match_the_credential() {
        let p = pattern();
        let t = p
            .target("ns-a", Some("etl.ns-a.eu1.spark.loams.dev"))
            .unwrap();
        assert_eq!(t.instance.as_deref(), Some("etl"));
        let err = p
            .target("ns-b", Some("etl.ns-a.eu1.spark.loams.dev"))
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::PermissionDenied);
        let off = HostPattern {
            suffix: String::new(),
        };
        assert_eq!(
            off.target("ns-b", Some("anything:1")).unwrap().instance,
            None
        );
    }

    fn route(ns: &str, inst: &str, default: bool) -> RouteConfig {
        RouteConfig {
            namespace: ns.into(),
            instance: inst.into(),
            default,
            uri: "http://127.0.0.1:50051".into(),
            token_env: None,
        }
    }

    #[tokio::test]
    async fn static_router_defaults_and_isolation() {
        let r = StaticRouter::with_env(
            &[
                route("ns-a", "one", false),
                route("ns-a", "two", true),
                route("ns-b", "one", false),
            ],
            |_| None,
        )
        .unwrap();
        let t = |ns: &str, i: Option<&str>| Target {
            namespace: ns.into(),
            instance: i.map(Into::into),
        };
        assert_eq!(
            r.resolve(&t("ns-a", None)).await.unwrap().instance_id,
            "ns-a/two"
        );
        assert_eq!(
            r.resolve(&t("ns-b", None)).await.unwrap().instance_id,
            "ns-b/one"
        );
        assert_eq!(
            r.resolve(&t("ns-c", None)).await.unwrap_err().code(),
            tonic::Code::NotFound
        );
        assert_eq!(
            r.resolve(&t("ns-b", Some("two"))).await.unwrap_err().code(),
            tonic::Code::NotFound
        );
        assert!(r.by_instance("ns-b/one").await.is_some());
    }

    #[tokio::test]
    async fn static_router_reads_tokens_and_rejects_bad_tables() {
        let mut with_token = route("ns-a", "one", false);
        with_token.token_env = Some("TOK".into());
        let r = StaticRouter::with_env(&[with_token.clone()], |v| {
            (v == "TOK").then(|| "s3cret".to_owned())
        })
        .unwrap();
        let b = r
            .resolve(&Target {
                namespace: "ns-a".into(),
                instance: None,
            })
            .await
            .unwrap();
        assert_eq!(b.token.as_deref(), Some("s3cret"));
        assert!(!format!("{b:?}").contains("s3cret"));
        assert!(StaticRouter::with_env(&[with_token], |_| None).is_err());
        assert!(
            StaticRouter::with_env(&[route("a", "x", false), route("a", "x", false)], |_| None)
                .is_err()
        );
        assert!(
            StaticRouter::with_env(&[route("a", "x", true), route("a", "y", true)], |_| None)
                .is_err()
        );
    }
}
