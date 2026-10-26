// The generated service traits return `impl Encodable<_>`; these impls name the concrete message
// type, which is the intended refinement (as `connect.rs` does).
#![allow(refining_impl_trait)]

//! `loams.repos.v1.ObjectService` on the main port and the immutable object route on the git
//! listener (Loams Git, GT1 Tasks 18, 20, 21 and 24; design §36 §19.5, §19.8, D829, D830, D833).
//!
//! With the `git` feature `GetObjects`, `GetTrees`, `HasObjects`, `Log`, `DiffStat`, `Blame` and
//! `Grep` are
//! served over the repository's packs (`loams_git::odb::RangeOdb`, through the server's H1 cache) and, when a
//! request names a workspace, the workspace's packs after them. Callers need the org role
//! `member`, as for every `RepoService` read; whoever may read a repository may read its
//! workspaces (GT-Q13's default, ruling T18.2), unless the server limits workspace reads to
//! owners (`--git-workspace-reads-owner`, GT1 Task 25). `PutWorkspacePack` (Task 24) stores a workspace
//! pack after `loams_git::ws::put_workspace_pack` has verified it; writing a workspace needs its
//! owner or an org admin (Q750's default). Without the feature every RPC answers
//! `feature_not_in_variant`.

use std::sync::Arc;

use connectrpc::{
    ConnectError, InboundStream, RequestContext, Router, ServiceRequest, ServiceResult,
    ServiceStream,
};
use loams_proto::loams::repos::v1 as pb;
use loams_proto::loams::repos::v1::{ObjectService, ObjectServiceExt};

use super::AppState;
use super::repos::{PACKAGE, SERVED};

/// Registers `ObjectService`: served with the `git` feature and a git runtime, else the stub.
pub(super) fn register(router: Router, state: &AppState) -> Router {
    #[cfg(feature = "git")]
    if let Some(git) = &state.git {
        return Arc::new(served::ObjectsServed {
            repos: super::repos::served::ReposServed {
                meta: Arc::clone(&state.meta),
                git: Arc::clone(git),
                principals: state.principals.clone(),
            },
        })
        .register(router);
    }
    let _ = state;
    Arc::new(ObjectsAbsent).register(router)
}

/// `ObjectService` when this binary does not serve it.
#[derive(Debug)]
struct ObjectsAbsent;

