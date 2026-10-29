//! The engine's principal store (API2 Tasks 6 and 7): the org, its people
//! and roles, teams, projects and environments, agents and service
//! accounts, the API keys `KeyService/CreateKey` makes, console sessions and
//! the audit log, kept in the metastore.
//!
//! **Storage.** The whole directory is one JSON document ([`Doc`]): an
//! object under `system/directory/` in the bucket, named by the manifest
//! pointer `principal-directory` of the reserved namespace `_system`. A
//! write puts a new object and moves the pointer with a compare-and-swap, so
//! concurrent writers serialise and a lost race is redone on the new
//! document (the graph catalog's pattern, `loams-graph/src/catalog.rs`).
//! Reads check the pointer's version and reuse the parsed document while it
//! is unchanged, which is one metastore read per authenticated call.
//!
//! **Secrets are hashes.** An API key is stored as its argon2id PHC string
//! (the same [`api_key::hash_key`] the key file uses), a password too, and a
//! session token as its SHA-256 (it is 256 random bits, so a fast hash is
//! enough). No answer and no log ever carries one.
//!
//! **Bounded.** The audit log keeps the newest [`AUDIT_KEPT`] events and the
//! token revocation list the newest [`REVOKED_TOKENS_KEPT`] ids; expired
//! sessions are dropped on the next write.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use bytes::Bytes;
use http::HeaderMap;
use loams_common::NamespaceId;
use loams_common::meta::{ApplyError, Consistency, MetaError, MetaStore, PointerCas};
use loams_proto::google::protobuf::Timestamp;
use loams_proto::loams::admin::v1 as pb;
use loams_store::Store;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tokio::sync::broadcast;

use super::{AuthError, Credential, Grant, Principal, PrincipalKind, api_key, reason};

/// The prefix of a console session token.
pub const SESSION_PREFIX: &str = "ls_";
/// The random characters of a session token (base62, ~256 bits).
const SESSION_SECRET_LEN: usize = 43;
/// The session cookie (API2 ruling 6/7.9).
pub const SESSION_COOKIE: &str = "loams_session";
/// The header a cookie-authenticated call carries its CSRF token in.
pub const CSRF_HEADER: &str = "x-csrf-token";
/// How long a session lasts.
pub const SESSION_TTL: Duration = Duration::from_secs(12 * 3600);
/// The newest audit events the directory keeps.
pub const AUDIT_KEPT: usize = 1_000;
/// The newest revoked token ids the directory keeps.
pub const REVOKED_TOKENS_KEPT: usize = 10_000;
/// The reserved namespace whose pointer names the directory.
pub use loams_common::meta::SYSTEM_NAMESPACE;
/// The pointer key.
pub const POINTER_KEY: &str = "principal-directory";
/// Where the directory's objects live in the bucket.
const OBJECT_PREFIX: &str = "system/directory/";
/// How many times a write is redone after losing a race.
const MAX_ATTEMPTS: usize = 16;

/// Whether `value` has a session token's shape.
pub fn looks_like_session(value: &str) -> bool {
    value.strip_prefix(SESSION_PREFIX).is_some_and(|secret| {
        secret.len() == SESSION_SECRET_LEN && secret.bytes().all(|b| b.is_ascii_alphanumeric())
    })
}

/// The `loams_session` cookie's value, if the request has one.
pub fn session_cookie(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(http::header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, value)| *name == SESSION_COOKIE && !value.is_empty())
        .map(|(_, value)| value.trim_matches('"').to_owned())
}

/// The session token a request carries: an `ls_…` bearer or the cookie.
pub fn session_token(headers: &HeaderMap) -> Option<String> {
    let bearer = headers
        .get(http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().split_once(' '))
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("bearer"))
        .map(|(_, token)| token.trim().to_owned())
        .filter(|token| looks_like_session(token));
    bearer.or_else(|| session_cookie(headers))
}

/// A new session token.
pub fn new_session_token() -> String {
    format!(
        "{SESSION_PREFIX}{}",
        api_key::random_base62(SESSION_SECRET_LEN)
    )
}

/// The hex SHA-256 a session token is stored as.
pub fn token_digest(token: &str) -> String {
    Sha256::digest(token.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// A protobuf timestamp from milliseconds since the Unix epoch.
pub fn timestamp(ms: i64) -> Timestamp {
    Timestamp {
        seconds: ms.div_euclid(1000),
        nanos: i32::try_from(ms.rem_euclid(1000) * 1_000_000).unwrap_or(0),
        ..Default::default()
    }
}

/// Milliseconds since the Unix epoch of a protobuf timestamp.
pub fn ms_of(ts: &Timestamp) -> i64 {
    ts.seconds
        .saturating_mul(1000)
        .saturating_add(i64::from(ts.nanos) / 1_000_000)
}

/// A new id: `<prefix>_<ULID>`.
pub fn new_id(prefix: &str) -> String {
    format!("{prefix}_{}", ulid::Ulid::generate())
}

/// A role in the org, lowest first, so `role >= need` is the check.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Member,
    Admin,
    Owner,
}

impl Role {
    /// The proto enum.
    pub fn to_proto(self) -> pb::OrgRole {
        match self {
            Self::Member => pb::OrgRole::ORG_ROLE_MEMBER,
            Self::Admin => pb::OrgRole::ORG_ROLE_ADMIN,
            Self::Owner => pb::OrgRole::ORG_ROLE_OWNER,
        }
    }

    /// From the proto enum; `None` for unspecified.
    pub fn from_proto(role: pb::OrgRole) -> Option<Self> {
        match role {
            pb::OrgRole::ORG_ROLE_MEMBER => Some(Self::Member),
            pb::OrgRole::ORG_ROLE_ADMIN => Some(Self::Admin),
            pb::OrgRole::ORG_ROLE_OWNER => Some(Self::Owner),
            _ => None,
        }
    }

    /// The role's name, as `ErrorInfo.metadata["role"]` carries it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Member => "member",
            Self::Admin => "admin",
            Self::Owner => "owner",
        }
    }
}

