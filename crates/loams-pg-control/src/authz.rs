//! Who may call `loams.postgres.v1` (design §46 §4.1; PG2 Task 9).
//!
//! Two seams, each one trait, so MT1's identity and OpenFGA drop in without
//! touching a handler (API2-Q1):
//!
//! - **[`Authenticator`]**: request headers to a [`Principal`]. Before MT1
//!   there are two: [`DevLoopbackAuthenticator`], which answers one fixed
//!   principal and can only be built for a loopback listener, and
//!   [`ApiKeyAuthenticator`], which checks `authorization: Bearer lk_…`
//!   against the SHA-256 digests of a keys file ([`KeysFile`]).
//! - **[`Authorizer`]**: does a principal hold a [`Relation`] on an
//!   [`Object`]? The relations are §46 §4.1's `viewer`, `editor`, `admin`
//!   and `connect` on `pg_project:<id>`, inherited from the namespace.
//!   Before MT1: [`DevAuthorizer`] (everything, behind the dev
//!   authenticator only) and [`GrantsAuthorizer`] (the grants of the keys
//!   file). OpenFGA replaces the second when MT1 lands.
//!
//! **Agents never hold `admin`** (§46 §4.1), whatever an authorizer says:
//! [`authorize`] refuses `admin` to a principal that is an agent or acts
//! through one (its `act` chain), and [`GrantsAuthorizer`] refuses to load a
//! grant of `admin` to an agent.
//!
//! The relation each RPC needs is [`required`]'s table. Nothing here logs or
//! keeps an API key: the keys file holds digests, and a key is shown once,
//! by [`mint_api_key`].

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::net::SocketAddr;
use std::path::Path;

use async_trait::async_trait;
use axum::http::HeaderMap;
use rand::TryRngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use ulid::Ulid;

use crate::secrets::Secret;
use crate::service::{Reason, ServiceError};

/// What kind of principal calls (API2's `PrincipalKind`; MT1 adds the
/// identities behind `user`, `agent` and `service_account`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrincipalKind {
    User,
    Agent,
    ServiceAccount,
    /// The one principal of `--pg-authz dev`, on a loopback listener.
    DevLoopback,
}

impl PrincipalKind {
    pub fn as_str(self) -> &'static str {
        match self {
            PrincipalKind::User => "user",
            PrincipalKind::Agent => "agent",
            PrincipalKind::ServiceAccount => "service_account",
            PrincipalKind::DevLoopback => "dev_loopback",
        }
    }
}

/// One link of an `act` chain (RFC 8693 §4.1): who acted for the principal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Actor {
    pub subject: String,
    pub kind: PrincipalKind,
}

/// Who calls: the subject the call is made for, and the chain of actors
/// that made it (an agent acting for a person, §41's `act`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Principal {
    pub subject: String,
    pub kind: PrincipalKind,
    /// The actors, nearest first; empty when the subject called itself.
    #[serde(default)]
    pub act: Vec<Actor>,
    /// The credential's public id (an API key's id), never the credential.
    #[serde(default)]
    pub credential: Option<String>,
}

impl Principal {
    /// `--pg-authz dev`'s principal.
    pub fn dev() -> Self {
        Principal {
            subject: "dev".into(),
            kind: PrincipalKind::DevLoopback,
            act: Vec::new(),
            credential: None,
        }
    }

    /// Whether an agent is the subject or anywhere in the `act` chain.
    pub fn acts_as_agent(&self) -> bool {
        self.kind == PrincipalKind::Agent || self.act.iter().any(|a| a.kind == PrincipalKind::Agent)
    }
}

/// §46 §4.1's relations on a project, inherited from its namespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Relation {
    Viewer,
    Editor,
    Admin,
    /// May get connection details and credentials: `editor` and `admin`
    /// imply it; `viewer` does not (R1.9).
    Connect,
}

impl Relation {
    pub fn as_str(self) -> &'static str {
        match self {
            Relation::Viewer => "viewer",
            Relation::Editor => "editor",
            Relation::Admin => "admin",
            Relation::Connect => "connect",
        }
    }

    /// Whether holding `self` grants `wanted`: `admin` ⊇ `editor` ⊇
    /// `viewer`, and `editor` ⊇ `connect`.
    pub fn implies(self, wanted: Relation) -> bool {
        match (self, wanted) {
            (a, b) if a == b => true,
            (Relation::Admin, _) => true,
            (Relation::Editor, Relation::Viewer | Relation::Connect) => true,
            _ => false,
        }
    }
}

