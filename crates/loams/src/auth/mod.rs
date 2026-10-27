//! Engine authentication for the preview release (API2-Q1, design §44 §5).
//!
//! One [`Auth`] guards every HTTP surface the engine serves to the network:
//! the main port (Connect, gRPC, gRPC-Web, the native REST and the House
//! proxy), the Qdrant REST and gRPC gateways, the Elasticsearch gateway and
//! Flight SQL, and Loams Live's own listener (`LiveService` and
//! `LiveAdminService`, LV1 plan Task 10). The wire protocols (PostgreSQL,
//! MySQL, native stream gRPC, durable) already refuse any address but
//! loopback, and the SQL gate has its own users; none of them is reachable
//! from a tailnet.
//!
//! The mode comes from `LOAMS_AUTH_MODE` ([`AuthConfig::from_env`]):
//!
//! - `apikey`: `Authorization: Bearer lk_…` (or the key as a Basic password,
//!   an Elasticsearch `ApiKey` or Qdrant's `api-key` header), checked against
//!   argon2id hashes in `LOAMS_API_KEYS_FILE` ([`api_key`]);
//! - `oidc`: an RS256 or EdDSA JWT from one of the issuers in
//!   `LOAMS_OIDC_ISSUER` (a comma list) for one of `LOAMS_OIDC_AUDIENCE`,
//!   verified against that issuer's JWKS ([`oidc`]), and admitted only when
//!   its subject, verified email or group is on the allowlist (a shared
//!   issuer signs tokens for everyone's console). With `LOAMS_API_KEYS_FILE`
//!   set, API keys are taken as well;
//! - `tailscale`: the `Tailscale-User-Login` header that `tailscale serve`
//!   adds, honoured only on the dedicated `LOAMS_TAILSCALE_LISTEN` listener
//!   (loopback, the `tailscale serve` target) and stripped everywhere else,
//!   optionally limited to `LOAMS_TAILSCALE_ALLOWED_USERS` ([`tailscale`]);
//! - a comma list of those three, such as `oidc,apikey`;
//! - `off` (the default): no authentication, and the server refuses to start
//!   unless every listener is on loopback.
//!
//! Whatever the mode, the engine also takes the credentials of its own
//! principal store ([`directory`], API2 Tasks 6 and 7): the console's
//! session (the `loams_session` cookie or an `ls_…` bearer, from
//! `AuthService/CreateSession`) and the API keys `KeyService/CreateKey`
//! keeps in the metastore. With `off`, a valid session still names its
//! person, and anything else is the developer on loopback.
//!
//! [`Authenticator`] is the hook MT1 extends (API2 Task 6): the layer
//! ([`layer`]) turns a request's credential into a [`Principal`] in the
//! request extensions, and no handler parses a header. [`authorizer`] is the
//! second hook: what an admitted principal may do in the org.

pub mod api_key;
pub mod authorizer;
pub mod authz;
pub mod directory;
pub mod identity_sync;
pub mod layer;
pub mod oidc;
pub mod tailscale;

use std::collections::BTreeSet;
use std::fmt;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use http::StatusCode;

pub use layer::{AuthLayerState, Cors};

/// The environment variable that picks the mode.
pub const MODE_ENV: &str = "LOAMS_AUTH_MODE";

/// The environment variable holding the first-run setup token
/// (`AuthService/CompleteSetup` on an engine with authentication on).
pub const SETUP_TOKEN_ENV: &str = "LOAMS_SETUP_TOKEN";

/// The RPCs answered without a credential (the API2 shared contract's
/// `PUBLIC_RPCS`, with ruling 6/7.9's `CreateSession` and the server half's
/// `DeleteSession`). [`layer::MAIN_PUBLIC_PATHS`] is these plus the probes.
pub const PUBLIC_RPCS: &[&str] = &[
    "/loams.instance.v1.InstanceService/GetInstance",
    "/loams.auth.v1.AuthService/ListProviders",
    "/loams.auth.v1.AuthService/CompleteSetup",
    "/loams.auth.v1.AuthService/CreateSession",
    "/loams.auth.v1.AuthService/DeleteSession",
    "/grpc.health.v1.Health/Check",
    "/grpc.health.v1.Health/Watch",
];

/// Who made a request (API2 shared contract).
#[derive(Clone, PartialEq, Eq)]
pub struct Principal {
    /// The OIDC `sub`, the API key's id, or `dev-loopback`.
    pub subject: String,
    pub kind: PrincipalKind,
    /// The issuer that signed it, for an OIDC JWT (its exact `iss`): with `subject`, the key of
    /// its principal link (design 76 §5, D1628). `None` for every other credential.
    pub issuer: Option<String>,
    /// A name to show: the token's `name` or `preferred_username`, or the
    /// API key's label.
    pub display_name: String,
    /// A verified email, for users.
    pub email: Option<String>,
    /// What the credential reaches (LV1 plan Task 14): its org,
    /// environment, scopes and expiry where its source names them. Empty
    /// is the self-hosted default: the install's one org and environment,
    /// every scope, no expiry beyond the credential's own checks.
    pub grant: Grant,
}

/// What a credential reaches, as its source states it (LV1 plan Task 14,
/// design §45 §5). `None` in a field means the source does not say, which
/// on a self-hosted install is its one org, its one environment, and every
/// scope. MT1 and API2 fill these for every credential later.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Grant {
    /// The org's slug (an OIDC `org` claim; the org of a session or of a
    /// key `KeyService/CreateKey` made).
    pub org: Option<String>,
    /// The environment the credential is bound to, `<project>/<environment>`
    /// (an OIDC `env` claim; a `KeyService` key's project and environment).
    pub environment: Option<String>,
    /// The scopes it carries (an OIDC `scp` or `scope` claim; a key's
    /// scopes). `None`: every scope.
    pub scopes: Option<BTreeSet<String>>,
    /// When it expires, Unix ms (a token's `exp`, a session's or a key's
    /// expiry).
    pub expires_at_ms: Option<u64>,
}

