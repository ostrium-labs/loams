// The generated service trait returns `impl Encodable<_>`; these impls name the
// concrete message type, which is the intended refinement (as the engine's
// `crates/loams/src/api/graph.rs` does).
#![allow(refining_impl_trait)]

//! `loams.house.v1` (design §49 §18, D778; HS1 Task 8): the House's Connect API,
//! served by the front on its HTTP listener beside the ClickHouse HTTP interface
//! ([`crate::http`] hands it every `/loams.house.v1.*` path).
//!
//! # One surface, one set of checks
//!
//! `ExecuteQuery`, `ExplainQuery` and `PreviewTable` run a statement the way the
//! HTTP interface does: the same credentials ([`crate::auth`]: Basic or the
//! `X-ClickHouse-*` headers, which the engine's proxy forwards), the same settings
//! checks, the classifier, the front's own answers and the deny list
//! ([`crate::gate`]), the same pool, and the same worker frames. What differs is
//! the wire: a stream of `header`, `batch`, `progress` and `summary` frames
//! ([`frames`]) instead of a ClickHouse format on an HTTP body, and errors as
//! Connect errors carrying `loams.errors.v1.ErrorInfo` with the ClickHouse code
//! and name in `metadata` ([`to_connect`]).
//!
//! # A result's header
//!
//! The header comes first, with ClickHouse's type names. `JSON_ROWS` reads run as
//! `JSONCompactEachRowWithNamesAndTypes`, whose first two lines are the header.
//! chDB's `ArrowStream` carries no ClickHouse types, so an `ARROW_IPC` query is
//! described first on the same worker (`DESCRIBE TABLE (<query>)`, which analyses
//! it without reading its rows, though the analyser does evaluate constant scalar
//! subqueries), and any other read (`SHOW`, `EXISTS`, `DESCRIBE`, `EXPLAIN`) is
//! run once for its names and types lines (`max_result_rows = 1`). Both steps are
//! cancellable and under the statement's deadline.
//!
//! # Caps
//!
//! `max_rows` is held exactly ([`frames`]): the front cuts the result and says
//! `truncated`, and chDB is asked to stop past it (`max_result_rows` with
//! `result_overflow_mode = 'break'`, which stops the outermost result only:
//! measured, subqueries are unaffected). What chDB still sends after the cut is
//! drained, up to [`DRAIN_BUDGET`], and the worker is stopped past that.

pub mod catalog;
pub mod frames;
mod run;

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use connectrpc::{
    ConnectError, ErrorCode, ErrorDetail, RequestContext, Response, Router, ServiceRequest,
    ServiceResult, ServiceStream,
};
use loams_house_proto::loams::errors::v1::ErrorInfo;
use loams_house_proto::loams::house::v1 as pb;
use loams_house_proto::loams::house::v1::{HouseService, HouseServiceExt};
use loams_house_proto::loams::operations::v1::{Operation, OperationState};

pub use catalog::{ApiCatalog, FixtureCatalog, LakeCatalog};

use crate::catalog::Lake;
use frames::Caps;
use run::{Output, Statement, sorted};

use crate::admission::WorkerPool;
use crate::auth;
use crate::config::{HouseConfig, UserMap};
use crate::errors::{ChError, HouseError, status_for};

/// `max_rows` when the request leaves it 0.
pub const DEFAULT_MAX_ROWS: u32 = 1_000;
/// The most `max_rows` may be.
pub const MAX_ROWS: u32 = 100_000;
/// `max_bytes` when the request leaves it 0.
pub const DEFAULT_MAX_BYTES: u64 = 64 * 1024 * 1024;
/// The most `max_bytes` may be.
pub const MAX_BYTES: u64 = 1024 * 1024 * 1024;
/// The most `timeout_ms` may be, and the limit when it is 0: §49 §12's
/// `max_execution_time` cap.
pub const MAX_TIMEOUT_MS: u32 = 300_000;
/// How long past its limit a statement's worker is killed (§49 §12).
pub const KILL_GRACE: Duration = Duration::from_secs(2);
/// `PreviewTable`'s `limit` when the request leaves it 0.
pub const DEFAULT_PREVIEW_ROWS: u32 = 100;
/// The most rows `PreviewTable` returns.
pub const MAX_PREVIEW_ROWS: u32 = 1_000;
/// AIP-158: a page when the request leaves `page_size` 0, and the most.
pub const DEFAULT_PAGE_SIZE: usize = 100;
/// The most items a page holds.
pub const MAX_PAGE_SIZE: usize = 1_000;
/// What a truncated statement may still send before its worker is stopped.
pub const DRAIN_BUDGET: u64 = 16 * 1024 * 1024;
/// The most an `ExplainQuery` answer, or a header pre-step, may hold.
pub const EXPLAIN_MAX_BYTES: usize = 8 * 1024 * 1024;
/// The longest `query_id` the API accepts.
pub const MAX_QUERY_ID_BYTES: usize = 256;

