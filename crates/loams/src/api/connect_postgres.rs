// The generated service traits return `impl Encodable<_>`; these impls name
// the concrete message type, which is the intended refinement (`connect.rs`
// does the same).
#![allow(refining_impl_trait)]

//! `loams.postgres.v1.PostgresService`, and `loams.operations.v1`'s reads of
//! its operations, on `pg-control`'s API listener (design §46 §4; PG2
//! Task 9).
//!
//! Every call goes through the [`Gate`] first:
//!
//! 1. **authentication**: the [`Authenticator`] turns the headers into a
//!    [`Principal`] (`unauthenticated` otherwise);
//! 2. **authorization**: the relation [`authz::required`] names for the RPC,
//!    on the request's namespace or project, through the [`Authorizer`]
//!    (`permission_denied` otherwise). A mutation on a project also asks for
//!    `admin`, which decides [`Caller::admin`] (protected branches); an
//!    agent never has it;
//! 3. the service call, whose plain types [`super::postgres_messages`] maps
//!    to and from the protos;
//! 4. **audit**: every mutation, and every refusal before the service, is
//!    one [`AuditEvent`] (D221). Reads that succeed are not audited.
//!
//! The RPCs whose behaviour later PG2 tasks build (restore, upgrade,
//! connecting, promotion) answer `unimplemented` with the reason
//! `not_implemented`, after the same checks, and are audited the same way.
//! The endpoint RPCs and `WatchProject` are Task 11's.

use std::future::Future;
use std::sync::Arc;

use buffa::MessageField;
use connectrpc::{
    ConnectError, ErrorCode, RequestContext, Response, ServiceRequest, ServiceResult, ServiceStream,
};
use futures::StreamExt;
use loams_pg_control::audit::{AuditEvent, AuditOutcome, AuditSink};
use loams_pg_control::authz::{
    self, Authenticator, Authorizer, Object, Principal, Relation, Scope, authorize,
};
use loams_pg_control::neon::NeonClients;
use loams_pg_control::service::branches::{
    BranchPoint, CreateBranch, DeleteBranch, SetDefaultBranch, UpdateBranch,
};
use loams_pg_control::service::connect::GetConnectionInfo;
use loams_pg_control::service::databases::{CreateDatabase, DeleteDatabase};
use loams_pg_control::service::endpoints::{
    CreateEndpoint, DeleteEndpoint, EndpointAction, UpdateEndpoint,
};
use loams_pg_control::service::projects::{CreateProject, DeleteProject, UpdateProject};
use loams_pg_control::service::roles::{CreateRole, DeleteRole, ResetRolePassword};
use loams_pg_control::service::watch::WatchOptions;
use loams_pg_control::service::{Caller, Clock, PgService, Reason, ServiceError};
use loams_proto::loams::operations::v1 as ops;
use loams_proto::loams::operations::v1::{OperationsService, OperationsServiceExt};
use loams_proto::loams::postgres::v1 as pb;
use loams_proto::loams::postgres::v1::{PostgresService, PostgresServiceExt};

use super::connect::refuse;
use super::postgres_messages as msg;

/// The service's path prefix.
const SERVICE: &str = "/loams.postgres.v1.PostgresService/";

/// The checks and the audit every call goes through.
pub(crate) struct Gate {
    pub authenticator: Arc<dyn Authenticator>,
    pub authorizer: Arc<dyn Authorizer>,
    pub audit: Arc<dyn AuditSink>,
    pub clock: Clock,
}

/// What a call is about: for its checks and its audit event.
struct Call {
    /// The method's name, `CreateBranch`.
    method: &'static str,
    namespace: String,
    project_id: Option<String>,
    /// Names and ids the request gave besides the project.
    target: Vec<(&'static str, String)>,
    idempotency_key: Option<String>,
}

impl Call {
    fn new(method: &'static str, namespace: &str) -> Self {
        Call {
            method,
            namespace: namespace.to_owned(),
            project_id: None,
            target: Vec::new(),
            idempotency_key: None,
        }
    }

    fn project(mut self, project_id: &str) -> Self {
        self.project_id = Some(project_id.to_owned());
        self
    }

    /// Adds a target, when the request named it.
    fn target(mut self, key: &'static str, value: &str) -> Self {
        if !value.is_empty() {
            self.target.push((key, value.to_owned()));
        }
        self
    }

    fn key(mut self, key: &str) -> Self {
        if !key.is_empty() {
            self.idempotency_key = Some(key.to_owned());
        }
        self
    }