impl Grant {
    /// Whether the credential carries `scope` (every scope when it names
    /// none).
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.as_ref().is_none_or(|s| s.contains(scope))
    }

    /// Scopes from a list, `None` when it is empty (a key or token that
    /// names no scope keeps the self-hosted default: every scope).
    pub fn scopes_of<I, S>(scopes: I) -> Option<BTreeSet<String>>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let set: BTreeSet<String> = scopes
            .into_iter()
            .map(|s| s.as_ref().trim().to_owned())
            .filter(|s| !s.is_empty())
            .collect();
        (!set.is_empty()).then_some(set)
    }
}

impl fmt::Debug for Principal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Principal")
            .field("subject", &self.subject)
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

/// What kind of credential a [`Principal`] presented.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrincipalKind {
    /// An OIDC user (a JWT from the issuer).
    User,
    /// A person signed in to the console (`AuthService/CreateSession`);
    /// `subject` is their user id in the org.
    Session,
    /// An API key: from `LOAMS_API_KEYS_FILE` (`subject` `apikey:<id>`), or
    /// one `KeyService/CreateKey` made (the same `subject` shape; the
    /// metastore has its record).
    ApiKey,
    /// A tailnet user, as `tailscale serve` named them.
    Tailscale,
    /// `LOAMS_AUTH_MODE=off` on loopback: everyone on this host.
    DevLoopback,
}

/// How a refusal is answered: `401`, `403` or `503`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthErrorKind {
    /// No usable credential: `401`, gRPC `UNAUTHENTICATED`.
    Unauthenticated,
    /// A valid credential this engine does not admit: `403`, gRPC
    /// `PERMISSION_DENIED`.
    Forbidden,
    /// The credential could not be checked now (the JWKS is unreachable):
    /// `503`, gRPC `UNAVAILABLE`.
    Unavailable,
}

/// A refused request. `reason` is a stable machine-readable code; `message`
/// says what to do. Neither ever contains the credential.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthError {
    pub kind: AuthErrorKind,
    pub reason: &'static str,
    pub message: String,
}

impl AuthError {
    pub(crate) fn unauthenticated(reason: &'static str, message: impl Into<String>) -> Self {
        Self {
            kind: AuthErrorKind::Unauthenticated,
            reason,
            message: message.into(),
        }
    }

    pub(crate) fn forbidden(reason: &'static str, message: impl Into<String>) -> Self {
        Self {
            kind: AuthErrorKind::Forbidden,
            reason,
            message: message.into(),
        }
    }

    pub(crate) fn unavailable(reason: &'static str, message: impl Into<String>) -> Self {
        Self {
            kind: AuthErrorKind::Unavailable,
            reason,
            message: message.into(),
        }
    }

    /// The HTTP status of the refusal.
    pub fn status(&self) -> StatusCode {
        match self.kind {
            AuthErrorKind::Unauthenticated => StatusCode::UNAUTHORIZED,
            AuthErrorKind::Forbidden => StatusCode::FORBIDDEN,
            AuthErrorKind::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        }
    }
}

/// The reasons a refusal carries (`loams-auth-reason`, the body's `reason`).
pub mod reason {
    pub const TOKEN_MISSING: &str = "token_missing";
    pub const TOKEN_MALFORMED: &str = "token_malformed";
    pub const TOKEN_EXPIRED: &str = "token_expired";
    /// A revoked API key, or a session that was signed out or whose person
    /// left the org.
    pub const TOKEN_REVOKED: &str = "token_revoked";
    /// A cookie-authenticated call that is not a `GET` without the
    /// session's `X-CSRF-Token`.
    pub const CSRF_TOKEN_INVALID: &str = "csrf_token_invalid";
    pub const TOKEN_NOT_YET_VALID: &str = "token_not_yet_valid";
    pub const TOKEN_BAD_SIGNATURE: &str = "token_bad_signature";
    pub const TOKEN_WRONG_AUDIENCE: &str = "token_wrong_audience";
    pub const TOKEN_WRONG_ISSUER: &str = "token_wrong_issuer";
    pub const TOKEN_UNKNOWN_KEY: &str = "token_unknown_key";
    pub const TOKEN_ALGORITHM: &str = "token_algorithm_not_allowed";
    pub const API_KEY_INVALID: &str = "api_key_invalid";
    pub const CREDENTIAL_NOT_ACCEPTED: &str = "credential_not_accepted";
    pub const CREDENTIAL_IN_QUERY_STRING: &str = "credential_in_query_string";
    pub const PRINCIPAL_NOT_ALLOWED: &str = "principal_not_allowed";
    pub const AUTH_UNAVAILABLE: &str = "auth_unavailable";
    pub const CORS_ORIGIN_NOT_ALLOWED: &str = "cors_origin_not_allowed";
}

/// A credential as the request carried it.
#[derive(Clone, PartialEq, Eq)]
pub enum Credential {
    /// An `lk_…` API key (from a bearer, a Basic password, an `ApiKey` or an
    /// `api-key` header).
    ApiKey(String),
    /// A bearer that is not an API key: a JWT.
    Jwt(String),
    /// A console session: an `ls_…` bearer, or the `loams_session` cookie.
    Session(SessionCredential),
}

