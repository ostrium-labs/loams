//! The factory host: configure, test, list, remove and query (`factory/host.ts`,
//! ported). Replies never carry a secret, and every error message is scrubbed of
//! every form of every stored secret.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use loams_agentd_proto::factory::{
    FactoryAppInfo, FactoryAppList, FactoryAppReply, FactoryConfigureParams, FactoryError,
    FactoryHealth, FactoryQueryParams, FactoryQueryReply,
};
use url::Url;

use crate::apps::AppId;
use crate::error::OpError;
use crate::ops::{self, Connection, OpContext};
use crate::redact::{redact, secret_forms};
use crate::store::{AppEntry, ConfigStore, SecretStore, account};
use crate::upstream::Upstream;

/// The longest a query or a health check may take.
pub const QUERY_TIMEOUT: Duration = Duration::from_secs(15);

/// Milliseconds since the Unix epoch.
pub type Clock = fn() -> i64;

fn system_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

fn err(code: &str, message: impl Into<String>) -> FactoryError {
    FactoryError {
        code: code.to_owned(),
        message: message.into(),
    }
}

/// The factory's app connections.
#[derive(Debug)]
pub struct FactoryHost {
    config: ConfigStore,
    secrets: Arc<dyn SecretStore>,
    upstream: Arc<dyn Upstream>,
    health: Mutex<HashMap<AppId, FactoryHealth>>,
    /// Bumped by every configure and remove, so a stale check never writes health.
    generation: Mutex<HashMap<AppId, u64>>,
    clock: Clock,
}

impl FactoryHost {
    pub fn new(
        config: ConfigStore,
        secrets: Arc<dyn SecretStore>,
        upstream: Arc<dyn Upstream>,
    ) -> Self {
        Self::with_clock(config, secrets, upstream, system_now)
    }

    pub fn with_clock(
        config: ConfigStore,
        secrets: Arc<dyn SecretStore>,
        upstream: Arc<dyn Upstream>,
        clock: Clock,
    ) -> Self {
        Self {
            config,
            secrets,
            upstream,
            health: Mutex::new(HashMap::new()),
            generation: Mutex::new(HashMap::new()),
            clock,
        }
    }

    fn generation(&self, app: AppId) -> u64 {
        self.generation
            .lock()
            .map_or(0, |g| g.get(&app).copied().unwrap_or(0))
    }

    fn bump(&self, app: AppId) {
        if let Ok(mut g) = self.generation.lock() {
            *g.entry(app).or_insert(0) += 1;
        }
        if let Ok(mut h) = self.health.lock() {
            h.remove(&app);
        }
    }

    /// Every field of a stored app, secrets read from the store; `None` when a
    /// stored secret cannot be read (locked).
    fn values(&self, app: AppId, entry: &AppEntry) -> Option<BTreeMap<String, String>> {
        let mut values = entry.fields.clone();
        for key in &entry.secrets {
            match self.secrets.get(&account(app, key)) {
                Ok(Some(v)) => {
                    values.insert(key.clone(), v);
                }
                _ => return None,
            }
        }
        Some(values)
    }

    /// Every form of an app's secrets, for scrubbing.
    fn forms(&self, app: AppId, values: &BTreeMap<String, String>) -> Vec<String> {
        let secrets: Vec<String> = values
            .iter()
            .filter(|(k, _)| app.is_secret(k))
            .map(|(_, v)| v.clone())
            .collect();
        let plain: Vec<String> = values
            .iter()
            .filter(|(k, _)| !app.is_secret(k))
            .map(|(_, v)| v.clone())
            .collect();
        secret_forms(&secrets, &plain)
    }

