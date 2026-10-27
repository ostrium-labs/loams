//! The identity sync in the engine (design 76 §8.1, Q1213; plan IA1, IA3): on a single
//! install it runs as a task of the `loams` process, with its state in the metastore.
//!
//! Off unless `LOAMS_IDENTITY_SYNC=on`, and only with `LOAMS_AUTHZ=openfga`: its tuples go to
//! OpenFGA (the built-in authorizer reads the principal store instead, so there is nothing
//! to sync to).
//!
//! | Variable | |
//! |---|---|
//! | `LOAMS_IDENTITY_ZITADEL_URL` | the Zitadel instance |
//! | `LOAMS_IDENTITY_ZITADEL_TOKEN_FILE` | a personal access token with `events.read` and the management API's user-grant reads |
//! | `LOAMS_IDENTITY_ZITADEL_PROJECT` | the Loams project's id: grants on it are memberships (D1623) |
//! | `LOAMS_IDENTITY_SYNC_ISSUER` | the issuer the links name; default the first `LOAMS_OIDC_ISSUER`, else the instance URL |
//! | `LOAMS_IDENTITY_SYNC_WEBHOOK_LISTEN` | optional: where Zitadel's Actions v2 wake-up calls are taken (`POST /wake`) |
//! | `LOAMS_IDENTITY_SYNC_WEBHOOK_KEYS_FILE` | with the listener: the targets' signing keys, one per line |
//!
//! - The links go to the principal store ([`DirectoryPrincipals`]): a link it writes names
//!   the subject itself as the principal (`user:<sub>`, `service_account:<sub>`, D1605).
//! - The state is a document of its own in the metastore and the bucket
//!   ([`MetaSyncState`], pointer `identity-sync`).
//! - The webhook listener answers only `POST /wake`, never applies a body, and has no other
//!   route; without it the sync still reads every few seconds (D1622).

use std::fmt;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use bytes::Bytes;
use loams_identity_sync::webhook::{MAX_BODY, SIGNATURE_HEADER, Wake};
use loams_identity_sync::{
    Config, IdentitySync, Kind, Principals as SyncPrincipals, State, StateStore,
};
use tokio::sync::Notify;

use super::authz::AuthzConfig;
use super::directory::{
    Directory, DirectoryStore, LinkKind, MetaDirectoryStore, PrincipalLink, Unavailable,
};

/// The metastore pointer of the sync's state.
pub const STATE_POINTER: &str = "identity-sync";
const STATE_PREFIX: &str = "system/identity-sync/";

/// The identity sync's settings (module docs).
#[derive(Clone, PartialEq, Eq)]
pub struct SyncSettings {
    pub zitadel_url: String,
    pub token_file: PathBuf,
    pub project: String,
    pub issuer: String,
    pub webhook_listen: Option<SocketAddr>,
    pub webhook_keys_file: Option<PathBuf>,
}

impl fmt::Debug for SyncSettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SyncSettings")
            .field("zitadel_url", &self.zitadel_url)
            .field("project", &self.project)
            .field("issuer", &self.issuer)
            .field("webhook_listen", &self.webhook_listen)
            .finish_non_exhaustive()
    }
}

impl SyncSettings {
    /// From the environment through `var`; `None` while `LOAMS_IDENTITY_SYNC` is not `on`.
    ///
    /// # Errors
    ///
    /// A missing or malformed variable, named; `on` without `LOAMS_AUTHZ=openfga`.
    pub fn from_vars(
        var: impl Fn(&str) -> Option<String>,
        authz: &AuthzConfig,
        oidc_issuer: Option<&str>,
    ) -> Result<Option<Self>, String> {
        let get = |name: &str| {
            var(name)
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        };
        match get("LOAMS_IDENTITY_SYNC")
            .map(|v| v.to_ascii_lowercase())
            .as_deref()
        {
            None | Some("off") => return Ok(None),
            Some("on") => {}
            Some(other) => {
                return Err(format!(
                    "LOAMS_IDENTITY_SYNC={other:?} is not one of on, off"
                ));
            }
        }
        if !matches!(authz, AuthzConfig::OpenFga { .. }) {
            return Err(
                "LOAMS_IDENTITY_SYNC=on writes OpenFGA's tuples, so it needs LOAMS_AUTHZ=openfga"
                    .into(),
            );
        }
        let required =
            |name: &str| get(name).ok_or_else(|| format!("LOAMS_IDENTITY_SYNC=on needs {name}"));
        let zitadel_url = required("LOAMS_IDENTITY_ZITADEL_URL")?;
        let webhook_listen = get("LOAMS_IDENTITY_SYNC_WEBHOOK_LISTEN")
            .map(|v| {
                v.parse::<SocketAddr>()
                    .map_err(|e| format!("LOAMS_IDENTITY_SYNC_WEBHOOK_LISTEN={v:?}: {e}"))
            })
            .transpose()?;
        let webhook_keys_file = get("LOAMS_IDENTITY_SYNC_WEBHOOK_KEYS_FILE").map(PathBuf::from);
        if webhook_listen.is_some() && webhook_keys_file.is_none() {
            return Err(
                "LOAMS_IDENTITY_SYNC_WEBHOOK_LISTEN needs LOAMS_IDENTITY_SYNC_WEBHOOK_KEYS_FILE: \
                 an unsigned wake-up call is never taken"
                    .into(),
            );
        }
        Ok(Some(Self {
            issuer: get("LOAMS_IDENTITY_SYNC_ISSUER")
                .or_else(|| oidc_issuer.map(str::to_owned))
                .unwrap_or_else(|| zitadel_url.clone()),
            zitadel_url,
            token_file: PathBuf::from(required("LOAMS_IDENTITY_ZITADEL_TOKEN_FILE")?),
            project: required("LOAMS_IDENTITY_ZITADEL_PROJECT")?,
            webhook_listen,
            webhook_keys_file,
        }))
    }
}