/// A console session token as the request carried it.
#[derive(Clone, PartialEq, Eq)]
pub struct SessionCredential {
    pub token: String,
    /// `Some` when the token came in the cookie on a call that must prove it
    /// is the console's own (anything but a `GET` and `GetSession`): the
    /// `X-CSRF-Token` header's value, empty when it was absent. The session
    /// is admitted only if it matches.
    pub csrf: Option<String>,
}

impl fmt::Debug for SessionCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SessionCredential([redacted])")
    }
}

impl fmt::Debug for Credential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ApiKey(_) => f.write_str("ApiKey([redacted])"),
            Self::Jwt(_) => f.write_str("Jwt([redacted])"),
            Self::Session(_) => f.write_str("Session([redacted])"),
        }
    }
}

/// Turns a credential into a principal. One implementation per credential
/// kind; MT1 adds its own without touching handlers (API2 Task 6).
#[async_trait]
pub trait Authenticator: Send + Sync + 'static {
    /// `None`: this authenticator does not take this kind of credential.
    async fn authenticate(&self, credential: &Credential) -> Option<Result<Principal, AuthError>>;
}

/// Which credentials the engine takes (`LOAMS_AUTH_MODE`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthMode {
    /// No authentication; loopback only.
    Off,
    /// API keys, OIDC JWTs, Tailscale identities, or several.
    On {
        api_keys: bool,
        oidc: bool,
        tailscale: bool,
    },
}

impl AuthMode {
    /// Parses `off`, `apikey`, `oidc`, `tailscale`, or a comma list of the
    /// last three.
    pub fn parse(value: &str) -> Result<Self, String> {
        let value = value.trim();
        if value.is_empty() || value.eq_ignore_ascii_case("off") {
            return Ok(Self::Off);
        }
        let (mut api_keys, mut oidc, mut tailscale) = (false, false, false);
        for part in value.split(',').map(str::trim) {
            match part.to_ascii_lowercase().as_str() {
                "apikey" | "api-key" | "api_key" => api_keys = true,
                "oidc" | "jwt" => oidc = true,
                "tailscale" | "ts" => tailscale = true,
                "off" => return Err(format!("{MODE_ENV}={value:?}: off cannot be combined")),
                other => {
                    return Err(format!(
                        "{MODE_ENV}={value:?}: unknown mode {other:?}; expected off, apikey, \
                         oidc, tailscale or a comma list such as oidc,apikey"
                    ));
                }
            }
        }
        Ok(Self::On {
            api_keys,
            oidc,
            tailscale,
        })
    }
}

/// One `LOAMS_CORS_ORIGINS` entry: an exact origin, `scheme://host[:port]`
/// (a trailing `/` is dropped), lowercased as browsers send it. Wildcards,
/// `null`, paths, queries and credentials are refused: an origin list that
/// could match more than it names is not an allowlist.
pub fn parse_origin(text: &str) -> Result<String, String> {
    let origin = text.trim().trim_end_matches('/');
    let refuse = |why: &str| {
        Err(format!(
            "{text:?} is not an origin ({why}; expected scheme://host[:port])"
        ))
    };
    let Some((scheme, authority)) = origin.split_once("://") else {
        return refuse("no scheme");
    };
    let scheme_ok = scheme
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    if !scheme_ok {
        return refuse("a bad scheme");
    }
    if authority.is_empty() {
        return refuse("no host");
    }
    if let Some(c) = authority
        .chars()
        .find(|c| matches!(c, '/' | '?' | '#' | '@' | '*' | '\\') || c.is_whitespace())
    {
        return refuse(&format!("{c:?} is not allowed"));
    }
    Ok(origin.to_ascii_lowercase())
}

/// The authentication settings ([`AuthConfig::from_env`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthConfig {
    pub mode: AuthMode,
    /// `LOAMS_API_KEYS_FILE`: `<id> <argon2id PHC> [label]` per line.
    pub api_keys_file: Option<PathBuf>,
    /// The key file was given beside `oidc` (not `apikey`): a missing file
    /// turns API keys off with a warning instead of refusing to start.
    pub api_keys_optional: bool,
    pub oidc: Option<oidc::OidcConfig>,
    /// `LOAMS_TAILSCALE_LISTEN` and `LOAMS_TAILSCALE_ALLOWED_USERS`.
    pub tailscale: Option<tailscale::TailscaleConfig>,
    /// `LOAMS_CORS_ORIGINS`: the extra browser origins allowed to call the
    /// engine cross-origin, each an exact `scheme://host[:port]`. Empty (the
    /// default) allows none: the console ships with the deployment and is
    /// served by the engine itself ([`crate::console`]), so its calls are
    /// same-origin (the request's own `Host`), and Loams Desktop's main
    /// process sends the engine's own origin. Deny by default, on every
    /// listener; the `tailscale serve` target, whose identity is ambient,
    /// also refuses a request from any other origin outright.
    pub cors_origins: Vec<String>,
    /// `LOAMS_SETUP_TOKEN`: what `AuthService/CompleteSetup` must carry on
    /// an engine with authentication on. With authentication off (loopback
    /// only) setup needs no token; with it on and no token set, setup is
    /// refused until the operator sets one.
    pub setup_token: Option<String>,
    /// Authenticators to consult after the mode's own (MT1's, or a test's):
    /// the hook that proves a new credential kind needs no handler change
    /// (API2 Task 6, `authenticator_is_swappable`). Any of them turns
    /// authentication on.
    pub extra_authenticators: Authenticators,
    /// `LOAMS_AUTHZ`: which authorizer answers the `can_*` checks (design 76 §9, D1610).
    pub authz: authz::AuthzConfig,
}

/// A list of [`Authenticator`]s that a config can carry (cloned by
/// reference, equal when they are the same objects).
#[derive(Clone, Default)]
pub struct Authenticators(pub Vec<Arc<dyn Authenticator>>);