    /// The object the method's relation is checked on.
    fn object(&self, scope: Scope) -> Object {
        match (scope, &self.project_id) {
            (Scope::Project, Some(project_id)) => Object::Project {
                namespace: self.namespace.clone(),
                project_id: project_id.clone(),
            },
            _ => Object::Namespace(self.namespace.clone()),
        }
    }
}

/// What a mutation's audit event names that only its answer knows.
#[derive(Default)]
struct Made {
    /// The operation it answered.
    operation: Option<String>,
    /// The project it created.
    project: Option<String>,
}

impl Made {
    fn operation(id: &str) -> Self {
        Made {
            operation: Some(id.to_owned()),
            project: None,
        }
    }
}

/// How a call failed: the service's refusal, or an RPC a later task builds.
enum Failure {
    Service(ServiceError),
    /// The PG2 task that builds the RPC.
    NotBuilt(&'static str),
}

impl From<ServiceError> for Failure {
    fn from(e: ServiceError) -> Self {
        Failure::Service(e)
    }
}

/// The Connect code of a registered reason.
fn code(reason: Reason) -> ErrorCode {
    match reason.code() {
        "not_found" => ErrorCode::NotFound,
        "failed_precondition" => ErrorCode::FailedPrecondition,
        "unavailable" => ErrorCode::Unavailable,
        "invalid_argument" => ErrorCode::InvalidArgument,
        "already_exists" => ErrorCode::AlreadyExists,
        "permission_denied" => ErrorCode::PermissionDenied,
        "unauthenticated" => ErrorCode::Unauthenticated,
        "resource_exhausted" => ErrorCode::ResourceExhausted,
        "aborted" => ErrorCode::Aborted,
        _ => ErrorCode::Internal,
    }
}

/// A service refusal as a Connect error, with its reason and metadata.
fn refused(e: &ServiceError) -> ConnectError {
    let metadata: Vec<(&str, String)> = e
        .metadata
        .iter()
        .map(|(k, v)| (k.as_str(), v.clone()))
        .collect();
    refuse(
        code(e.reason),
        e.reason.as_str(),
        e.message.clone(),
        &metadata,
    )
}

impl Failure {
    fn reason(&self) -> &str {
        match self {
            Failure::Service(e) => e.reason.as_str(),
            Failure::NotBuilt(_) => "not_implemented",
        }
    }

    fn into_connect(self, method: &str) -> ConnectError {
        match self {
            Failure::Service(e) => refused(&e),
            Failure::NotBuilt(task) => refuse(
                ErrorCode::Unimplemented,
                "not_implemented",
                format!("loams.postgres.v1.PostgresService/{method} is not built yet ({task})"),
                &[],
            ),
        }
    }
}

/// The ledger's principal: kind and subject, so a user and an agent of the
/// same name never share idempotency keys.
fn ledger_principal(p: &Principal) -> String {
    format!("{}:{}", p.kind.as_str(), p.subject)
}

impl Gate {
    fn emit(&self, call: &Call, principal: Option<&Principal>, outcome: AuditOutcome, made: Made) {
        let mut event = AuditEvent::new(
            format!("{SERVICE}{}", call.method),
            self.clock.now_ms(),
            outcome,
        );
        if let Some(p) = principal {
            event.principal = p.subject.clone();
            event.principal_kind = Some(p.kind);
            event.act = p.act.clone();
            event.credential = p.credential.clone();
        }
        event.namespace = call.namespace.clone();
        event.project_id = made.project.or_else(|| call.project_id.clone());
        event.target = call
            .target
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.clone()))
            .collect();
        event.idempotency_key = call.idempotency_key.clone();
        event.operation_id = made.operation;
        self.audit.emit(&event);
    }

    /// Authenticates and authorizes `call`; a refusal is audited.
    async fn admit(&self, ctx: &RequestContext, call: &Call) -> Result<Principal, ConnectError> {
        let principal = match self.authenticator.authenticate(ctx.headers()).await {
            Ok(p) => p,
            Err(e) => {
                let e = ServiceError::from(e);
                self.emit(call, None, refusal(&e), Made::default());
                return Err(refused(&e));
            }
        };
        let Some((relation, scope)) = authz::required(call.method) else {
            let e = ServiceError::new(Reason::Internal, "internal error");
            tracing::error!(
                method = call.method,
                "an RPC with no relation in authz::required"
            );
            return Err(refused(&e));
        };
        if let Err(e) = authorize(
            self.authorizer.as_ref(),
            &principal,
            relation,
            &call.object(scope),
        )
        .await
        {
            self.emit(call, Some(&principal), refusal(&e), Made::default());
            return Err(refused(&e));
        }
        Ok(principal)
    }

    /// A read: admitted, then `f`. Only refusals before `f` are audited.
    async fn read<T, Fut>(
        &self,
        ctx: &RequestContext,
        call: Call,
        f: impl FnOnce() -> Fut,
    ) -> Result<T, ConnectError>
    where
        Fut: Future<Output = Result<T, Failure>>,
    {
        self.admit(ctx, &call).await?;
        f().await.map_err(|e| e.into_connect(call.method))
    }

    /// A mutation: admitted, then `f` with the caller, then audited with
    /// its outcome and the operation it answered.
    async fn mutate<T, Fut>(
        &self,
        ctx: &RequestContext,
        call: Call,
        f: impl FnOnce(Caller) -> Fut,
    ) -> Result<T, ConnectError>
    where
        Fut: Future<Output = Result<(T, Made), Failure>>,
    {
        let principal = self.admit(ctx, &call).await?;
        let admin = match call.object(Scope::Project) {
            o @ Object::Project { .. } => {
                authorize(self.authorizer.as_ref(), &principal, Relation::Admin, &o)
                    .await
                    .is_ok()
            }
            Object::Namespace(_) => false,
        };
        let caller = Caller {
            principal: ledger_principal(&principal),
            admin,
        };
        match f(caller).await {
            Ok((answer, made)) => {
                self.emit(&call, Some(&principal), AuditOutcome::Ok, made);
                Ok(answer)
            }
            Err(e) => {
                self.emit(
                    &call,
                    Some(&principal),
                    AuditOutcome::Refused {
                        reason: e.reason().to_owned(),
                    },
                    Made::default(),
                );
                Err(e.into_connect(call.method))
            }
        }
    }
}

fn refusal(e: &ServiceError) -> AuditOutcome {
    AuditOutcome::Refused {
        reason: e.reason.as_str().to_owned(),
    }
}

/// `loams.postgres.v1.PostgresService` over the service.
pub(crate) struct Postgres {
    pub service: PgService<NeonClients>,
    pub gate: Arc<Gate>,
}

