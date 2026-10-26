//! Who may do what with Loams Git (GT1 Task 16, design §36 §10, D182): git tokens, the
//! `--git-listen` listener stock git authenticates against, and the grant of a main-port caller.
//!
//! - **Git tokens** ([`LoopbackDevAuthorizer`]): a TOML file (`--git-tokens`) of SHA-256 token
//!   hashes, each mapped to a grant in one namespace. Stock git sends the token as the password
//!   of HTTP Basic (user `x-token`, or any), or as a Bearer token. The file holds hashes only,
//!   and no log line, error or answer ever carries a token.
//! - **The git listener** answers `/git/<ns>/<repo>.git/...`. The namespace comes from the
//!   token: a URL that names another namespace, or a repository the token does not cover, is
//!   answered `404` with no detail, as a repository that does not exist is. Loopback only until
//!   MT1 (D111, plan GT1 Ruling 30). Smart HTTP itself is GT1 Task 46: until then an admitted
//!   request for a repository that exists answers `501`.
//! - **Main-port callers** (`RepoService`): the principal the engine's authentication admitted,
//!   at its org role: `owner` and `admin` administer repositories, `member` writes. The operator
//!   (`LOAMS_AUTH_MODE=off` on loopback, or a key from `LOAMS_API_KEYS_FILE`) administers even
//!   before first-run setup.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use axum::Router;
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use loams_common::meta::{Consistency, MetaStore};
use loams_git::{
    EnvAccess, EnvGrant, GitAccess, GitAuthError, GitAuthorizer, GitGrant, NamespaceId,
    Principal as GitPrincipal, RepoCatalog, RepoLifecycle, RepoMatch, RepoName, RepoPath, Secret,
    TrustClass, WsId, WsMatch,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::auth::authorizer::Denied;
use crate::auth::directory::Role;
use crate::auth::{Principal, PrincipalKind, Principals};

/// The organisation of every grant until MT1 names one (ruling T13.4).
const LOCAL_ORG: &str = "local";

/// The realm stock git shows when it asks for credentials.
const REALM: &str = "Basic realm=\"Loams Git\"";

/// The git token file, as written (`--git-tokens`).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TokensFile {
    #[serde(default)]
    tokens: Vec<TokenEntry>,
}