impl fmt::Debug for Authenticators {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Authenticators({})", self.0.len())
    }
}

impl PartialEq for Authenticators {
    fn eq(&self, other: &Self) -> bool {
        self.0.len() == other.0.len() && self.0.iter().zip(&other.0).all(|(a, b)| Arc::ptr_eq(a, b))
    }
}

impl Eq for Authenticators {}

impl Default for AuthConfig {
    fn default() -> Self {
        Self::off()
    }
}

impl AuthConfig {
    /// No authentication (loopback only).
    pub fn off() -> Self {
        Self {
            mode: AuthMode::Off,
            api_keys_file: None,
            api_keys_optional: false,
            oidc: None,
            tailscale: None,
            cors_origins: Vec::new(),
            setup_token: None,
            extra_authenticators: Authenticators::default(),
            authz: authz::AuthzConfig::Builtin,
        }
    }

    /// API keys from `file` only.
    pub fn api_keys(file: impl Into<PathBuf>) -> Self {
        Self {
            mode: AuthMode::On {
                api_keys: true,
                oidc: false,
                tailscale: false,
            },
            api_keys_file: Some(file.into()),
            ..Self::off()
        }
    }

    /// Whether requests are authenticated at all.
    pub fn enforced(&self) -> bool {
        self.mode != AuthMode::Off || !self.extra_authenticators.0.is_empty()
    }

    /// Reads `LOAMS_AUTH_MODE`, `LOAMS_API_KEYS_FILE`, `LOAMS_OIDC_*`,
    /// `LOAMS_TAILSCALE_*` and `LOAMS_CORS_ORIGINS` through `var`. Errors name the variable.
    pub fn from_vars(var: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let mode = AuthMode::parse(&var(MODE_ENV).unwrap_or_default())?;
        let list = |name: &str| -> Vec<String> {
            var(name)
                .unwrap_or_default()
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect()
        };
        let mut config = Self {
            mode: mode.clone(),
            api_keys_file: var("LOAMS_API_KEYS_FILE")
                .filter(|v| !v.trim().is_empty())
                .map(PathBuf::from),
            api_keys_optional: false,
            oidc: None,
            tailscale: None,
            cors_origins: list("LOAMS_CORS_ORIGINS")
                .iter()
                .map(|o| parse_origin(o))
                .collect::<Result<_, _>>()
                .map_err(|e| format!("LOAMS_CORS_ORIGINS: {e}"))?,
            setup_token: var(SETUP_TOKEN_ENV)
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty()),
            extra_authenticators: Authenticators::default(),
            authz: authz::AuthzConfig::from_vars(&var, mode == AuthMode::Off)?,
        };
        if let AuthMode::On {
            api_keys,
            oidc,
            tailscale,
        } = mode
        {
            // `oidc` with a key file also takes API keys (the compose default:
            // OIDC for the console, keys for the CLI and SDKs); the file may
            // be absent then.
            if oidc && !api_keys && config.api_keys_file.is_some() {
                config.mode = AuthMode::On {
                    api_keys: true,
                    oidc: true,
                    tailscale,
                };
                config.api_keys_optional = true;
            }
            if api_keys && config.api_keys_file.is_none() {
                return Err(format!(
                    "{MODE_ENV} includes apikey but LOAMS_API_KEYS_FILE is not set; create one \
                     with `loams auth new-key`"
                ));
            }
            if tailscale {
                let listen = match var("LOAMS_TAILSCALE_LISTEN").filter(|v| !v.trim().is_empty()) {
                    Some(v) => v.trim().parse::<SocketAddr>().map_err(|e| {
                        format!("LOAMS_TAILSCALE_LISTEN={v:?}: {e} (expected 127.0.0.1:7781)")
                    })?,
                    None => tailscale::DEFAULT_LISTEN,
                };
                config.tailscale = Some(tailscale::TailscaleConfig {
                    listen,
                    allowed_users: list("LOAMS_TAILSCALE_ALLOWED_USERS")
                        .into_iter()
                        .map(|u| u.to_ascii_lowercase())
                        .collect(),
                });
                config.tailscale_listener()?;
            }
            if oidc {
                let required = |name: &str| {
                    let values = list(name);
                    if values.is_empty() {
                        Err(format!("{MODE_ENV} includes oidc but {name} is not set"))
                    } else {
                        Ok(values)
                    }
                };
                let issuers = required("LOAMS_OIDC_ISSUER")?;
                // The n-th JWKS URL is the n-th issuer's; an empty entry
                // means discovery.
                let jwks: Vec<String> = var("LOAMS_OIDC_JWKS_URL")
                    .unwrap_or_default()
                    .split(',')
                    .map(|s| s.trim().to_owned())
                    .collect();
                if jwks.len() > issuers.len() {
                    return Err(
                        "LOAMS_OIDC_JWKS_URL has more entries than LOAMS_OIDC_ISSUER".to_owned(),
                    );
                }
                config.oidc = Some(oidc::OidcConfig {
                    issuers: issuers
                        .into_iter()
                        .enumerate()
                        .map(|(n, issuer)| oidc::Issuer {
                            issuer,
                            jwks_url: jwks.get(n).filter(|u| !u.is_empty()).cloned(),
                        })
                        .collect(),
                    audiences: required("LOAMS_OIDC_AUDIENCE")?,
                    allowed_subjects: list("LOAMS_OIDC_ALLOWED_SUBJECTS").into_iter().collect(),
                    allowed_emails: list("LOAMS_OIDC_ALLOWED_EMAILS")
                        .into_iter()
                        .map(|e| e.to_ascii_lowercase())
                        .collect(),
                    allowed_groups: list("LOAMS_OIDC_ALLOWED_GROUPS").into_iter().collect(),
                });
            }
        }
        Ok(config)
    }

    /// [`AuthConfig::from_vars`] on the process environment.
    pub fn from_env() -> Result<Self, String> {
        Self::from_vars(|name| std::env::var(name).ok())
    }

    /// The `tailscale serve` target, in `tailscale` mode. It must be
    /// loopback: whoever reaches it may name any tailnet user.
    pub fn tailscale_listener(&self) -> Result<Option<SocketAddr>, String> {
        let Some(ts) = &self.tailscale else {
            return Ok(None);
        };
        if !ts.listen.ip().is_loopback() {
            return Err(format!(
                "LOAMS_TAILSCALE_LISTEN {} is not a loopback address; whoever reaches it can \
                 claim any tailnet identity, so only the tailscale sidecar (sharing the \
                 engine's network namespace) may: bind it to 127.0.0.1",
                ts.listen
            ));
        }
        Ok(Some(ts.listen))
    }

    /// Refuses `off` unless every listener in `listeners` is loopback. The
    /// error names the first one that is not.
    pub fn check_listeners(&self, listeners: &[(&str, SocketAddr)]) -> Result<(), String> {
        if self.enforced() {
            return Ok(());
        }
        match listeners.iter().find(|(_, addr)| !addr.ip().is_loopback()) {
            Some((name, addr)) => Err(format!(
                "{name} {addr} is not a loopback address and {MODE_ENV} is off; bind it to \
                 127.0.0.1, or set {MODE_ENV}=apikey (with LOAMS_API_KEYS_FILE), oidc \
                 (with LOAMS_OIDC_ISSUER and LOAMS_OIDC_AUDIENCE) or tailscale"
            )),
            None => Ok(()),
        }
    }
}