/// What a relation is held on.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Object {
    /// A namespace: creating and listing its projects.
    Namespace(String),
    /// `pg_project:<project_id>`, in its namespace (whose grants it
    /// inherits).
    Project {
        namespace: String,
        project_id: String,
    },
}

impl Object {
    /// The object as OpenFGA names it: `namespace:<ns>`, `pg_project:<id>`.
    pub fn fga(&self) -> String {
        match self {
            Object::Namespace(ns) => format!("namespace:{ns}"),
            Object::Project { project_id, .. } => format!("pg_project:{project_id}"),
        }
    }

    pub fn namespace(&self) -> &str {
        match self {
            Object::Namespace(ns) | Object::Project { namespace: ns, .. } => ns,
        }
    }
}

/// Whether an RPC's relation is checked on its namespace or its project.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Namespace,
    Project,
}

/// The relation each `loams.postgres.v1.PostgresService` method needs, and
/// on what (by the method's name). `None` for a name that is not one.
pub fn required(method: &str) -> Option<(Relation, Scope)> {
    use Relation::{Connect, Editor, Viewer};
    use Scope::{Namespace, Project};
    Some(match method {
        "ListProjects" => (Viewer, Namespace),
        "CreateProject" => (Editor, Namespace),
        "GetProject" | "GetBranch" | "ListBranches" | "GetEndpoint" | "ListEndpoints"
        | "ListRoles" | "ListDatabases" | "WatchProject" => (Viewer, Project),
        "UpdateProject" | "DeleteProject" | "UpgradeProject" | "CreateBranch" | "UpdateBranch"
        | "DeleteBranch" | "RestoreBranch" | "SetDefaultBranch" | "CreateEndpoint"
        | "UpdateEndpoint" | "StartEndpoint" | "SuspendEndpoint" | "RestartEndpoint"
        | "DeleteEndpoint" | "CreateRole" | "ResetRolePassword" | "DeleteRole"
        | "CreateDatabase" | "DeleteDatabase" => (Editor, Project),
        "GetConnectionInfo" | "IssueConnectCredential" => (Connect, Project),
        _ => return None,
    })
}

/// An authorizer that could not decide.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("the authorizer is unavailable: {0}")]
pub struct AuthzError(pub String);

/// Whether a principal holds a relation on an object (OpenFGA's `Check`).
#[async_trait]
pub trait Authorizer: Send + Sync + 'static {
    async fn check(
        &self,
        principal: &Principal,
        relation: Relation,
        object: &Object,
    ) -> Result<bool, AuthzError>;
}

/// Checks `relation` on `object` for `principal`: `permission_denied` when
/// it is not held, and always for `admin` to an agent (§46 §4.1).
///
/// # Errors
///
/// `permission_denied`, or `unavailable` when the authorizer cannot decide.
pub async fn authorize(
    authorizer: &dyn Authorizer,
    principal: &Principal,
    relation: Relation,
    object: &Object,
) -> Result<(), ServiceError> {
    let denied = || {
        ServiceError::new(
            Reason::PermissionDenied,
            format!("needs {} on {}", relation.as_str(), object.fga()),
        )
        .with("relation", relation.as_str())
    };
    if relation == Relation::Admin && principal.acts_as_agent() {
        return Err(denied());
    }
    match authorizer.check(principal, relation, object).await {
        Ok(true) => Ok(()),
        Ok(false) => Err(denied()),
        Err(e) => {
            tracing::warn!(error = %e, "authorization could not be decided");
            Err(ServiceError::new(
                Reason::Unavailable,
                "authorization is unavailable; retry",
            ))
        }
    }
}

/// `--pg-authz openfga`: the relations through `loams-authz` (model v1, design 76 §6 and §9;
/// plan IA1, IA2): OpenFGA in a deployment, any [`loams_authz::Authorizer`] in tests.
///
/// - A namespace is `environment:<ns>`; a project is `database:<project id>`, hung off its
///   namespace with a contextual tuple (`pg-control` keys projects by namespace, so a project
///   named in the wrong namespace is not found after the check).
/// - `viewer`, `editor`, `admin` and `connect` ask `can_read`, `can_write`, `can_admin` and
///   `can_connect`; `admin` and `connect` (which issues credentials) ask with
///   `HigherConsistency` (D1611).
/// - The subject and every actor of its `act` chain must hold the relation, so an agent acting
///   for a person reaches no more than its own grants. [`authorize`] still refuses `admin` to
///   an agent whatever the model says.
/// - The dev principal holds nothing here.
#[derive(Clone)]
pub struct ModelAuthorizer {
    authz: std::sync::Arc<dyn loams_authz::Authorizer>,
}