/// One token: its hash and its grant.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TokenEntry {
    /// SHA-256 of the token, 64 lowercase hex digits.
    sha256: String,
    /// Who holds it (one path segment of printable ASCII).
    principal: String,
    /// The namespace's name: the one namespace the token reaches (D182).
    namespace: String,
    /// The repositories, by name; omitted (or `["*"]`) is every repository.
    #[serde(default)]
    repos: Option<Vec<String>>,
    /// `read`, `write` or `admin`.
    access: String,
    #[serde(default)]
    branch_prefixes: Vec<String>,
    #[serde(default)]
    cones: Vec<String>,
    /// `own` (the default), `all`, or a list of workspace ids.
    #[serde(default)]
    workspaces: Option<WsSpec>,
    /// Environments; none unless named (agents carry none by default, D844).
    #[serde(default)]
    env: Vec<EnvEntry>,
    /// `trusted`, `untrusted` or `scratch`.
    #[serde(default)]
    cache_class: Option<String>,
    #[serde(default)]
    registry_read: bool,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum WsSpec {
    Word(String),
    Ids(Vec<String>),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EnvEntry {
    environment: String,
    access: String,
}

/// A token's grant before its namespace name is resolved to an id.
#[derive(Clone, Debug)]
struct TokenGrant {
    namespace: String,
    /// The grant, with a placeholder namespace id.
    grant: GitGrant,
}

fn parse_access(value: &str) -> Result<GitAccess, String> {
    match value {
        "read" => Ok(GitAccess::Read),
        "write" => Ok(GitAccess::Write),
        "admin" => Ok(GitAccess::Admin),
        other => Err(format!("access {other:?}: expected read, write or admin")),
    }
}

impl TokenEntry {
    fn into_grant(self) -> Result<([u8; 32], TokenGrant), String> {
        let hash = hex::decode(&self.sha256)
            .ok()
            .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
            .filter(|_| self.sha256.bytes().all(|b| !b.is_ascii_uppercase()))
            .ok_or_else(|| "sha256: expected 64 lowercase hex digits".to_string())?;
        let principal = GitPrincipal::parse(&self.principal).map_err(|err| err.to_string())?;
        if self.namespace.is_empty() {
            return Err("namespace: a namespace name is required".into());
        }
        let repos = match self.repos {
            None => RepoMatch::All,
            Some(names) if names.iter().any(|n| n == "*") => RepoMatch::All,
            Some(names) => RepoMatch::Names(
                names
                    .iter()
                    .map(|n| RepoName::parse(n).map_err(|err| err.to_string()))
                    .collect::<Result<_, _>>()?,
            ),
        };
        let workspaces = match self.workspaces {
            None => WsMatch::Own,
            Some(WsSpec::Word(word)) if word == "own" => WsMatch::Own,
            Some(WsSpec::Word(word)) if word == "all" => WsMatch::All,
            Some(WsSpec::Word(word)) => {
                return Err(format!(
                    "workspaces {word:?}: expected own, all or a list of ids"
                ));
            }
            Some(WsSpec::Ids(ids)) => WsMatch::Ids(
                ids.iter()
                    .map(|id| WsId::parse(id).map_err(|err| err.to_string()))
                    .collect::<Result<_, _>>()?,
            ),
        };
        let env = self
            .env
            .into_iter()
            .map(|e| {
                let access = match e.access.as_str() {
                    "read" => EnvAccess::Read,
                    "write" => EnvAccess::Write,
                    "promote" => EnvAccess::Promote,
                    other => {
                        return Err(format!(
                            "env access {other:?}: expected read, write or promote"
                        ));
                    }
                };
                Ok(EnvGrant {
                    environment: e.environment,
                    access,
                })
            })
            .collect::<Result<_, String>>()?;
        let cache_class = match self.cache_class.as_deref() {
            None => None,
            Some("trusted") => Some(TrustClass::Trusted),
            Some("untrusted") => Some(TrustClass::Untrusted),
            Some("scratch") => Some(TrustClass::Scratch),
            Some(other) => {
                return Err(format!(
                    "cache_class {other:?}: expected trusted, untrusted or scratch"
                ));
            }
        };
        let grant = GitGrant {
            org: LOCAL_ORG.to_string(),
            namespace: NamespaceId(0),
            principal,
            repos,
            access: parse_access(&self.access)?,
            branch_prefixes: self.branch_prefixes,
            cones: self
                .cones
                .iter()
                .map(|c| RepoPath::parse(c).map_err(|err| err.to_string()))
                .collect::<Result<_, _>>()?,
            workspaces,
            env,
            cache_class,
            registry_read: self.registry_read,
        };
        Ok((
            hash,
            TokenGrant {
                namespace: self.namespace,
                grant,
            },
        ))
    }
}

/// Git tokens from a file of hashes, for loopback serving until MT1 (plan GT1 Task 16).
pub struct LoopbackDevAuthorizer {
    meta: Arc<dyn MetaStore>,
    tokens: HashMap<[u8; 32], TokenGrant>,
}

impl std::fmt::Debug for LoopbackDevAuthorizer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoopbackDevAuthorizer")
            .field("tokens", &self.tokens.len())
            .finish_non_exhaustive()
    }
}

impl LoopbackDevAuthorizer {
    /// The tokens of `text` (the file's TOML). An error names the entry, never a token.
    pub fn parse(text: &str, meta: Arc<dyn MetaStore>) -> Result<Self, String> {
        let file: TokensFile = toml::from_str(text).map_err(|err| err.to_string())?;
        let mut tokens = HashMap::new();
        for (i, entry) in file.tokens.into_iter().enumerate() {
            let (hash, grant) = entry
                .into_grant()
                .map_err(|err| format!("tokens[{i}]: {err}"))?;
            if tokens.insert(hash, grant).is_some() {
                return Err(format!("tokens[{i}]: the same token hash appears twice"));
            }
        }
        Ok(Self { meta, tokens })
    }

    /// The tokens of the file at `path`.
    pub fn load(path: &Path, meta: Arc<dyn MetaStore>) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|err| format!("reading {}: {err}", path.display()))?;
        Self::parse(&text, meta).map_err(|err| format!("{}: {err}", path.display()))
    }

    /// How many tokens it knows.
    pub fn len(&self) -> usize {
        self.tokens.len()
    }

    /// Whether it knows no token.
    pub fn is_empty(&self) -> bool {
        self.tokens.is_empty()
    }
}