/// Who may run first-run setup (`AuthService/CompleteSetup`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SetupGate {
    /// Anyone who reaches the engine: authentication is off, so every
    /// listener is loopback.
    Open,
    /// Whoever carries `LOAMS_SETUP_TOKEN`.
    Token(String),
    /// Nobody: authentication is on and no setup token is set.
    Closed,
}

/// What the main port's `loams.auth.v1` and `loams.admin.v1` stand on: the
/// principal store, the authorizer that reads roles from it, and the setup
/// gate.
#[derive(Clone)]
pub struct Principals {
    pub directory: Arc<directory::Directory>,
    /// The org roles, for the admin RPCs (until IA2 Task 5 declares a `can_*` per RPC).
    pub authorizer: Arc<dyn authorizer::Authorizer>,
    /// The `can_*` checks (design 76 §9, D1610): [`authz::DirectoryAuthorizer`] by default,
    /// OpenFGA with `LOAMS_AUTHZ=openfga`.
    pub authz: Arc<dyn loams_authz::Authorizer>,
    pub setup: SetupGate,
}

impl fmt::Debug for Principals {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Principals")
            .field("directory", &self.directory)
            .field("setup", &matches!(self.setup, SetupGate::Open))
            .finish_non_exhaustive()
    }
}

impl Principals {
    /// `directory` with [`authorizer::OrgRoleAuthorizer`] and the built-in
    /// [`authz::DirectoryAuthorizer`] over it.
    pub fn new(directory: Arc<directory::Directory>, setup: SetupGate) -> Self {
        Self {
            authorizer: Arc::new(authorizer::OrgRoleAuthorizer::new(directory.clone())),
            authz: Arc::new(authz::DirectoryAuthorizer::new(directory.clone())),
            directory,
            setup,
        }
    }

    /// The same, with `authz` answering the `can_*` checks.
    #[must_use]
    pub fn with_authz(mut self, authz: Arc<dyn loams_authz::Authorizer>) -> Self {
        self.authz = authz;
        self
    }

    /// The principal store over `meta` and `store`, attached to `auth` so
    /// its sessions and keys authenticate, with the setup gate `auth`'s
    /// mode implies.
    pub fn attach(
        auth: &Auth,
        meta: Arc<dyn loams_common::meta::MetaStore>,
        store: loams_store::Store,
    ) -> Self {
        let directory = match auth.directory() {
            Some(existing) => existing.clone(),
            None => {
                let directory = directory::Directory::new(meta, store);
                auth.attach_directory(directory.clone());
                directory
            }
        };
        let setup = match (auth.enforced(), auth.setup_token()) {
            (false, _) => SetupGate::Open,
            (true, Some(token)) => SetupGate::Token(token.to_owned()),
            (true, None) => SetupGate::Closed,
        };
        Self::new(directory, setup)
    }
}

/// The engine's authenticators, built from an [`AuthConfig`].
pub struct Auth {
    authenticators: Vec<Arc<dyn Authenticator>>,
    tailscale: Option<tailscale::TailscaleAuth>,
    enforced: bool,
    cors: Cors,
    /// CORS on the `tailscale serve` target: never any origin, since the
    /// tailnet identity there is ambient, like a cookie.
    tailscale_cors: Cors,
    /// The engine's own principal store (sessions and metastore API keys),
    /// attached once the metastore is up ([`Auth::attach_directory`]).
    directory: OnceLock<Arc<directory::Directory>>,
    /// `LOAMS_SETUP_TOKEN`.
    setup_token: Option<String>,
}

impl fmt::Debug for Auth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Auth")
            .field("authenticators", &self.authenticators.len())
            .field("enforced", &self.enforced)
            .finish_non_exhaustive()
    }
}