    fn info(&self, app: AppId, health: Option<FactoryHealth>) -> FactoryAppInfo {
        let entry = self.config.get(app);
        let locked = entry
            .as_ref()
            .is_some_and(|e| self.values(app, e).is_none());
        let health = match (&entry, locked) {
            (None, _) => FactoryHealth::Unconfigured,
            (Some(_), true) => FactoryHealth::Locked,
            (Some(_), false) => health
                .or_else(|| self.health.lock().ok().and_then(|h| h.get(&app).copied()))
                .unwrap_or(FactoryHealth::Unreachable),
        };
        FactoryAppInfo {
            id: app.as_str().to_owned(),
            label: app.label().to_owned(),
            url: entry.as_ref().map(|e| e.url.clone()),
            health,
            has_panels: app.has_panels(),
            credential_fields: app.credential_fields(),
            fields: entry.map(|e| e.fields),
            persistent: self.secrets.persistent(),
            preview: app.preview().map(str::to_owned),
        }
    }

    /// Every app. A configured app whose health is not known yet (the daemon just
    /// started) is checked first, all of them at once.
    pub async fn list(&self) -> FactoryAppList {
        let unknown: Vec<AppId> = AppId::ALL
            .into_iter()
            .filter(|a| self.config.get(*a).is_some())
            .filter(|a| self.health.lock().map_or(true, |h| !h.contains_key(a)))
            .collect();
        futures::future::join_all(unknown.into_iter().map(|a| self.check(a))).await;
        FactoryAppList {
            apps: AppId::ALL.into_iter().map(|a| self.info(a, None)).collect(),
        }
    }

    fn connection(&self, app: AppId) -> Option<(Connection, Vec<String>)> {
        let entry = self.config.get(app)?;
        let values = self.values(app, &entry)?;
        let forms = self.forms(app, &values);
        Some((
            Connection {
                url: entry.url,
                values,
                upstream: Arc::clone(&self.upstream),
            },
            forms,
        ))
    }

    /// Check an app and record its health (unless it was reconfigured meanwhile).
    async fn check(&self, app: AppId) -> Option<FactoryHealth> {
        let generation = self.generation(app);
        let (conn, _) = self.connection(app)?;
        let ctx = OpContext {
            now_ms: (self.clock)(),
        };
        let result = match tokio::time::timeout(QUERY_TIMEOUT, ops::health(app, &conn, ctx)).await {
            Ok(r) => r,
            Err(_) => Err(OpError::timeout()),
        };
        let health = match result {
            Ok(()) => FactoryHealth::Ok,
            Err(e) if e.code() == "auth_failed" => FactoryHealth::AuthFailed,
            Err(_) => FactoryHealth::Unreachable,
        };
        if self.generation(app) == generation
            && let Ok(mut h) = self.health.lock()
        {
            h.insert(app, health);
        }
        Some(health)
    }

    /// Test an app's stored connection.
    pub async fn test(&self, app: &str) -> FactoryAppReply {
        let Some(app) = AppId::parse(app) else {
            return FactoryAppReply {
                app: None,
                error: Some(err("unknown_app", "Unknown app")),
            };
        };
        let health = self.check(app).await;
        FactoryAppReply {
            app: Some(self.info(app, health)),
            error: None,
        }
    }

    /// Store an app's URL and fields, then test them. A secret left blank keeps its
    /// stored value, except across a change of origin.
    pub async fn configure(&self, params: &FactoryConfigureParams) -> FactoryAppReply {
        match self.store(params) {
            Ok(app) => self.test(app.as_str()).await,
            Err(e) => FactoryAppReply {
                app: None,
                error: Some(e),
            },
        }
    }