/// A person in the org.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemberRec {
    pub user: pb::User,
    pub role: Role,
    pub joined_at_ms: i64,
    /// The argon2id PHC string of their password; empty when they sign in
    /// another way (OIDC, an invitation accepted through SSO).
    #[serde(default)]
    pub password: String,
}

/// A team and its people (user ids).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TeamRec {
    pub team: pb::Team,
    #[serde(default)]
    pub members: Vec<String>,
}

/// A project, its grants and its environments.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectRec {
    pub project: pb::Project,
    #[serde(default)]
    pub grants: Vec<pb::Grant>,
    #[serde(default)]
    pub environments: Vec<pb::Environment>,
}

/// An agent and its trust policies.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentRec {
    pub agent: pb::Agent,
    #[serde(default)]
    pub trust_policies: Vec<pb::TrustPolicy>,
}

/// An API key `CreateKey` made.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KeyRec {
    /// What `ListKeys` answers (no secret).
    pub key: pb::ApiKey,
    /// The project's slug.
    pub project: String,
    /// The key's argon2id PHC string.
    pub hash: String,
    /// What the key may do in the org: its creator's role for a key that
    /// acts as its creator, `member` for a service account's.
    pub role: Role,
    /// The `idempotency_key` that created it: a replay is
    /// `secret_already_issued`.
    pub idempotency_key: String,
    /// Who asked for it (the actor's id), for the idempotency scope.
    pub created_by: String,
    #[serde(default)]
    pub revoked: bool,
}

/// A console session.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionRec {
    /// [`token_digest`] of the token.
    pub digest: String,
    pub csrf: String,
    /// The user's id.
    pub user: String,
    pub expires_ms: i64,
}

/// What kind of Loams principal an identity provider's subject is (design 76 §5, D1628).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkKind {
    /// A person: `principal` is their member's user id.
    User,
    /// An AI agent (a Zitadel machine user): `principal` is the agent's id.
    Agent,
    /// Any other machine: `principal` is the service account's id.
    ServiceAccount,
}

/// A principal link (design 76 §5, D1605, D1624, D1628): the identity provider's `(issuer,
/// subject)` and the Loams principal it is. A JWT does not say whether its subject is a person
/// or a machine, so the link is the source of the kind; a subject with no link is a person.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrincipalLink {
    /// The token's `iss`, byte for byte.
    pub issuer: String,
    /// The token's `sub`.
    pub subject: String,
    pub kind: LinkKind,
    /// The Loams principal's id.
    pub principal: String,
    /// A disabled principal is refused on every request, whatever its token's signature and
    /// expiry say (D1624).
    #[serde(default)]
    pub disabled: bool,
}

/// One audit event, numbered.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AuditRec {
    pub seq: u64,
    pub event: pb::AuditEvent,
}

/// What a `KeyService` key reaches (LV1 plan Task 14): the org, its
/// project's environment (`<project>/<environment>`), its scopes (every
/// scope when it lists none) and its expiry.
pub fn key_grant(record: &KeyRec, org: Option<String>) -> Grant {
    let environment = (!record.key.environment.is_empty())
        .then(|| format!("{}/{}", record.project, record.key.environment));
    Grant {
        org,
        environment,
        scopes: Grant::scopes_of(&record.key.scopes),
        expires_at_ms: record
            .key
            .expires_at
            .as_option()
            .and_then(|t| u64::try_from(ms_of(t)).ok()),
    }
}

/// The person a key acts as, when it acts as one (a `KeyService` key made for oneself).
pub fn key_person(key: &KeyRec) -> Option<&str> {
    key.key
        .principal
        .as_option()
        .filter(|p| p.kind.as_known() == Some(pb::SubjectKind::SUBJECT_KIND_USER))
        .map(|p| p.id.as_str())
}

/// The whole directory.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Doc {
    /// `None` until `CompleteSetup`.
    pub org: Option<pb::Org>,
    pub members: Vec<MemberRec>,
    pub invitations: Vec<pb::Invitation>,
    pub teams: Vec<TeamRec>,
    pub projects: Vec<ProjectRec>,
    pub agents: Vec<AgentRec>,
    pub service_accounts: Vec<pb::ServiceAccount>,
    pub keys: Vec<KeyRec>,
    /// The principal links, `(issuer, subject)` → principal (D1628).
    pub links: Vec<PrincipalLink>,
    /// Revoked agent token ids (`jti`), oldest first.
    pub revoked_tokens: Vec<String>,
    pub sessions: Vec<SessionRec>,
    /// The newest [`AUDIT_KEPT`] events, oldest first.
    pub audit: Vec<AuditRec>,
    /// The last audit `seq` handed out.
    pub audit_seq: u64,
}