impl fmt::Debug for ModelAuthorizer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ModelAuthorizer").finish_non_exhaustive()
    }
}

impl ModelAuthorizer {
    pub fn new(authz: std::sync::Arc<dyn loams_authz::Authorizer>) -> Self {
        Self { authz }
    }

    fn subject(kind: PrincipalKind, id: &str) -> Option<loams_authz::Subject> {
        use loams_authz::SubjectKind as K;
        let kind = match kind {
            PrincipalKind::User => K::User,
            PrincipalKind::Agent => K::Agent,
            PrincipalKind::ServiceAccount => K::ServiceAccount,
            PrincipalKind::DevLoopback => return None,
        };
        Some(loams_authz::Subject::new(kind, id))
    }

    /// The model's relation for `relation`.
    pub fn model_relation(relation: Relation) -> &'static str {
        match relation {
            Relation::Viewer => "can_read",
            Relation::Editor => "can_write",
            Relation::Admin => "can_admin",
            Relation::Connect => "can_connect",
        }
    }

    fn request(
        subject: loams_authz::Subject,
        relation: Relation,
        object: &Object,
    ) -> loams_authz::CheckRequest {
        let (target, roots) = match object {
            Object::Namespace(ns) => (loams_authz::ObjectRef::new("environment", ns), Vec::new()),
            Object::Project {
                namespace,
                project_id,
            } => (
                loams_authz::ObjectRef::new("database", project_id),
                vec![loams_authz::Tuple::new(
                    format!("environment:{namespace}"),
                    "environment",
                    format!("database:{project_id}"),
                )],
            ),
        };
        let mut req =
            loams_authz::CheckRequest::new(subject, Self::model_relation(relation), target);
        req.contextual_tuples = roots;
        if matches!(relation, Relation::Admin | Relation::Connect) {
            req = req.consistency(loams_authz::Consistency::HigherConsistency);
        }
        req
    }
}