/// `Ok` for a service answer with no operation.
fn plain<T>(r: Result<T, ServiceError>) -> Result<(T, Made), Failure> {
    r.map(|t| (t, Made::default())).map_err(Failure::Service)
}

impl PostgresService for Postgres {
    async fn create_project(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::CreateProjectRequest>,
    ) -> ServiceResult<pb::CreateProjectResponse> {
        let r = request.to_owned_message();
        let call = Call::new("CreateProject", &r.namespace)
            .target("name", &r.name)
            .key(&r.idempotency_key);
        let response = self
            .gate
            .mutate(&ctx, call, |caller| async move {
                let history_retention = r
                    .history_retention
                    .as_option()
                    .map(|d| msg::duration(d, "history_retention"))
                    .transpose()?;
                let out = self
                    .service
                    .create_project(
                        &caller,
                        CreateProject {
                            namespace: r.namespace,
                            name: r.name,
                            pg_version: r.pg_version,
                            region: r.region,
                            history_retention,
                            wal_pool: r.wal_pool,
                            allow_beta_major: r.allow_beta_major,
                            idempotency_key: r.idempotency_key,
                        },
                    )
                    .await?;
                // The audit names the project the create made.
                let made = Made {
                    operation: Some(out.operation.id.clone()),
                    project: Some(out.project.record.id.clone()),
                };
                Ok((
                    pb::CreateProjectResponse {
                        operation: MessageField::some(msg::operation(&out.operation)),
                        project: MessageField::some(msg::project(&out.project)),
                        ..Default::default()
                    },
                    made,
                ))
            })
            .await?;
        Response::ok(response)
    }

    async fn get_project(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::GetProjectRequest>,
    ) -> ServiceResult<pb::GetProjectResponse> {
        let call = Call::new("GetProject", request.namespace).project(request.project_id);
        let p = self
            .gate
            .read(&ctx, call, || async {
                Ok(self
                    .service
                    .get_project(request.namespace, request.project_id)
                    .await?)
            })
            .await?;
        Response::ok(pb::GetProjectResponse {
            project: MessageField::some(msg::project(&p)),
            ..Default::default()
        })
    }

    async fn list_projects(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::ListProjectsRequest>,
    ) -> ServiceResult<pb::ListProjectsResponse> {
        let call = Call::new("ListProjects", request.namespace);
        let (projects, next) = self
            .gate
            .read(&ctx, call, || async {
                Ok(self
                    .service
                    .list_projects(request.namespace, request.page_size, request.page_token)
                    .await?)
            })
            .await?;
        Response::ok(pb::ListProjectsResponse {
            projects: projects.iter().map(msg::project).collect(),
            next_page_token: next,
            ..Default::default()
        })
    }

    async fn update_project(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::UpdateProjectRequest>,
    ) -> ServiceResult<pb::UpdateProjectResponse> {
        let r = request.to_owned_message();
        let project = r.project.as_option().cloned().unwrap_or_default();
        let call = Call::new("UpdateProject", &r.namespace)
            .project(&project.id)
            .key(&r.idempotency_key);
        let p = self
            .gate
            .mutate(&ctx, call, |caller| async move {
                let paths = msg::mask(r.update_mask.as_option(), &["name", "history_retention"])?;
                let history_retention = if paths.contains(&"history_retention") {
                    Some(msg::duration(
                        project.history_retention.as_option().ok_or_else(|| {
                            ServiceError::invalid(
                                "project.history_retention",
                                "the mask names history_retention, which is unset",
                            )
                        })?,
                        "project.history_retention",
                    )?)
                } else {
                    None
                };
                plain(
                    self.service
                        .update_project(
                            &caller,
                            UpdateProject {
                                namespace: r.namespace,
                                project_id: project.id,
                                name: paths.contains(&"name").then_some(project.name),
                                history_retention,
                                expected_version: r.expected_version,
                                idempotency_key: r.idempotency_key,
                            },
                        )
                        .await,
                )
            })
            .await?;
        Response::ok(pb::UpdateProjectResponse {
            project: MessageField::some(msg::project(&p)),
            ..Default::default()
        })
    }

    async fn delete_project(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::DeleteProjectRequest>,
    ) -> ServiceResult<pb::DeleteProjectResponse> {
        let r = request.to_owned_message();
        let call = Call::new("DeleteProject", &r.namespace)
            .project(&r.project_id)
            .key(&r.idempotency_key);
        let op = self
            .gate
            .mutate(&ctx, call, |caller| async move {
                let op = self
                    .service
                    .delete_project(
                        &caller,
                        DeleteProject {
                            namespace: r.namespace,
                            project_id: r.project_id,
                            expected_version: r.expected_version,
                            idempotency_key: r.idempotency_key,
                        },
                    )
                    .await?;
                let made = Made::operation(&op.id);
                Ok((op, made))
            })
            .await?;
        Response::ok(pb::DeleteProjectResponse {
            operation: MessageField::some(msg::operation(&op)),
            ..Default::default()
        })
    }

    async fn upgrade_project(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::UpgradeProjectRequest>,
    ) -> ServiceResult<pb::UpgradeProjectResponse> {
        let call = Call::new("UpgradeProject", request.namespace)
            .project(request.project_id)
            .key(request.idempotency_key);
        self.gate
            .mutate(&ctx, call, |_| async {
                Err(Failure::NotBuilt("PG2 Task 51"))
            })
            .await
    }

