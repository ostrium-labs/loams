// The generated service traits return `impl Encodable<_>`; these impls name the concrete message
// type, which is the intended refinement (as `connect.rs` does).
#![allow(refining_impl_trait)]

//! `loams.repos.v1` on the main port (Loams Git, GT1 Task 12; design §36 §19.10, D834).
//!
//! With the `git` feature `RepoService` is served over the bucket's repository catalog and this
//! node's repository sequencers (GT1 Task 13, [`served`]), behind the engine's authentication
//! like every RPC of the main port. Without it every RPC answers `unimplemented` with the reason
//! `feature_not_in_variant` ([`ReposAbsent`]), so a caller can tell "not in this build" from a
//! missing route, and reflection lists a service a route answers. The package's other services
//! are `ObjectService` (GT1 Task 18, `super::objects`), `WorkspaceService` (Task 25,
//! `super::workspaces`) and `EnvService` (Task 61).

use std::sync::Arc;

use connectrpc::{
    ConnectError, RequestContext, Router, ServiceRequest, ServiceResult, ServiceStream,
};
use loams_proto::loams::repos::v1 as pb;
use loams_proto::loams::repos::v1::{RepoService, RepoServiceExt};

use super::AppState;

/// The package, as the catalogue names it.
pub const PACKAGE: &str = "loams.repos.v1";

/// Whether this binary serves `loams.repos.v1` (the `git` feature).
pub const SERVED: bool = cfg!(feature = "git");

/// Registers `RepoService`: served with the `git` feature and a git runtime, else the stub.
pub(super) fn register(router: Router, state: &AppState) -> Router {
    #[cfg(feature = "git")]
    if let Some(git) = &state.git {
        return Arc::new(served::ReposServed {
            meta: Arc::clone(&state.meta),
            git: Arc::clone(git),
            principals: state.principals.clone(),
        })
        .register(router);
    }
    let _ = state;
    Arc::new(ReposAbsent).register(router)
}

#[cfg(feature = "git")]
pub use served::GitRuntime;

/// `RepoService` when this binary does not serve it (yet).
#[derive(Debug)]
struct ReposAbsent;

/// What every RPC answers: `feature_not_in_variant` without the `git` feature, and `unavailable`
/// on a node of a `git` build that does not serve it (no `gateway` role, or `[git]` off).
fn absent(method: &str) -> ConnectError {
    let rpc = format!("{PACKAGE}.RepoService/{method}");
    if SERVED {
        super::connect::refuse(
            connectrpc::ErrorCode::Unavailable,
            "unavailable",
            format!("{rpc} is not served by this node"),
            &[],
        )
    } else {
        super::connect::not_in_variant(&rpc)
    }
}

impl RepoService for ReposAbsent {
    async fn create_repo(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::CreateRepoRequest>,
    ) -> ServiceResult<pb::CreateRepoResponse> {
        Err(absent("CreateRepo"))
    }

    async fn get_repo(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::GetRepoRequest>,
    ) -> ServiceResult<pb::GetRepoResponse> {
        Err(absent("GetRepo"))
    }

    async fn list_repos(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::ListReposRequest>,
    ) -> ServiceResult<pb::ListReposResponse> {
        Err(absent("ListRepos"))
    }

    async fn fork_repo(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::ForkRepoRequest>,
    ) -> ServiceResult<pb::ForkRepoResponse> {
        Err(absent("ForkRepo"))
    }

    async fn delete_repo(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::DeleteRepoRequest>,
    ) -> ServiceResult<pb::DeleteRepoResponse> {
        Err(absent("DeleteRepo"))
    }

    async fn update_repo_config(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::UpdateRepoConfigRequest>,
    ) -> ServiceResult<pb::UpdateRepoConfigResponse> {
        Err(absent("UpdateRepoConfig"))
    }

    async fn list_refs(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::ListRefsRequest>,
    ) -> ServiceResult<pb::ListRefsResponse> {
        Err(absent("ListRefs"))
    }

    async fn compact_repo(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::CompactRepoRequest>,
    ) -> ServiceResult<pb::CompactRepoResponse> {
        Err(absent("CompactRepo"))
    }

    async fn watch_repo(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::WatchRepoRequest>,
    ) -> ServiceResult<ServiceStream<pb::WatchRepoResponse>> {
        Err(absent("WatchRepo"))
    }
}

/// `RepoService` over the bucket catalog and the repository sequencers (GT1 Task 13).
#[cfg(feature = "git")]
pub(crate) mod served {
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::time::Duration;

    use buffa::{EnumValue, MessageField};
    use connectrpc::{
        ConnectError, ErrorCode, RequestContext, Response, ServiceRequest, ServiceResult,
        ServiceStream,
    };
    use futures::StreamExt;
    use loams_common::meta::{ApplyError, Consistency, MetaError, MetaStore};
    use loams_git::catalog::{
        BucketRepoCatalog, CatalogError, CreateRepo, Idempotency, RepoCatalog, RepoLifecycle,
        RepoRecord,
    };
    use loams_git::format::event::TxnBody;
    use loams_git::format::pb as gitpb;
    use loams_git::owner::RepoSequencers;
    use loams_git::{
        BucketRefLog, CommittedTxn, GitAccess, IdemKey, NamespaceId, ReadAt, RefError, RefLog,
        RefName, RefSnapshot, RejectReason, RepoId, RepoName, Seq, StateError,
    };
    use loams_proto::google::protobuf::Timestamp;
    use loams_proto::loams::operations::v1 as ops;
    use loams_proto::loams::repos::v1 as pb;
    use loams_proto::loams::repos::v1::__buffa::oneof::{transaction, watch_repo_response};
    use loams_proto::loams::repos::v1::RepoService;
    use sha2::{Digest, Sha256};
    use tokio::sync::mpsc;