#[async_trait]
impl Authorizer for ModelAuthorizer {
    async fn check(
        &self,
        principal: &Principal,
        relation: Relation,
        object: &Object,
    ) -> Result<bool, AuthzError> {
        let mut who = vec![(principal.kind, principal.subject.as_str())];
        who.extend(principal.act.iter().map(|a| (a.kind, a.subject.as_str())));
        for (kind, id) in who {
            let Some(subject) = Self::subject(kind, id) else {
                return Ok(false);
            };
            let decision = self
                .authz
                .check(Self::request(subject, relation, object))
                .await
                .map_err(|e| AuthzError(e.to_string()))?;
            if !decision.allowed {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

/// `--pg-authz dev`: every relation, to the dev principal only.
#[derive(Debug, Clone, Copy, Default)]
pub struct DevAuthorizer;

#[async_trait]
impl Authorizer for DevAuthorizer {
    async fn check(
        &self,
        principal: &Principal,
        _relation: Relation,
        _object: &Object,
    ) -> Result<bool, AuthzError> {
        Ok(principal.kind == PrincipalKind::DevLoopback)
    }
}

/// A grant of a relation on a namespace or a project, to a subject.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    pub relation: Relation,
    #[serde(flatten)]
    pub on: GrantObject,
}

/// What a [`Grant`] is on: `{"namespace": "<ns>"}` or `{"project": "prj-…"}`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrantObject {
    Namespace(String),
    Project(String),
}

impl GrantObject {
    /// Whether a grant on `self` covers `object`: the object itself, or a
    /// project in a granted namespace.
    fn covers(&self, object: &Object) -> bool {
        match (self, object) {
            (GrantObject::Namespace(ns), o) => o.namespace() == ns,
            (GrantObject::Project(id), Object::Project { project_id, .. }) => id == project_id,
            (GrantObject::Project(_), Object::Namespace(_)) => false,
        }
    }
}

/// Why a keys file or its grants were refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KeysError {
    #[error("the keys file {0}: {1}")]
    Read(String, String),
    #[error("the keys file does not parse: {0}")]
    Parse(String),
    #[error("key {id}: {why}")]
    Invalid { id: String, why: String },
    #[error("{subject} is an agent, and agents cannot hold admin")]
    AgentAdmin { subject: String },
}

/// The grants of the keys file, by subject: what [`GrantsAuthorizer`]
/// checks until OpenFGA (MT1) holds them.
#[derive(Debug, Clone, Default)]
pub struct GrantsAuthorizer {
    grants: HashMap<String, Vec<Grant>>,
}

impl GrantsAuthorizer {
    /// The grants of `keys`, merged by subject.
    ///
    /// # Errors
    ///
    /// [`KeysError::AgentAdmin`] for `admin` granted to an agent.
    pub fn new(keys: &KeysFile) -> Result<Self, KeysError> {
        let mut grants: HashMap<String, Vec<Grant>> = HashMap::new();
        for entry in &keys.keys {
            if entry.kind == PrincipalKind::Agent
                && entry.grants.iter().any(|g| g.relation == Relation::Admin)
            {
                return Err(KeysError::AgentAdmin {
                    subject: entry.subject.clone(),
                });
            }
            let held = grants.entry(entry.subject.clone()).or_default();
            for g in &entry.grants {
                if !held.contains(g) {
                    held.push(g.clone());
                }
            }
        }
        Ok(GrantsAuthorizer { grants })
    }
}

#[async_trait]
impl Authorizer for GrantsAuthorizer {
    async fn check(
        &self,
        principal: &Principal,
        relation: Relation,
        object: &Object,
    ) -> Result<bool, AuthzError> {
        Ok(self.grants.get(&principal.subject).is_some_and(|grants| {
            grants
                .iter()
                .any(|g| g.relation.implies(relation) && g.on.covers(object))
        }))
    }
}

/// Why a request was not authenticated. The text names no part of the
/// credential.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AuthError {
    #[error("no credential: send `authorization: Bearer <api key>`")]
    Missing,
    #[error("the credential is malformed")]
    Malformed,
    #[error("the credential is not known")]
    Unknown,
}

impl From<AuthError> for ServiceError {
    fn from(e: AuthError) -> Self {
        ServiceError::new(Reason::Unauthenticated, e.to_string())
    }
}

/// Request headers to a principal (API2's `Authenticator`).
#[async_trait]
pub trait Authenticator: Send + Sync + 'static {
    async fn authenticate(&self, headers: &HeaderMap) -> Result<Principal, AuthError>;
}

/// `--pg-authz dev`: every request is [`Principal::dev`]. Only a loopback
/// listener can have one, so only this machine reaches it.
#[derive(Debug)]
pub struct DevLoopbackAuthenticator(());

impl DevLoopbackAuthenticator {
    /// The authenticator for a listener on `listen`.
    ///
    /// # Errors
    ///
    /// A message when `listen` is not a loopback address.
    pub fn new(listen: SocketAddr) -> Result<Self, String> {
        if listen.ip().is_loopback() {
            Ok(DevLoopbackAuthenticator(()))
        } else {
            Err(format!(
                "--pg-authz dev serves only a loopback listener, not {listen}; use --pg-authz api-key"
            ))
        }
    }
}

#[async_trait]
impl Authenticator for DevLoopbackAuthenticator {
    async fn authenticate(&self, _headers: &HeaderMap) -> Result<Principal, AuthError> {
        Ok(Principal::dev())
    }
}

/// The prefix of an API key (API2: `lk_<26-char ULID>_<32 base62>`).
pub const API_KEY_PREFIX: &str = "lk_";

/// The random part's length.
const API_KEY_SECRET_LEN: usize = 32;

const BASE62: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// The keys file of `--pg-authz api-key` (`--pg-api-keys`): one entry per
/// key, each with the SHA-256 of the key (never the key) and the grants of
/// its subject. JSON.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeysFile {
    pub keys: Vec<KeyEntry>,
}

/// One API key: its public id, the hex SHA-256 of the whole key, and whom it
/// authenticates.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyEntry {
    /// The key's ULID, as in the key.
    pub id: String,
    /// Lower-case hex SHA-256 of the whole key.
    pub sha256: String,
    pub subject: String,
    /// `user`, `agent` or `service_account`.
    pub kind: PrincipalKind,
    /// The `act` chain the key's calls carry, nearest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub act: Vec<Actor>,
    #[serde(default)]
    pub grants: Vec<Grant>,
}

