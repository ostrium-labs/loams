//! `loams.sql.v1` on the main port (design §44 §5.1; API2 Task 1; SDKP P0b).
//!
//! `SqlService/Query` and `SqlService/QueryArrow` run one read-only statement
//! through the same DataFusion context `POST /v1/namespaces/{ns}/sql`
//! ([`super::sql`]) and Flight SQL use: the same catalog, the same read-only
//! rule, the same row cap and timeout, and the same graph-function check of
//! the caller. What this adds over the REST route:
//!
//! - **Parameters.** `params` binds `$1…$n`
//!   ([`loams_query::sql::plan_read_only_with_params`]); a count mismatch is
//!   `invalid_argument` with reason `sql_param_count` and the two counts in
//!   `metadata`.
//! - **`max_rows` per call**, at most [`MAX_ROWS_CAP`].
//! - **Arrow.** `QueryArrow` answers the result as one Arrow IPC stream, capped
//!   at the server's `arrow_max_bytes`; past it the call is
//!   `resource_exhausted` with reason `result_too_large` and `metadata.use =
//!   "flight"`.
//!
//! Rows are JSON-shaped values in a `ListValue` per row. An integer a `double`
//! cannot hold exactly (past ±2^53) is sent as its decimal string, so it stays
//! exact.

// The generated service traits return `impl Encodable<_>`; these impls name
// the concrete message type (`connect.rs` does the same).
#![allow(refining_impl_trait)]

use std::sync::Arc;

use buffa::MessageFieldView;
use connectrpc::{
    ConnectError, ErrorCode, RequestContext, Response, ServiceRequest, ServiceResult,
};
use loams_common::meta::is_reserved_namespace;
use loams_proto::google::protobuf::__buffa::view::ValueView;
use loams_proto::google::protobuf::{ListValue, Value as ProtoValue};
use loams_proto::loams::collection::v1::__buffa::view::ConsistencyView;
use loams_proto::loams::sql::v1::{
    Column, QueryArrowRequest, QueryArrowResponse, QueryRequest, QueryResponse, SqlService,
    SqlServiceExt,
};
use loams_query::ReadConsistency;
use loams_query::sql::{
    ParamsError, SqlResult, arrow_ipc, plan_read_only_with_params, rows_to_json,
};
use serde_json::Value;

use super::connect::refuse;
use super::connect_errors::{invalid, refused, refused_service};
use super::connect_messages as msg;
use super::errors::ApiError;
use super::{AppState, read_consistency};

/// The most rows one call may ask for (`QueryRequest.max_rows`).
pub(super) const MAX_ROWS_CAP: u32 = 100_000;

/// The reason a parameter count mismatch answers.
pub(super) const SQL_PARAM_COUNT: &str = "sql_param_count";

/// The reason an Arrow answer over the cap answers.
pub(super) const RESULT_TOO_LARGE: &str = "result_too_large";

#[derive(Debug)]
struct Sql {
    state: AppState,
}

/// One statement's result and the token it was read at.
struct Answer {
    result: SqlResult,
    token: String,
}

impl Sql {
    fn new(state: AppState) -> Arc<Self> {
        Arc::new(Self { state })
    }