impl Auth {
    /// Builds the authenticators: reads and checks the API key file and the
    /// OIDC settings (the JWKS is fetched on first use).
    pub fn from_config(config: &AuthConfig) -> Result<Arc<Self>, String> {
        let mut authenticators: Vec<Arc<dyn Authenticator>> = Vec::new();
        let mut tailscale = None;
        if let AuthMode::On {
            api_keys,
            oidc,
            tailscale: ts,
        } = &config.mode
        {
            if *ts {
                let ts = config.tailscale.clone().unwrap_or_default();
                if ts.allowed_users.is_empty() {
                    tracing::warn!(
                        "tailscale mode without LOAMS_TAILSCALE_ALLOWED_USERS: every tailnet user \
                         your ACLs let reach this node is admitted"
                    );
                }
                tailscale = Some(tailscale::TailscaleAuth::new(&ts));
            }
            if *api_keys {
                let file = config
                    .api_keys_file
                    .as_ref()
                    .ok_or("apikey mode needs LOAMS_API_KEYS_FILE")?;
                match (config.api_keys_optional, file.try_exists()) {
                    (true, Ok(false)) => {
                        tracing::warn!(
                            path = %file.display(),
                            "LOAMS_API_KEYS_FILE does not exist: API keys are off; create keys \
                             with `loams auth new-key --append <file>` and restart"
                        );
                    }
                    // Not known to be missing (say its directory is not searchable): say so.
                    (true, Err(err)) => {
                        tracing::warn!(
                            "{}; API keys are off until the engine can read it and restarts",
                            api_key::read_error(file, &err)
                        );
                    }
                    _ => {
                        let keys = api_key::ApiKeyAuthenticator::from_file(file)?;
                        authenticators.push(Arc::new(keys));
                    }
                }
            }
            if *oidc {
                let oidc = config.oidc.as_ref().ok_or("oidc mode needs LOAMS_OIDC_*")?;
                authenticators.push(Arc::new(oidc::OidcAuthenticator::new(oidc.clone())?));
            }
        }
        authenticators.extend(config.extra_authenticators.0.iter().cloned());
        // Deny by default: only the configured origins, beside the engine's
        // own (the layer's same-host check), on every listener.
        let origins = config.cors_origins.iter().cloned().collect::<BTreeSet<_>>();
        let cors = Cors::new(origins.clone());
        let tailscale_cors = Cors::new(origins);
        Ok(Arc::new(Self {
            authenticators,
            tailscale,
            enforced: config.enforced(),
            cors,
            tailscale_cors,
            directory: OnceLock::new(),
            setup_token: config.setup_token.clone(),
        }))
    }

    /// The given authenticators (tests, and MT1's own).
    pub fn with_authenticators(
        authenticators: Vec<Arc<dyn Authenticator>>,
        cors_origins: BTreeSet<String>,
    ) -> Arc<Self> {
        Arc::new(Self {
            authenticators,
            tailscale: None,
            enforced: true,
            cors: Cors::new(cors_origins),
            tailscale_cors: Cors::default(),
            directory: OnceLock::new(),
            setup_token: None,
        })
    }

    /// No authentication: every request is [`PrincipalKind::DevLoopback`]
    /// (or the person whose console session it carries).
    pub fn off() -> Arc<Self> {
        Arc::new(Self {
            authenticators: Vec::new(),
            tailscale: None,
            enforced: false,
            cors: Cors::new(BTreeSet::new()),
            tailscale_cors: Cors::default(),
            directory: OnceLock::new(),
            setup_token: None,
        })
    }

    pub fn enforced(&self) -> bool {
        self.enforced
    }

    /// Takes the principal store's credentials (console sessions and the
    /// metastore's API keys) from now on. The first directory attached
    /// stays; a second call is ignored.
    pub fn attach_directory(&self, directory: Arc<directory::Directory>) {
        let _ = self.directory.set(directory);
    }

    /// The attached principal store.
    pub fn directory(&self) -> Option<&Arc<directory::Directory>> {
        self.directory.get()
    }

    /// `LOAMS_SETUP_TOKEN`.
    pub(crate) fn setup_token(&self) -> Option<&str> {
        self.setup_token.as_deref()
    }

    pub(crate) fn tailscale(&self) -> Option<&tailscale::TailscaleAuth> {
        self.tailscale.as_ref()
    }

    pub(crate) fn tailscale_cors(&self) -> &Cors {
        &self.tailscale_cors
    }

    pub(crate) fn cors(&self) -> &Cors {
        &self.cors
    }

    /// The principal for `credential`: the first authenticator that takes
    /// it decides.
    pub async fn authenticate(&self, credential: &Credential) -> Result<Principal, AuthError> {
        // The principal store first: it decides its own sessions, and the
        // API keys it holds (a key it does not hold falls through to the
        // key file).
        if let Some(directory) = self.directory.get()
            && let Some(result) = directory.authenticate(credential).await
        {
            return result;
        }
        for authenticator in &self.authenticators {
            if let Some(result) = authenticator.authenticate(credential).await {
                return match result {
                    Ok(principal) => self.refuse_disabled(principal).await,
                    Err(err) => Err(err),
                };
            }
        }
        let what = match credential {
            Credential::ApiKey(_) if self.directory.get().is_some() => {
                return Err(AuthError::unauthenticated(
                    reason::API_KEY_INVALID,
                    "the API key is not valid",
                ));
            }
            Credential::ApiKey(_) => "API keys are not enabled on this engine (LOAMS_AUTH_MODE)",
            Credential::Jwt(_) => "OIDC tokens are not enabled on this engine (LOAMS_AUTH_MODE)",
            Credential::Session(_) => "console sessions are not served by this listener",
        };
        Err(AuthError::unauthenticated(
            reason::CREDENTIAL_NOT_ACCEPTED,
            what,
        ))
    }