/// The principal store as the sync's link store (D1624, D1628).
#[derive(Debug, Clone)]
pub struct DirectoryPrincipals(pub Arc<Directory>);

fn link_kind(kind: Kind) -> LinkKind {
    match kind {
        Kind::User => LinkKind::User,
        Kind::Agent => LinkKind::Agent,
        Kind::ServiceAccount => LinkKind::ServiceAccount,
    }
}

fn principals_error(err: Unavailable) -> loams_identity_sync::Error {
    loams_identity_sync::Error::Principals(err.0)
}

#[async_trait]
impl SyncPrincipals for DirectoryPrincipals {
    async fn link_if_absent(
        &self,
        issuer: &str,
        subject: &str,
        kind: Kind,
    ) -> loams_identity_sync::Result<()> {
        self.0
            .update(|doc| {
                if doc.link(issuer, subject).is_none() {
                    doc.links.push(PrincipalLink {
                        issuer: issuer.to_owned(),
                        subject: subject.to_owned(),
                        kind: link_kind(kind),
                        principal: subject.to_owned(),
                        disabled: false,
                    });
                }
                Ok::<_, Unavailable>(())
            })
            .await
            .map_err(principals_error)
    }

    async fn set_disabled(
        &self,
        issuer: &str,
        subject: &str,
        kind_if_absent: Kind,
        disabled: bool,
    ) -> loams_identity_sync::Result<()> {
        self.0
            .update(|doc| {
                match doc
                    .links
                    .iter_mut()
                    .find(|l| l.issuer == issuer && l.subject == subject)
                {
                    Some(link) => link.disabled = disabled,
                    None => doc.links.push(PrincipalLink {
                        issuer: issuer.to_owned(),
                        subject: subject.to_owned(),
                        kind: link_kind(kind_if_absent),
                        principal: subject.to_owned(),
                        disabled,
                    }),
                }
                Ok::<_, Unavailable>(())
            })
            .await
            .map_err(principals_error)
    }
}

/// The sync's state in the metastore and the bucket, written with compare-and-set: a second
/// engine running the sync against the same store loses the race and retries on its next
/// step, rather than overwriting the position.
pub struct MetaSyncState {
    store: Arc<dyn DirectoryStore>,
    version: Mutex<Option<u64>>,
}

impl fmt::Debug for MetaSyncState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MetaSyncState").finish_non_exhaustive()
    }
}

impl MetaSyncState {
    pub fn new(meta: Arc<dyn loams_common::meta::MetaStore>, store: loams_store::Store) -> Self {
        Self::over(Arc::new(MetaDirectoryStore::with_key(
            meta,
            store,
            STATE_POINTER,
            STATE_PREFIX,
        )))
    }

    /// Over any document store (tests use memory).
    pub fn over(store: Arc<dyn DirectoryStore>) -> Self {
        Self {
            store,
            version: Mutex::new(None),
        }
    }

    fn state_error(err: impl fmt::Display) -> loams_identity_sync::Error {
        loams_identity_sync::Error::State(err.to_string())
    }
}