impl Doc {
    /// The member with user id `id`.
    pub fn member(&self, id: &str) -> Option<&MemberRec> {
        self.members.iter().find(|m| m.user.id == id)
    }

    /// The keys that act as the person `user` (made by them for themselves, not a service
    /// account's).
    pub fn keys_of_person<'d>(&'d self, user: &'d str) -> impl Iterator<Item = &'d KeyRec> + 'd {
        self.keys
            .iter()
            .filter(move |k| key_person(k) == Some(user))
    }

    /// Whether `key` acts as a person who is no longer a member: a key `RemoveMember` did not
    /// revoke, made before it did (plan IA1, ruling T5.R1). Such a key is refused.
    pub fn key_person_left(&self, key: &KeyRec) -> bool {
        key_person(key).is_some_and(|user| self.member(user).is_none())
    }

    /// The member with `email` (compared without case).
    pub fn member_by_email(&self, email: &str) -> Option<&MemberRec> {
        self.members
            .iter()
            .find(|m| m.user.email.eq_ignore_ascii_case(email))
    }

    /// The project with slug `slug`.
    pub fn project(&self, slug: &str) -> Option<&ProjectRec> {
        self.projects.iter().find(|p| p.project.slug == slug)
    }

    pub fn project_mut(&mut self, slug: &str) -> Option<&mut ProjectRec> {
        self.projects.iter_mut().find(|p| p.project.slug == slug)
    }

    /// The team with id or slug `team`.
    pub fn team(&self, team: &str) -> Option<&TeamRec> {
        self.teams
            .iter()
            .find(|t| t.team.id == team || t.team.slug == team)
    }

    /// The agent with id `id`.
    pub fn agent(&self, id: &str) -> Option<&AgentRec> {
        self.agents.iter().find(|a| a.agent.id == id)
    }

    pub fn agent_mut(&mut self, id: &str) -> Option<&mut AgentRec> {
        self.agents.iter_mut().find(|a| a.agent.id == id)
    }

    /// The key with id `id`.
    /// The link of the identity provider's `(issuer, subject)`; the issuer must match exactly.
    pub fn link(&self, issuer: &str, subject: &str) -> Option<&PrincipalLink> {
        self.links
            .iter()
            .find(|l| l.issuer == issuer && l.subject == subject)
    }

    pub fn key(&self, id: &str) -> Option<&KeyRec> {
        self.keys.iter().find(|k| k.key.id == id)
    }

    /// Appends `event` to the audit log, numbering it and stamping it now.
    pub fn record(&mut self, mut event: pb::AuditEvent) {
        self.audit_seq += 1;
        event.id = format!("evt_{}", ulid::Ulid::generate());
        event.at = timestamp(now_ms()).into();
        self.audit.push(AuditRec {
            seq: self.audit_seq,
            event,
        });
        if self.audit.len() > AUDIT_KEPT {
            let drop = self.audit.len() - AUDIT_KEPT;
            self.audit.drain(..drop);
        }
    }

    /// Starts a session for `user`; returns its CSRF token and expiry.
    pub fn start_session(&mut self, user: &str, token: &str, ttl: Duration) -> (String, i64) {
        let now = now_ms();
        self.sessions.retain(|s| s.expires_ms > now);
        let csrf = api_key::random_base62(32);
        let expires_ms = now.saturating_add(i64::try_from(ttl.as_millis()).unwrap_or(i64::MAX));
        self.sessions.push(SessionRec {
            digest: token_digest(token),
            csrf: csrf.clone(),
            user: user.to_owned(),
            expires_ms,
        });
        (csrf, expires_ms)
    }

    /// Revokes `jti`, keeping the newest [`REVOKED_TOKENS_KEPT`].
    pub fn revoke_token(&mut self, jti: &str) {
        if !self.revoked_tokens.iter().any(|t| t == jti) {
            self.revoked_tokens.push(jti.to_owned());
        }
        if self.revoked_tokens.len() > REVOKED_TOKENS_KEPT {
            let drop = self.revoked_tokens.len() - REVOKED_TOKENS_KEPT;
            self.revoked_tokens.drain(..drop);
        }
    }
}

/// The directory could not be read or written now.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("the principal directory is unavailable: {0}")]
pub struct Unavailable(pub String);

/// Where the directory document is kept: the metastore and the bucket
/// ([`MetaDirectoryStore`]), or memory in unit tests ([`MemoryStore`]).
#[async_trait]
pub trait DirectoryStore: Send + Sync + fmt::Debug + 'static {
    /// The stored version (`None`: never written), without reading the
    /// document.
    async fn version(&self) -> Result<Option<u64>, Unavailable>;
    /// The stored version and document.
    async fn load(&self) -> Result<Option<(u64, Bytes)>, Unavailable>;
    /// Stores `doc` if the version is still `expected`: `Some(new version)`,
    /// or `None` when another writer moved it first.
    async fn save(&self, expected: Option<u64>, doc: Bytes) -> Result<Option<u64>, Unavailable>;
}

/// A directory kept in memory (unit tests).
#[derive(Debug, Default)]
pub struct MemoryStore(Mutex<Option<(u64, Bytes)>>);