impl fmt::Debug for KeyEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KeyEntry")
            .field("id", &self.id)
            .field("subject", &self.subject)
            .field("kind", &self.kind)
            .field("act", &self.act)
            .field("grants", &self.grants)
            .finish_non_exhaustive()
    }
}

impl KeysFile {
    /// Reads and checks the keys file at `path`.
    ///
    /// # Errors
    ///
    /// [`KeysError`].
    pub fn read(path: &Path) -> Result<Self, KeysError> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| KeysError::Read(path.display().to_string(), e.to_string()))?;
        Self::parse(&text)
    }

    /// Parses and checks a keys file.
    ///
    /// # Errors
    ///
    /// [`KeysError`].
    pub fn parse(text: &str) -> Result<Self, KeysError> {
        let file: KeysFile =
            serde_json::from_str(text).map_err(|e| KeysError::Parse(e.to_string()))?;
        file.check()?;
        Ok(file)
    }

    /// Each entry well formed, ids unique, no `admin` for an agent.
    fn check(&self) -> Result<(), KeysError> {
        let mut seen = std::collections::HashSet::new();
        for e in &self.keys {
            let invalid = |why: &str| KeysError::Invalid {
                id: e.id.clone(),
                why: why.into(),
            };
            if Ulid::from_string(&e.id).is_err() || e.id.len() != 26 {
                return Err(invalid("the id is not a ULID"));
            }
            if !seen.insert(e.id.clone()) {
                return Err(invalid("the id is listed twice"));
            }
            if e.sha256.len() != 64
                || !e
                    .sha256
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                return Err(invalid("sha256 is not 64 lower-case hex digits"));
            }
            if e.subject.is_empty() {
                return Err(invalid("the subject is empty"));
            }
            if e.kind == PrincipalKind::DevLoopback {
                return Err(invalid("dev_loopback is not a key's kind"));
            }
            for g in &e.grants {
                match &g.on {
                    GrantObject::Namespace(ns) if ns.is_empty() => {
                        return Err(invalid("a grant names an empty namespace"));
                    }
                    GrantObject::Project(id) if !id.starts_with("prj-") => {
                        return Err(invalid("a project grant must name a prj- id"));
                    }
                    _ => {}
                }
            }
            if e.kind == PrincipalKind::Agent
                && e.grants.iter().any(|g| g.relation == Relation::Admin)
            {
                return Err(KeysError::AgentAdmin {
                    subject: e.subject.clone(),
                });
            }
        }
        Ok(())
    }
}

/// A new API key for `subject`, and its keys-file entry. The key is shown
/// once, here; the entry holds only its digest.
///
/// # Errors
///
/// A message when the OS random generator fails.
pub fn mint_api_key(
    subject: &str,
    kind: PrincipalKind,
    act: Vec<Actor>,
    grants: Vec<Grant>,
) -> Result<(Secret<String>, KeyEntry), String> {
    let id = Ulid::generate().to_string();
    let mut random = zeroize::Zeroizing::new([0u8; 64]);
    let mut secret = zeroize::Zeroizing::new(String::with_capacity(API_KEY_SECRET_LEN));
    // Rejection sampling: bytes below 248 (4 × 62) map uniformly.
    while secret.len() < API_KEY_SECRET_LEN {
        rand::rngs::OsRng
            .try_fill_bytes(random.as_mut())
            .map_err(|e| format!("the OS random generator: {e}"))?;
        for b in random.iter().filter(|b| **b < 248) {
            if secret.len() == API_KEY_SECRET_LEN {
                break;
            }
            secret.push(char::from(BASE62[usize::from(*b % 62)]));
        }
    }
    let key = format!("{API_KEY_PREFIX}{id}_{}", secret.as_str());
    let entry = KeyEntry {
        sha256: hex::encode(Sha256::digest(key.as_bytes())),
        id,
        subject: subject.into(),
        kind,
        act,
        grants,
    };
    Ok((Secret::new(key), entry))
}

/// `--pg-authz api-key`: `authorization: Bearer lk_…`, checked against the
/// keys file's digests in constant time.
#[derive(Debug, Clone)]
pub struct ApiKeyAuthenticator {
    by_id: BTreeMap<String, KeyEntry>,
}