#[async_trait]
impl GitAuthorizer for LoopbackDevAuthorizer {
    async fn authorize(&self, token: &Secret<String>) -> Result<GitGrant, GitAuthError> {
        let hash: [u8; 32] = Sha256::digest(token.expose().as_bytes()).into();
        let found = self
            .tokens
            .get(&hash)
            .ok_or(GitAuthError::Unauthenticated)?;
        let namespace = self
            .meta
            .namespace_by_name(Consistency::Local, &found.namespace)
            .await
            .map_err(|err| GitAuthError::Unavailable(err.to_string()))?;
        let Some(namespace) = namespace else {
            tracing::warn!(
                principal = found.grant.principal.as_str(),
                namespace = found.namespace.as_str(),
                "a git token names a namespace that does not exist"
            );
            return Err(GitAuthError::Unauthenticated);
        };
        Ok(GitGrant {
            namespace: namespace.id,
            ..found.grant.clone()
        })
    }
}

/// The token in a request: the password of HTTP Basic (whatever the user), or a Bearer token.
fn credential(headers: &HeaderMap) -> Option<Secret<String>> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, rest) = value.split_once(' ')?;
    let token = if scheme.eq_ignore_ascii_case("bearer") {
        rest.trim().to_string()
    } else if scheme.eq_ignore_ascii_case("basic") {
        let decoded = STANDARD.decode(rest.trim()).ok()?;
        let pair = String::from_utf8(decoded).ok()?;
        pair.split_once(':')?.1.to_string()
    } else {
        return None;
    };
    (!token.is_empty()).then(|| Secret::new(token))
}

/// What the git listener needs.
#[derive(Clone)]
pub(crate) struct GitHttp {
    pub(crate) authorizer: Arc<dyn GitAuthorizer>,
    pub(crate) meta: Arc<dyn MetaStore>,
    pub(crate) runtime: Arc<super::repos::GitRuntime>,
}

/// The git listener's routes (`/git/<ns>/<repo>.git/...`).
pub(crate) fn git_router(state: GitHttp) -> Router {
    Router::new()
        .route("/git/{ns}/{repo}", get(serve).post(serve))
        .route("/git/{ns}/{repo}/{*rest}", get(serve).post(serve))
        .fallback(|| async { not_found() })
        .with_state(state)
}

/// The one `404` of the listener: no detail, the same for a repository outside the token's
/// namespace as for one that does not exist.
pub(crate) fn not_found() -> Response {
    (StatusCode::NOT_FOUND, "Not Found\n").into_response()
}

fn unauthenticated() -> Response {
    let mut response = (
        StatusCode::UNAUTHORIZED,
        "loams: a git token is required (HTTP Basic with the token as the password)\n",
    )
        .into_response();
    response
        .headers_mut()
        .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static(REALM));
    response
}