    /// The majors the service serves (design 62 §9.2). It describes the
    /// service, not a namespace's data: any authenticated caller may read it.
    async fn list_pg_versions(
        &self,
        ctx: RequestContext,
        _request: ServiceRequest<'_, pb::ListPgVersionsRequest>,
    ) -> ServiceResult<pb::ListPgVersionsResponse> {
        if let Err(e) = self.gate.authenticator.authenticate(ctx.headers()).await {
            let e = ServiceError::from(e);
            return Err(refused(&e));
        }
        Response::ok(pb::ListPgVersionsResponse {
            versions: self
                .service
                .list_pg_versions()
                .iter()
                .map(msg::pg_version)
                .collect(),
            ..Default::default()
        })
    }

    async fn create_branch(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::CreateBranchRequest>,
    ) -> ServiceResult<pb::CreateBranchResponse> {
        let r = request.to_owned_message();
        let call = Call::new("CreateBranch", &r.namespace)
            .project(&r.project_id)
            .target("name", &r.name)
            .target("parent_id", &r.parent_id)
            .key(&r.idempotency_key);
        let out = self
            .gate
            .mutate(&ctx, call, |caller| async move {
                use pb::__buffa::oneof::create_branch_request::Point;
                let point = match r.point {
                    None => BranchPoint::Head,
                    Some(Point::Lsn(lsn)) => BranchPoint::Lsn(lsn),
                    Some(Point::Time(t)) => BranchPoint::Time(msg::timestamp_ms(&t, "time")?),
                };
                let ttl = r
                    .ttl
                    .as_option()
                    .map(|d| msg::duration(d, "ttl"))
                    .transpose()?;
                let out = self
                    .service
                    .create_branch(
                        &caller,
                        CreateBranch {
                            namespace: r.namespace,
                            project_id: r.project_id,
                            name: r.name,
                            parent_id: r.parent_id,
                            point,
                            protected: r.protected,
                            ttl,
                            idempotency_key: r.idempotency_key,
                        },
                    )
                    .await?;
                let made = Made::operation(&out.operation.id);
                Ok((out, made))
            })
            .await?;
        Response::ok(pb::CreateBranchResponse {
            operation: MessageField::some(msg::operation(&out.operation)),
            branch: MessageField::some(msg::branch(&out.branch)),
            ..Default::default()
        })
    }

    async fn get_branch(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::GetBranchRequest>,
    ) -> ServiceResult<pb::GetBranchResponse> {
        let call = Call::new("GetBranch", request.namespace)
            .project(request.project_id)
            .target("branch_id", request.branch_id);
        let b = self
            .gate
            .read(&ctx, call, || async {
                Ok(self
                    .service
                    .get_branch(request.namespace, request.project_id, request.branch_id)
                    .await?)
            })
            .await?;
        Response::ok(pb::GetBranchResponse {
            branch: MessageField::some(msg::branch(&b)),
            ..Default::default()
        })
    }

    async fn list_branches(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::ListBranchesRequest>,
    ) -> ServiceResult<pb::ListBranchesResponse> {
        let call = Call::new("ListBranches", request.namespace).project(request.project_id);
        let (branches, next) = self
            .gate
            .read(&ctx, call, || async {
                Ok(self
                    .service
                    .list_branches(
                        request.namespace,
                        request.project_id,
                        request.page_size,
                        request.page_token,
                    )
                    .await?)
            })
            .await?;
        Response::ok(pb::ListBranchesResponse {
            branches: branches.iter().map(msg::branch).collect(),
            next_page_token: next,
            ..Default::default()
        })
    }

    async fn update_branch(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::UpdateBranchRequest>,
    ) -> ServiceResult<pb::UpdateBranchResponse> {
        let r = request.to_owned_message();
        let branch = r.branch.as_option().cloned().unwrap_or_default();
        let call = Call::new("UpdateBranch", &r.namespace)
            .project(&r.project_id)
            .target("branch_id", &branch.id)
            .key(&r.idempotency_key);
        let b = self
            .gate
            .mutate(&ctx, call, |caller| async move {
                let paths = msg::mask(
                    r.update_mask.as_option(),
                    &["name", "expire_time", "protected"],
                )?;
                let expire_at_ms = if paths.contains(&"expire_time") {
                    Some(
                        branch
                            .expire_time
                            .as_option()
                            .map(|t| msg::timestamp_ms(t, "branch.expire_time"))
                            .transpose()?,
                    )
                } else {
                    None
                };
                plain(
                    self.service
                        .update_branch(
                            &caller,
                            UpdateBranch {
                                namespace: r.namespace,
                                project_id: r.project_id,
                                branch_id: branch.id,
                                name: paths.contains(&"name").then_some(branch.name),
                                expire_at_ms,
                                protected: paths.contains(&"protected").then_some(branch.protected),
                                expected_version: r.expected_version,
                                idempotency_key: r.idempotency_key,
                            },
                        )
                        .await,
                )
            })
            .await?;
        Response::ok(pb::UpdateBranchResponse {
            branch: MessageField::some(msg::branch(&b)),
            ..Default::default()
        })
    }

    async fn delete_branch(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::DeleteBranchRequest>,
    ) -> ServiceResult<pb::DeleteBranchResponse> {
        let r = request.to_owned_message();
        let call = Call::new("DeleteBranch", &r.namespace)
            .project(&r.project_id)
            .target("branch_id", &r.branch_id)
            .key(&r.idempotency_key);
        let op = self
            .gate
            .mutate(&ctx, call, |caller| async move {
                let op = self
                    .service
                    .delete_branch(
                        &caller,
                        DeleteBranch {
                            namespace: r.namespace,
                            project_id: r.project_id,
                            branch_id: r.branch_id,
                            expected_version: r.expected_version,
                            idempotency_key: r.idempotency_key,
                        },
                    )
                    .await?;
                let made = Made::operation(&op.id);
                Ok((op, made))
            })
            .await?;
        Response::ok(pb::DeleteBranchResponse {
            operation: MessageField::some(msg::operation(&op)),
            ..Default::default()
        })
    }