#[async_trait]
impl DirectoryStore for MemoryStore {
    async fn version(&self) -> Result<Option<u64>, Unavailable> {
        Ok(self
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|(v, _)| *v))
    }

    async fn load(&self) -> Result<Option<(u64, Bytes)>, Unavailable> {
        Ok(self.0.lock().unwrap_or_else(|e| e.into_inner()).clone())
    }

    async fn save(&self, expected: Option<u64>, doc: Bytes) -> Result<Option<u64>, Unavailable> {
        let mut slot = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if slot.as_ref().map(|(v, _)| *v) != expected {
            return Ok(None);
        }
        let version = expected.map_or(1, |v| v + 1);
        *slot = Some((version, doc));
        Ok(Some(version))
    }
}

/// The directory in the metastore (the pointer) and the bucket (the
/// document).
pub struct MetaDirectoryStore {
    meta: Arc<dyn MetaStore>,
    store: Store,
    namespace: tokio::sync::OnceCell<NamespaceId>,
    /// The pointer key in the system namespace, and the bucket prefix of the documents.
    pointer_key: &'static str,
    object_prefix: &'static str,
}

impl fmt::Debug for MetaDirectoryStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MetaDirectoryStore").finish_non_exhaustive()
    }
}

fn unavailable(err: impl fmt::Display) -> Unavailable {
    Unavailable(err.to_string())
}

impl MetaDirectoryStore {
    pub fn new(meta: Arc<dyn MetaStore>, store: Store) -> Self {
        Self::with_key(meta, store, POINTER_KEY, OBJECT_PREFIX)
    }

    /// Another document kept the same way, under its own pointer key and bucket prefix (the
    /// identity sync's state, plan IA1 IA3).
    pub fn with_key(
        meta: Arc<dyn MetaStore>,
        store: Store,
        pointer_key: &'static str,
        object_prefix: &'static str,
    ) -> Self {
        Self {
            meta,
            store,
            namespace: tokio::sync::OnceCell::new(),
            pointer_key,
            object_prefix,
        }
    }

    /// Whether an upgrade snapshot may name directory object `path` (its
    /// ULID is not newer than the oldest kept snapshot, UP1 Task 8): it then
    /// stays, so a rollback finds the directory it restores.
    async fn pinned_by_snapshot(&self, path: &str) -> bool {
        let written_ms = path
            .strip_prefix(self.object_prefix)
            .and_then(|name| name.strip_suffix(".json"))
            .and_then(|id| id.parse::<ulid::Ulid>().ok())
            .map(|id| id.timestamp_ms());
        match loams_common::meta::snapshot_gc_floor(&*self.meta).await {
            Ok(floor) => written_ms.is_none_or(|ms| loams_common::meta::pinned_by(floor, ms)),
            // Unknown: keep it; it only costs storage.
            Err(_) => true,
        }
    }

    /// The reserved namespace, if it exists yet (reads never create it).
    async fn existing_namespace(&self) -> Result<Option<NamespaceId>, Unavailable> {
        if let Some(id) = self.namespace.get() {
            return Ok(Some(*id));
        }
        let found = self
            .meta
            .namespace_by_name(Consistency::Linearizable, SYSTEM_NAMESPACE)
            .await
            .map_err(unavailable)?;
        Ok(match found {
            Some(ns) => Some(*self.namespace.get_or_init(|| async { ns.id }).await),
            None => None,
        })
    }

    /// The reserved namespace, created on the first write.
    async fn namespace(&self) -> Result<NamespaceId, Unavailable> {
        if let Some(id) = self.existing_namespace().await? {
            return Ok(id);
        }
        let id = match self.meta.create_namespace(SYSTEM_NAMESPACE).await {
            Ok(id) | Err(MetaError::Rejected(ApplyError::NamespaceExists(id))) => id,
            Err(err) => return Err(unavailable(err)),
        };
        Ok(*self.namespace.get_or_init(|| async { id }).await)
    }
}

#[async_trait]
impl DirectoryStore for MetaDirectoryStore {
    async fn version(&self) -> Result<Option<u64>, Unavailable> {
        let Some(ns) = self.existing_namespace().await? else {
            return Ok(None);
        };
        let pointer = self
            .meta
            .pointer(Consistency::Local, ns, self.pointer_key)
            .await
            .map_err(unavailable)?;
        Ok(pointer.map(|p| p.version))
    }

    async fn load(&self) -> Result<Option<(u64, Bytes)>, Unavailable> {
        let Some(ns) = self.existing_namespace().await? else {
            return Ok(None);
        };
        // A writer deletes the object it replaced, so a read that lost that
        // race reads the pointer again.
        let mut last = None;
        for _ in 0..4 {
            let Some(pointer) = self
                .meta
                .pointer(Consistency::Linearizable, ns, self.pointer_key)
                .await
                .map_err(unavailable)?
            else {
                return Ok(None);
            };
            match self.store.get(&pointer.value).await {
                Ok((bytes, _)) => return Ok(Some((pointer.version, bytes))),
                Err(err) => last = Some(err),
            }
        }
        Err(unavailable(last.map_or_else(
            || "the directory object is missing".to_owned(),
            |e| e.to_string(),
        )))
    }