async fn serve(
    State(state): State<GitHttp>,
    UrlPath(params): UrlPath<HashMap<String, String>>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let Some(token) = credential(&headers) else {
        return unauthenticated();
    };
    let grant = match state.authorizer.authorize(&token).await {
        Ok(grant) => grant,
        Err(GitAuthError::Unauthenticated) => return unauthenticated(),
        Err(GitAuthError::Unavailable(reason)) => {
            tracing::warn!(%reason, "checking a git token failed");
            return (StatusCode::SERVICE_UNAVAILABLE, "loams: try again later\n").into_response();
        }
    };
    let (Some(ns), Some(repo)) = (params.get("ns"), params.get("repo")) else {
        return not_found();
    };
    let Some(repo) = repo
        .strip_suffix(".git")
        .and_then(|name| RepoName::parse(name).ok())
    else {
        return not_found();
    };
    let namespace = match state.meta.namespace_by_name(Consistency::Local, ns).await {
        Ok(Some(namespace)) => namespace.id,
        Ok(None) => return not_found(),
        Err(err) => {
            tracing::warn!(%err, "reading a namespace for the git listener failed");
            return (StatusCode::SERVICE_UNAVAILABLE, "loams: try again later\n").into_response();
        }
    };
    // D182: the token decides the namespace; anything outside it does not exist.
    if !grant.covers(namespace, &repo) {
        return not_found();
    }
    let record = match state.runtime.catalog.get(namespace, &repo).await {
        Ok(Some(record)) if record.state == RepoLifecycle::Active => record,
        Ok(_) => return not_found(),
        Err(err) => {
            tracing::warn!(%err, "reading the repository catalog for the git listener failed");
            return (StatusCode::SERVICE_UNAVAILABLE, "loams: try again later\n").into_response();
        }
    };
    // The immutable object route (GT1 Task 18, §19.5.1).
    if let Some(oid) = params
        .get("rest")
        .and_then(|rest| rest.strip_prefix("loams/objects/"))
    {
        return super::objects::serve_object(
            &state.runtime,
            super::objects::ObjectRoute {
                grant: &grant,
                ns: namespace,
                ns_name: ns,
                record: &record,
                oid,
                workspace: query.get("ws").map(String::as_str),
            },
        )
        .await;
    }
    (
        StatusCode::NOT_IMPLEMENTED,
        "loams: Smart HTTP is not served yet (GT1 Task 46); use git-remote-loams (loams::)\n",
    )
        .into_response()
}

/// Serves the git listener on `listener` until `stop`.
pub(crate) fn serve_git_listener(
    listener: tokio::net::TcpListener,
    state: GitHttp,
    stop: tokio_util::sync::CancellationToken,
) -> tokio::task::JoinHandle<()> {
    let router = git_router(state);
    tokio::spawn(async move {
        let served = axum::serve(listener, router)
            .with_graceful_shutdown(async move { stop.cancelled().await })
            .await;
        if let Err(err) = served {
            tracing::warn!(%err, "the git listener stopped with an error");
        }
    })
}

/// The message refusing a non-loopback `--git-listen` (the plan's listener rule, D111).
pub fn non_loopback_message(addr: SocketAddr) -> String {
    format!(
        "git listen on {addr}: only loopback addresses are served until the unified auth plan \
         (D111)"
    )
}

/// The org role a main-port caller needs for `need` (`RepoService`, GT1 Task 16): reads and
/// writes need `member`, administration (create, fork, delete, configure, compact) `admin`.
pub(crate) fn role_for(need: GitAccess) -> Role {
    match need {
        GitAccess::Read | GitAccess::Write => Role::Member,
        GitAccess::Admin => Role::Admin,
    }
}

/// The model relation on `environment:<namespace>` that `need` asks (design 76 §9).
pub(crate) fn relation_for(need: GitAccess) -> &'static str {
    match need {
        GitAccess::Read => "can_read",
        GitAccess::Write => "can_write",
        GitAccess::Admin => "can_admin",
    }
}