fn absent(method: &str) -> ConnectError {
    let rpc = format!("{PACKAGE}.ObjectService/{method}");
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

impl ObjectService for ObjectsAbsent {
    async fn get_objects(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::GetObjectsRequest>,
    ) -> ServiceResult<ServiceStream<pb::GetObjectsResponse>> {
        Err(absent("GetObjects"))
    }

    async fn get_trees(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::GetTreesRequest>,
    ) -> ServiceResult<pb::GetTreesResponse> {
        Err(absent("GetTrees"))
    }

    async fn has_objects(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::HasObjectsRequest>,
    ) -> ServiceResult<pb::HasObjectsResponse> {
        Err(absent("HasObjects"))
    }

    async fn log(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::LogRequest>,
    ) -> ServiceResult<ServiceStream<pb::LogResponse>> {
        Err(absent("Log"))
    }

    async fn diff_stat(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::DiffStatRequest>,
    ) -> ServiceResult<pb::DiffStatResponse> {
        Err(absent("DiffStat"))
    }

    async fn blame(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::BlameRequest>,
    ) -> ServiceResult<pb::BlameResponse> {
        Err(absent("Blame"))
    }

    async fn grep(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::GrepRequest>,
    ) -> ServiceResult<ServiceStream<pb::GrepResponse>> {
        Err(absent("Grep"))
    }

    async fn put_workspace_pack(
        &self,
        _ctx: RequestContext,
        _requests: InboundStream<pb::PutWorkspacePackRequest>,
    ) -> ServiceResult<pb::PutWorkspacePackResponse> {
        Err(absent("PutWorkspacePack"))
    }
}

#[cfg(feature = "git")]
pub(crate) use served::{
    ObjectRoute, internal, odb_error, open_odb, pack_pb, parse_oid, serve_object,
    workspace_not_found, ws_pack_error,
};

#[cfg(feature = "git")]
mod served {
    use std::sync::Arc;

    use axum::http::{HeaderValue, StatusCode, header};
    use axum::response::{IntoResponse, Response as HttpResponse};
    use buffa::{EnumValue, MessageField};
    use bytes::BytesMut;
    use connectrpc::{
        ConnectError, ErrorCode, InboundStream, RequestContext, Response, ServiceRequest,
        ServiceResult, ServiceStream,
    };
    use futures::StreamExt;
    use loams_git::blob::Put;
    use loams_git::catalog::RepoRecord;
    use loams_git::derived::DerivedError;
    use loams_git::format::pb as gitpb;
    use loams_git::history::{
        BlameQuery, ChangeKind, ChangedPath, GrepFile, GrepFlags, GrepQuery, HistoryError,
        LogEntry, LogFields, LogQuery, Pathspec, Signature, blame, diff_stat, grep, log,
        resolve_tree,
    };
    use loams_git::limits::{RECEIVE_IDLE_TIMEOUT, UPLOAD_DEADLINE};
    use loams_git::objects::{
        Kind, ObjectRead, ObjectsError, WsView, bitmap, get_objects, get_trees,
        grant_reads_workspace, has_objects, loose_object, workspace_odb,
    };
    use loams_git::odb::{OdbError, RangeOdb};
    use loams_git::ws::{
        NoScan, WsPackBlobs, WsPackError, WsPackUploads, check_pack_len, put_workspace_pack,
    };
    use loams_git::{
        BlobError, DerivedStore, ForkError, GitAccess, GitGrant, NamespaceId, ObjectId, ReadAt,
        RefSnapshot, RepoPath, RepoPaths, StoreBlobStore, WsId, open_fork_range_odb,
    };
    use loams_proto::loams::repos::v1 as pb;
    use loams_proto::loams::repos::v1::ObjectService;
    use tokio::sync::mpsc;

    use crate::api::connect::refuse;
    use crate::api::repos::GitRuntime;
    use crate::api::repos::served::{ReposServed, invalid, repo_name, token, unavailable};
    use crate::api::workspaces::{may_read_workspace, may_write_workspace};

    /// Commits a `Log` stream (files a `Grep` stream) buffers before its walk waits.
    const LOG_BUFFER: usize = 64;
    /// The largest `data` of one `GetObjectsResponse`; larger objects come in chunks.
    const CHUNK: usize = 1024 * 1024;
    /// The `Cache-Control` of the object route: objects are immutable (§19.5.1).
    const IMMUTABLE: &str = "private, max-age=31536000, immutable";

    /// The `PutWorkspacePack` uploads this node verifies at once (`MAX_WS_PACK_UPLOADS`,
    /// ruling T24.7).
    static PACK_UPLOADS: std::sync::LazyLock<WsPackUploads> =
        std::sync::LazyLock::new(WsPackUploads::default);

    /// `ObjectService`, served.
    #[derive(Debug)]
    pub(super) struct ObjectsServed {
        pub(super) repos: ReposServed,
    }

    /// The objects `snapshot` of `record` reads, with workspace `ws` searched last.
    pub(crate) async fn open_odb(
        git: &GitRuntime,
        record: &RepoRecord,
        snapshot: &RefSnapshot,
        ws: Option<(&WsId, &WsView)>,
    ) -> Result<RangeOdb, String> {
        let failed = |err: &dyn std::fmt::Display| err.to_string();
        let odb = open_fork_range_odb(
            &git.store,
            record.ns,
            &record.repo_id,
            snapshot,
            Some(git.h1.clone()),
        )
        .await
        .map_err(|err: ForkError| failed(&err))?;
        let Some((ws, view)) = ws else {
            return Ok(odb);
        };
        let paths = RepoPaths::new(record.ns, &record.repo_id);
        // A fork's packs may lie under its sources' prefixes (ruling T23.3).
        let blobs = WsPackBlobs::new(&git.store, &paths, ws, &view.sources, Some(&git.h1));
        workspace_odb(&odb, Arc::new(blobs), view).map_err(|err| failed(&err))
    }

    /// The workspace a request names (`None` for an empty field).
    async fn workspace(
        git: &GitRuntime,
        record: &RepoRecord,
        value: &str,
    ) -> Result<Option<(WsId, WsView)>, ConnectError> {
        if value.is_empty() {
            return Ok(None);
        }
        let ws = WsId::parse(value).map_err(|err| invalid("workspace", err.to_string()))?;
        match git
            .workspaces
            .workspace(record.ns, &record.repo_id, &ws)
            .await
        {
            Ok(Some(view)) => Ok(Some((ws, view))),
            Ok(None) => Err(workspace_not_found(value)),
            Err(err) => Err(unavailable("the workspace lookup", &err)),
        }
    }

    pub(crate) fn workspace_not_found(ws: &str) -> ConnectError {
        refuse(
            ErrorCode::NotFound,
            "workspace_not_found",
            format!("no workspace {ws:?}"),
            &[("workspace", ws.to_owned())],
        )
    }

    fn object_missing(oid: &ObjectId) -> ConnectError {
        refuse(
            ErrorCode::FailedPrecondition,
            "object_missing",
            format!("object {oid} is not in the repository"),
            &[("oid", oid.to_string())],
        )
    }

    pub(crate) fn internal(what: &str, err: &dyn std::fmt::Display) -> ConnectError {
        tracing::error!(error = %err, "{what}");
        refuse(ErrorCode::Internal, "internal", what.to_owned(), &[])
    }

    pub(crate) fn odb_error(err: OdbError) -> ConnectError {
        match err {
            OdbError::NotFound(oid) => object_missing(&oid),
            OdbError::Blob(err) => unavailable("reading packs", &err),
            other => internal("a corrupt pack or object", &other),
        }
    }

    fn objects_error(err: ObjectsError) -> ConnectError {
        match err {
            ObjectsError::TooMany { max, got } => {
                invalid("oids", format!("{got} object ids; at most {max} per call"))
            }
            ObjectsError::TooDeep { max, got } => {
                invalid("depth", format!("depth {got}; at most {max}"))
            }
            ObjectsError::NotFound(oid) => object_missing(&oid),
            ObjectsError::Lookup(reason) => unavailable("the workspace lookup", &reason),
            ObjectsError::Odb(err) => odb_error(err),
            ObjectsError::Derived(DerivedError::WrongKind { oid, found, .. }) => invalid(
                "root",
                format!("{oid} is a {found}, not a tree, commit or tag"),
            ),
            ObjectsError::Derived(DerivedError::Odb(err)) => odb_error(err),
            ObjectsError::Derived(DerivedError::Store(err)) => unavailable("derived data", &err),
            ObjectsError::Derived(err) => internal("corrupt derived data", &err),
        }
    }

    fn history_error(err: HistoryError) -> ConnectError {
        match err {
            HistoryError::Pathspec { spec, reason } => {
                invalid("paths", format!("pathspec {spec:?}: {reason}"))
            }
            HistoryError::Rev { rev, reason } => {
                invalid("revs", format!("revision {rev:?}: {reason}"))
            }
            HistoryError::TooMany { what, max, got } => {
                invalid(what, format!("{got} {what}; at most {max} per call"))
            }
            HistoryError::Path { path, reason } => invalid("path", format!("{path:?}: {reason}")),
            HistoryError::Lines { reason } => invalid("start_line", reason),
            HistoryError::Pattern { pattern, reason } => {
                invalid("patterns", format!("pattern {pattern:?}: {reason}"))
            }
            HistoryError::Derived(DerivedError::BlameUnsupported(reason)) => refuse(
                ErrorCode::Unimplemented,
                "not_implemented",
                format!("blame is not served for this file ({reason}); run git blame"),
                &[],
            ),
            HistoryError::Odb(err) => odb_error(err),
            HistoryError::Derived(DerivedError::Odb(err)) => odb_error(err),
            HistoryError::Derived(DerivedError::Store(err)) => {
                unavailable("the commit graph", &err)
            }
            HistoryError::Derived(err) => internal("a corrupt commit graph", &err),
        }
    }

    pub(crate) fn ws_pack_error(err: WsPackError) -> ConnectError {
        match err {
            err @ (WsPackError::TooLarge { .. }
            | WsPackError::Invalid(_)
            | WsPackError::BadHash { .. }) => invalid("data", err.to_string()),
            WsPackError::Refused(refusal) => refuse(
                ErrorCode::FailedPrecondition,
                "secret_detected",
                format!(
                    "object {} matches the secret scanner's rule {}",
                    refusal.oid, refusal.rule
                ),
                &[("rule", refusal.rule.clone())],
            ),
            WsPackError::Blob(err @ BlobError::Mismatch { .. }) => {
                internal("another pack is stored under this pack's name", &err)
            }
            WsPackError::Blob(err) => unavailable("storing the workspace pack", &err),
        }
    }

    pub(crate) fn pack_pb(pack: &gitpb::PackRef) -> pb::PackRef {
        pb::PackRef {
            checksum: pack.checksum.clone(),
            object_count: pack.object_count,
            pack_len: pack.pack_len,
            idx_offset: pack.idx_offset,
            idx_len: pack.idx_len,
            ..Default::default()
        }
    }

    fn signature_pb(sig: Signature) -> pb::Signature {
        pb::Signature {
            name: sig.name,
            email: sig.email,
            time: sig.time,
            tz: sig.tz,
            ..Default::default()
        }
    }

    fn changed_pb(change: ChangedPath) -> pb::ChangedPath {
        let kind = match change.kind {
            ChangeKind::Added => pb::ChangeKind::CHANGE_KIND_ADDED,
            ChangeKind::Deleted => pb::ChangeKind::CHANGE_KIND_DELETED,
            ChangeKind::Modified => pb::ChangeKind::CHANGE_KIND_MODIFIED,
            ChangeKind::TypeChanged => pb::ChangeKind::CHANGE_KIND_TYPE_CHANGED,
        };
        pb::ChangedPath {
            path: change.path,
            kind: EnumValue::Known(kind),
            old_mode: change.old.map(|(mode, _)| mode),
            old_oid: change
                .old
                .map(|(_, oid)| oid.to_string())
                .unwrap_or_default(),
            new_mode: change.new.map(|(mode, _)| mode),
            new_oid: change
                .new
                .map(|(_, oid)| oid.to_string())
                .unwrap_or_default(),
            ..Default::default()
        }
    }

    fn log_commit(entry: LogEntry) -> pb::LogCommit {
        pb::LogCommit {
            oid: entry.oid.to_string(),
            tree: entry.tree.to_string(),
            parents: entry.parents.iter().map(ToString::to_string).collect(),
            author: match entry.author {
                Some(sig) => MessageField::some(signature_pb(sig)),
                None => MessageField::none(),
            },
            committer: match entry.committer {
                Some(sig) => MessageField::some(signature_pb(sig)),
                None => MessageField::none(),
            },
            message: entry.message.map(|m| m.to_vec()).unwrap_or_default(),
            changed: entry
                .changed
                .unwrap_or_default()
                .into_iter()
                .map(changed_pb)
                .collect(),
            ..Default::default()
        }
    }

    fn log_fields(values: &[EnumValue<pb::LogField>]) -> Result<LogFields, ConnectError> {
        if values.is_empty() {
            return Ok(LogFields::default());
        }
        let mut fields = LogFields {
            signatures: false,
            message: false,
            changed: false,
        };
        for value in values {
            match value {
                EnumValue::Known(pb::LogField::LOG_FIELD_SIGNATURES) => fields.signatures = true,
                EnumValue::Known(pb::LogField::LOG_FIELD_MESSAGE) => fields.message = true,
                EnumValue::Known(pb::LogField::LOG_FIELD_CHANGED) => fields.changed = true,
                _ => return Err(invalid("fields", "an unknown log field")),
            }
        }
        Ok(fields)
    }

    fn oids(field: &str, values: &[String]) -> Result<Vec<ObjectId>, ConnectError> {
        values.iter().map(|value| parse_oid(field, value)).collect()
    }

    pub(crate) fn parse_oid(field: &str, value: &str) -> Result<ObjectId, ConnectError> {
        let hex = value.len() == 40
            && value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if !hex {
            return Err(invalid(
                field,
                format!("{value:?} is not 40 lowercase hex digits"),
            ));
        }
        ObjectId::from_hex(value.as_bytes()).map_err(|err| invalid(field, err.to_string()))
    }

    fn kind_pb(kind: Kind) -> pb::ObjectKind {
        match kind {
            Kind::Commit => pb::ObjectKind::OBJECT_KIND_COMMIT,
            Kind::Tree => pb::ObjectKind::OBJECT_KIND_TREE,
            Kind::Blob => pb::ObjectKind::OBJECT_KIND_BLOB,
            Kind::Tag => pb::ObjectKind::OBJECT_KIND_TAG,
        }
    }

    /// One read's chunks.
    fn chunks(read: ObjectRead, token: &str) -> Vec<pb::GetObjectsResponse> {
        match read {
            ObjectRead::Missing(oid) => vec![pb::GetObjectsResponse {
                oid: oid.to_string(),
                missing: true,
                consistency_token: token.to_owned(),
                ..Default::default()
            }],
            ObjectRead::Found { oid, kind, data } => {
                let size = data.len() as u64;
                let mut out = Vec::new();
                let mut offset = 0usize;
                loop {
                    let end = data.len().min(offset + CHUNK);
                    out.push(pb::GetObjectsResponse {
                        oid: oid.to_string(),
                        kind: EnumValue::Known(kind_pb(kind)),
                        size,
                        offset: offset as u64,
                        data: data.slice(offset..end).to_vec(),
                        consistency_token: token.to_owned(),
                        ..Default::default()
                    });
                    offset = end;
                    if offset >= data.len() {
                        return out;
                    }
                }
            }
        }
    }

    impl ObjectsServed {
        /// The repository a request names, its snapshot and the object database to read.
        async fn open(
            &self,
            ctx: &RequestContext,
            ns_name: &str,
            name: &str,
            ws: &str,
            consistency: &str,
        ) -> Result<(RepoRecord, Arc<RefSnapshot>, Arc<RangeOdb>), ConnectError> {
            self.repos.require(ctx, GitAccess::Read, ns_name).await?;
            let name = repo_name("name", name)?;
            let ns = self.repos.namespace(ns_name, false).await?;
            let record = self.repos.active(ns, &name).await?;
            let (_, snapshot) = self.repos.read(ns_name, &record, consistency).await?;
            let ws_view = workspace(&self.repos.git, &record, ws).await?;
            if let Some((_, view)) = &ws_view {
                may_read_workspace(&self.repos, ctx, view.owner.as_str()).await?;
            }
            let odb = open_odb(
                &self.repos.git,
                &record,
                &snapshot,
                ws_view.as_ref().map(|(id, view)| (id, view)),
            )
            .await
            .map_err(|reason| unavailable("opening the repository's objects", &reason))?;
            Ok((record, snapshot, Arc::new(odb)))
        }
    }

    impl ObjectsServed {
        /// Refuses a caller who may not write workspace `view` (Q750's default): its owner, an
        /// org admin and the operator may.
        async fn may_write(&self, ctx: &RequestContext, view: &WsView) -> Result<(), ConnectError> {
            may_write_workspace(&self.repos, ctx, view.owner.as_str()).await
        }

        fn derived(&self, record: &RepoRecord) -> DerivedStore {
            DerivedStore::new(
                self.repos.git.store.clone(),
                RepoPaths::new(record.ns, &record.repo_id),
            )
            .with_range_cache(self.repos.git.h1.clone())
        }
    }

    impl ObjectService for ObjectsServed {
        async fn get_objects(
            &self,
            ctx: RequestContext,
            request: ServiceRequest<'_, pb::GetObjectsRequest>,
        ) -> ServiceResult<ServiceStream<pb::GetObjectsResponse>> {
            let req = request.to_owned_message();
            let wanted = oids("oids", &req.oids)?;
            let (record, snapshot, odb) = self
                .open(
                    &ctx,
                    &req.namespace,
                    &req.name,
                    &req.workspace,
                    &req.consistency_token,
                )
                .await?;
            let at = token(&record.repo_id, snapshot.seq);
            let reads = get_objects(odb, wanted).await.map_err(objects_error)?;
            let stream = reads.flat_map(move |read| {
                let messages: Vec<Result<pb::GetObjectsResponse, ConnectError>> = match read {
                    Ok(read) => chunks(read, &at).into_iter().map(Ok).collect(),
                    Err(err) => vec![Err(objects_error(err))],
                };
                futures::stream::iter(messages)
            });
            Response::ok(Box::pin(stream) as ServiceStream<pb::GetObjectsResponse>)
        }

        async fn get_trees(
            &self,
            ctx: RequestContext,
            request: ServiceRequest<'_, pb::GetTreesRequest>,
        ) -> ServiceResult<pb::GetTreesResponse> {
            let req = request.to_owned_message();
            let root = parse_oid("root", &req.root)?;
            let depth = match req.depth {
                None => loams_git::limits::TREE_PREFETCH_DEPTH,
                Some(depth) => u8::try_from(depth)
                    .ok()
                    .filter(|depth| *depth <= loams_git::objects::MAX_TREE_DEPTH)
                    .ok_or_else(|| invalid("depth", "0 to 8"))?,
            };
            let (record, snapshot, odb) = self
                .open(
                    &ctx,
                    &req.namespace,
                    &req.name,
                    &req.workspace,
                    &req.consistency_token,
                )
                .await?;
            let derived = self.derived(&record);
            let got = get_trees(&odb, &derived, &root, depth)
                .await
                .map_err(objects_error)?;
            Response::ok(pb::GetTreesResponse {
                trees: got
                    .trees
                    .iter()
                    .map(|tree| pb::Tree {
                        oid: tree.tree.to_string(),
                        entries: tree
                            .entries
                            .iter()
                            .map(|entry| pb::TreeEntry {
                                name: entry.name.clone(),
                                mode: entry.mode,
                                oid: entry.oid.to_string(),
                                size: entry.size,
                                ..Default::default()
                            })
                            .collect(),
                        ..Default::default()
                    })
                    .collect(),
                truncated: got.truncated,
                consistency_token: token(&record.repo_id, snapshot.seq),
                ..Default::default()
            })
        }

        async fn has_objects(
            &self,
            ctx: RequestContext,
            request: ServiceRequest<'_, pb::HasObjectsRequest>,
        ) -> ServiceResult<pb::HasObjectsResponse> {
            let req = request.to_owned_message();
            let wanted = oids("oids", &req.oids)?;
            let (record, snapshot, odb) = self
                .open(
                    &ctx,
                    &req.namespace,
                    &req.name,
                    &req.workspace,
                    &req.consistency_token,
                )
                .await?;
            let held = has_objects(&odb, &wanted).await.map_err(objects_error)?;
            Response::ok(pb::HasObjectsResponse {
                present: bitmap(&held),
                consistency_token: token(&record.repo_id, snapshot.seq),
                ..Default::default()
            })
        }

        async fn log(
            &self,
            ctx: RequestContext,
            request: ServiceRequest<'_, pb::LogRequest>,
        ) -> ServiceResult<ServiceStream<pb::LogResponse>> {
            let req = request.to_owned_message();
            let query = LogQuery {
                revs: req.revs.clone(),
                pathspec: Pathspec::parse(&req.paths).map_err(history_error)?,
                limit: req.limit.map(|limit| limit as usize),
                fields: log_fields(&req.fields)?,
            };
            // The commit graph holds repository commits only, so `Log` takes no workspace.
            let (record, snapshot, odb) = self
                .open(&ctx, &req.namespace, &req.name, "", &req.consistency_token)
                .await?;
            let derived = self.derived(&record);
            let at = token(&record.repo_id, snapshot.seq);
            let (tx, mut rx) = mpsc::channel::<Result<pb::LogResponse, ConnectError>>(LOG_BUFFER);
            tokio::spawn(async move {
                let (entries_tx, mut entries_rx) = mpsc::channel::<LogEntry>(LOG_BUFFER);
                let forward = {
                    let tx = tx.clone();
                    async move {
                        while let Some(entry) = entries_rx.recv().await {
                            let message = pb::LogResponse {
                                commit: MessageField::some(log_commit(entry)),
                                consistency_token: at.clone(),
                                ..Default::default()
                            };
                            if tx.send(Ok(message)).await.is_err() {
                                return;
                            }
                        }
                    }
                };
                let walk = async move {
                    let result = log(&odb, &derived, &snapshot, &query, &entries_tx).await;
                    drop(entries_tx);
                    result
                };
                let (result, ()) = tokio::join!(walk, forward);
                if let Err(err) = result {
                    let _ = tx.send(Err(history_error(err))).await;
                }
            });
            Response::ok(
                Box::pin(futures::stream::poll_fn(move |cx| rx.poll_recv(cx)))
                    as ServiceStream<pb::LogResponse>,
            )
        }

        async fn diff_stat(
            &self,
            ctx: RequestContext,
            request: ServiceRequest<'_, pb::DiffStatRequest>,
        ) -> ServiceResult<pb::DiffStatResponse> {
            let req = request.to_owned_message();
            let spec = Pathspec::parse(&req.paths).map_err(history_error)?;
            let (record, snapshot, odb) = self
                .open(
                    &ctx,
                    &req.namespace,
                    &req.name,
                    &req.workspace,
                    &req.consistency_token,
                )
                .await?;
            let from = resolve_tree(&odb, &snapshot, &req.from_rev)
                .await
                .map_err(history_error)?;
            let to = resolve_tree(&odb, &snapshot, &req.to_rev)
                .await
                .map_err(history_error)?;
            let files = diff_stat(&odb, Some(from), Some(to), &spec)
                .await
                .map_err(history_error)?;
            Response::ok(pb::DiffStatResponse {
                files: files
                    .into_iter()
                    .map(|file| pb::FileStat {
                        change: MessageField::some(changed_pb(file.change)),
                        added: file.added,
                        removed: file.removed,
                        ..Default::default()
                    })
                    .collect(),
                consistency_token: token(&record.repo_id, snapshot.seq),
                ..Default::default()
            })
        }

        async fn blame(
            &self,
            ctx: RequestContext,
            request: ServiceRequest<'_, pb::BlameRequest>,
        ) -> ServiceResult<pb::BlameResponse> {
            let req = request.to_owned_message();
            let path =
                RepoPath::parse(&req.path).map_err(|err| invalid("path", err.to_string()))?;
            if path.as_str().is_empty() {
                return Err(invalid("path", "a file is needed"));
            }
            let lines = match (req.start_line, req.end_line) {
                (None, None) => None,
                (Some(start), Some(end)) => Some(start..=end),
                (Some(start), None) => Some(start..=u32::MAX),
                (None, Some(end)) => Some(1..=end),
            };
            // The blame walks repository commits only, as `Log` does.
            let (record, snapshot, odb) = self
                .open(&ctx, &req.namespace, &req.name, "", &req.consistency_token)
                .await?;
            let derived = self.derived(&record);
            let query = BlameQuery {
                rev: req.rev.clone(),
                path,
                lines,
            };
            let answer = blame(&odb, &derived, &snapshot, &query)
                .await
                .map_err(history_error)?;
            let bytes = |path: Option<Vec<u8>>| path.unwrap_or_default();
            Response::ok(pb::BlameResponse {
                commit: answer.commit.to_string(),
                blob: answer.blob.to_string(),
                lines: answer.lines,
                hunks: answer
                    .hunks
                    .into_iter()
                    .map(|hunk| pb::BlameHunk {
                        start_line: hunk.start + 1,
                        source_line: hunk.source_start + 1,
                        lines: hunk.len,
                        commit: hunk.commit.to_string(),
                        source_path: bytes(hunk.source_path),
                        previous: hunk
                            .previous
                            .as_ref()
                            .map(|(oid, _)| oid.to_string())
                            .unwrap_or_default(),
                        previous_path: bytes(hunk.previous.map(|(_, path)| path)),
                        ..Default::default()
                    })
                    .collect(),
                commits: answer
                    .commits
                    .into_iter()
                    .map(|commit| pb::BlameCommit {
                        oid: commit.oid.to_string(),
                        author: MessageField::some(signature_pb(commit.author)),
                        committer: MessageField::some(signature_pb(commit.committer)),
                        summary: commit.summary,
                        boundary: commit.boundary,
                        ..Default::default()
                    })
                    .collect(),
                consistency_token: token(&record.repo_id, snapshot.seq),
                ..Default::default()
            })
        }

        async fn grep(
            &self,
            ctx: RequestContext,
            request: ServiceRequest<'_, pb::GrepRequest>,
        ) -> ServiceResult<ServiceStream<pb::GrepResponse>> {
            let req = request.to_owned_message();
            let mut query = GrepQuery::new(req.rev.clone(), req.patterns.clone());
            query.pathspec = Pathspec::parse(&req.paths).map_err(history_error)?;
            query.flags = GrepFlags {
                ignore_case: req.ignore_case,
                word: req.word,
                fixed: req.fixed_strings,
                extended: req.extended,
                files_only: req.files_only,
            };
            if let Some(max) = req.max_matches {
                query.max_matches = query.max_matches.min(max as usize);
            }
            // Refuse a bad pattern before the stream starts.
            loams_git::history::grep::matcher(&query.patterns, query.flags)
                .map_err(history_error)?;
            let (record, snapshot, odb) = self
                .open(
                    &ctx,
                    &req.namespace,
                    &req.name,
                    &req.workspace,
                    &req.consistency_token,
                )
                .await?;
            let at = token(&record.repo_id, snapshot.seq);
            let (tx, mut rx) = mpsc::channel::<Result<pb::GrepResponse, ConnectError>>(LOG_BUFFER);
            tokio::spawn(async move {
                let (files_tx, mut files_rx) = mpsc::channel::<GrepFile>(LOG_BUFFER);
                let forward = {
                    let tx = tx.clone();
                    let at = at.clone();
                    async move {
                        while let Some(file) = files_rx.recv().await {
                            let message = pb::GrepResponse {
                                file: MessageField::some(grep_file_pb(file)),
                                consistency_token: at.clone(),
                                ..Default::default()
                            };
                            if tx.send(Ok(message)).await.is_err() {
                                return;
                            }
                        }
                    }
                };
                let search = async move {
                    let result = grep(&odb, &snapshot, &query, &files_tx).await;
                    drop(files_tx);
                    result
                };
                let (result, ()) = tokio::join!(search, forward);
                let last = match result {
                    Ok(stats) => Ok(pb::GrepResponse {
                        truncated: stats.truncated,
                        deadline_exceeded: stats.deadline_exceeded,
                        done: true,
                        consistency_token: at,
                        ..Default::default()
                    }),
                    Err(err) => Err(history_error(err)),
                };
                let _ = tx.send(last).await;
            });
            Response::ok(
                Box::pin(futures::stream::poll_fn(move |cx| rx.poll_recv(cx)))
                    as ServiceStream<pb::GrepResponse>,
            )
        }

        async fn put_workspace_pack(
            &self,
            ctx: RequestContext,
            mut requests: InboundStream<pb::PutWorkspacePackRequest>,
        ) -> ServiceResult<pb::PutWorkspacePackResponse> {
            let first = requests
                .next()
                .await
                .ok_or_else(|| invalid("workspace", "an upload needs at least one message"))??
                .to_owned_message();
            // The namespace comes with the first message: checked before anything is used.
            self.repos
                .require(&ctx, GitAccess::Write, &first.namespace)
                .await?;
            if first.workspace.is_empty() {
                return Err(invalid("workspace", "a workspace is required"));
            }
            let name = repo_name("name", &first.name)?;
            let ns = self.repos.namespace(&first.namespace, false).await?;
            let record = self.repos.active(ns, &name).await?;
            let Some((ws, view)) = workspace(&self.repos.git, &record, &first.workspace).await?
            else {
                return Err(workspace_not_found(&first.workspace));
            };
            self.may_write(&ctx, &view).await?;
            // Each upload holds its bundle in memory: a node verifies a few at once, and a
            // stream that stalls or runs past the upload deadline is dropped.
            let _permit = PACK_UPLOADS.try_acquire().ok_or_else(|| {
                refuse(
                    ErrorCode::ResourceExhausted,
                    "resource_exhausted",
                    "this node is verifying as many workspace packs as it takes; retry",
                    &[("retry_after_ms", "1000".to_owned())],
                )
            })?;
            let deadline = tokio::time::Instant::now() + UPLOAD_DEADLINE;
            let mut lpk = BytesMut::from(&first.data[..]);
            check_pack_len(lpk.len() as u64).map_err(ws_pack_error)?;
            loop {
                let wait = deadline
                    .saturating_duration_since(tokio::time::Instant::now())
                    .min(RECEIVE_IDLE_TIMEOUT);
                let Ok(next) = tokio::time::timeout(wait, requests.next()).await else {
                    return Err(refuse(
                        ErrorCode::DeadlineExceeded,
                        "deadline_exceeded",
                        "the upload stalled or ran past its deadline",
                        &[],
                    ));
                };
                let Some(message) = next else {
                    break;
                };
                let message = message?.to_owned_message();
                check_pack_len((lpk.len() + message.data.len()) as u64).map_err(ws_pack_error)?;
                lpk.extend_from_slice(&message.data);
            }
            let paths = RepoPaths::new(record.ns, &record.repo_id);
            let blobs = StoreBlobStore::workspace(self.repos.git.store.clone(), &paths, &ws);
            let (verified, put) =
                put_workspace_pack(&record.repo_id, &blobs, lpk.freeze(), &NoScan)
                    .await
                    .map_err(ws_pack_error)?;
            Response::ok(pb::PutWorkspacePackResponse {
                pack: MessageField::some(pack_pb(&verified.pack)),
                existed: put == Put::Existed,
                ..Default::default()
            })
        }
    }

    fn grep_file_pb(file: GrepFile) -> pb::GrepFile {
        pb::GrepFile {
            path: file.path,
            blob: file.blob.to_string(),
            binary: file.binary,
            lines: file
                .lines
                .into_iter()
                .map(|line| pb::GrepLine {
                    number: line.number,
                    line: line.line.to_vec(),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }

    /// What the git listener asks of the object route.
    #[derive(Debug)]
    pub(crate) struct ObjectRoute<'a> {
        pub(crate) grant: &'a GitGrant,
        pub(crate) ns: NamespaceId,
        pub(crate) ns_name: &'a str,
        pub(crate) record: &'a RepoRecord,
        pub(crate) oid: &'a str,
        pub(crate) workspace: Option<&'a str>,
    }

    fn text(status: StatusCode, body: &'static str) -> HttpResponse {
        (status, body).into_response()
    }

    /// `GET /git/<ns>/<repo>.git/loams/objects/<oid>[?ws=<ws_id>]` (§19.5.1): the object in
    /// git's loose format, cacheable for a year. Objects only in a workspace's packs need the
    /// workspace, and a token whose `WsMatch` reaches it (ruling T18.2); otherwise they do not
    /// exist (404).
    pub(crate) async fn serve_object(git: &GitRuntime, route: ObjectRoute<'_>) -> HttpResponse {
        let unavailable = || text(StatusCode::SERVICE_UNAVAILABLE, "loams: try again later\n");
        // The listener's one 404, so a hidden object looks like any other absent one.
        let not_found = crate::api::git_auth::not_found;
        if route
            .grant
            .require(route.ns, &route.record.name, GitAccess::Read)
            .is_err()
        {
            return not_found();
        }
        let Ok(oid) = parse_oid("oid", route.oid) else {
            return not_found();
        };
        let ws = match route.workspace {
            None | Some("") => None,
            Some(value) => {
                let Ok(ws) = WsId::parse(value) else {
                    return not_found();
                };
                match git
                    .workspaces
                    .workspace(route.ns, &route.record.repo_id, &ws)
                    .await
                {
                    Ok(Some(view)) if grant_reads_workspace(route.grant, &ws, &view) => {
                        Some((ws, view))
                    }
                    Ok(_) => return not_found(),
                    Err(err) => {
                        tracing::warn!(error = %err, "the workspace lookup failed");
                        return unavailable();
                    }
                }
            }
        };
        let log = match git
            .sequencers
            .get(
                route.ns,
                &format!("local/{}", route.ns_name),
                &route.record.repo_id,
            )
            .await
        {
            Ok(log) => log,
            Err(err) => {
                tracing::warn!(error = %err, "opening a repository for the object route failed");
                return unavailable();
            }
        };
        let snapshot = match loams_git::RefLog::snapshot(&log, ReadAt::Latest).await {
            Ok(snapshot) => snapshot,
            Err(err) => {
                tracing::warn!(error = %err, "reading a repository for the object route failed");
                return unavailable();
            }
        };
        let odb = match open_odb(
            git,
            route.record,
            &snapshot,
            ws.as_ref().map(|(id, v)| (id, v)),
        )
        .await
        {
            Ok(odb) => odb,
            Err(err) => {
                tracing::warn!(error = %err, "opening objects for the object route failed");
                return unavailable();
            }
        };
        match loose_object(&odb, &oid).await {
            Ok(Some(body)) => {
                let mut response = (StatusCode::OK, body).into_response();
                let headers = response.headers_mut();
                headers.insert(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("application/x-git-loose-object"),
                );
                headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(IMMUTABLE));
                response
            }
            Ok(None) => not_found(),
            Err(err) => {
                tracing::warn!(error = %err, "reading an object for the object route failed");
                unavailable()
            }
        }
    }
}