    async fn restore_branch(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::RestoreBranchRequest>,
    ) -> ServiceResult<pb::RestoreBranchResponse> {
        let call = Call::new("RestoreBranch", request.namespace)
            .project(request.project_id)
            .target("branch_id", request.branch_id)
            .key(request.idempotency_key);
        self.gate
            .mutate(&ctx, call, |_| async {
                Err(Failure::NotBuilt("PG2 Task 44"))
            })
            .await
    }

    async fn set_default_branch(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::SetDefaultBranchRequest>,
    ) -> ServiceResult<pb::SetDefaultBranchResponse> {
        let r = request.to_owned_message();
        let call = Call::new("SetDefaultBranch", &r.namespace)
            .project(&r.project_id)
            .target("branch_id", &r.branch_id)
            .key(&r.idempotency_key);
        let p = self
            .gate
            .mutate(&ctx, call, |caller| async move {
                plain(
                    self.service
                        .set_default_branch(
                            &caller,
                            SetDefaultBranch {
                                namespace: r.namespace,
                                project_id: r.project_id,
                                branch_id: r.branch_id,
                                expected_version: r.expected_version,
                                idempotency_key: r.idempotency_key,
                            },
                        )
                        .await,
                )
            })
            .await?;
        Response::ok(pb::SetDefaultBranchResponse {
            project: MessageField::some(msg::project(&p)),
            ..Default::default()
        })
    }