    fn store(&self, params: &FactoryConfigureParams) -> Result<AppId, FactoryError> {
        let app = AppId::parse(&params.app).ok_or_else(|| err("unknown_app", "Unknown app"))?;
        let url = check_url(&params.url, "Enter an http(s) URL")?;
        if let Some(app_url) = params.fields.get("appUrl").filter(|v| !v.is_empty()) {
            check_url(app_url, "Enter an http(s) App URL")?;
        }
        let mut clean: BTreeMap<String, String> = params
            .fields
            .iter()
            .filter(|(k, v)| app.has_field(k) && !v.is_empty())
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let stored = self.config.get(app);
        let same_origin = stored
            .as_ref()
            .and_then(|s| Url::parse(&s.url).ok())
            .is_some_and(|s| s.origin() == url.origin());
        if let Some(stored) = &stored {
            let kept = self.values(app, stored).unwrap_or_default();
            for (key, _, secret) in app.fields() {
                if *secret && !same_origin {
                    continue;
                }
                if !clean.contains_key(*key)
                    && let Some(v) = kept.get(*key).filter(|v| !v.is_empty())
                {
                    clean.insert((*key).to_owned(), v.clone());
                }
            }
        }
        if let Some((_, label, _)) = app
            .fields()
            .iter()
            .find(|(k, _, s)| *s && !clean.contains_key(*k))
        {
            return Err(err(
                "bad_fields",
                if stored.is_some() && !same_origin {
                    format!("{label} is required again: the URL points to a different origin")
                } else {
                    format!("{label} is required")
                },
            ));
        }
        self.bump(app);
        let mut entry = AppEntry {
            url: params.url.clone(),
            ..AppEntry::default()
        };
        for (key, value) in &clean {
            if app.is_secret(key) {
                self.secrets.set(&account(app, key), value).map_err(|e| {
                    err(
                        "store_failed",
                        format!("The keyring refused the secret: {e}"),
                    )
                })?;
                entry.secrets.push(key.clone());
            } else {
                entry.fields.insert(key.clone(), value.clone());
            }
        }
        // Studio's pull requests (G4) read Forgejo's URL beside its token.
        if app == AppId::Forgejo {
            let _ = self.secrets.set(&account(app, "url"), &params.url);
        }
        self.config.set(app, entry).map_err(|e| {
            err(
                "store_failed",
                format!("The factory's settings could not be saved: {e}"),
            )
        })?;
        Ok(app)
    }

    /// Forget an app and its secrets.
    pub fn remove(&self, app: &str) -> Result<(), FactoryError> {
        let app = AppId::parse(app).ok_or_else(|| err("unknown_app", "Unknown app"))?;
        self.bump(app);
        if let Some(entry) = self.config.get(app) {
            for key in &entry.secrets {
                let _ = self.secrets.delete(&account(app, key));
            }
        }
        if app == AppId::Forgejo {
            let _ = self.secrets.delete(&account(app, "url"));
        }
        self.config.remove(app).map_err(|e| {
            err(
                "store_failed",
                format!("The factory's settings could not be saved: {e}"),
            )
        })
    }

    /// Run one op from the allowlist.
    pub async fn query(&self, q: &FactoryQueryParams) -> FactoryQueryReply {
        let fail = |e: FactoryError| FactoryQueryReply {
            value: None,
            error: Some(e),
        };
        let Some(app) = AppId::parse(&q.app) else {
            return fail(err("unknown_app", "Unknown app"));
        };
        if !ops::ops(app).contains(&q.op.as_str()) {
            let shown: String = q.op.chars().take(40).collect();
            return fail(err("unknown_op", format!("Unknown operation: {shown}")));
        }
        let params = match ops::check(app, &q.op, &q.params) {
            Ok(p) => p,
            Err(e) => return fail(err(e.code(), e.message)),
        };
        let Some((conn, forms)) = self.connection(app) else {
            return fail(err("unconfigured", "This app is not configured yet"));
        };
        let ctx = OpContext {
            now_ms: (self.clock)(),
        };
        let result =
            match tokio::time::timeout(QUERY_TIMEOUT, ops::run(app, &q.op, &params, &conn, ctx))
                .await
            {
                Ok(r) => r,
                Err(_) => Err(OpError::timeout()),
            };
        match result {
            Ok(value) => FactoryQueryReply {
                value: Some(value),
                error: None,
            },
            Err(e) => fail(err(e.code(), redact(&e.message, &forms))),
        }
    }
}

fn check_url(raw: &str, message: &str) -> Result<Url, FactoryError> {
    let url = Url::parse(raw).map_err(|_| err("invalid_url", message))?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(err("invalid_url", message));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(err(
            "invalid_url",
            "Put credentials in the credential fields, not in the URL",
        ));
    }
    Ok(url)
}