    /// Runs one statement: the shared path of both RPCs.
    async fn run(
        &self,
        ctx: &RequestContext,
        namespace: &str,
        sql: &str,
        params: Vec<Value>,
        consistency: &MessageFieldView<ConsistencyView<'_>>,
        max_rows: Option<u32>,
    ) -> Result<Answer, ConnectError> {
        if namespace.is_empty() {
            return Err(invalid("namespace", "a namespace is required"));
        }
        if is_reserved_namespace(namespace) {
            return Err(refused(ApiError::namespace_reserved(namespace)));
        }
        if sql.trim().is_empty() {
            return Err(invalid("sql", "a statement is required"));
        }
        let asked = match consistency.as_option() {
            None => None,
            Some(asked) => Some(super::connect_documents::asked_of(asked)?),
        };
        let consistency = read_consistency(ctx.headers(), asked).map_err(refused)?;
        let token = match &consistency {
            ReadConsistency::AtLeast(token) => token.to_string(),
            _ => String::new(),
        };
        let mut config = self.state.collections.config().sql.clone();
        if let Some(rows) = max_rows {
            config.max_rows = rows.clamp(1, MAX_ROWS_CAP) as usize;
        }
        let caller = super::graph_caller(
            &self.state.principals,
            ctx.extensions().get::<crate::auth::Principal>(),
        );
        let context = self
            .state
            .collections
            .sql_context_with(namespace, consistency, caller);
        let given = params.len();
        let frame = match plan_read_only_with_params(&context, sql, &params).await {
            Ok(frame) => frame,
            Err(ParamsError::Count { expected, given: _ }) => {
                return Err(refuse(
                    ErrorCode::InvalidArgument,
                    SQL_PARAM_COUNT,
                    format!(
                        "the statement has {expected} parameter(s) ($1…${expected}) and {given} \
                         value(s) were given"
                    ),
                    &[
                        ("field", "params".to_owned()),
                        ("expected", expected.to_string()),
                        ("given", given.to_string()),
                    ],
                ));
            }
            Err(ParamsError::Service(err)) => return Err(refused_service(err)),
        };
        let result = loams_query::sql::run_bounded(&config, async { Ok(frame) })
            .await
            .map_err(refused_service)?;
        Ok(Answer { result, token })
    }
}

fn params_of<'a>(params: impl Iterator<Item = &'a ValueView<'a>>) -> Vec<Value> {
    params.map(msg::json_of_value_view).collect()
}

/// A JSON cell as a protobuf value, keeping an integer past ±2^53 exact as text.
fn cell(value: &Value) -> ProtoValue {
    const EXACT: u64 = 1 << 53;
    if let Value::Number(number) = value {
        let inexact = match (number.as_i64(), number.as_u64()) {
            (Some(i), _) => i.unsigned_abs() > EXACT,
            (None, Some(u)) => u > EXACT,
            _ => false,
        };
        if inexact {
            return ProtoValue::from(number.to_string().as_str());
        }
    }
    msg::proto_value(value)
}

impl SqlService for Sql {
    async fn query(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, QueryRequest>,
    ) -> ServiceResult<QueryResponse> {
        let params = params_of(request.params.iter());
        let answer = self
            .run(
                &ctx,
                request.namespace,
                request.sql,
                params,
                &request.consistency,
                request.max_rows,
            )
            .await?;
        let json = rows_to_json(&answer.result);
        let columns = answer
            .result
            .schema
            .fields()
            .iter()
            .map(|field| Column {
                name: field.name().clone(),
                r#type: format!("{}", field.data_type()),
                ..Default::default()
            })
            .collect();
        let rows = json["rows"]
            .as_array()
            .map(|rows| {
                rows.iter()
                    .map(|row| ListValue {
                        values: row
                            .as_array()
                            .map(|cells| cells.iter().map(cell).collect())
                            .unwrap_or_default(),
                        ..Default::default()
                    })
                    .collect()
            })
            .unwrap_or_default();
        Response::ok(QueryResponse {
            columns,
            rows,
            truncated: answer.result.truncated,
            consistency_token: answer.token,
            ..Default::default()
        })
    }

    async fn query_arrow(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, QueryArrowRequest>,
    ) -> ServiceResult<QueryArrowResponse> {
        let params = params_of(request.params.iter());
        let answer = self
            .run(
                &ctx,
                request.namespace,
                request.sql,
                params,
                &request.consistency,
                request.max_rows,
            )
            .await?;
        let cap = self.state.collections.config().sql.arrow_max_bytes;
        let Some(bytes) = arrow_ipc(&answer.result, cap).map_err(refused_service)? else {
            return Err(refuse(
                ErrorCode::ResourceExhausted,
                RESULT_TOO_LARGE,
                format!(
                    "the result is larger than {cap} bytes as Arrow IPC; read it over Flight SQL, \
                     which streams a result of any size"
                ),
                &[("use", "flight".to_owned()), ("max_bytes", cap.to_string())],
            ));
        };
        Response::ok(QueryArrowResponse {
            arrow_ipc: bytes,
            truncated: answer.result.truncated,
            consistency_token: answer.token,
            ..Default::default()
        })
    }
}

pub(super) fn register(router: connectrpc::Router, state: &AppState) -> connectrpc::Router {
    SqlServiceExt::register(Sql::new(state.clone()), router)
}