/// `loams.house.v1.HouseService` over the front's pool.
#[derive(Debug)]
pub struct HouseApi {
    config: HouseConfig,
    pool: WorkerPool,
    catalog: Arc<dyn ApiCatalog>,
    /// The catalog of lake tables (HS1 Task 10), when the House has one.
    lake: Option<Arc<Lake>>,
    governance: Governance,
}

/// What the API shares with the front's HTTP interface (HS1 Task 21): its
/// admission and its process list.
#[derive(Debug, Clone)]
pub struct Governance {
    /// Namespace queues and node memory.
    pub admission: Arc<crate::admission::Admission>,
    /// Running statements, both surfaces.
    pub processes: Arc<crate::watchdog::Processes>,
    /// Every statement's end, and the query log (HS1 Task 23).
    pub observer: Arc<crate::querylog::Observer>,
}

impl HouseApi {
    /// The service over `pool`, with `config`'s users and limits, reading metadata
    /// from `catalog`; DDL goes to `lake`, when there is one.
    pub fn new(
        config: HouseConfig,
        pool: WorkerPool,
        catalog: Arc<dyn ApiCatalog>,
        lake: Option<Arc<Lake>>,
        governance: Governance,
    ) -> Arc<Self> {
        Arc::new(Self {
            config,
            pool,
            catalog,
            lake,
            governance,
        })
    }

    /// A Connect router serving this service.
    pub fn router(self: Arc<Self>) -> Router {
        HouseServiceExt::register(self, Router::new())
    }

    /// The caller, who must hold the `read` scope (the browsing RPCs).
    fn reader(&self, ctx: &RequestContext) -> Result<UserMap, ConnectError> {
        let user = self.caller(ctx)?;
        if !user.scopes.read {
            let err = crate::identity::lacks(&user, crate::identity::Scope::Read, "browsing");
            return Err(to_connect(&err));
        }
        Ok(user)
    }