    /// Refuses a principal whose link is disabled, or whose agent is suspended, on every
    /// request (design 76 §10, D1624): an identity provider's JWT stays valid until `exp`
    /// after the principal is deactivated, so its signature alone does not admit it.
    async fn refuse_disabled(&self, principal: Principal) -> Result<Principal, AuthError> {
        let (Some(issuer), Some(directory)) = (principal.issuer.as_deref(), self.directory.get())
        else {
            return Ok(principal);
        };
        let doc = directory.read().await.map_err(|err| {
            tracing::warn!(error = %err.0, "reading the principal store to admit a token failed");
            AuthError::unavailable(
                reason::AUTH_UNAVAILABLE,
                "the principal store could not be read; retry",
            )
        })?;
        let Some(link) = doc.link(issuer, &principal.subject) else {
            return Ok(principal);
        };
        let suspended = link.kind == directory::LinkKind::Agent
            && doc.agent(&link.principal).is_none_or(|a| {
                a.agent.status.as_known()
                    == Some(loams_proto::loams::admin::v1::AgentStatus::AGENT_STATUS_SUSPENDED)
            });
        if link.disabled || suspended {
            return Err(AuthError::unauthenticated(
                reason::TOKEN_REVOKED,
                "this principal is disabled; its tokens are refused",
            ));
        }
        Ok(principal)
    }