#[async_trait]
impl StateStore for MetaSyncState {
    async fn load(&self) -> loams_identity_sync::Result<Option<State>> {
        let loaded = self.store.load().await.map_err(Self::state_error)?;
        let (version, state) = match loaded {
            Some((version, bytes)) => (
                Some(version),
                Some(serde_json::from_slice(&bytes).map_err(Self::state_error)?),
            ),
            None => (None, None),
        };
        *self
            .version
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = version;
        Ok(state)
    }

    async fn save(&self, state: &State) -> loams_identity_sync::Result<()> {
        let expected = *self
            .version
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let bytes = Bytes::from(serde_json::to_vec(state).map_err(Self::state_error)?);
        match self
            .store
            .save(expected, bytes)
            .await
            .map_err(Self::state_error)?
        {
            Some(version) => {
                *self
                    .version
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(version);
                Ok(())
            }
            None => Err(Self::state_error(
                "another writer moved the identity sync's state; the next step reads it again",
            )),
        }
    }
}

/// The running sync: stop it with [`Running::stop`].
#[derive(Debug)]
pub struct Running {
    shutdown: Arc<Notify>,
    pub webhook: Option<SocketAddr>,
}

impl Running {
    pub fn stop(&self) {
        self.shutdown.notify_one();
    }
}

/// Starts the sync (and the webhook listener when configured).
///
/// # Errors
///
/// An unreadable token or keys file, a bad OpenFGA endpoint, or a listener that cannot bind.
pub async fn start(
    settings: &SyncSettings,
    authz: &AuthzConfig,
    directory: Arc<Directory>,
    state: Arc<dyn StateStore>,
) -> Result<Running, String> {
    let AuthzConfig::OpenFga {
        grpc,
        store,
        model,
        key,
    } = authz
    else {
        return Err("the identity sync needs LOAMS_AUTHZ=openfga".into());
    };
    let token = std::fs::read_to_string(&settings.token_file)
        .map_err(|e| format!("LOAMS_IDENTITY_ZITADEL_TOKEN_FILE: {e}"))?
        .trim()
        .to_owned();
    if token.is_empty() {
        return Err("LOAMS_IDENTITY_ZITADEL_TOKEN_FILE is empty".into());
    }
    // The platform's roots, with the process's provider (ring when none was installed).
    let _ =
        rustls::crypto::CryptoProvider::install_default(rustls::crypto::ring::default_provider());
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("the identity sync's HTTP client: {e}"))?;
    let zitadel = loams_zitadel::Zitadel::new(&settings.zitadel_url, token, http)
        .map_err(|e| format!("LOAMS_IDENTITY_ZITADEL_URL: {e}"))?;
    let tuples = loams_authz::OpenFgaAuthorizer::connect_lazy(grpc, store, model, key.as_deref())
        .map_err(|e| format!("LOAMS_AUTHZ_OPENFGA_GRPC: {e}"))?;
    let sync = Arc::new(IdentitySync::new(
        zitadel,
        Arc::new(tuples),
        Arc::new(DirectoryPrincipals(directory)),
        state,
        Config::new(&settings.project, &settings.issuer),
    ));
    let shutdown = Arc::new(Notify::new());
    let webhook = match (&settings.webhook_listen, &settings.webhook_keys_file) {
        (Some(listen), Some(keys)) => {
            let keys: Vec<String> = std::fs::read_to_string(keys)
                .map_err(|e| format!("LOAMS_IDENTITY_SYNC_WEBHOOK_KEYS_FILE: {e}"))?
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .map(str::to_owned)
                .collect();
            if keys.is_empty() {
                return Err("LOAMS_IDENTITY_SYNC_WEBHOOK_KEYS_FILE holds no key".into());
            }
            Some(serve_webhook(*listen, Wake::new(keys, sync.waker()), shutdown.clone()).await?)
        }
        _ => None,
    };
    tokio::spawn(sync.run(shutdown.clone()));
    tracing::info!(
        zitadel = %settings.zitadel_url,
        project = %settings.project,
        webhook = ?webhook,
        "the identity sync is running"
    );
    Ok(Running { shutdown, webhook })
}