    async fn save(&self, expected: Option<u64>, doc: Bytes) -> Result<Option<u64>, Unavailable> {
        let ns = self.namespace().await?;
        let previous = match expected {
            Some(_) => self
                .meta
                .pointer(Consistency::Linearizable, ns, self.pointer_key)
                .await
                .map_err(unavailable)?
                .map(|p| p.value),
            None => None,
        };
        let object = format!("{}{}.json", self.object_prefix, ulid::Ulid::generate());
        self.store.put(&object, doc).await.map_err(unavailable)?;
        for _ in 0..4 {
            let cas = self
                .meta
                .cas_pointer(PointerCas {
                    namespace: ns,
                    key: self.pointer_key.to_owned(),
                    expected,
                    value: object.clone(),
                    fence: None,
                    fresh: None,
                })
                .await;
            match cas.result {
                Ok(version) => {
                    if let Some(old) = previous.filter(|old| *old != object)
                        && !self.pinned_by_snapshot(&old).await
                        && let Err(err) = self.store.delete(&old).await
                    {
                        tracing::debug!(%err, "the replaced directory object stays");
                    }
                    return Ok(Some(version));
                }
                // A lost acknowledgement of this very write.
                Err(MetaError::Rejected(ApplyError::VersionMismatch {
                    current: Some(current),
                })) if current.value == object => return Ok(Some(current.version)),
                Err(MetaError::Rejected(ApplyError::VersionMismatch { .. })) => {
                    let _ = self.store.delete(&object).await;
                    return Ok(None);
                }
                // Unknown outcome: the CAS is retry-safe and recognises its
                // own effect.
                Err(
                    MetaError::NotLeader { .. } | MetaError::Timeout | MetaError::Unavailable(_),
                ) => {}
                Err(err) => return Err(unavailable(err)),
            }
        }
        Err(Unavailable(
            "the metastore did not settle the write".to_owned(),
        ))
    }
}

/// The parsed document at a version.
#[derive(Debug, Clone)]
struct Cached {
    version: Option<u64>,
    doc: Arc<Doc>,
}

/// The principal store: read and update the [`Doc`], authenticate its
/// sessions and API keys, and stream its audit events.
pub struct Directory {
    store: Arc<dyn DirectoryStore>,
    cache: tokio::sync::Mutex<Option<Cached>>,
    writes: tokio::sync::Mutex<()>,
    audit: broadcast::Sender<AuditRec>,
    session_ttl: Duration,
    /// SHA-256 of API keys that verified, by key id (argon2id is slow).
    verified: Mutex<HashMap<String, [u8; 32]>>,
}

impl fmt::Debug for Directory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Directory")
            .field("store", &self.store)
            .finish_non_exhaustive()
    }
}

impl Directory {
    /// The directory in the metastore and the bucket.
    pub fn new(meta: Arc<dyn MetaStore>, store: Store) -> Arc<Self> {
        Self::with_store(Arc::new(MetaDirectoryStore::new(meta, store)), SESSION_TTL)
    }

    /// A directory in memory (unit tests).
    pub fn in_memory() -> Arc<Self> {
        Self::with_store(Arc::new(MemoryStore::default()), SESSION_TTL)
    }

    /// A directory over `store`, with sessions lasting `session_ttl`.
    pub fn with_store(store: Arc<dyn DirectoryStore>, session_ttl: Duration) -> Arc<Self> {
        Arc::new(Self {
            store,
            cache: tokio::sync::Mutex::new(None),
            writes: tokio::sync::Mutex::new(()),
            audit: broadcast::channel(1024).0,
            session_ttl,
            verified: Mutex::new(HashMap::new()),
        })
    }

    /// How long a new session lasts.
    pub fn session_ttl(&self) -> Duration {
        self.session_ttl
    }

    /// The audit events this process records from now on.
    pub fn subscribe(&self) -> broadcast::Receiver<AuditRec> {
        self.audit.subscribe()
    }

    /// The current document.
    pub async fn read(&self) -> Result<Arc<Doc>, Unavailable> {
        Ok(self.current().await?.doc)
    }

    async fn current(&self) -> Result<Cached, Unavailable> {
        let mut cache = self.cache.lock().await;
        let version = self.store.version().await?;
        if let Some(cached) = cache.as_ref()
            && cached.version == version
        {
            return Ok(cached.clone());
        }
        let cached = match self.store.load().await? {
            Some((version, bytes)) => Cached {
                version: Some(version),
                doc: Arc::new(serde_json::from_slice(&bytes).map_err(|err| {
                    Unavailable(format!("the stored directory does not parse: {err}"))
                })?),
            },
            None => Cached {
                version: None,
                doc: Arc::new(Doc::default()),
            },
        };
        *cache = Some(cached.clone());
        Ok(cached)
    }