    async fn create_endpoint(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::CreateEndpointRequest>,
    ) -> ServiceResult<pb::CreateEndpointResponse> {
        let r = request.to_owned_message();
        let call = Call::new("CreateEndpoint", &r.namespace)
            .project(&r.project_id)
            .target("branch_id", &r.branch_id)
            .key(&r.idempotency_key);
        let view = self
            .gate
            .mutate(&ctx, call, |caller| async move {
                let suspend_timeout_s = r
                    .suspend_timeout
                    .as_option()
                    .map(|d| msg::suspend_timeout(d, "suspend_timeout"))
                    .transpose()?;
                let req = CreateEndpoint {
                    namespace: r.namespace,
                    project_id: r.project_id,
                    branch_id: r.branch_id,
                    kind: msg::endpoint_type(r.r#type, "type")?,
                    min_cu: msg::cu(r.min_cu, "min_cu")?,
                    max_cu: msg::cu(r.max_cu, "max_cu")?,
                    suspend_timeout_s,
                    pool_mode: msg::pool_mode(r.pool_mode, "pool_mode")?,
                    pg_settings: r.pg_settings.into_iter().collect(),
                    idempotency_key: r.idempotency_key,
                };
                plain(self.service.create_endpoint(&caller, req).await)
            })
            .await?;
        Response::ok(pb::CreateEndpointResponse {
            endpoint: MessageField::some(msg::endpoint(&view)),
            ..Default::default()
        })
    }

    async fn get_endpoint(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::GetEndpointRequest>,
    ) -> ServiceResult<pb::GetEndpointResponse> {
        let call = Call::new("GetEndpoint", request.namespace)
            .project(request.project_id)
            .target("endpoint_id", request.endpoint_id);
        let view = self
            .gate
            .read(&ctx, call, || async {
                Ok(self
                    .service
                    .get_endpoint(request.namespace, request.project_id, request.endpoint_id)
                    .await?)
            })
            .await?;
        Response::ok(pb::GetEndpointResponse {
            endpoint: MessageField::some(msg::endpoint(&view)),
            ..Default::default()
        })
    }

    async fn list_endpoints(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::ListEndpointsRequest>,
    ) -> ServiceResult<pb::ListEndpointsResponse> {
        let call = Call::new("ListEndpoints", request.namespace)
            .project(request.project_id)
            .target("branch_id", request.branch_id);
        let (endpoints, next) = self
            .gate
            .read(&ctx, call, || async {
                Ok(self
                    .service
                    .list_endpoints(
                        request.namespace,
                        request.project_id,
                        request.branch_id,
                        request.page_size,
                        request.page_token,
                    )
                    .await?)
            })
            .await?;
        Response::ok(pb::ListEndpointsResponse {
            endpoints: endpoints.iter().map(msg::endpoint).collect(),
            next_page_token: next,
            ..Default::default()
        })
    }

    async fn update_endpoint(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::UpdateEndpointRequest>,
    ) -> ServiceResult<pb::UpdateEndpointResponse> {
        let r = request.to_owned_message();
        let endpoint = r.endpoint.as_option().cloned().unwrap_or_default();
        let call = Call::new("UpdateEndpoint", &r.namespace)
            .project(&r.project_id)
            .target("endpoint_id", &endpoint.id)
            .key(&r.idempotency_key);
        let view = self
            .gate
            .mutate(&ctx, call, |caller| async move {
                let paths = msg::mask(
                    r.update_mask.as_option(),
                    &[
                        "min_cu",
                        "max_cu",
                        "suspend_timeout",
                        "pool_mode",
                        "pg_settings",
                    ],
                )?;
                let has = |p: &str| paths.contains(&p);
                let suspend_timeout_s = if has("suspend_timeout") {
                    Some(endpoint.suspend_timeout.as_option().map_or(Ok(0), |d| {
                        msg::suspend_timeout(d, "endpoint.suspend_timeout")
                    })?)
                } else {
                    None
                };
                let req = UpdateEndpoint {
                    namespace: r.namespace,
                    project_id: r.project_id,
                    endpoint_id: endpoint.id,
                    min_cu: if has("min_cu") {
                        msg::cu(endpoint.min_cu, "endpoint.min_cu")?
                    } else {
                        None
                    },
                    max_cu: if has("max_cu") {
                        msg::cu(endpoint.max_cu, "endpoint.max_cu")?
                    } else {
                        None
                    },
                    suspend_timeout_s,
                    pool_mode: if has("pool_mode") {
                        msg::pool_mode(endpoint.pool_mode, "endpoint.pool_mode")?
                    } else {
                        None
                    },
                    pg_settings: has("pg_settings")
                        .then(|| endpoint.pg_settings.into_iter().collect()),
                    expected_version: r.expected_version,
                    idempotency_key: r.idempotency_key,
                };
                plain(self.service.update_endpoint(&caller, req).await)
            })
            .await?;
        Response::ok(pb::UpdateEndpointResponse {
            endpoint: MessageField::some(msg::endpoint(&view)),
            ..Default::default()
        })
    }

    async fn start_endpoint(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::StartEndpointRequest>,
    ) -> ServiceResult<pb::StartEndpointResponse> {
        let r = request.to_owned_message();
        let call = Call::new("StartEndpoint", &r.namespace)
            .project(&r.project_id)
            .target("endpoint_id", &r.endpoint_id)
            .key(&r.idempotency_key);
        let op = self
            .gate
            .mutate(&ctx, call, |caller| async move {
                let action = EndpointAction {
                    namespace: r.namespace,
                    project_id: r.project_id,
                    endpoint_id: r.endpoint_id,
                    idempotency_key: r.idempotency_key,
                };
                let op = self.service.start_endpoint(&caller, action).await?;
                let made = Made::operation(&op.id);
                Ok((op, made))
            })
            .await?;
        Response::ok(pb::StartEndpointResponse {
            operation: MessageField::some(msg::operation(&op)),
            ..Default::default()
        })
    }

    async fn suspend_endpoint(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::SuspendEndpointRequest>,
    ) -> ServiceResult<pb::SuspendEndpointResponse> {
        let r = request.to_owned_message();
        let call = Call::new("SuspendEndpoint", &r.namespace)
            .project(&r.project_id)
            .target("endpoint_id", &r.endpoint_id)
            .key(&r.idempotency_key);
        let op = self
            .gate
            .mutate(&ctx, call, |caller| async move {
                let action = EndpointAction {
                    namespace: r.namespace,
                    project_id: r.project_id,
                    endpoint_id: r.endpoint_id,
                    idempotency_key: r.idempotency_key,
                };
                let op = self.service.suspend_endpoint(&caller, action).await?;
                let made = Made::operation(&op.id);
                Ok((op, made))
            })
            .await?;
        Response::ok(pb::SuspendEndpointResponse {
            operation: MessageField::some(msg::operation(&op)),
            ..Default::default()
        })
    }

    async fn restart_endpoint(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::RestartEndpointRequest>,
    ) -> ServiceResult<pb::RestartEndpointResponse> {
        let r = request.to_owned_message();
        let call = Call::new("RestartEndpoint", &r.namespace)
            .project(&r.project_id)
            .target("endpoint_id", &r.endpoint_id)
            .key(&r.idempotency_key);
        let op = self
            .gate
            .mutate(&ctx, call, |caller| async move {
                let action = EndpointAction {
                    namespace: r.namespace,
                    project_id: r.project_id,
                    endpoint_id: r.endpoint_id,
                    idempotency_key: r.idempotency_key,
                };
                let op = if r.promote {
                    self.service.promote_endpoint(&caller, action).await?
                } else {
                    self.service.restart_endpoint(&caller, action).await?
                };
                let made = Made::operation(&op.id);
                Ok((op, made))
            })
            .await?;
        Response::ok(pb::RestartEndpointResponse {
            operation: MessageField::some(msg::operation(&op)),
            ..Default::default()
        })
    }

    async fn delete_endpoint(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::DeleteEndpointRequest>,
    ) -> ServiceResult<pb::DeleteEndpointResponse> {
        let r = request.to_owned_message();
        let call = Call::new("DeleteEndpoint", &r.namespace)
            .project(&r.project_id)
            .target("endpoint_id", &r.endpoint_id)
            .key(&r.idempotency_key);
        let op = self
            .gate
            .mutate(&ctx, call, |caller| async move {
                let req = DeleteEndpoint {
                    namespace: r.namespace,
                    project_id: r.project_id,
                    endpoint_id: r.endpoint_id,
                    expected_version: r.expected_version,
                    idempotency_key: r.idempotency_key,
                };
                let op = self.service.delete_endpoint(&caller, req).await?;
                let made = Made::operation(&op.id);
                Ok((op, made))
            })
            .await?;
        Response::ok(pb::DeleteEndpointResponse {
            operation: MessageField::some(msg::operation(&op)),
            ..Default::default()
        })
    }

    async fn create_role(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::CreateRoleRequest>,
    ) -> ServiceResult<pb::CreateRoleResponse> {
        let r = request.to_owned_message();
        let call = Call::new("CreateRole", &r.namespace)
            .project(&r.project_id)
            .target("branch_id", &r.branch_id)
            .target("name", &r.name)
            .key(&r.idempotency_key);
        let issued = self
            .gate
            .mutate(&ctx, call, |caller| async move {
                plain(
                    self.service
                        .create_role(
                            &caller,
                            CreateRole {
                                namespace: r.namespace,
                                project_id: r.project_id,
                                branch_id: r.branch_id,
                                name: r.name,
                                idempotency_key: r.idempotency_key,
                            },
                        )
                        .await,
                )
            })
            .await?;
        Response::ok(pb::CreateRoleResponse {
            role: MessageField::some(msg::role(&issued.role.record)),
            password: issued.password.expose().clone(),
            ..Default::default()
        })
    }

    async fn list_roles(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::ListRolesRequest>,
    ) -> ServiceResult<pb::ListRolesResponse> {
        let call = Call::new("ListRoles", request.namespace)
            .project(request.project_id)
            .target("branch_id", request.branch_id);
        let (roles, next) = self
            .gate
            .read(&ctx, call, || async {
                Ok(self
                    .service
                    .list_roles(
                        request.namespace,
                        request.project_id,
                        request.branch_id,
                        request.page_size,
                        request.page_token,
                    )
                    .await?)
            })
            .await?;
        Response::ok(pb::ListRolesResponse {
            roles: roles.iter().map(|r| msg::role(&r.record)).collect(),
            next_page_token: next,
            ..Default::default()
        })
    }

    async fn reset_role_password(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::ResetRolePasswordRequest>,
    ) -> ServiceResult<pb::ResetRolePasswordResponse> {
        let r = request.to_owned_message();
        let call = Call::new("ResetRolePassword", &r.namespace)
            .project(&r.project_id)
            .target("branch_id", &r.branch_id)
            .target("name", &r.name)
            .key(&r.idempotency_key);
        let issued = self
            .gate
            .mutate(&ctx, call, |caller| async move {
                plain(
                    self.service
                        .reset_role_password(
                            &caller,
                            ResetRolePassword {
                                namespace: r.namespace,
                                project_id: r.project_id,
                                branch_id: r.branch_id,
                                name: r.name,
                                idempotency_key: r.idempotency_key,
                            },
                        )
                        .await,
                )
            })
            .await?;
        Response::ok(pb::ResetRolePasswordResponse {
            role: MessageField::some(msg::role(&issued.role.record)),
            password: issued.password.expose().clone(),
            ..Default::default()
        })
    }

    async fn delete_role(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::DeleteRoleRequest>,
    ) -> ServiceResult<pb::DeleteRoleResponse> {
        let r = request.to_owned_message();
        let call = Call::new("DeleteRole", &r.namespace)
            .project(&r.project_id)
            .target("branch_id", &r.branch_id)
            .target("name", &r.name)
            .key(&r.idempotency_key);
        self.gate
            .mutate(&ctx, call, |caller| async move {
                plain(
                    self.service
                        .delete_role(
                            &caller,
                            DeleteRole {
                                namespace: r.namespace,
                                project_id: r.project_id,
                                branch_id: r.branch_id,
                                name: r.name,
                                idempotency_key: r.idempotency_key,
                            },
                        )
                        .await,
                )
            })
            .await?;
        Response::ok(pb::DeleteRoleResponse::default())
    }

    async fn create_database(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::CreateDatabaseRequest>,
    ) -> ServiceResult<pb::CreateDatabaseResponse> {
        let r = request.to_owned_message();
        let call = Call::new("CreateDatabase", &r.namespace)
            .project(&r.project_id)
            .target("branch_id", &r.branch_id)
            .target("name", &r.name)
            .target("owner_role", &r.owner_role)
            .key(&r.idempotency_key);
        let d = self
            .gate
            .mutate(&ctx, call, |caller| async move {
                plain(
                    self.service
                        .create_database(
                            &caller,
                            CreateDatabase {
                                namespace: r.namespace,
                                project_id: r.project_id,
                                branch_id: r.branch_id,
                                name: r.name,
                                owner_role: r.owner_role,
                                idempotency_key: r.idempotency_key,
                            },
                        )
                        .await,
                )
            })
            .await?;
        Response::ok(pb::CreateDatabaseResponse {
            database: MessageField::some(msg::database(&d.record)),
            ..Default::default()
        })
    }

    async fn list_databases(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::ListDatabasesRequest>,
    ) -> ServiceResult<pb::ListDatabasesResponse> {
        let call = Call::new("ListDatabases", request.namespace)
            .project(request.project_id)
            .target("branch_id", request.branch_id);
        let (databases, next) = self
            .gate
            .read(&ctx, call, || async {
                Ok(self
                    .service
                    .list_databases(
                        request.namespace,
                        request.project_id,
                        request.branch_id,
                        request.page_size,
                        request.page_token,
                    )
                    .await?)
            })
            .await?;
        Response::ok(pb::ListDatabasesResponse {
            databases: databases.iter().map(|d| msg::database(&d.record)).collect(),
            next_page_token: next,
            ..Default::default()
        })
    }

    async fn delete_database(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::DeleteDatabaseRequest>,
    ) -> ServiceResult<pb::DeleteDatabaseResponse> {
        let r = request.to_owned_message();
        let call = Call::new("DeleteDatabase", &r.namespace)
            .project(&r.project_id)
            .target("branch_id", &r.branch_id)
            .target("name", &r.name)
            .key(&r.idempotency_key);
        self.gate
            .mutate(&ctx, call, |caller| async move {
                plain(
                    self.service
                        .delete_database(
                            &caller,
                            DeleteDatabase {
                                namespace: r.namespace,
                                project_id: r.project_id,
                                branch_id: r.branch_id,
                                name: r.name,
                                idempotency_key: r.idempotency_key,
                            },
                        )
                        .await,
                )
            })
            .await?;
        Response::ok(pb::DeleteDatabaseResponse::default())
    }

    async fn get_connection_info(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::GetConnectionInfoRequest>,
    ) -> ServiceResult<pb::GetConnectionInfoResponse> {
        let call = Call::new("GetConnectionInfo", request.namespace)
            .project(request.project_id)
            .target("branch_id", request.branch_id);
        let info = self
            .gate
            .read(&ctx, call, || async {
                Ok(self
                    .service
                    .connection_info(GetConnectionInfo {
                        namespace: request.namespace.to_owned(),
                        project_id: request.project_id.to_owned(),
                        branch_id: request.branch_id.to_owned(),
                        endpoint_id: request.endpoint_id.to_owned(),
                        role: request.role.to_owned(),
                        database: request.database.to_owned(),
                        read_only: request.read_only,
                    })
                    .await?)
            })
            .await?;
        Response::ok(pb::GetConnectionInfoResponse {
            connection: MessageField::some(msg::connection_info(info)),
            ..Default::default()
        })
    }

    async fn issue_connect_credential(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::IssueConnectCredentialRequest>,
    ) -> ServiceResult<pb::IssueConnectCredentialResponse> {
        let call = Call::new("IssueConnectCredential", request.namespace)
            .project(request.project_id)
            .key(request.idempotency_key);
        self.gate
            .mutate(&ctx, call, |_| async {
                Err(Failure::NotBuilt("PG2 Task 25"))
            })
            .await
    }

    async fn watch_project(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::WatchProjectRequest>,
    ) -> ServiceResult<ServiceStream<pb::WatchProjectResponse>> {
        let call = Call::new("WatchProject", request.namespace).project(request.project_id);
        let items = self
            .gate
            .read(&ctx, call, || async {
                Ok(self
                    .service
                    .watch_project(
                        request.namespace,
                        request.project_id,
                        request.resume_cursor,
                        WatchOptions::default(),
                    )
                    .await?)
            })
            .await?;
        let stream: ServiceStream<pb::WatchProjectResponse> =
            Box::pin(items.map(|item| match item {
                Ok(item) => Ok(msg::watch_item(&item)),
                Err(e) => Err(refused(&e)),
            }));
        Response::ok(stream)
    }
}

/// `loams.operations.v1.OperationsService`'s reads of `pg-control`'s
/// operations; the watch and the cancel are not built.
pub(crate) struct Operations {
    pub service: PgService<NeonClients>,
    pub gate: Arc<Gate>,
}

impl Operations {
    async fn principal(&self, ctx: &RequestContext) -> Result<Principal, ConnectError> {
        self.gate
            .authenticator
            .authenticate(ctx.headers())
            .await
            .map_err(|e| refused(&ServiceError::from(e)))
    }