/// `POST /wake` on `listen`, and nothing else.
async fn serve_webhook(
    listen: SocketAddr,
    wake: Wake,
    shutdown: Arc<Notify>,
) -> Result<SocketAddr, String> {
    use axum::extract::DefaultBodyLimit;
    use axum::http::{HeaderMap, StatusCode};
    use axum::routing::post;
    let app = axum::Router::new()
        .route(
            "/wake",
            post(move |headers: HeaderMap, body: Bytes| {
                let wake = wake.clone();
                async move {
                    let header = headers.get(SIGNATURE_HEADER).and_then(|v| v.to_str().ok());
                    StatusCode::from_u16(wake.handle(header, &body))
                        .unwrap_or(StatusCode::UNAUTHORIZED)
                }
            }),
        )
        .layer(DefaultBodyLimit::max(MAX_BODY));
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .map_err(|e| format!("LOAMS_IDENTITY_SYNC_WEBHOOK_LISTEN {listen}: {e}"))?;
    let addr = listener
        .local_addr()
        .map_err(|e| format!("LOAMS_IDENTITY_SYNC_WEBHOOK_LISTEN {listen}: {e}"))?;
    tokio::spawn(async move {
        let stop = async move { shutdown.notified().await };
        if let Err(err) = axum::serve(listener, app)
            .with_graceful_shutdown(stop)
            .await
        {
            tracing::warn!(%err, "the identity sync's webhook listener stopped");
        }
    });
    Ok(addr)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use loams_identity_sync::StateStore as _;

    use super::*;
    use crate::auth::directory::MemoryStore;

    fn vars(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |name| map.get(name).cloned()
    }

    fn openfga() -> AuthzConfig {
        AuthzConfig::OpenFga {
            grpc: "http://openfga:8081".into(),
            store: "s".into(),
            model: "m".into(),
            key: None,
        }
    }

    #[test]
    fn off_by_default_and_only_with_openfga() {
        assert_eq!(
            SyncSettings::from_vars(vars(&[]), &openfga(), None),
            Ok(None)
        );
        let on = [
            ("LOAMS_IDENTITY_SYNC", "on"),
            ("LOAMS_IDENTITY_ZITADEL_URL", "https://auth.example"),
            ("LOAMS_IDENTITY_ZITADEL_TOKEN_FILE", "/run/pat"),
            ("LOAMS_IDENTITY_ZITADEL_PROJECT", "p1"),
        ];
        let err = SyncSettings::from_vars(vars(&on), &AuthzConfig::Builtin, None).unwrap_err();
        assert!(err.contains("LOAMS_AUTHZ=openfga"), "{err}");
        let s = SyncSettings::from_vars(vars(&on), &openfga(), Some("https://iss.example"))
            .unwrap()
            .unwrap();
        assert_eq!(
            s.issuer, "https://iss.example",
            "the OIDC issuer by default"
        );
        assert_eq!(s.webhook_listen, None);
        let mut listen = on.to_vec();
        listen.push(("LOAMS_IDENTITY_SYNC_WEBHOOK_LISTEN", "127.0.0.1:0"));
        let err = SyncSettings::from_vars(vars(&listen), &openfga(), None).unwrap_err();
        assert!(
            err.contains("KEYS_FILE"),
            "an unsigned webhook is refused: {err}"
        );
    }

    #[tokio::test]
    async fn links_are_written_once_and_disabled_in_place() {
        let directory = Directory::in_memory();
        let p = DirectoryPrincipals(directory.clone());
        p.link_if_absent("iss", "a1", Kind::Agent).await.unwrap();
        p.link_if_absent("iss", "a1", Kind::ServiceAccount)
            .await
            .unwrap();
        p.set_disabled("iss", "a1", Kind::User, true).await.unwrap();
        p.set_disabled("iss", "u9", Kind::User, true).await.unwrap();
        let doc = directory.read().await.unwrap();
        let a1 = doc.link("iss", "a1").unwrap();
        assert_eq!((a1.kind, a1.disabled), (LinkKind::Agent, true));
        let u9 = doc.link("iss", "u9").unwrap();
        assert_eq!(
            (u9.kind, u9.disabled, u9.principal.as_str()),
            (LinkKind::User, true, "u9")
        );
    }

    #[tokio::test]
    async fn the_state_round_trips_and_refuses_a_lost_race() {
        let store: Arc<dyn DirectoryStore> = Arc::new(MemoryStore::default());
        let a = MetaSyncState::over(store.clone());
        let b = MetaSyncState::over(store);
        assert_eq!(a.load().await.unwrap(), None);
        assert_eq!(b.load().await.unwrap(), None);
        let mut state = State::default();
        state.applied.insert("user/u1".into(), 3);
        a.save(&state).await.unwrap();
        assert!(
            b.save(&State::default()).await.is_err(),
            "b read before a wrote"
        );
        assert_eq!(b.load().await.unwrap(), Some(state.clone()));
        state.applied.insert("user/u2".into(), 1);
        b.save(&state).await.unwrap();
        assert_eq!(a.load().await.unwrap(), Some(state));
    }
}