    /// Applies `change` to the document and stores the result. `change` may
    /// run more than once (after losing a race it is redone on the newer
    /// document), so it must decide from the document alone. An `Err` from
    /// it stores nothing.
    pub async fn update<T, E>(
        &self,
        mut change: impl FnMut(&mut Doc) -> Result<T, E>,
    ) -> Result<T, E>
    where
        E: From<Unavailable>,
    {
        let _writer = self.writes.lock().await;
        for _ in 0..MAX_ATTEMPTS {
            let current = self.current().await?;
            let mut doc = (*current.doc).clone();
            let out = change(&mut doc)?;
            let bytes = serde_json::to_vec(&doc)
                .map_err(|err| Unavailable(format!("the directory does not serialise: {err}")))?;
            let Some(version) = self.store.save(current.version, Bytes::from(bytes)).await? else {
                continue;
            };
            let seen = current.doc.audit_seq;
            let fresh: Vec<AuditRec> = doc.audit.iter().filter(|r| r.seq > seen).cloned().collect();
            *self.cache.lock().await = Some(Cached {
                version: Some(version),
                doc: Arc::new(doc),
            });
            for record in fresh {
                // No watcher is fine.
                let _ = self.audit.send(record);
            }
            return Ok(out);
        }
        Err(Unavailable("the directory is contended; retry".to_owned()).into())
    }

    /// The principal for a session or an API key this directory holds;
    /// `None` for any other credential (an API key it does not hold
    /// included, which the key file may).
    pub async fn authenticate(
        &self,
        credential: &Credential,
    ) -> Option<Result<Principal, AuthError>> {
        match credential {
            Credential::Session(session) => Some(
                self.session_principal(&session.token, session.csrf.as_deref())
                    .await,
            ),
            Credential::ApiKey(key) => self.key_principal(key).await,
            Credential::Jwt(_) => None,
        }
    }

    async fn session_principal(
        &self,
        token: &str,
        csrf: Option<&str>,
    ) -> Result<Principal, AuthError> {
        let doc = self
            .read()
            .await
            .map_err(|err| AuthError::unavailable(reason::AUTH_UNAVAILABLE, err.to_string()))?;
        let digest = token_digest(token);
        let ended = || {
            AuthError::unauthenticated(
                reason::TOKEN_REVOKED,
                "this session has ended; sign in again",
            )
        };
        let session = doc
            .sessions
            .iter()
            .find(|s| s.digest == digest)
            .ok_or_else(ended)?;
        if session.expires_ms <= now_ms() {
            return Err(AuthError::unauthenticated(
                reason::TOKEN_EXPIRED,
                "this session has expired; sign in again",
            ));
        }
        if let Some(csrf) = csrf
            && !bool::from(csrf.as_bytes().ct_eq(session.csrf.as_bytes()))
        {
            return Err(AuthError::forbidden(
                reason::CSRF_TOKEN_INVALID,
                "a cookie-authenticated call must send the session's X-CSRF-Token",
            ));
        }
        let member = doc.member(&session.user).ok_or_else(ended)?;
        Ok(Principal {
            subject: member.user.id.clone(),
            kind: PrincipalKind::Session,
            issuer: None,
            display_name: member.user.name.clone(),
            email: Some(member.user.email.clone()),
            // A person in the org: every environment of it (LV1 plan Task
            // 14).
            grant: Grant {
                org: doc.org.as_ref().map(|o| o.slug.clone()),
                expires_at_ms: u64::try_from(session.expires_ms).ok(),
                ..Grant::default()
            },
        })
    }

    async fn key_principal(&self, key: &str) -> Option<Result<Principal, AuthError>> {
        let id = api_key::key_id(key)?;
        let doc = match self.read().await {
            Ok(doc) => doc,
            Err(err) => {
                return Some(Err(AuthError::unavailable(
                    reason::AUTH_UNAVAILABLE,
                    err.to_string(),
                )));
            }
        };
        let record = doc.key(id)?;
        let org = doc.org.as_ref().map(|o| o.slug.clone());
        let person_left = doc.key_person_left(record);
        Some(self.check_key(key, id, record, org, person_left).await)
    }

    async fn check_key(
        &self,
        key: &str,
        id: &str,
        record: &KeyRec,
        org: Option<String>,
        person_left: bool,
    ) -> Result<Principal, AuthError> {
        if record.revoked {
            return Err(AuthError::unauthenticated(
                reason::TOKEN_REVOKED,
                "this API key was revoked",
            ));
        }
        if let Some(expires) = record.key.expires_at.as_option()
            && ms_of(expires) <= now_ms()
        {
            return Err(AuthError::unauthenticated(
                reason::TOKEN_EXPIRED,
                "this API key has expired",
            ));
        }
        let digest: [u8; 32] = Sha256::digest(key.as_bytes()).into();
        let known = self
            .verified
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
            .is_some_and(|known| bool::from(known.ct_eq(&digest)));
        if !known {
            if !api_key::verify_hash(key, &record.hash).await {
                return Err(AuthError::unauthenticated(
                    reason::API_KEY_INVALID,
                    "the API key is not valid",
                ));
            }
            self.verified
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(id.to_owned(), digest);
        }
        // After the secret is verified, so this answer says nothing to a caller without it.
        if person_left {
            return Err(AuthError::unauthenticated(
                reason::TOKEN_REVOKED,
                "this API key acts as a person who is no longer a member of the org",
            ));
        }
        Ok(Principal {
            subject: format!("apikey:{id}"),
            kind: PrincipalKind::ApiKey,
            issuer: None,
            display_name: record.key.name.clone(),
            email: None,
            grant: key_grant(record, org),
        })
    }