impl ApiKeyAuthenticator {
    pub fn new(keys: &KeysFile) -> Self {
        ApiKeyAuthenticator {
            by_id: keys
                .keys
                .iter()
                .map(|e| (e.id.clone(), e.clone()))
                .collect(),
        }
    }
}

/// The id in a well-formed key.
fn key_id(key: &str) -> Option<&str> {
    let rest = key.strip_prefix(API_KEY_PREFIX)?;
    let (id, secret) = rest.split_once('_')?;
    (id.len() == 26
        && secret.len() == API_KEY_SECRET_LEN
        && secret.bytes().all(|b| b.is_ascii_alphanumeric()))
    .then_some(id)
}

#[async_trait]
impl Authenticator for ApiKeyAuthenticator {
    async fn authenticate(&self, headers: &HeaderMap) -> Result<Principal, AuthError> {
        let mut values = headers.get_all(axum::http::header::AUTHORIZATION).iter();
        let value = values.next().ok_or(AuthError::Missing)?;
        if values.next().is_some() {
            return Err(AuthError::Malformed);
        }
        let value = value.to_str().map_err(|_| AuthError::Malformed)?;
        let (scheme, key) = value.split_once(' ').ok_or(AuthError::Malformed)?;
        if !scheme.eq_ignore_ascii_case("bearer") {
            return Err(AuthError::Malformed);
        }
        let key = key.trim();
        let id = key_id(key).ok_or(AuthError::Malformed)?;
        let entry = self.by_id.get(id).ok_or(AuthError::Unknown)?;
        let digest = hex::encode(Sha256::digest(key.as_bytes()));
        if !bool::from(digest.as_bytes().ct_eq(entry.sha256.as_bytes())) {
            return Err(AuthError::Unknown);
        }
        Ok(Principal {
            subject: entry.subject.clone(),
            kind: entry.kind,
            act: entry.act.clone(),
            credential: Some(entry.id.clone()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(subject: &str) -> Principal {
        Principal {
            subject: subject.into(),
            kind: PrincipalKind::User,
            act: Vec::new(),
            credential: None,
        }
    }

    fn project(ns: &str, id: &str) -> Object {
        Object::Project {
            namespace: ns.into(),
            project_id: id.into(),
        }
    }

    #[test]
    fn relations_imply_as_the_model_says() {
        use Relation::*;
        assert!(Admin.implies(Editor) && Admin.implies(Viewer) && Admin.implies(Connect));
        assert!(Editor.implies(Viewer) && Editor.implies(Connect) && !Editor.implies(Admin));
        assert!(!Viewer.implies(Connect) && !Viewer.implies(Editor));
        assert!(!Connect.implies(Viewer));
    }

    #[tokio::test]
    async fn grants_inherit_from_the_namespace() {
        let keys = KeysFile {
            keys: vec![KeyEntry {
                id: Ulid::generate().to_string(),
                sha256: "0".repeat(64),
                subject: "alice".into(),
                kind: PrincipalKind::User,
                act: Vec::new(),
                grants: vec![
                    Grant {
                        relation: Relation::Viewer,
                        on: GrantObject::Namespace("acme".into()),
                    },
                    Grant {
                        relation: Relation::Editor,
                        on: GrantObject::Project("prj-1".into()),
                    },
                ],
            }],
        };
        let authz = GrantsAuthorizer::new(&keys).expect("grants");
        let alice = user("alice");
        let check = |r, o: Object| {
            let (authz, alice) = (authz.clone(), alice.clone());
            async move { authz.check(&alice, r, &o).await.expect("decided") }
        };
        assert!(check(Relation::Viewer, project("acme", "prj-2")).await);
        assert!(check(Relation::Viewer, Object::Namespace("acme".into())).await);
        assert!(!check(Relation::Editor, project("acme", "prj-2")).await);
        assert!(check(Relation::Editor, project("acme", "prj-1")).await);
        assert!(check(Relation::Connect, project("acme", "prj-1")).await);
        assert!(!check(Relation::Viewer, project("other", "prj-3")).await);
        assert!(
            !authz
                .check(&user("bob"), Relation::Viewer, &project("acme", "prj-2"))
                .await
                .expect("decided")
        );
    }

    #[tokio::test]
    async fn agents_never_hold_admin() {
        let agent = Principal {
            kind: PrincipalKind::Agent,
            ..user("bot")
        };
        let through_agent = Principal {
            act: vec![Actor {
                subject: "bot".into(),
                kind: PrincipalKind::Agent,
            }],
            ..user("alice")
        };
        /// An authorizer that says yes to everything.
        struct Yes;
        #[async_trait]
        impl Authorizer for Yes {
            async fn check(
                &self,
                _: &Principal,
                _: Relation,
                _: &Object,
            ) -> Result<bool, AuthzError> {
                Ok(true)
            }
        }
        let o = project("acme", "prj-1");
        for p in [&agent, &through_agent] {
            let e = authorize(&Yes, p, Relation::Admin, &o)
                .await
                .expect_err("no admin for an agent");
            assert_eq!(e.reason, Reason::PermissionDenied);
            authorize(&Yes, p, Relation::Editor, &o)
                .await
                .expect("editor is fine");
        }
        authorize(&Yes, &user("alice"), Relation::Admin, &o)
            .await
            .expect("a person may");

        let (_, mut entry) = mint_api_key(
            "bot",
            PrincipalKind::Agent,
            Vec::new(),
            vec![Grant {
                relation: Relation::Admin,
                on: GrantObject::Namespace("acme".into()),
            }],
        )
        .expect("minted");
        let keys = KeysFile {
            keys: vec![entry.clone()],
        };
        assert!(matches!(
            GrantsAuthorizer::new(&keys),
            Err(KeysError::AgentAdmin { .. })
        ));
        let text = serde_json::to_string(&keys).expect("json");
        assert!(matches!(
            KeysFile::parse(&text),
            Err(KeysError::AgentAdmin { .. })
        ));
        entry.kind = PrincipalKind::User;
        assert!(GrantsAuthorizer::new(&KeysFile { keys: vec![entry] }).is_ok());
    }

    #[tokio::test]
    async fn api_keys_authenticate_by_digest() {
        let (key, entry) =
            mint_api_key("alice", PrincipalKind::User, Vec::new(), Vec::new()).expect("minted");
        assert!(key.expose().starts_with("lk_"));
        assert_eq!(key.expose().len(), 3 + 26 + 1 + 32);
        assert!(!format!("{entry:?}").contains(&entry.sha256));
        let file = KeysFile {
            keys: vec![entry.clone()],
        };
        let text = serde_json::to_string_pretty(&file).expect("json");
        assert!(
            !text.contains(key.expose().as_str()),
            "only the digest is kept"
        );
        let auth = ApiKeyAuthenticator::new(&KeysFile::parse(&text).expect("parses"));

        let headers = |v: &str| {
            let mut h = HeaderMap::new();
            h.insert(
                axum::http::header::AUTHORIZATION,
                v.parse().expect("a header"),
            );
            h
        };
        let p = auth
            .authenticate(&headers(&format!("Bearer {}", key.expose())))
            .await
            .expect("known");
        assert_eq!(p.subject, "alice");
        assert_eq!(p.credential.as_deref(), Some(entry.id.as_str()));

        assert_eq!(
            auth.authenticate(&HeaderMap::new()).await,
            Err(AuthError::Missing)
        );
        assert_eq!(
            auth.authenticate(&headers("Basic abc")).await,
            Err(AuthError::Malformed)
        );
        // Same id, another secret: unknown.
        let mut forged = key.expose().clone();
        let last = forged.pop().expect("a char");
        forged.push(if last == 'a' { 'b' } else { 'a' });
        assert_eq!(
            auth.authenticate(&headers(&format!("Bearer {forged}")))
                .await,
            Err(AuthError::Unknown)
        );
        let (other, _) =
            mint_api_key("bob", PrincipalKind::User, Vec::new(), Vec::new()).expect("minted");
        assert_eq!(
            auth.authenticate(&headers(&format!("Bearer {}", other.expose())))
                .await,
            Err(AuthError::Unknown)
        );
    }

    #[test]
    fn dev_needs_a_loopback_listener() {
        assert!(DevLoopbackAuthenticator::new("127.0.0.1:7680".parse().expect("addr")).is_ok());
        assert!(DevLoopbackAuthenticator::new("[::1]:7680".parse().expect("addr")).is_ok());
        assert!(DevLoopbackAuthenticator::new("0.0.0.0:7680".parse().expect("addr")).is_err());
        assert!(DevLoopbackAuthenticator::new("10.0.0.5:7680".parse().expect("addr")).is_err());
    }

    #[test]
    fn keys_files_are_checked() {
        let (_, entry) =
            mint_api_key("alice", PrincipalKind::User, Vec::new(), Vec::new()).expect("minted");
        let twice = KeysFile {
            keys: vec![entry.clone(), entry.clone()],
        };
        assert!(KeysFile::parse(&serde_json::to_string(&twice).expect("json")).is_err());
        let bad_grant = r#"{"keys":[{"id":"ID","sha256":"HASH","subject":"a","kind":"user","grants":[{"relation":"viewer","project":"main"}]}]}"#
            .replace("ID", &entry.id)
            .replace("HASH", &entry.sha256);
        assert!(KeysFile::parse(&bad_grant).is_err());
        let good = bad_grant.replace("\"main\"", "\"prj-01J\"");
        let parsed = KeysFile::parse(&good).expect("a project grant");
        assert_eq!(
            parsed.keys[0].grants[0].on,
            GrantObject::Project("prj-01J".into())
        );
    }

    #[test]
    fn every_rpc_needs_a_relation() {
        assert_eq!(
            required("CreateBranch"),
            Some((Relation::Editor, Scope::Project))
        );
        assert_eq!(
            required("ListProjects"),
            Some((Relation::Viewer, Scope::Namespace))
        );
        assert_eq!(
            required("GetConnectionInfo"),
            Some((Relation::Connect, Scope::Project))
        );
        assert_eq!(required("Nope"), None);
    }

    /// `--pg-authz openfga` asks model v1 what the grants file would decide: a person with
    /// `editor` on the namespace's project edits and connects, a viewer reads and never
    /// connects, an agent never administers, and nothing reaches another namespace.
    #[tokio::test]
    async fn the_model_authorizer_decides_as_model_v1() {
        use loams_authz::{BuiltinAuthorizer, Tuple};
        let fga = BuiltinAuthorizer::new();
        fga.write(&[
            Tuple::new("org:o", "org", "project:p"),
            Tuple::new("project:p", "project", "environment:ns1"),
            Tuple::new("project:p", "project", "environment:ns2"),
            Tuple::new("user:ed", "editor", "project:p"),
            Tuple::new("user:vi", "viewer", "project:p"),
            Tuple::new("user:dep", "deployer", "environment:ns1"),
            Tuple::new("agent:a1", "deployer", "environment:ns1"),
            Tuple::new("user:other", "editor", "environment:ns2"),
        ])
        .unwrap();
        let m = ModelAuthorizer::new(std::sync::Arc::new(fga));
        let pj = project("ns1", "01J0000000000000000000000P");
        let ns = Object::Namespace("ns1".into());
        let can = |p: Principal, r: Relation, o: Object| {
            let m = m.clone();
            async move { authorize(&m, &p, r, &o).await.is_ok() }
        };
        let agent = |id: &str| Principal {
            kind: PrincipalKind::Agent,
            ..user(id)
        };
        use Relation::*;
        assert!(can(user("ed"), Editor, pj.clone()).await);
        assert!(can(user("ed"), Connect, pj.clone()).await);
        assert!(
            can(user("ed"), Editor, ns.clone()).await,
            "creates projects"
        );
        assert!(!can(user("ed"), Admin, pj.clone()).await);
        assert!(can(user("vi"), Viewer, pj.clone()).await);
        assert!(
            !can(user("vi"), Connect, pj.clone()).await,
            "a viewer never connects"
        );
        assert!(!can(user("vi"), Editor, pj.clone()).await);
        assert!(can(user("dep"), Admin, pj.clone()).await);
        assert!(
            !can(agent("a1"), Admin, pj.clone()).await,
            "an agent never administers"
        );
        assert!(can(agent("a1"), Editor, pj.clone()).await);
        assert!(
            !can(user("other"), Viewer, pj.clone()).await,
            "another namespace's grant"
        );
        assert!(!can(user("nobody"), Viewer, ns.clone()).await);
        assert!(
            !can(Principal::dev(), Viewer, ns.clone()).await,
            "dev holds nothing here"
        );
        // An agent acting for the editor reaches no more than the agent itself may.
        let acting = Principal {
            act: vec![Actor {
                subject: "a2".into(),
                kind: PrincipalKind::Agent,
            }],
            ..user("ed")
        };
        assert!(!can(acting, Viewer, pj.clone()).await);
    }
}