    /// `viewer` on `object`.
    async fn viewer(&self, principal: &Principal, object: &Object) -> Result<(), ConnectError> {
        authorize(
            self.gate.authorizer.as_ref(),
            principal,
            Relation::Viewer,
            object,
        )
        .await
        .map_err(|e| refused(&e))
    }
}

fn not_built(rpc: &str) -> ConnectError {
    refuse(
        ErrorCode::Unimplemented,
        "not_implemented",
        format!("loams.operations.v1.OperationsService/{rpc} is not built for Loams Postgres yet"),
        &[],
    )
}

impl OperationsService for Operations {
    async fn get_operation(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, ops::GetOperationRequest>,
    ) -> ServiceResult<ops::GetOperationResponse> {
        // Authenticated before the store is read, so an unknown caller
        // learns nothing about which ids exist.
        let principal = self.principal(&ctx).await?;
        let op = self
            .service
            .get_operation(request.operation_id)
            .await
            .map_err(|e| refused(&e))?;
        self.viewer(
            &principal,
            &Object::Project {
                namespace: op.namespace.clone(),
                project_id: op.project_id.clone(),
            },
        )
        .await?;
        Response::ok(ops::GetOperationResponse {
            operation: MessageField::some(msg::operation(&op)),
            ..Default::default()
        })
    }