    /// The member whose `email` and `password` these are. A wrong email
    /// costs the same argon2id check as a wrong password.
    pub async fn check_password(
        &self,
        email: &str,
        password: &str,
    ) -> Result<Option<MemberRec>, Unavailable> {
        static DUMMY: LazyLock<String> = LazyLock::new(|| api_key::hash_key("not a password"));
        let doc = self.read().await?;
        let member = doc
            .member_by_email(email)
            .filter(|m| !m.password.is_empty())
            .cloned();
        let hash = member
            .as_ref()
            .map_or(DUMMY.as_str(), |m| m.password.as_str());
        let ok = api_key::verify_hash(password, hash).await;
        Ok(member.filter(|_| ok))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::SessionCredential;

    fn user(id: &str, email: &str) -> pb::User {
        pb::User {
            id: id.to_owned(),
            name: "Ada".to_owned(),
            email: email.to_owned(),
            ..Default::default()
        }
    }

    #[test]
    fn session_tokens_and_cookies_are_recognised() {
        let token = new_session_token();
        assert!(looks_like_session(&token), "{token}");
        assert!(!looks_like_session("ls_short"));
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::COOKIE,
            format!("a=1; {SESSION_COOKIE}={token}; b=2")
                .parse()
                .unwrap(),
        );
        assert_eq!(session_cookie(&headers).as_deref(), Some(token.as_str()));
        assert_eq!(session_token(&headers).as_deref(), Some(token.as_str()));
        let mut bearer = HeaderMap::new();
        bearer.insert(
            http::header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        assert_eq!(session_token(&bearer).as_deref(), Some(token.as_str()));
        assert_ne!(token_digest(&token), token);
    }

    /// LV1 plan Task 14: a `KeyService` key reaches its project's
    /// environment with its scopes and expiry; a key with no environment or
    /// scopes keeps the self-hosted default.
    #[test]
    fn key_grants_name_the_environment_and_scopes() {
        let mut record = KeyRec {
            key: pb::ApiKey {
                id: "k".to_owned(),
                environment: "prod".to_owned(),
                scopes: vec!["live:call".to_owned(), " ".to_owned()],
                expires_at: buffa::MessageField::some(Timestamp {
                    seconds: 1_800_000_000,
                    ..Default::default()
                }),
                ..Default::default()
            },
            project: "shop".to_owned(),
            hash: String::new(),
            role: Role::Member,
            idempotency_key: String::new(),
            created_by: String::new(),
            revoked: false,
        };
        let grant = key_grant(&record, Some("acme".to_owned()));
        assert_eq!(grant.org.as_deref(), Some("acme"));
        assert_eq!(grant.environment.as_deref(), Some("shop/prod"));
        assert!(grant.has_scope("live:call") && !grant.has_scope("live:deploy"));
        assert_eq!(grant.expires_at_ms, Some(1_800_000_000_000));
        record.key.environment.clear();
        record.key.scopes.clear();
        record.key.expires_at = buffa::MessageField::none();
        let open = key_grant(&record, None);
        assert_eq!(open, Grant::default());
    }

    #[test]
    fn roles_are_ordered() {
        assert!(Role::Owner > Role::Admin && Role::Admin > Role::Member);
        for role in [Role::Member, Role::Admin, Role::Owner] {
            assert_eq!(Role::from_proto(role.to_proto()), Some(role));
        }
        assert_eq!(Role::from_proto(pb::OrgRole::ORG_ROLE_UNSPECIFIED), None);
    }

    #[tokio::test]
    async fn updates_persist_and_a_lost_race_is_redone() {
        let store = Arc::new(MemoryStore::default());
        let a = Directory::with_store(store.clone(), SESSION_TTL);
        let b = Directory::with_store(store, SESSION_TTL);
        a.update(|doc| {
            doc.members.push(MemberRec {
                user: user("usr_1", "ada@example.test"),
                role: Role::Owner,
                joined_at_ms: 0,
                password: String::new(),
            });
            Ok::<_, Unavailable>(())
        })
        .await
        .unwrap();
        // `b` read nothing yet; its write lands on `a`'s document.
        b.update(|doc| {
            doc.members.push(MemberRec {
                user: user("usr_2", "bob@example.test"),
                role: Role::Member,
                joined_at_ms: 0,
                password: String::new(),
            });
            Ok::<_, Unavailable>(())
        })
        .await
        .unwrap();
        let doc = a.read().await.unwrap();
        assert_eq!(doc.members.len(), 2);
        assert!(doc.member_by_email("ADA@example.test").is_some());
    }

    #[tokio::test]
    async fn sessions_expire_end_and_check_csrf() {
        let directory = Directory::with_store(Arc::new(MemoryStore::default()), Duration::ZERO);
        let token = new_session_token();
        let (csrf, _) = directory
            .update(|doc| {
                doc.members.push(MemberRec {
                    user: user("usr_1", "ada@example.test"),
                    role: Role::Owner,
                    joined_at_ms: 0,
                    password: String::new(),
                });
                Ok::<_, Unavailable>(doc.start_session("usr_1", &token, Duration::from_secs(60)))
            })
            .await
            .unwrap();
        let session = |csrf: Option<&str>| {
            Credential::Session(SessionCredential {
                token: token.clone(),
                csrf: csrf.map(str::to_owned),
            })
        };
        let p = directory
            .authenticate(&session(None))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            (p.kind, p.subject.as_str()),
            (PrincipalKind::Session, "usr_1")
        );
        directory
            .authenticate(&session(Some(&csrf)))
            .await
            .unwrap()
            .unwrap();
        let err = directory
            .authenticate(&session(Some("")))
            .await
            .unwrap()
            .unwrap_err();
        assert_eq!(err.reason, reason::CSRF_TOKEN_INVALID);
        // Expired.
        directory
            .update(|doc| {
                doc.sessions[0].expires_ms = 0;
                Ok::<_, Unavailable>(())
            })
            .await
            .unwrap();
        let err = directory
            .authenticate(&session(None))
            .await
            .unwrap()
            .unwrap_err();
        assert_eq!(err.reason, reason::TOKEN_EXPIRED);
        // Ended.
        directory
            .update(|doc| {
                doc.sessions.clear();
                Ok::<_, Unavailable>(())
            })
            .await
            .unwrap();
        let err = directory
            .authenticate(&session(None))
            .await
            .unwrap()
            .unwrap_err();
        assert_eq!(err.reason, reason::TOKEN_REVOKED);
    }