    /// The caller, from the request's credentials.
    fn caller(&self, ctx: &RequestContext) -> Result<UserMap, ConnectError> {
        let headers: Vec<(String, String)> = ctx
            .headers()
            .iter()
            .filter_map(|(name, value)| {
                value
                    .to_str()
                    .ok()
                    .map(|v| (name.as_str().to_string(), v.to_string()))
            })
            .collect();
        auth::identify(&self.config, &headers, &[]).map_err(|err| {
            // A refused sign-in is audited too, with what it claimed (HS1 Task 20).
            if let Some(sink) = &self.config.audit {
                let mut event = crate::audit::AuditEvent::now("api", "");
                event.user = auth::credentials(&headers, &[])
                    .map(|c| c.user)
                    .unwrap_or_default();
                event.principal = headers
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(crate::identity::PRINCIPAL_HEADER))
                    .map(|(_, v)| v.clone());
                event.code = err.code();
                sink.record(&event);
            }
            to_connect(&err)
        })
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// The Connect code and `reason` a ClickHouse code answers as (§49 §18.2; the
/// registry is `docs/api/reasons.md`).
pub fn reason_for(code: i32) -> (ErrorCode, &'static str) {
    match code {
        62 => (ErrorCode::InvalidArgument, "house_syntax_error"),
        60 => (ErrorCode::NotFound, "house_unknown_table"),
        81 => (ErrorCode::NotFound, "house_unknown_database"),
        497 => (ErrorCode::PermissionDenied, "house_access_denied"),
        516 => (ErrorCode::Unauthenticated, "unauthenticated"),
        159 => (ErrorCode::DeadlineExceeded, "house_timeout"),
        241 => (ErrorCode::ResourceExhausted, "house_memory_limit"),
        202 => (ErrorCode::ResourceExhausted, "house_too_many_queries"),
        344 => (ErrorCode::PermissionDenied, "house_disabled_function"),
        394 => (ErrorCode::Canceled, "house_cancelled"),
        48 => (ErrorCode::Unimplemented, "not_implemented"),
        210 => (ErrorCode::Unavailable, "unavailable"),
        // The front's own refusals of a request's arguments and settings.
        36 | 115 | 164 | 452 => (ErrorCode::InvalidArgument, "invalid_argument"),
        // Any other error the engine raised for the statement: ClickHouse's HTTP
        // status says whether it is the statement's fault (4xx); otherwise the
        // engine gave no more than its code, which is what `unknown` is for.
        _ => match status_for(code) {
            400..=499 => (ErrorCode::InvalidArgument, "invalid_argument"),
            _ => (ErrorCode::Unknown, "house_query_failed"),
        },
    }
}

/// A Connect error with one `ErrorInfo`.
fn refuse(
    code: ErrorCode,
    reason: &str,
    message: impl Into<String>,
    metadata: &[(&str, String)],
) -> ConnectError {
    let info = ErrorInfo {
        reason: reason.to_string(),
        metadata: metadata
            .iter()
            .map(|(key, value)| ((*key).to_string(), value.clone()))
            .collect(),
        ..Default::default()
    };
    ConnectError::new(code, message).with_detail(ErrorDetail::from_message(
        "loams.errors.v1.ErrorInfo",
        &info,
    ))
}

fn clickhouse_metadata(err: &HouseError) -> [(&'static str, String); 2] {
    [
        ("clickhouse_code", err.code().to_string()),
        ("clickhouse_name", err.name().to_string()),
    ]
}

/// A House error as a Connect error: [`reason_for`] its code, the message, and
/// `clickhouse_code` and `clickhouse_name` in `metadata`.
///
/// A namespace quota's refusal (HS1 Task 22, `202` or `164`) is
/// `resource_exhausted`, `house_quota_exceeded`, with the `quota` and its
/// `limit` in `metadata` too.
pub fn to_connect(err: &HouseError) -> ConnectError {
    if let Some((quota, limit)) = crate::quota::refusal_parts(err.message()) {
        let [code, name] = clickhouse_metadata(err);
        return refuse(
            ErrorCode::ResourceExhausted,
            crate::quota::QUOTA_EXCEEDED,
            err.message(),
            &[
                code,
                name,
                ("quota", quota.to_string()),
                ("limit", limit.to_string()),
            ],
        );
    }
    let (code, reason) = reason_for(err.code());
    refuse(code, reason, err.message(), &clickhouse_metadata(err))
}

/// The refusal of a statement that would write when the request is read-only:
/// `failed_precondition`, `house_read_only`, ClickHouse's `164 READONLY`.
fn read_only_refusal() -> ConnectError {
    let err = HouseError::from(ChError::readonly(
        "Cannot execute this statement with read_only set: it writes or changes \
         state. Run it again with read_only = false",
    ));
    refuse(
        ErrorCode::FailedPrecondition,
        "house_read_only",
        err.message(),
        &clickhouse_metadata(&err),
    )
}

/// A read-only user's refusal: `permission_denied`, `house_access_denied`, `164`.
fn read_only_user_refusal(user: &str) -> ConnectError {
    let err = HouseError::from(ChError::readonly(format!(
        "{user}: Cannot execute query in readonly mode: the user may only read"
    )));
    refuse(
        ErrorCode::PermissionDenied,
        "house_access_denied",
        err.message(),
        &clickhouse_metadata(&err),
    )
}

fn invalid(field: &str, message: impl Into<String>) -> ConnectError {
    refuse(
        ErrorCode::InvalidArgument,
        "invalid_argument",
        message,
        &[("field", field.to_string())],
    )
}

fn not_yet(what: &str, task: &str) -> ConnectError {
    refuse(
        ErrorCode::Unimplemented,
        "not_implemented",
        format!("{what} is served from {task}; this House does not serve it yet"),
        &[("task", task.to_string())],
    )
}

// ---------------------------------------------------------------------------
// Requests
// ---------------------------------------------------------------------------

fn output_of(format: buffa::EnumValue<pb::ResultFormat>) -> Result<Output, ConnectError> {
    match format.as_known() {
        Some(
            pb::ResultFormat::RESULT_FORMAT_UNSPECIFIED | pb::ResultFormat::RESULT_FORMAT_ARROW_IPC,
        ) => Ok(Output::Arrow),
        Some(pb::ResultFormat::RESULT_FORMAT_JSON_ROWS) => Ok(Output::Json),
        None => Err(invalid("format", "unknown result format")),
    }
}

fn query_id_of(given: &str) -> Result<String, ConnectError> {
    if given.is_empty() {
        return Ok(uuid::Uuid::new_v4().to_string());
    }
    if given.len() > MAX_QUERY_ID_BYTES || given.chars().any(char::is_control) {
        return Err(invalid(
            "query_id",
            format!(
                "query_id must be at most {MAX_QUERY_ID_BYTES} bytes, without control characters"
            ),
        ));
    }
    Ok(given.to_string())
}

impl HouseApi {
    fn execute_statement(
        &self,
        request: &pb::ExecuteQueryRequest,
    ) -> Result<Statement, ConnectError> {
        let max_rows = match request.max_rows {
            0 => DEFAULT_MAX_ROWS,
            n if n > MAX_ROWS => {
                return Err(invalid(
                    "max_rows",
                    format!("max_rows is at most {MAX_ROWS}"),
                ));
            }
            n => n,
        };
        let max_bytes = match request.max_bytes {
            0 => DEFAULT_MAX_BYTES,
            n if n > MAX_BYTES => {
                return Err(invalid(
                    "max_bytes",
                    format!("max_bytes is at most {MAX_BYTES}"),
                ));
            }
            n => n,
        };
        if request.timeout_ms > MAX_TIMEOUT_MS {
            return Err(invalid(
                "timeout_ms",
                format!("timeout_ms is at most {MAX_TIMEOUT_MS}"),
            ));
        }
        Ok(Statement {
            sql: request.sql.clone(),
            database: request.database.clone(),
            settings: sorted(&request.settings),
            params: sorted(&request.params),
            output: output_of(request.format)?,
            caps: Caps {
                max_rows: u64::from(max_rows),
                max_bytes,
            },
            timeout_ms: request.timeout_ms,
            query_id: query_id_of(&request.query_id)?,
            read_only: request.read_only.unwrap_or(true),
            explain: None,
        })
    }
}

/// A ClickHouse identifier in backquotes, escaped.
fn quote_identifier(field: &str, name: &str) -> Result<String, ConnectError> {
    if name.is_empty() || name.chars().any(char::is_control) {
        return Err(invalid(
            field,
            format!("{field} must be a name, without control characters"),
        ));
    }
    Ok(format!(
        "`{}`",
        name.replace('\\', "\\\\").replace('`', "\\`")
    ))
}

fn database_or_default(database: &str) -> &str {
    if database.is_empty() {
        "default"
    } else {
        database
    }
}

/// A query log row as the API shows it.
fn record_of(e: &crate::querylog::Entry) -> pb::QueryRecord {
    pb::QueryRecord {
        query_id: e.query_id.clone(),
        principal: e.user.clone(),
        started_at: buffa::MessageField::some(loams_house_proto::google::protobuf::Timestamp {
            seconds: e.started_us.div_euclid(1_000_000),
            nanos: i32::try_from(e.started_us.rem_euclid(1_000_000) * 1000).unwrap_or(0),
            ..Default::default()
        }),
        duration_ms: e.duration_ms(),
        status: match e.code {
            0 => pb::QueryStatus::QUERY_STATUS_FINISHED,
            crate::querylog::CANCELLED => pb::QueryStatus::QUERY_STATUS_CANCELLED,
            _ => pb::QueryStatus::QUERY_STATUS_FAILED,
        }
        .into(),
        error_code: e.code,
        query: e.query.clone(),
        query_truncated: false,
        read_rows: e.read_rows,
        read_bytes: e.read_bytes,
        result_rows: e.result_rows,
        error_message: e.exception.clone(),
        ..Default::default()
    }
}

/// AIP-158 paging over a whole list: an opaque token for the next offset.
fn page<T>(items: Vec<T>, page_size: i32, token: &str) -> Result<(Vec<T>, String), ConnectError> {
    let size = match usize::try_from(page_size) {
        Err(_) => return Err(invalid("page_size", "page_size must not be negative")),
        Ok(0) => DEFAULT_PAGE_SIZE,
        Ok(n) => n.min(MAX_PAGE_SIZE),
    };
    let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let offset = if token.is_empty() {
        0
    } else {
        engine
            .decode(token)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .and_then(|text| {
                text.strip_prefix("o:")
                    .and_then(|n| n.parse::<usize>().ok())
            })
            .ok_or_else(|| invalid("page_token", "page_token is not one this server issued"))?
    };
    let total = items.len();
    let items: Vec<T> = items.into_iter().skip(offset).take(size).collect();
    let next = offset.saturating_add(size);
    let token = if next < total {
        engine.encode(format!("o:{next}"))
    } else {
        String::new()
    };
    Ok((items, token))
}

impl HouseService for HouseApi {
    async fn execute_query(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::ExecuteQueryRequest>,
    ) -> ServiceResult<ServiceStream<pb::ExecuteQueryResponse>> {
        let user = self.caller(&ctx)?;
        let st = self.execute_statement(&request.to_owned_message())?;
        let frames = self.frames(&user, st).await?;
        Response::stream_ok(futures::StreamExt::map(frames, |f| f.map(Into::into)))
    }

    async fn cancel_query(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::CancelQueryRequest>,
    ) -> ServiceResult<pb::CancelQueryResponse> {
        let user = self.caller(&ctx)?;
        let request = request.to_owned_message();
        if request.query_id.is_empty() {
            return Err(invalid("query_id", "query_id is empty"));
        }
        // One process list for both surfaces (HS1 Task 21): the caller's own,
        // or any of its namespace's with `house:admin`.
        match self
            .governance
            .processes
            .kill(&request.query_id, &user, false)
        {
            Ok(_) => {}
            Err(crate::watchdog::KillRefusal::NotFound) => {
                return Err(refuse(
                    ErrorCode::NotFound,
                    "house_query_not_found",
                    format!("No running query has the id {}", request.query_id),
                    &[("query_id", request.query_id.clone())],
                ));
            }
            Err(crate::watchdog::KillRefusal::Denied) => {
                return Err(refuse(
                    ErrorCode::PermissionDenied,
                    "house_access_denied",
                    format!(
                        "Query {} is another user's: cancelling it needs house:admin",
                        request.query_id
                    ),
                    &[("query_id", request.query_id.clone())],
                ));
            }
        }
        Response::ok(pb::CancelQueryResponse::default())
    }

    async fn explain_query(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::ExplainQueryRequest>,
    ) -> ServiceResult<pb::ExplainQueryResponse> {
        let user = self.caller(&ctx)?;
        let request = request.to_owned_message();
        let kind = match request.kind.as_known() {
            Some(
                pb::ExplainKind::EXPLAIN_KIND_UNSPECIFIED | pb::ExplainKind::EXPLAIN_KIND_PLAN,
            ) => "PLAN",
            Some(pb::ExplainKind::EXPLAIN_KIND_PIPELINE) => "PIPELINE",
            Some(pb::ExplainKind::EXPLAIN_KIND_SYNTAX) => "SYNTAX",
            Some(pb::ExplainKind::EXPLAIN_KIND_ESTIMATE) => "ESTIMATE",
            None => return Err(invalid("kind", "unknown explain kind")),
        };
        let st = Statement {
            sql: request.sql,
            database: request.database,
            settings: sorted(&request.settings),
            params: sorted(&request.params),
            output: Output::Collect,
            caps: Caps {
                max_rows: u64::MAX,
                max_bytes: EXPLAIN_MAX_BYTES as u64,
            },
            timeout_ms: 0,
            query_id: uuid::Uuid::new_v4().to_string(),
            read_only: true,
            explain: Some(kind),
        };
        let out = self.collect(&user, st).await?;
        let text = String::from_utf8_lossy(&out);
        Response::ok(pb::ExplainQueryResponse {
            text: text.trim_end_matches('\n').to_string(),
            ..Default::default()
        })
    }

    async fn list_databases(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::ListDatabasesRequest>,
    ) -> ServiceResult<pb::ListDatabasesResponse> {
        let user = self.reader(&ctx)?;
        let request = request.to_owned_message();
        let databases: Vec<pb::Database> = self
            .catalog
            .databases(user.namespace)
            .await
            .map_err(|err| to_connect(&err))?
            .into_iter()
            .filter(|d| {
                user.grants
                    .as_ref()
                    .is_none_or(|g| g.mentions_database(&d.name))
            })
            .collect();
        let (databases, next_page_token) = page(databases, request.page_size, &request.page_token)?;
        Response::ok(pb::ListDatabasesResponse {
            databases,
            next_page_token,
            ..Default::default()
        })
    }

    async fn list_tables(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::ListTablesRequest>,
    ) -> ServiceResult<pb::ListTablesResponse> {
        let user = self.reader(&ctx)?;
        let request = request.to_owned_message();
        let filter = request.filter.to_lowercase();
        let database = database_or_default(&request.database);
        let tables: Vec<pb::Table> = self
            .catalog
            .tables(user.namespace, database)
            .await
            .map_err(|err| to_connect(&err))?
            .into_iter()
            .filter(|t| filter.is_empty() || t.name.to_lowercase().contains(&filter))
            // A table outside the caller's grants is not listed (HS1 Task 20).
            .filter(|t| crate::identity::granted(&user, database, &t.name))
            .collect();
        let (tables, next_page_token) = page(tables, request.page_size, &request.page_token)?;
        Response::ok(pb::ListTablesResponse {
            tables,
            next_page_token,
            ..Default::default()
        })
    }

    async fn describe_table(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::DescribeTableRequest>,
    ) -> ServiceResult<pb::DescribeTableResponse> {
        let user = self.reader(&ctx)?;
        let request = request.to_owned_message();
        if request.table.is_empty() {
            return Err(invalid("table", "table is empty"));
        }
        // A table outside the caller's grants is as unknown as a missing one.
        if !crate::identity::granted(
            &user,
            database_or_default(&request.database),
            &request.table,
        ) {
            return Err(to_connect(&HouseError::from(ChError::unknown_table(
                format!(
                    "Table {}.{} does not exist",
                    database_or_default(&request.database),
                    request.table
                ),
            ))));
        }
        let described = self
            .catalog
            .describe(
                user.namespace,
                database_or_default(&request.database),
                &request.table,
            )
            .await
            .map_err(|err| to_connect(&err))?;
        Response::ok(described)
    }

    async fn preview_table(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::PreviewTableRequest>,
    ) -> ServiceResult<ServiceStream<pb::PreviewTableResponse>> {
        let user = self.caller(&ctx)?;
        let request = request.to_owned_message();
        let limit = match request.limit {
            0 => DEFAULT_PREVIEW_ROWS,
            n if n > MAX_PREVIEW_ROWS => {
                return Err(invalid(
                    "limit",
                    format!("limit is at most {MAX_PREVIEW_ROWS}"),
                ));
            }
            n => n,
        };
        let database = quote_identifier("database", database_or_default(&request.database))?;
        let table = quote_identifier("table", &request.table)?;
        let st = Statement {
            sql: format!("SELECT * FROM {database}.{table} LIMIT {limit}"),
            database: String::new(),
            settings: Vec::new(),
            params: Vec::new(),
            output: output_of(request.format)?,
            caps: Caps {
                max_rows: u64::from(limit),
                max_bytes: DEFAULT_MAX_BYTES,
            },
            timeout_ms: 0,
            query_id: uuid::Uuid::new_v4().to_string(),
            read_only: true,
            explain: None,
        };
        let frames = self.frames(&user, st).await?;
        Response::stream_ok(futures::StreamExt::map(frames, |f| f.map(Into::into)))
    }

    async fn list_query_history(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::ListQueryHistoryRequest>,
    ) -> ServiceResult<pb::ListQueryHistoryResponse> {
        let user = self.caller(&ctx)?;
        let request = request.to_owned_message();
        let namespace_wide =
            request.scope.as_known() == Some(pb::HistoryScope::HISTORY_SCOPE_NAMESPACE);
        // The namespace's whole history needs `house:admin` (§49 §18).
        if namespace_wide && !user.scopes.admin {
            return Err(refuse(
                ErrorCode::PermissionDenied,
                "house_access_denied",
                "scope NAMESPACE needs the house:admin scope",
                &[],
            ));
        }
        let status = match request.status.as_known() {
            None | Some(pb::QueryStatus::QUERY_STATUS_UNSPECIFIED) => None,
            Some(pb::QueryStatus::QUERY_STATUS_FINISHED) => {
                Some(crate::querylog::StatusFilter::Finished)
            }
            Some(pb::QueryStatus::QUERY_STATUS_FAILED) => {
                Some(crate::querylog::StatusFilter::Failed)
            }
            Some(pb::QueryStatus::QUERY_STATUS_CANCELLED) => {
                Some(crate::querylog::StatusFilter::Cancelled)
            }
            // The log holds ended statements only (ruling R23.9).
            Some(pb::QueryStatus::QUERY_STATUS_RUNNING) => {
                return Response::ok(pb::ListQueryHistoryResponse::default());
            }
        };
        let since = request.since.as_option().cloned();
        let log: Vec<pb::QueryRecord> = match &self.governance.observer.log {
            // The namespace's `_house.query_log` (HS1 Task 23).
            Some(log) => {
                let filter = crate::querylog::Filter {
                    user: (!namespace_wide).then(|| user.user.clone()),
                    since_us: since
                        .as_ref()
                        .map(|t| t.seconds.saturating_mul(1_000_000) + i64::from(t.nanos) / 1000),
                    status,
                    text: (!request.text_contains.is_empty())
                        .then(|| request.text_contains.clone()),
                    ..crate::querylog::Filter::default()
                };
                log.read(&self.pool, user.namespace, &filter)
                    .await
                    .map_err(|err| to_connect(&err))?
                    .iter()
                    .map(record_of)
                    .collect()
            }
            // A House without a catalog has no query log: its fixtures.
            None => {
                let Some(log) = self.catalog.query_log(user.namespace) else {
                    return Err(not_yet("The query log", catalog::QUERY_LOG_TASK));
                };
                let text = request.text_contains.to_lowercase();
                let wanted = request.status.as_known();
                log.into_iter()
                    .filter(|q| namespace_wide || q.principal == user.user)
                    .filter(|q| match wanted {
                        None | Some(pb::QueryStatus::QUERY_STATUS_UNSPECIFIED) => true,
                        Some(wanted) => q.status.as_known() == Some(wanted),
                    })
                    .filter(|q| match (&since, q.started_at.as_option()) {
                        (Some(since), Some(at)) => {
                            (at.seconds, at.nanos) >= (since.seconds, since.nanos)
                        }
                        (Some(_), None) => false,
                        (None, _) => true,
                    })
                    .filter(|q| text.is_empty() || q.query.to_lowercase().contains(&text))
                    .collect()
            }
        };
        let rows: Vec<pb::QueryRecord> = log
            .into_iter()
            .map(|mut q| {
                if q.query.len() > 4096 {
                    let mut end = 4096;
                    while !q.query.is_char_boundary(end) {
                        end -= 1;
                    }
                    q.query.truncate(end);
                    q.query_truncated = true;
                }
                q
            })
            .collect();
        let (queries, next_page_token) = page(rows, request.page_size, &request.page_token)?;
        Response::ok(pb::ListQueryHistoryResponse {
            queries,
            next_page_token,
            ..Default::default()
        })
    }

    async fn get_query(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::GetQueryRequest>,
    ) -> ServiceResult<pb::GetQueryResponse> {
        let user = self.caller(&ctx)?;
        let request = request.to_owned_message();
        let not_found = || {
            refuse(
                ErrorCode::NotFound,
                "house_query_not_found",
                format!("No query of yours has the id {}", request.query_id),
                &[("query_id", request.query_id.clone())],
            )
        };
        if request.query_id.is_empty() {
            return Err(invalid("query_id", "query_id is empty"));
        }
        let found = match &self.governance.observer.log {
            // Another user's statement only with `house:admin`.
            Some(log) => {
                let filter = crate::querylog::Filter {
                    user: (!user.scopes.admin).then(|| user.user.clone()),
                    query_id: Some(request.query_id.clone()),
                    ..crate::querylog::Filter::default()
                };
                log.read(&self.pool, user.namespace, &filter)
                    .await
                    .map_err(|err| to_connect(&err))?
                    .first()
                    .map(record_of)
            }
            None => {
                let Some(log) = self.catalog.query_log(user.namespace) else {
                    return Err(not_yet("The query log", catalog::QUERY_LOG_TASK));
                };
                log.into_iter().find(|q| {
                    q.query_id == request.query_id
                        && (q.principal == user.user || user.scopes.admin)
                })
            }
        };
        let found = found.ok_or_else(not_found)?;
        Response::ok(pb::GetQueryResponse {
            query: buffa::MessageField::some(found),
            ..Default::default()
        })
    }

    async fn list_pipes(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::ListPipesRequest>,
    ) -> ServiceResult<pb::ListPipesResponse> {
        let user = self.caller(&ctx)?;
        let request = request.to_owned_message();
        let pipes = self
            .catalog
            .pipes(user.namespace)
            .await
            .map_err(|err| to_connect(&err))?;
        let Some(pipes) = pipes else {
            return Err(refuse(
                ErrorCode::Unimplemented,
                "not_implemented",
                "Pipes need lake tables: this House was started without a catalog",
                &[],
            ));
        };
        // Only the views the identity may touch (HS1 Task 20).
        let pipes: Vec<pb::Pipe> = pipes
            .into_iter()
            .filter(|p| match p.name.split_once('.') {
                Some((database, name)) => crate::identity::granted(&user, database, name),
                None => true,
            })
            .collect();
        let (pipes, next_page_token) = page(pipes, request.page_size, &request.page_token)?;
        Response::ok(pb::ListPipesResponse {
            pipes,
            next_page_token,
            ..Default::default()
        })
    }

    async fn restore_table(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::RestoreTableRequest>,
    ) -> ServiceResult<Operation> {
        let user = self.caller(&ctx)?;
        let request = request.to_owned_message();
        let Some(lake) = &self.lake else {
            return Err(refuse(
                ErrorCode::Unimplemented,
                "not_implemented",
                "RestoreTable restores lake tables: this House was started without a catalog",
                &[],
            ));
        };
        let database = database_or_default(&request.database).to_string();
        let result = self.restore(lake, &user, &database, &request).await;
        if let Some(sink) = &self.config.audit {
            let mut event = crate::audit::AuditEvent::now("api", "");
            event.user = user.user.clone();
            event.principal = user.principal.clone();
            event.namespace = Some(user.namespace);
            event.statement = Some("RestoreTable".to_string());
            event.code = result.as_ref().map_or_else(HouseError::code, |_| 0);
            sink.record(&event);
        }
        let restored = result.map_err(|err| to_connect(&err))?;
        let at = catalog::timestamp(restored.at_ms);
        let mut fields = std::collections::HashMap::new();
        fields.insert(
            "snapshot_id".to_string(),
            loams_house_proto::google::protobuf::Value::from(restored.snapshot_id.to_string()),
        );
        Response::ok(Operation {
            id: restored.operation_id,
            kind: RESTORE_KIND.to_string(),
            namespace: user.namespace.to_string(),
            target: [
                ("database".to_string(), database),
                ("table".to_string(), request.table.clone()),
            ]
            .into_iter()
            .collect(),
            state: OperationState::OPERATION_STATE_SUCCEEDED.into(),
            created_at: buffa::MessageField::some(at.clone()),
            updated_at: buffa::MessageField::some(at),
            result: buffa::MessageField::some(loams_house_proto::google::protobuf::Struct {
                fields: fields.into_iter().collect(),
                ..Default::default()
            }),
            ..Default::default()
        })
    }
}

/// `RestoreTable`'s operation kind.
pub const RESTORE_KIND: &str = "house.restore_table";

impl HouseApi {
    /// `RestoreTable` for `user` (HS1 Task 15): the `admin` scope, a grant on
    /// the table, then [`crate::retention::restore_table`].
    async fn restore(
        &self,
        lake: &Lake,
        user: &UserMap,
        database: &str,
        request: &pb::RestoreTableRequest,
    ) -> Result<crate::retention::Restored, HouseError> {
        use crate::retention::RestorePoint;
        use loams_house_proto::loams::house::v1::__buffa::oneof::restore_table_request::Point;
        if !user.scopes.admin {
            return Err(crate::identity::lacks(
                user,
                crate::identity::Scope::Admin,
                "RestoreTable",
            ));
        }
        if request.table.is_empty() {
            return Err(HouseError::from(ChError::bad_arguments(
                "RestoreTable needs the table to restore",
            )));
        }
        if !crate::identity::granted(user, database, &request.table) {
            return Err(crate::identity::not_granted(user, database, &request.table));
        }
        let point = match &request.point {
            Some(Point::SnapshotId(id)) => RestorePoint::Snapshot(*id),
            Some(Point::Timestamp(at)) => RestorePoint::Timestamp(
                at.seconds
                    .saturating_mul(1000)
                    .saturating_add(i64::from(at.nanos) / 1_000_000),
            ),
            None => {
                return Err(HouseError::from(ChError::bad_arguments(
                    "RestoreTable needs a snapshot_id or a timestamp to restore to",
                )));
            }
        };
        crate::retention::restore_table(
            lake,
            user.namespace,
            database,
            &request.table,
            point,
            &request.idempotency_key,
            crate::retention::now_ms(),
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[test]
    fn every_house_reason_maps_from_its_code() {
        for (code, reason) in [
            (62, "house_syntax_error"),
            (60, "house_unknown_table"),
            (81, "house_unknown_database"),
            (497, "house_access_denied"),
            (159, "house_timeout"),
            (241, "house_memory_limit"),
            (202, "house_too_many_queries"),
            (344, "house_disabled_function"),
            (394, "house_cancelled"),
        ] {
            assert_eq!(reason_for(code).1, reason, "{code}");
        }
        assert_eq!(reason_for(115).1, "invalid_argument");
        assert_eq!(reason_for(395), (ErrorCode::Unknown, "house_query_failed"));
    }

    /// Every reason this API raises is a row of `docs/api/reasons.md`, under the
    /// Connect code it is raised with (the registry's rule: a reason is added there
    /// before an RPC returns it).
    #[test]
    fn every_reason_raised_is_registered() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../docs/api/reasons.md");
        let text = std::fs::read_to_string(path).expect("docs/api/reasons.md");
        let registered: HashMap<String, String> = text
            .lines()
            .filter_map(|line| {
                let cells: Vec<&str> = line.split('|').map(str::trim).collect();
                let reason = cells.get(1)?.strip_prefix('`')?.strip_suffix('`')?;
                Some((reason.to_string(), cells.get(2)?.to_string()))
            })
            .collect();
        let mut raised: Vec<(ErrorCode, &str)> = (0..1000).map(reason_for).collect();
        raised.extend([
            (ErrorCode::FailedPrecondition, "house_read_only"),
            (ErrorCode::PermissionDenied, "house_access_denied"),
            (ErrorCode::NotFound, "house_query_not_found"),
            (ErrorCode::InvalidArgument, "invalid_argument"),
            (ErrorCode::Unimplemented, "not_implemented"),
            (ErrorCode::ResourceExhausted, "house_quota_exceeded"),
        ]);
        for (code, reason) in raised {
            assert_eq!(
                registered.get(reason).map(String::as_str),
                Some(code.as_str()),
                "{reason} is registered under {}",
                code.as_str()
            );
        }
    }

    /// A quota's refusal (HS1 Task 22) is `house_quota_exceeded` whatever its
    /// ClickHouse code, with the quota and its limit.
    #[test]
    fn quota_refusals_are_house_quota_exceeded() {
        use base64::Engine as _;
        use buffa::Message as _;
        for code in [202, 164] {
            let err = crate::quota::refusal(code, "scan_bytes_per_day", 1000, "used");
            let connect = to_connect(&err);
            assert_eq!(connect.code, ErrorCode::ResourceExhausted);
            let detail = connect
                .details
                .iter()
                .find(|d| d.type_url.ends_with("loams.errors.v1.ErrorInfo"))
                .expect("an ErrorInfo");
            let value = detail.value.as_deref().expect("a value");
            let bytes = base64::engine::general_purpose::STANDARD_NO_PAD
                .decode(value.trim_end_matches('='))
                .expect("base64");
            let info = ErrorInfo::decode_from_slice(&bytes).expect("decodes");
            assert_eq!(info.reason, "house_quota_exceeded");
            assert_eq!(
                info.metadata.get("quota").map(String::as_str),
                Some("scan_bytes_per_day")
            );
            assert_eq!(info.metadata.get("limit").map(String::as_str), Some("1000"));
            assert_eq!(
                info.metadata.get("clickhouse_code").map(String::as_str),
                Some(code.to_string().as_str())
            );
        }
        // Admission's own 202 stays `house_too_many_queries`.
        let busy = HouseError::from(ChError::too_many_simultaneous_queries("busy"));
        assert!(crate::quota::refusal_parts(busy.message()).is_none());
    }

    #[test]
    fn pages_are_opaque_offsets() {
        let items: Vec<u32> = (0..250).collect();
        let (first, token) = page(items.clone(), 0, "").expect("first");
        assert_eq!(first.len(), DEFAULT_PAGE_SIZE);
        let (second, token) = page(items.clone(), 0, &token).expect("second");
        assert_eq!(second[0], 100);
        let (third, token) = page(items.clone(), 0, &token).expect("third");
        assert_eq!(third.len(), 50);
        assert!(token.is_empty());
        assert!(page(items.clone(), -1, "").is_err());
        assert!(page(items, 10, "not-a-token!").is_err());
    }

    #[test]
    fn identifiers_are_quoted() {
        assert_eq!(
            quote_identifier("t", "a`b\\c").expect("quoted"),
            "`a\\`b\\\\c`"
        );
        assert!(quote_identifier("t", "").is_err());
        assert!(quote_identifier("t", "a\nb").is_err());
    }
}