    async fn list_operations(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, ops::ListOperationsRequest>,
    ) -> ServiceResult<ops::ListOperationsResponse> {
        if request.namespace.is_empty() {
            return Err(refused(&ServiceError::invalid(
                "namespace",
                "Loams Postgres lists one namespace's operations at a time",
            )));
        }
        let principal = self.principal(&ctx).await?;
        self.viewer(&principal, &Object::Namespace(request.namespace.to_owned()))
            .await?;
        let states: Vec<ops::OperationState> =
            request.states.iter().filter_map(|s| s.as_known()).collect();
        let size = i32::try_from(request.page_size).unwrap_or(i32::MAX);
        let (page, next) = self
            .service
            .list_operations(request.namespace, size, request.page_token)
            .await
            .map_err(|e| refused(&e))?;
        Response::ok(ops::ListOperationsResponse {
            operations: page
                .iter()
                .filter(|o| states.is_empty() || states.contains(&msg::operation_state(o.state)))
                .map(msg::operation)
                .collect(),
            next_page_token: next,
            ..Default::default()
        })
    }

    async fn watch_operations(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, ops::WatchOperationsRequest>,
    ) -> ServiceResult<ServiceStream<ops::WatchOperationsResponse>> {
        Err(not_built("WatchOperations"))
    }

    async fn cancel_operation(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, ops::CancelOperationRequest>,
    ) -> ServiceResult<ops::CancelOperationResponse> {
        Err(not_built("CancelOperation"))
    }
}

/// Registers both services on `rpc`.
pub(crate) fn register(
    rpc: connectrpc::Router,
    service: PgService<NeonClients>,
    gate: Arc<Gate>,
) -> connectrpc::Router {
    let rpc = Arc::new(Postgres {
        service: service.clone(),
        gate: gate.clone(),
    })
    .register(rpc);
    Arc::new(Operations { service, gate }).register(rpc)
}