    #[tokio::test]
    async fn stored_keys_verify_and_revoke() {
        let directory = Directory::in_memory();
        let (key, id) = api_key::generate();
        let hash = api_key::hash_key(&key);
        directory
            .update(|doc| {
                doc.keys.push(KeyRec {
                    key: pb::ApiKey {
                        id: id.clone(),
                        name: "ci".to_owned(),
                        ..Default::default()
                    },
                    project: "web".to_owned(),
                    hash: hash.clone(),
                    role: Role::Admin,
                    idempotency_key: "k1".to_owned(),
                    created_by: "usr_1".to_owned(),
                    revoked: false,
                });
                Ok::<_, Unavailable>(())
            })
            .await
            .unwrap();
        let p = directory
            .authenticate(&Credential::ApiKey(key.clone()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(p.subject, format!("apikey:{id}"));
        // A key the directory does not hold falls through.
        let (other, _) = api_key::generate();
        assert!(
            directory
                .authenticate(&Credential::ApiKey(other))
                .await
                .is_none()
        );
        directory
            .update(|doc| {
                doc.keys[0].revoked = true;
                Ok::<_, Unavailable>(())
            })
            .await
            .unwrap();
        let err = directory
            .authenticate(&Credential::ApiKey(key))
            .await
            .unwrap()
            .unwrap_err();
        assert_eq!(err.reason, reason::TOKEN_REVOKED);
    }

    /// A key acting as a person who has left the org, made before `RemoveMember` revoked such
    /// keys, is refused once its secret checks out (ruling T5.R1).
    #[tokio::test]
    async fn a_key_of_a_person_who_left_is_refused() {
        let directory = Directory::in_memory();
        let (key, id) = api_key::generate();
        let hash = api_key::hash_key(&key);
        let person = |id: &str| pb::SubjectRef {
            kind: pb::SubjectKind::SUBJECT_KIND_USER.into(),
            id: id.to_owned(),
            ..Default::default()
        };
        directory
            .update(|doc| {
                doc.members.push(MemberRec {
                    user: pb::User {
                        id: "usr_1".to_owned(),
                        ..Default::default()
                    },
                    role: Role::Admin,
                    joined_at_ms: 0,
                    password: String::new(),
                });
                doc.keys.push(KeyRec {
                    key: pb::ApiKey {
                        id: id.clone(),
                        name: "mine".to_owned(),
                        principal: person("usr_1").into(),
                        ..Default::default()
                    },
                    project: "web".to_owned(),
                    hash: hash.clone(),
                    role: Role::Admin,
                    idempotency_key: "k1".to_owned(),
                    created_by: "usr_1".to_owned(),
                    revoked: false,
                });
                Ok::<_, Unavailable>(())
            })
            .await
            .unwrap();
        let p = directory
            .authenticate(&Credential::ApiKey(key.clone()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(p.subject, format!("apikey:{id}"));
        directory
            .update(|doc| {
                doc.members.clear();
                Ok::<_, Unavailable>(())
            })
            .await
            .unwrap();
        let err = directory
            .authenticate(&Credential::ApiKey(key))
            .await
            .unwrap()
            .unwrap_err();
        assert_eq!(err.reason, reason::TOKEN_REVOKED);
        // A wrong secret for that key id says only that it is not valid.
        let (forged, _) = api_key::generate();
        let forged = forged.replacen(api_key::key_id(&forged).unwrap(), &id, 1);
        let err = directory
            .authenticate(&Credential::ApiKey(forged))
            .await
            .unwrap()
            .unwrap_err();
        assert_eq!(err.reason, reason::API_KEY_INVALID);
    }

    #[tokio::test]
    async fn the_audit_log_is_bounded_and_streamed() {
        let directory = Directory::in_memory();
        let mut rx = directory.subscribe();
        directory
            .update(|doc| {
                for _ in 0..AUDIT_KEPT + 5 {
                    doc.record(pb::AuditEvent {
                        action: "org.update".to_owned(),
                        ..Default::default()
                    });
                }
                Ok::<_, Unavailable>(())
            })
            .await
            .unwrap();
        let doc = directory.read().await.unwrap();
        assert_eq!(doc.audit.len(), AUDIT_KEPT);
        assert_eq!(doc.audit_seq, (AUDIT_KEPT + 5) as u64);
        assert_eq!(rx.recv().await.unwrap().seq, 6);
    }
}