    use super::PACKAGE;
    use crate::api::connect::{not_implemented, refuse};

    /// `ListRepos`' page size when the request names none, and the largest it accepts.
    const REPOS_PAGE: (usize, usize) = (100, 1000);
    /// `ListRefs`' page size when the request names none, and the largest it accepts.
    const REFS_PAGE: (usize, usize) = (1000, 10_000);
    /// Messages a `WatchRepo` stream buffers before its producer waits.
    const WATCH_BUFFER: usize = 64;
    /// The organisation in a repository's events' `tenantid` until MT1 names one (D182).
    const LOCAL_ORG: &str = "local";

    /// What the served `RepoService` shares with the server (the purge sweep, shutdown).
    #[derive(Debug)]
    pub struct GitRuntime {
        pub(crate) catalog: BucketRepoCatalog,
        pub(crate) sequencers: Arc<RepoSequencers>,
        /// How long a `WatchRepo` stream stays quiet before a heartbeat (15 s, AP0).
        pub(crate) watch_heartbeat: Duration,
        /// The bucket, where packs and derived data are read (GT1 Task 18).
        pub(crate) store: loams_store::Store,
        /// The H1 range cache over `store` that object reads go through.
        pub(crate) h1: loams_cache::RangeCache,
        /// Finds workspaces for object reads: `loams_git::ws::StoreWsLookup` over `store`
        /// (GT1 Task 25, ruling T18.1).
        pub(crate) workspaces: Arc<dyn loams_git::objects::WsLookup>,
        /// Tells this node's `WatchWorkspace` streams that a workspace saved (repository and
        /// workspace ids); streams also poll, for saves through other nodes.
        pub(crate) ws_saved: tokio::sync::broadcast::Sender<(RepoId, loams_git::WsId)>,
        /// Workspaces are read only by their owner and org admins (GT-Q13's alternative);
        /// `false`, the default, lets whoever reads the repository read them.
        pub(crate) ws_reads_owner_only: bool,
    }

    /// `RepoService`, served.
    #[derive(Debug)]
    pub(crate) struct ReposServed {
        pub(crate) meta: Arc<dyn MetaStore>,
        pub(crate) git: Arc<GitRuntime>,
        /// Who may do what (GT1 Task 16): the engine's principal store and authorizer.
        pub(crate) principals: crate::auth::Principals,
    }

    pub(crate) fn invalid(field: &str, message: impl Into<String>) -> ConnectError {
        refuse(
            ErrorCode::InvalidArgument,
            "invalid_argument",
            message,
            &[("field", field.to_owned())],
        )
    }

    pub(crate) fn repo_not_found(name: &str) -> ConnectError {
        refuse(
            ErrorCode::NotFound,
            "repo_not_found",
            format!("no repository {name:?}"),
            &[("repo", name.to_owned())],
        )
    }

    pub(crate) fn unavailable(what: &str, err: &dyn std::fmt::Display) -> ConnectError {
        tracing::warn!(error = %err, "{what} failed");
        refuse(
            ErrorCode::Unavailable,
            "unavailable",
            format!("{what} failed; retry"),
            &[],
        )
    }

    /// A catalog error as the RPC answers it.
    fn catalog_error(name: &str, err: CatalogError) -> ConnectError {
        let repo = || vec![("repo", name.to_owned())];
        match err {
            CatalogError::NotFound => repo_not_found(name),
            CatalogError::AlreadyExists => refuse(
                ErrorCode::AlreadyExists,
                "repo_already_exists",
                format!("a repository named {name:?} exists"),
                &repo(),
            ),
            CatalogError::Deleting => refuse(
                ErrorCode::FailedPrecondition,
                "repo_deleting",
                format!("repository {name:?} is being deleted"),
                &repo(),
            ),
            CatalogError::IdempotencyMismatch => key_reused(),
            CatalogError::ForkTooDeep { max } => refuse(
                ErrorCode::FailedPrecondition,
                "fork_too_deep",
                format!("a fork may be at most {max} forks deep"),
                &[("max", max.to_string())],
            ),
            CatalogError::Ref(err) => ref_error(name, err),
            CatalogError::Corrupt { .. } => {
                tracing::error!(error = %err, "a corrupt catalog document");
                refuse(
                    ErrorCode::Internal,
                    "internal",
                    "a corrupt catalog document",
                    &[],
                )
            }
            CatalogError::Store(err) => unavailable("the repository catalog", &err),
        }
    }

    /// The tenant a repository's sequencer commits for: the namespace in this install's org.
    pub(crate) fn tenant(ns_name: &str) -> String {
        format!("{LOCAL_ORG}/{ns_name}")
    }

    pub(crate) fn key_reused() -> ConnectError {
        refuse(
            ErrorCode::FailedPrecondition,
            "idempotency_key_reused",
            "the idempotency key was used for another request",
            &[],
        )
    }