    /// The principal of an unauthenticated engine.
    pub(crate) fn dev_loopback() -> Principal {
        Principal {
            subject: "dev-loopback".to_owned(),
            kind: PrincipalKind::DevLoopback,
            issuer: None,
            display_name: "Local developer".to_owned(),
            email: None,
            grant: Grant::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn vars(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |name| map.get(name).cloned()
    }

    #[test]
    fn mode_parses() {
        assert_eq!(AuthMode::parse("").unwrap(), AuthMode::Off);
        assert_eq!(AuthMode::parse("off").unwrap(), AuthMode::Off);
        assert_eq!(
            AuthMode::parse("oidc, apikey").unwrap(),
            AuthMode::On {
                api_keys: true,
                oidc: true,
                tailscale: false,
            }
        );
        assert_eq!(
            AuthMode::parse("tailscale").unwrap(),
            AuthMode::On {
                api_keys: false,
                oidc: false,
                tailscale: true,
            }
        );
        assert!(AuthMode::parse("off,apikey").is_err());
        assert!(AuthMode::parse("basic").is_err());
    }

    #[test]
    fn cors_origins_are_exact_origins() {
        let config = AuthConfig::from_vars(vars(&[(
            "LOAMS_CORS_ORIGINS",
            " https://Console.Example.com/ , http://127.0.0.1:3000,loams-app://console",
        )]))
        .unwrap();
        assert_eq!(
            config.cors_origins,
            [
                "https://console.example.com",
                "http://127.0.0.1:3000",
                "loams-app://console"
            ]
        );
        for bad in [
            "*",
            "null",
            "console.example.com",
            "https://",
            "https://*.example.com",
            "https://example.com/ui",
            "https://example.com?x",
            "https://user@example.com",
            "1https://example.com",
        ] {
            let err = AuthConfig::from_vars(vars(&[("LOAMS_CORS_ORIGINS", bad)])).unwrap_err();
            assert!(err.starts_with("LOAMS_CORS_ORIGINS: "), "{bad}: {err}");
        }
    }

    /// Owner, 2026-10-23: the console ships with the deployment, so no
    /// origin is allowed by default, on either listener, whether or not
    /// authentication is on; the hosted console's origin is not special.
    #[test]
    fn cors_denies_every_origin_by_default() {
        let dir = tempfile::TempDir::new().unwrap();
        let (_, line) = api_key::new_key("test");
        let keys = dir.path().join("api-keys");
        std::fs::write(&keys, format!("{line}\n")).unwrap();
        for config in [
            AuthConfig::off(),
            AuthConfig::api_keys(&keys),
            AuthConfig {
                mode: AuthMode::On {
                    api_keys: false,
                    oidc: false,
                    tailscale: true,
                },
                tailscale: Some(tailscale::TailscaleConfig::default()),
                ..AuthConfig::off()
            },
        ] {
            let auth = Auth::from_config(&config).unwrap();
            for origin in [
                "https://console.loams.dev",
                "https://evil.example",
                "loams-app://console",
            ] {
                assert!(!auth.cors().allows(origin), "{origin} on {config:?}");
                assert!(
                    !auth.tailscale_cors().allows(origin),
                    "{origin} on {config:?}"
                );
            }
        }
        let auth = Auth::from_config(&AuthConfig {
            cors_origins: vec!["https://console.example.com".into()],
            ..AuthConfig::api_keys(&keys)
        })
        .unwrap();
        assert!(auth.cors().allows("https://console.example.com"));
        assert!(auth.tailscale_cors().allows("https://console.example.com/"));
        assert!(!auth.cors().allows("https://console.loams.dev"));
    }

    #[test]
    fn apikey_mode_needs_a_file() {
        let err = AuthConfig::from_vars(vars(&[(MODE_ENV, "apikey")])).unwrap_err();
        assert!(err.contains("LOAMS_API_KEYS_FILE"), "{err}");
    }

    #[test]
    fn oidc_mode_needs_issuer_and_audience() {
        let err = AuthConfig::from_vars(vars(&[(MODE_ENV, "oidc")])).unwrap_err();
        assert!(err.contains("LOAMS_OIDC_ISSUER"), "{err}");
        let err = AuthConfig::from_vars(vars(&[
            (MODE_ENV, "oidc"),
            (
                "LOAMS_OIDC_ISSUER",
                "https://auth.loams.dev/application/o/loams/",
            ),
        ]))
        .unwrap_err();
        assert!(err.contains("LOAMS_OIDC_AUDIENCE"), "{err}");
    }

    #[test]
    fn oidc_with_a_key_file_also_takes_api_keys() {
        let issuer = ("LOAMS_OIDC_ISSUER", "https://auth.loams.dev");
        let audience = ("LOAMS_OIDC_AUDIENCE", "loams-engine, loams-console");
        let config = AuthConfig::from_vars(vars(&[
            (MODE_ENV, "oidc"),
            issuer,
            audience,
            ("LOAMS_API_KEYS_FILE", "/etc/loams/secrets/api-keys"),
        ]))
        .unwrap();
        assert_eq!(
            config.mode,
            AuthMode::On {
                api_keys: true,
                oidc: true,
                tailscale: false,
            }
        );
        assert!(config.api_keys_optional);
        assert_eq!(
            config.oidc.unwrap().audiences,
            vec!["loams-engine".to_owned(), "loams-console".to_owned()]
        );
        let config = AuthConfig::from_vars(vars(&[(MODE_ENV, "oidc"), issuer, audience])).unwrap();
        assert_eq!(
            config.mode,
            AuthMode::On {
                api_keys: false,
                oidc: true,
                tailscale: false,
            }
        );
    }

    #[test]
    fn oidc_takes_several_issuers_and_tailscale_its_own_listener() {
        let config = AuthConfig::from_vars(vars(&[
            (MODE_ENV, "oidc,tailscale"),
            (
                "LOAMS_OIDC_ISSUER",
                "https://auth.loams.dev, https://acme.cloudflareaccess.com",
            ),
            ("LOAMS_OIDC_JWKS_URL", ",https://acme.example/certs"),
            ("LOAMS_OIDC_AUDIENCE", "loams-console,my-console"),
            ("LOAMS_OIDC_ALLOWED_EMAILS", "Me@Example.com"),
            ("LOAMS_TAILSCALE_ALLOWED_USERS", "Me@Example.com"),
        ]))
        .unwrap();
        assert_eq!(
            config.tailscale_listener().unwrap(),
            Some(tailscale::DEFAULT_LISTEN)
        );
        let oidc = config.oidc.unwrap();
        assert_eq!(oidc.issuers.len(), 2);
        assert_eq!(oidc.issuers[0].jwks_url, None);
        assert_eq!(
            oidc.issuers[1].jwks_url.as_deref(),
            Some("https://acme.example/certs")
        );
        assert_eq!(oidc.audiences, ["loams-console", "my-console"]);
        assert!(oidc.allowed_emails.contains("me@example.com"));
        assert!(
            config
                .tailscale
                .unwrap()
                .allowed_users
                .contains("me@example.com")
        );
        let err = AuthConfig::from_vars(vars(&[
            (MODE_ENV, "tailscale"),
            ("LOAMS_TAILSCALE_LISTEN", "0.0.0.0:7781"),
        ]))
        .unwrap_err();
        assert!(err.contains("loopback"), "{err}");
    }

    #[test]
    fn off_mode_refuses_non_loopback_listeners() {
        let config = AuthConfig::off();
        let lo: SocketAddr = "127.0.0.1:8080".parse().unwrap();
        assert!(config.check_listeners(&[("--listen", lo)]).is_ok());
        for bad in ["0.0.0.0:8080", "[::]:8080", "100.64.0.7:8080"] {
            let err = config
                .check_listeners(&[
                    ("--listen", lo),
                    ("--flight-sql-listen", bad.parse().unwrap()),
                ])
                .unwrap_err();
            assert!(err.contains("--flight-sql-listen"), "{err}");
            assert!(err.contains("loopback"), "{err}");
            assert!(err.contains(MODE_ENV), "{err}");
        }
        let on = AuthConfig::api_keys("/dev/null");
        assert!(
            on.check_listeners(&[("--listen", "0.0.0.0:8080".parse().unwrap())])
                .is_ok()
        );
    }

    /// A credential's `Debug`, which a panic or a `?` log prints (CodeQL
    /// rust/cleartext-logging, #1105), names its kind and never its value.
    #[test]
    fn a_credential_debug_never_shows_the_secret() {
        let (key, _) = api_key::generate();
        let session = directory::new_session_token();
        let jwt = "eyJhbGciOiJSUzI1NiJ9.eyJzdWIiOiJhIn0.c2lnbmF0dXJl".to_owned();
        let csrf = "csrf-0123456789abcdef".to_owned();
        let cases = [
            (
                Credential::ApiKey(key.clone()),
                key.as_str(),
                "ApiKey([redacted])",
            ),
            (
                Credential::Jwt(jwt.clone()),
                jwt.as_str(),
                "Jwt([redacted])",
            ),
            (
                Credential::Session(SessionCredential {
                    token: session.clone(),
                    csrf: Some(csrf.clone()),
                }),
                session.as_str(),
                "Session([redacted])",
            ),
        ];
        for (credential, secret, shown) in cases {
            let debug = format!("{credential:?} {credential:#?}");
            assert!(
                !debug.contains(secret),
                "{shown}: the secret is in the Debug output"
            );
            assert!(
                !debug.contains(&csrf),
                "{shown}: the CSRF token is in the Debug output"
            );
            assert_eq!(format!("{credential:?}"), shown);
        }
        let s = SessionCredential {
            token: session.clone(),
            csrf: Some(csrf.clone()),
        };
        assert_eq!(format!("{s:?}"), "SessionCredential([redacted])");
    }
}