/// Whether a main-port caller holds `need` on Loams Git: the operator always (the developer on
/// loopback, or a key from `LOAMS_API_KEYS_FILE` before first-run setup); anyone else by their
/// org role ([`role_for`]) and, in `namespace` when given, by model v1 through `loams-authz`
/// ([`relation_for`] on `environment:<namespace>`, plan IA1 ruling IA2.R4). Both must allow,
/// so the model only narrows what the role gives: an org member writes only the namespaces of
/// the projects they are granted, and the org's own namespaces.
pub(crate) async fn authorize_caller(
    principals: &Principals,
    principal: &Principal,
    need: GitAccess,
    namespace: Option<&str>,
) -> Result<(), Denied> {
    if principal.kind == PrincipalKind::DevLoopback {
        return Ok(());
    }
    match principals
        .authorizer
        .authorize(principal, role_for(need))
        .await
    {
        Ok(_) => {}
        // Before first-run setup the only keys are the operator's own.
        Err(Denied::SetupRequired) if principal.kind == PrincipalKind::ApiKey => return Ok(()),
        Err(denied) => return Err(denied),
    }
    let Some(namespace) = namespace else {
        return Ok(());
    };
    let refused = || Denied::RoleRequired {
        need: role_for(need),
        have: None,
    };
    let subject = match crate::auth::authz::standing(principals, principal).await? {
        crate::auth::authz::Standing::Operator => return Ok(()),
        crate::auth::authz::Standing::Outside => return Err(refused()),
        crate::auth::authz::Standing::Subject(subject) => subject,
    };
    let doc = principals.directory.read().await?;
    let mut check =
        crate::auth::authz::environment_check(&doc, &subject, relation_for(need), namespace);
    drop(doc);
    if matches!(need, GitAccess::Admin) {
        check = check.consistency(loams_authz::Consistency::HigherConsistency);
    }
    match principals.authz.check(check).await {
        Ok(decision) if decision.allowed => Ok(()),
        Ok(_) => Err(refused()),
        Err(err) => Err(Denied::Unavailable(err.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(token: &str) -> String {
        hex::encode(Sha256::digest(token.as_bytes()))
    }

    fn parse(text: &str) -> Result<HashMap<[u8; 32], TokenGrant>, String> {
        let file: TokensFile = toml::from_str(text).map_err(|err| err.to_string())?;
        file.tokens
            .into_iter()
            .map(TokenEntry::into_grant)
            .collect()
    }

    #[test]
    fn agent_grant_has_no_env_by_default() {
        let text = format!(
            "[[tokens]]\nsha256 = \"{}\"\nprincipal = \"agent-7\"\nnamespace = \"acme\"\n\
             access = \"write\"\nbranch_prefixes = [\"refs/heads/agent/\"]\n",
            hash("lgt_agent")
        );
        let tokens = parse(&text).expect("parse");
        let grant = &tokens.values().next().expect("one").grant;
        assert!(grant.env.is_empty());
        assert_eq!(grant.workspaces, WsMatch::Own);
        assert_eq!(grant.repos, RepoMatch::All);
        assert_eq!(grant.access, GitAccess::Write);
        assert_eq!(grant.branch_prefixes, vec!["refs/heads/agent/".to_string()]);
        // An environment is reached only when named.
        let text = format!(
            "{text}env = [{{ environment = \"staging\", access = \"read\" }}]\n\
             cache_class = \"untrusted\"\nrepos = [\"web\"]\nworkspaces = \"all\"\n"
        );
        let tokens = parse(&text).expect("parse");
        let grant = &tokens.values().next().expect("one").grant;
        assert_eq!(grant.env.len(), 1);
        assert_eq!(grant.cache_class, Some(TrustClass::Untrusted));
        assert_eq!(
            grant.repos,
            RepoMatch::Names(vec![RepoName::parse("web").expect("name")])
        );
    }

    #[test]
    fn bad_entries_are_refused_without_a_token() {
        for (text, needle) in [
            (
                "[[tokens]]\nsha256 = \"abc\"\nprincipal = \"p\"\nnamespace = \"n\"\naccess = \"read\"\n",
                "sha256",
            ),
            (
                &*format!(
                    "[[tokens]]\nsha256 = \"{}\"\nprincipal = \"p\"\nnamespace = \"n\"\naccess = \"root\"\n",
                    hash("t")
                ),
                "access",
            ),
            (
                &*format!(
                    "[[tokens]]\nsha256 = \"{}\"\nprincipal = \"p\"\nnamespace = \"n\"\naccess = \"read\"\nbogus = 1\n",
                    hash("t")
                ),
                "bogus",
            ),
        ] {
            let err = parse(text).expect_err(text);
            assert!(err.contains(needle), "{err}");
        }
    }

    #[test]
    fn credentials_from_basic_and_bearer() {
        let mut headers = HeaderMap::new();
        let basic = STANDARD.encode("x-token:lgt_abc");
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Basic {basic}")).expect("header"),
        );
        assert_eq!(credential(&headers).expect("token").expose(), "lgt_abc");
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer lgt_def"),
        );
        assert_eq!(credential(&headers).expect("token").expose(), "lgt_def");
        headers.insert(header::AUTHORIZATION, HeaderValue::from_static("Digest x"));
        assert!(credential(&headers).is_none());
    }

    #[test]
    fn admin_needed_for_delete_and_config() {
        // RepoService's administration needs the org role admin; reads and writes member.
        assert_eq!(role_for(GitAccess::Admin), Role::Admin);
        assert_eq!(role_for(GitAccess::Write), Role::Member);
        assert_eq!(role_for(GitAccess::Read), Role::Member);
    }
}