    /// A ref log error as the RPC answers it.
    pub(crate) fn ref_error(name: &str, err: RefError) -> ConnectError {
        match err {
            RefError::Rejected(rejected) => {
                let Some((name, reason)) = rejected.into_iter().next() else {
                    return refuse(ErrorCode::Aborted, "ref_stale", "a ref changed", &[]);
                };
                let meta = [("ref", name.as_str().to_owned())];
                match reason {
                    RejectReason::Stale { .. } | RejectReason::Exists => refuse(
                        ErrorCode::Aborted,
                        "ref_stale",
                        format!("{name} changed; read it again"),
                        &meta,
                    ),
                    RejectReason::Protected => refuse(
                        ErrorCode::FailedPrecondition,
                        "ref_protected",
                        format!("a protection rule refuses changing {name} this way"),
                        &meta,
                    ),
                    RejectReason::OutOfScope { path } => refuse(
                        ErrorCode::PermissionDenied,
                        "out_of_scope",
                        format!("{path} is outside the credential's scope"),
                        &[("path", path.as_str().to_owned())],
                    ),
                    RejectReason::InvalidName => invalid("ref", format!("{name} is named twice")),
                }
            }
            RefError::IdempotencyMismatch => key_reused(),
            RefError::TooLarge => invalid("", "the change is larger than one WAL segment"),
            RefError::Unavailable => refuse(
                ErrorCode::Unavailable,
                "sequencer_unavailable",
                "the repository's sequencer is unavailable; retry with the same idempotency key",
                &[("repo", name.to_owned())],
            ),
            RefError::Invalid { reason } => invalid("", reason),
            RefError::NotFound => repo_not_found(name),
            RefError::State(StateError::UnsupportedFormat { format }) => refuse(
                ErrorCode::Unimplemented,
                "object_format_unsupported",
                format!("object format {format} is not supported yet (Q388)"),
                &[("object_format", format.to_owned())],
            ),
            RefError::State(err) => {
                tracing::error!(error = %err, repo = name, "a repository's state is corrupt");
                refuse(
                    ErrorCode::Internal,
                    "internal",
                    "a repository's state is corrupt",
                    &[],
                )
            }
            RefError::Wal(err) => unavailable("the repository's WAL", &err),
        }
    }

    pub(crate) fn timestamp(ms: i64) -> Timestamp {
        Timestamp {
            seconds: ms.div_euclid(1000),
            nanos: i32::try_from(ms.rem_euclid(1000) * 1_000_000).unwrap_or(0),
            ..Default::default()
        }
    }

    pub(crate) fn now_ms() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
    }

    /// `git:<repo_id>:<seq>` (§36 §4.6).
    pub(crate) fn token(repo: &RepoId, seq: Seq) -> String {
        format!("git:{repo}:{}", seq.0)
    }

    /// The seq a token names for `repo`: `None` for an empty token, `invalid_argument` for a
    /// malformed one or one of another repository (one of the same name deleted before).
    pub(crate) fn parse_token(
        field: &str,
        value: &str,
        repo: &RepoId,
    ) -> Result<Option<Seq>, ConnectError> {
        if value.is_empty() {
            return Ok(None);
        }
        let bad = || {
            invalid(
                field,
                "not a token of this repository (git:<repo_id>:<seq>)",
            )
        };
        let rest = value.strip_prefix("git:").ok_or_else(bad)?;
        let (id, seq) = rest.rsplit_once(':').ok_or_else(bad)?;
        if id != repo.as_str() {
            return Err(bad());
        }
        seq.parse::<u64>().map(|n| Some(Seq(n))).map_err(|_| bad())
    }

    pub(crate) fn repo_name(field: &str, value: &str) -> Result<RepoName, ConnectError> {
        RepoName::parse(value).map_err(|err| invalid(field, err.to_string()))
    }

    pub(crate) fn idem_key(value: &str) -> Result<Option<IdemKey>, ConnectError> {
        if value.is_empty() {
            return Ok(None);
        }
        IdemKey::parse(value)
            .map(Some)
            .map_err(|err| invalid("idempotency_key", err.to_string()))
    }

    /// A branch to point `HEAD` at: a ref under `refs/heads/`.
    fn branch(field: &str, value: &str) -> Result<RefName, ConnectError> {
        let name = RefName::parse(value).map_err(|err| invalid(field, err.to_string()))?;
        if !name.as_str().starts_with("refs/heads/") {
            return Err(invalid(
                field,
                "the default branch is a ref under refs/heads/",
            ));
        }
        Ok(name)
    }

    /// SHA-256 over an RPC's name and its fields, NUL-separated: the request an idempotency
    /// key is bound to.
    fn digest(parts: &[&str]) -> [u8; 32] {
        let mut hash = Sha256::new();
        for part in parts {
            hash.update(part.as_bytes());
            hash.update([0]);
        }
        hash.finalize().into()
    }

    /// `op-` and 26 hex characters (D146), derived from the repository and the kind, so a
    /// repeated `DeleteRepo` answers the same operation.
    fn operation_id(repo: &RepoId, kind: &str) -> String {
        let hash = Sha256::digest(format!("{kind}\0{repo}").as_bytes());
        let hex: String = hash.iter().take(13).map(|b| format!("{b:02x}")).collect();
        format!("op-{hex}")
    }

    fn protection_to_pb(rule: &gitpb::Protection) -> pb::Protection {
        pb::Protection {
            pattern: rule.pattern.clone(),
            deny_delete: rule.deny_delete,
            deny_force: rule.deny_force,
            ..Default::default()
        }
    }

    fn protection_from_pb(rule: &pb::Protection) -> Result<gitpb::Protection, ConnectError> {
        if rule.pattern.is_empty() {
            return Err(invalid("protections", "a protection rule names a pattern"));
        }
        Ok(gitpb::Protection {
            pattern: rule.pattern.clone(),
            deny_delete: rule.deny_delete,
            deny_force: rule.deny_force,
        })
    }

    /// The `Repo` message of `record`, with its configuration and seq from `snapshot`.
    fn repo_pb(ns_name: &str, record: &RepoRecord, snapshot: Option<&RefSnapshot>) -> pb::Repo {
        let mut repo = pb::Repo {
            namespace: ns_name.to_owned(),
            name: record.name.as_str().to_owned(),
            repo_id: record.repo_id.as_str().to_owned(),
            object_format: EnumValue::Known(pb::ObjectFormat::OBJECT_FORMAT_SHA1),
            created_at: MessageField::some(timestamp(record.created_unix_ms)),
            state: EnumValue::Known(match record.state {
                RepoLifecycle::Active => pb::RepoState::REPO_STATE_ACTIVE,
                RepoLifecycle::Deleting => pb::RepoState::REPO_STATE_DELETING,
            }),
            ..Default::default()
        };
        if let Some(parent) = &record.parent {
            repo.fork_parent = MessageField::some(pb::ForkParent {
                name: parent.name.as_str().to_owned(),
                repo_id: parent.repo_id.as_str().to_owned(),
                seq: parent.seq.0,
                ..Default::default()
            });
        }
        if let Some(snapshot) = snapshot {
            repo.seq = snapshot.seq.0;
            repo.default_branch = snapshot
                .symrefs
                .get("HEAD")
                .map(|target| target.as_str().to_owned())
                .unwrap_or_default();
            repo.protections = snapshot.protections.iter().map(protection_to_pb).collect();
        }
        repo
    }

    /// Every ref of `snapshot` (symbolic refs other than `HEAD` included), by name.
    fn ref_entries(snapshot: &RefSnapshot) -> BTreeMap<String, pb::Ref> {
        let mut out: BTreeMap<String, pb::Ref> = snapshot
            .refs
            .iter()
            .map(|(name, oid)| {
                (
                    name.as_str().to_owned(),
                    pb::Ref {
                        name: name.as_str().to_owned(),
                        oid: oid.to_hex().to_string(),
                        ..Default::default()
                    },
                )
            })
            .collect();
        for (name, target) in &snapshot.symrefs {
            if name != "HEAD" {
                out.insert(name.clone(), symref(snapshot, name, target));
            }
        }
        out
    }

    fn symref(snapshot: &RefSnapshot, name: &str, target: &RefName) -> pb::Ref {
        pb::Ref {
            name: name.to_owned(),
            oid: snapshot
                .refs
                .get(target)
                .map(|oid| oid.to_hex().to_string())
                .unwrap_or_default(),
            symref_target: target.as_str().to_owned(),
            ..Default::default()
        }
    }

    /// `HEAD` (when set) and then every ref, as a watch snapshot lists them.
    fn all_refs(snapshot: &RefSnapshot) -> Vec<pb::Ref> {
        let head = snapshot
            .symrefs
            .get("HEAD")
            .map(|target| symref(snapshot, "HEAD", target));
        head.into_iter()
            .chain(ref_entries(snapshot).into_values())
            .collect()
    }

    /// A committed transaction as `WatchRepo` sends it.
    fn transaction_pb(txn: &CommittedTxn) -> pb::Transaction {
        let mut out = pb::Transaction {
            seq: txn.seq.0,
            index: txn.index,
            time: MessageField::some(timestamp(txn.event.time_unix_ms)),
            ..Default::default()
        };
        out.change = Some(match &txn.event.body {
            TxnBody::RefTxn(body) => {
                out.principal = body.principal.clone();
                transaction::Change::from(pb::RefUpdates {
                    updates: body
                        .updates
                        .iter()
                        .map(|update| pb::RefUpdate {
                            name: update.name.clone(),
                            new_oid: update.new_oid.iter().map(|b| format!("{b:02x}")).collect(),
                            ..Default::default()
                        })
                        .collect(),
                    ..Default::default()
                })
            }
            TxnBody::Config(change) => transaction::Change::from(pb::ConfigChange {
                default_branch: change
                    .symrefs
                    .iter()
                    .find(|symref| symref.name == "HEAD")
                    .map(|symref| symref.target.clone())
                    .unwrap_or_default(),
                protections: change.protections.iter().map(protection_to_pb).collect(),
                ..Default::default()
            }),
            TxnBody::PackSet(change) => transaction::Change::from(pb::PackSetChange {
                packs_added: u32::try_from(change.added.len()).unwrap_or(u32::MAX),
                packs_removed: u32::try_from(change.removed.len()).unwrap_or(u32::MAX),
                ..Default::default()
            }),
        });
        out
    }

    /// A page size from a request: `default` for 0, `invalid_argument` past `max` or below 0.
    pub(crate) fn page_size(
        value: i32,
        (default, max): (usize, usize),
    ) -> Result<usize, ConnectError> {
        match usize::try_from(value) {
            Ok(0) => Ok(default),
            Ok(n) if n <= max => Ok(n),
            _ => Err(invalid("page_size", format!("1 to {max}"))),
        }
    }

    impl ReposServed {
        /// Refuses a caller below `need` in namespace `namespace` (GT1 Task 16; design 76 §9,
        /// plan IA1 ruling IA2.R4): reads and writes need the org role `member` and
        /// `can_read`/`can_write` on `environment:<namespace>`, administration `admin` and
        /// `can_admin`; the operator always passes.
        pub(crate) async fn require(
            &self,
            ctx: &RequestContext,
            need: GitAccess,
            namespace: &str,
        ) -> Result<(), ConnectError> {
            let principal = ctx
                .extensions()
                .get::<crate::auth::Principal>()
                .ok_or_else(|| {
                    refuse(
                        ErrorCode::Unauthenticated,
                        crate::auth::reason::TOKEN_MISSING,
                        "this RPC needs a credential",
                        &[],
                    )
                })?;
            crate::api::git_auth::authorize_caller(
                &self.principals,
                principal,
                need,
                Some(namespace),
            )
            .await
            .map_err(crate::api::connect_admin::denied)
        }

        /// The namespace named `name`; with `create`, made when it does not exist.
        pub(crate) async fn namespace(
            &self,
            name: &str,
            create: bool,
        ) -> Result<NamespaceId, ConnectError> {
            if name.is_empty() {
                return Err(invalid("namespace", "a namespace is required"));
            }
            if loams_common::meta::is_reserved_namespace(name) {
                return Err(crate::api::connect_errors::refused(
                    crate::api::errors::ApiError::namespace_reserved(name),
                ));
            }
            let found = self
                .meta
                .namespace_by_name(Consistency::Linearizable, name)
                .await
                .map_err(|err| unavailable("reading the namespace", &err))?;
            match found {
                Some(ns) => Ok(ns.id),
                None if create => match self.meta.create_namespace(name).await {
                    Ok(id) | Err(MetaError::Rejected(ApplyError::NamespaceExists(id))) => Ok(id),
                    Err(err) => Err(unavailable("creating the namespace", &err)),
                },
                // A namespace that does not exist holds no repository.
                None => Err(repo_not_found(name)),
            }
        }

        /// The active repository `name` of `ns`.
        pub(crate) async fn active(
            &self,
            ns: NamespaceId,
            name: &RepoName,
        ) -> Result<RepoRecord, ConnectError> {
            let record = self
                .git
                .catalog
                .get(ns, name)
                .await
                .map_err(|err| catalog_error(name.as_str(), err))?
                .ok_or_else(|| repo_not_found(name.as_str()))?;
            if record.state == RepoLifecycle::Deleting {
                return Err(catalog_error(name.as_str(), CatalogError::Deleting));
            }
            Ok(record)
        }

        /// The ref log of `record`.
        pub(crate) async fn log(
            &self,
            ns_name: &str,
            record: &RepoRecord,
        ) -> Result<BucketRefLog, ConnectError> {
            self.git
                .sequencers
                .get(
                    record.ns,
                    &format!("{LOCAL_ORG}/{ns_name}"),
                    &record.repo_id,
                )
                .await
                .map_err(|err| ref_error(record.name.as_str(), err))
        }

        /// `record`'s state at least as new as `token` (or the latest).
        pub(crate) async fn read(
            &self,
            ns_name: &str,
            record: &RepoRecord,
            token: &str,
        ) -> Result<(BucketRefLog, Arc<RefSnapshot>), ConnectError> {
            let at = match parse_token("consistency_token", token, &record.repo_id)? {
                Some(seq) => ReadAt::AtLeast(seq),
                None => ReadAt::Latest,
            };
            let log = self.log(ns_name, record).await?;
            let snapshot = log
                .snapshot(at)
                .await
                .map_err(|err| ref_error(record.name.as_str(), err))?;
            Ok((log, snapshot))
        }
    }

    impl RepoService for ReposServed {
        async fn create_repo(
            &self,
            ctx: RequestContext,
            request: ServiceRequest<'_, pb::CreateRepoRequest>,
        ) -> ServiceResult<pb::CreateRepoResponse> {
            let req = request.to_owned_message();
            self.require(&ctx, GitAccess::Admin, &req.namespace).await?;
            let name = repo_name("name", &req.name)?;
            let object_format = match req.object_format {
                EnumValue::Known(pb::ObjectFormat::OBJECT_FORMAT_UNSPECIFIED)
                | EnumValue::Known(pb::ObjectFormat::OBJECT_FORMAT_SHA1) => {
                    gitpb::ObjectFormat::Sha1
                }
                EnumValue::Known(pb::ObjectFormat::OBJECT_FORMAT_SHA256) => {
                    gitpb::ObjectFormat::Sha256
                }
                EnumValue::Unknown(_) => {
                    return Err(invalid("object_format", "an unknown object format"));
                }
            };
            let default_branch = if req.default_branch.is_empty() {
                branch("default_branch", "refs/heads/main")?
            } else {
                branch("default_branch", &req.default_branch)?
            };
            let key = idem_key(&req.idempotency_key)?;
            let ns = self.namespace(&req.namespace, true).await?;
            let request_digest = digest(&[
                "CreateRepo",
                name.as_str(),
                if object_format == gitpb::ObjectFormat::Sha1 {
                    "sha1"
                } else {
                    "sha256"
                },
                default_branch.as_str(),
            ]);
            let created = self
                .git
                .catalog
                .create(
                    ns,
                    CreateRepo {
                        name: name.clone(),
                        object_format,
                        default_branch,
                        idempotency: Idempotency {
                            key,
                            request_digest,
                        },
                    },
                )
                .await
                .map_err(|err| catalog_error(name.as_str(), err))?
                .into_record();
            let (_, snapshot) = self.read(&req.namespace, &created, "").await?;
            Response::ok(pb::CreateRepoResponse {
                repo: MessageField::some(repo_pb(&req.namespace, &created, Some(&snapshot))),
                consistency_token: token(&created.repo_id, snapshot.seq),
                ..Default::default()
            })
        }

        async fn get_repo(
            &self,
            ctx: RequestContext,
            request: ServiceRequest<'_, pb::GetRepoRequest>,
        ) -> ServiceResult<pb::GetRepoResponse> {
            let req = request.to_owned_message();
            self.require(&ctx, GitAccess::Read, &req.namespace).await?;
            let name = repo_name("name", &req.name)?;
            let ns = self.namespace(&req.namespace, false).await?;
            let record = self
                .git
                .catalog
                .get(ns, &name)
                .await
                .map_err(|err| catalog_error(name.as_str(), err))?
                .ok_or_else(|| repo_not_found(name.as_str()))?;
            if record.state == RepoLifecycle::Deleting {
                // Deleted: the record only, no sequencer is opened for it.
                return Response::ok(pb::GetRepoResponse {
                    repo: MessageField::some(repo_pb(&req.namespace, &record, None)),
                    ..Default::default()
                });
            }
            let (_, snapshot) = self
                .read(&req.namespace, &record, &req.consistency_token)
                .await?;
            Response::ok(pb::GetRepoResponse {
                repo: MessageField::some(repo_pb(&req.namespace, &record, Some(&snapshot))),
                consistency_token: token(&record.repo_id, snapshot.seq),
                ..Default::default()
            })
        }

        async fn list_repos(
            &self,
            ctx: RequestContext,
            request: ServiceRequest<'_, pb::ListReposRequest>,
        ) -> ServiceResult<pb::ListReposResponse> {
            let req = request.to_owned_message();
            self.require(&ctx, GitAccess::Read, &req.namespace).await?;
            let limit = page_size(req.page_size, REPOS_PAGE)?;
            let after = match req.page_token.as_str() {
                "" => None,
                token => Some(repo_name("page_token", token)?),
            };
            let ns = match self.namespace(&req.namespace, false).await {
                Ok(ns) => ns,
                // No namespace, no repositories.
                Err(err) if err.code == ErrorCode::NotFound => {
                    return Response::ok(pb::ListReposResponse::default());
                }
                Err(err) => return Err(err),
            };
            let (records, more) = self
                .git
                .catalog
                .list(ns, after.as_ref(), limit)
                .await
                .map_err(|err| catalog_error("", err))?;
            let next_page_token = match (more, records.last()) {
                (true, Some(last)) => last.name.as_str().to_owned(),
                _ => String::new(),
            };
            Response::ok(pb::ListReposResponse {
                repos: records
                    .iter()
                    .map(|record| repo_pb(&req.namespace, record, None))
                    .collect(),
                next_page_token,
                ..Default::default()
            })
        }

        async fn fork_repo(
            &self,
            ctx: RequestContext,
            request: ServiceRequest<'_, pb::ForkRepoRequest>,
        ) -> ServiceResult<pb::ForkRepoResponse> {
            let req = request.to_owned_message();
            self.require(&ctx, GitAccess::Admin, &req.namespace).await?;
            let source = repo_name("source", &req.source)?;
            let name = repo_name("name", &req.name)?;
            let key = idem_key(&req.idempotency_key)?;
            let ns = self.namespace(&req.namespace, false).await?;
            let parent = self.active(ns, &source).await?;
            let at = match parse_token(
                "at_consistency_token",
                &req.at_consistency_token,
                &parent.repo_id,
            )? {
                Some(seq) => ReadAt::Exactly(seq),
                None => ReadAt::Latest,
            };
            let log = self.log(&req.namespace, &parent).await?;
            let request_digest = digest(&[
                "ForkRepo",
                source.as_str(),
                name.as_str(),
                &req.at_consistency_token,
            ]);
            let fork = self
                .git
                .catalog
                .fork(
                    ns,
                    loams_git::catalog::ForkRepo {
                        source,
                        name: name.clone(),
                        at,
                        idempotency: Idempotency {
                            key,
                            request_digest,
                        },
                    },
                    &log,
                )
                .await
                .map_err(|err| catalog_error(name.as_str(), err))?
                .into_record();
            let (_, snapshot) = self.read(&req.namespace, &fork, "").await?;
            Response::ok(pb::ForkRepoResponse {
                repo: MessageField::some(repo_pb(&req.namespace, &fork, Some(&snapshot))),
                consistency_token: token(&fork.repo_id, snapshot.seq),
                ..Default::default()
            })
        }

        async fn delete_repo(
            &self,
            ctx: RequestContext,
            request: ServiceRequest<'_, pb::DeleteRepoRequest>,
        ) -> ServiceResult<pb::DeleteRepoResponse> {
            let req = request.to_owned_message();
            self.require(&ctx, GitAccess::Admin, &req.namespace).await?;
            let name = repo_name("name", &req.name)?;
            idem_key(&req.idempotency_key)?;
            let ns = self.namespace(&req.namespace, false).await?;
            let record = self
                .git
                .catalog
                .mark_deleting(ns, &name, now_ms())
                .await
                .map_err(|err| catalog_error(name.as_str(), err))?;
            self.git.sequencers.close(ns, &record.repo_id).await;
            let deleted = timestamp(record.deleted_unix_ms.unwrap_or_default());
            let mut operation = ops::Operation {
                id: operation_id(&record.repo_id, "repo.delete"),
                kind: "repo.delete".to_owned(),
                namespace: req.namespace.clone(),
                // Hidden now; purged after the grace period, once no fork reads it.
                state: EnumValue::Known(ops::OperationState::OPERATION_STATE_RUNNING),
                created_at: MessageField::some(deleted.clone()),
                updated_at: MessageField::some(deleted),
                ..Default::default()
            };
            operation
                .target
                .insert("repo".to_owned(), name.as_str().to_owned());
            operation
                .target
                .insert("repo_id".to_owned(), record.repo_id.as_str().to_owned());
            Response::ok(pb::DeleteRepoResponse {
                operation: MessageField::some(operation),
                ..Default::default()
            })
        }

        async fn update_repo_config(
            &self,
            ctx: RequestContext,
            request: ServiceRequest<'_, pb::UpdateRepoConfigRequest>,
        ) -> ServiceResult<pb::UpdateRepoConfigResponse> {
            let req = request.to_owned_message();
            self.require(&ctx, GitAccess::Admin, &req.namespace).await?;
            let name = repo_name("name", &req.name)?;
            let paths: Vec<String> = req
                .update_mask
                .as_option()
                .map(|mask| mask.paths.clone())
                .unwrap_or_default();
            if paths.is_empty() {
                return Err(invalid("update_mask", "name the fields to change"));
            }
            let mut head = None;
            let mut protections = None;
            for path in &paths {
                match path.as_str() {
                    // FieldMask's JSON form is lowerCamelCase.
                    "default_branch" | "defaultBranch" => {
                        head = Some(branch("default_branch", &req.default_branch)?);
                    }
                    "protections" => {
                        protections = Some(
                            req.protections
                                .iter()
                                .map(protection_from_pb)
                                .collect::<Result<Vec<_>, _>>()?,
                        );
                    }
                    other => {
                        return Err(invalid(
                            "update_mask",
                            format!("{other:?} is not a field UpdateRepoConfig changes"),
                        ));
                    }
                }
            }
            let key = match idem_key(&req.idempotency_key)? {
                Some(key) => key,
                // Unkeyed: a fresh key, so the change is never replayed.
                None => IdemKey::parse(&ulid::Ulid::generate().to_string().to_lowercase())
                    .map_err(|err| invalid("idempotency_key", err.to_string()))?,
            };
            // The key binds the request, not the change merged with the current state, so a
            // retry after another writer's change still replays (PR #467 review).
            let mut mask: Vec<&str> = paths.iter().map(String::as_str).collect();
            mask.sort_unstable();
            mask.dedup();
            let rules: Vec<String> = req
                .protections
                .iter()
                .map(|r| format!("{}\u{1}{}\u{1}{}", r.pattern, r.deny_delete, r.deny_force))
                .collect();
            let mut parts = vec![
                "UpdateRepoConfig",
                name.as_str(),
                req.default_branch.as_str(),
            ];
            parts.extend(mask.iter().copied());
            parts.push("\u{2}");
            parts.extend(rules.iter().map(String::as_str));
            let request_digest = digest(&parts);
            let ns = self.namespace(&req.namespace, false).await?;
            let record = self.active(ns, &name).await?;
            let (log, current) = self.read(&req.namespace, &record, "").await?;
            let mut symrefs = current.symrefs.clone();
            if let Some(head) = head {
                symrefs.insert("HEAD".to_owned(), head);
            }
            let change = gitpb::ConfigChange {
                symrefs: symrefs
                    .iter()
                    .map(|(name, target)| gitpb::SymRef {
                        name: name.clone(),
                        target: target.as_str().to_owned(),
                    })
                    .collect(),
                protections: protections.unwrap_or_else(|| current.protections.clone()),
                request_digest: request_digest.to_vec(),
            };
            // Committed on the repository's sequencer owner (GT1 Task 14): here, forwarded,
            // or here without the lease when the owner cannot be reached.
            let receipt = self
                .git
                .sequencers
                .commit_config(
                    record.ns,
                    &format!("{LOCAL_ORG}/{}", req.namespace),
                    &record.repo_id,
                    key,
                    change,
                )
                .await
                .map_err(|err| ref_error(name.as_str(), err))?;
            let snapshot = log
                .snapshot(ReadAt::AtLeast(receipt.seq))
                .await
                .map_err(|err| ref_error(name.as_str(), err))?;
            Response::ok(pb::UpdateRepoConfigResponse {
                repo: MessageField::some(repo_pb(&req.namespace, &record, Some(&snapshot))),
                consistency_token: token(&record.repo_id, receipt.seq),
                ..Default::default()
            })
        }

        async fn list_refs(
            &self,
            ctx: RequestContext,
            request: ServiceRequest<'_, pb::ListRefsRequest>,
        ) -> ServiceResult<pb::ListRefsResponse> {
            let req = request.to_owned_message();
            self.require(&ctx, GitAccess::Read, &req.namespace).await?;
            let name = repo_name("name", &req.name)?;
            let limit = page_size(req.page_size, REFS_PAGE)?;
            let ns = self.namespace(&req.namespace, false).await?;
            let record = self.active(ns, &name).await?;
            // A later page reads the seq the first one read: "<seq>:<last ref name>".
            let (snapshot, after) = if req.page_token.is_empty() {
                let (_, snapshot) = self
                    .read(&req.namespace, &record, &req.consistency_token)
                    .await?;
                (snapshot, None)
            } else {
                let bad = || invalid("page_token", "not a ListRefs page token");
                let (seq, last) = req.page_token.split_once(':').ok_or_else(bad)?;
                let seq = Seq(seq.parse().map_err(|_| bad())?);
                let log = self.log(&req.namespace, &record).await?;
                let snapshot = log
                    .snapshot(ReadAt::Exactly(seq))
                    .await
                    .map_err(|err| ref_error(name.as_str(), err))?;
                (snapshot, Some(last.to_owned()))
            };
            let mut refs = Vec::new();
            if after.is_none()
                && "HEAD".starts_with(&req.prefix)
                && let Some(target) = snapshot.symrefs.get("HEAD")
            {
                refs.push(symref(&snapshot, "HEAD", target));
            }
            let entries = ref_entries(&snapshot);
            let mut more = false;
            for (ref_name, entry) in entries.range::<String, _>((
                after
                    .clone()
                    .map_or(std::ops::Bound::Unbounded, std::ops::Bound::Excluded),
                std::ops::Bound::Unbounded,
            )) {
                if !ref_name.starts_with(&req.prefix) {
                    continue;
                }
                if refs.len() == limit {
                    more = true;
                    break;
                }
                refs.push(entry.clone());
            }
            let next_page_token = match (more, refs.last()) {
                (true, Some(last)) => format!("{}:{}", snapshot.seq.0, last.name),
                _ => String::new(),
            };
            Response::ok(pb::ListRefsResponse {
                refs,
                next_page_token,
                consistency_token: token(&record.repo_id, snapshot.seq),
                ..Default::default()
            })
        }

        async fn compact_repo(
            &self,
            ctx: RequestContext,
            request: ServiceRequest<'_, pb::CompactRepoRequest>,
        ) -> ServiceResult<pb::CompactRepoResponse> {
            let req = request.to_owned_message();
            self.require(&ctx, GitAccess::Admin, &req.namespace).await?;
            let name = repo_name("name", &req.name)?;
            let ns = self.namespace(&req.namespace, false).await?;
            self.active(ns, &name).await?;
            Err(not_implemented(&format!(
                "{PACKAGE}.RepoService/CompactRepo (GT1 Task 53)"
            )))
        }

        async fn watch_repo(
            &self,
            ctx: RequestContext,
            request: ServiceRequest<'_, pb::WatchRepoRequest>,
        ) -> ServiceResult<ServiceStream<pb::WatchRepoResponse>> {
            let req = request.to_owned_message();
            self.require(&ctx, GitAccess::Read, &req.namespace).await?;
            let name = repo_name("name", &req.name)?;
            let ns = self.namespace(&req.namespace, false).await?;
            let record = self.active(ns, &name).await?;
            let resume = parse_token("consistency_token", &req.consistency_token, &record.repo_id)?;
            let log = self.log(&req.namespace, &record).await?;
            let (tx, mut rx) = mpsc::channel(WATCH_BUFFER);
            let watch = Watch {
                log,
                record,
                ns_name: req.namespace.clone(),
                heartbeat: self.git.watch_heartbeat,
                tx,
            };
            tokio::spawn(watch.run(resume));
            Response::ok(
                Box::pin(futures::stream::poll_fn(move |cx| rx.poll_recv(cx)))
                    as ServiceStream<pb::WatchRepoResponse>,
            )
        }
    }

    /// One `WatchRepo` stream's producer.
    struct Watch {
        log: BucketRefLog,
        record: RepoRecord,
        ns_name: String,
        heartbeat: Duration,
        tx: mpsc::Sender<Result<pb::WatchRepoResponse, ConnectError>>,
    }

    impl Watch {
        /// Sends one message; `false` once the client has gone.
        async fn send(&self, message: Result<pb::WatchRepoResponse, ConnectError>) -> bool {
            self.tx.send(message).await.is_ok()
        }

        /// The snapshot message at the latest seq; the seq it is at.
        async fn snapshot(&self, reset: bool) -> Option<Seq> {
            let name = self.record.name.as_str();
            match self.log.snapshot(ReadAt::Latest).await {
                Ok(snapshot) => {
                    let message = pb::WatchRepoResponse {
                        event: Some(watch_repo_response::Event::from(pb::RepoSnapshot {
                            repo: MessageField::some(repo_pb(
                                &self.ns_name,
                                &self.record,
                                Some(&snapshot),
                            )),
                            refs: all_refs(&snapshot),
                            ..Default::default()
                        })),
                        consistency_token: token(&self.record.repo_id, snapshot.seq),
                        snapshot_reset: reset,
                        ..Default::default()
                    };
                    self.send(Ok(message)).await.then_some(snapshot.seq)
                }
                Err(err) => {
                    self.send(Err(ref_error(name, err))).await;
                    None
                }
            }
        }

        async fn run(self, resume: Option<Seq>) {
            let mut at = match resume {
                Some(seq) => seq,
                None => match self.snapshot(false).await {
                    Some(seq) => seq,
                    None => return,
                },
            };
            let mut reset_once = resume.is_some();
            let mut changes = self.log.watch(at.next());
            loop {
                let next = tokio::select! {
                    next = changes.next() => next,
                    () = tokio::time::sleep(self.heartbeat) => {
                        let beat = pb::WatchRepoResponse {
                            event: Some(watch_repo_response::Event::from(pb::Heartbeat {
                                time: MessageField::some(timestamp(now_ms())),
                                ..Default::default()
                            })),
                            consistency_token: token(&self.record.repo_id, at),
                            ..Default::default()
                        };
                        if !self.send(Ok(beat)).await {
                            return;
                        }
                        continue;
                    }
                    () = self.tx.closed() => return,
                };
                match next {
                    Some(Ok(txn)) => {
                        at = txn.seq;
                        let message = pb::WatchRepoResponse {
                            event: Some(watch_repo_response::Event::from(transaction_pb(&txn))),
                            consistency_token: token(&self.record.repo_id, txn.seq),
                            ..Default::default()
                        };
                        if !self.send(Ok(message)).await {
                            return;
                        }
                    }
                    // The resume token's transactions are gone: a fresh snapshot replaces them.
                    Some(Err(RefError::Invalid { .. })) if reset_once => {
                        reset_once = false;
                        let Some(seq) = self.snapshot(true).await else {
                            return;
                        };
                        at = seq;
                        changes = self.log.watch(at.next());
                    }
                    Some(Err(err)) => {
                        self.send(Err(ref_error(self.record.name.as_str(), err)))
                            .await;
                        return;
                    }
                    None => return,
                }
            }
        }
    }
}
