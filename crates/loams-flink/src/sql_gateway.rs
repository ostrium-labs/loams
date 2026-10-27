//! A typed client for the Flink SQL Gateway REST API (D1971).
//!
//! Endpoints (each under `/<version>/`, `version` one of `v1`..`v4`):
//!
//! | Call | Endpoint | Since |
//! |---|---|---|
//! | [`SqlGatewayClient::api_versions`] | `GET /api_versions` | v1 |
//! | [`SqlGatewayClient::info`] | `GET /info` | v1 |
//! | [`SqlGatewayClient::open_session`] | `POST /sessions` | v1 |
//! | [`SqlGatewayClient::session_config`] | `GET /sessions/:h` | v1 |
//! | [`SqlGatewayClient::heartbeat`] | `POST /sessions/:h/heartbeat` | v1 |
//! | [`SqlGatewayClient::close_session`] | `DELETE /sessions/:h` | v1 |
//! | [`SqlGatewayClient::configure_session`] | `POST /sessions/:h/configure-session` | v2 |
//! | [`SqlGatewayClient::complete_statement`] | `GET /sessions/:h/complete-statement` | v2 |
//! | [`SqlGatewayClient::execute_statement`] | `POST /sessions/:h/statements` | v1 |
//! | [`SqlGatewayClient::operation_status`] | `GET /sessions/:h/operations/:o/status` | v1 |
//! | [`SqlGatewayClient::cancel_operation`] | `POST /sessions/:h/operations/:o/cancel` | v1 |
//! | [`SqlGatewayClient::close_operation`] | `DELETE /sessions/:h/operations/:o/close` | v1 |
//! | [`SqlGatewayClient::fetch_result`] | `GET /sessions/:h/operations/:o/result/:token?rowFormat=` | v2 (`rowFormat`) |
//! | [`SqlGatewayClient::deploy_script`] | `POST /sessions/:h/scripts` | v4 |
//!
//! Results are paged: each page names the next one in `nextResultUri`
//! (its last path segment is the token), `NOT_READY` means "ask again",
//! and `EOS` ends the stream. [`SqlGatewayClient::fetch_all`] walks the
//! pages for a bounded result.

use std::collections::BTreeMap;
use std::fmt;
use std::time::{Duration, Instant};

use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::FlinkError;
use crate::http::{Http, Ignored, check_segment};

/// A SQL Gateway REST API version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ApiVersion {
    /// v1: no `configure-session`, `complete-statement` or `rowFormat`.
    V1,
    /// v2: adds `configure-session`, `complete-statement` and `rowFormat`.
    V2,
    /// v3: adds materialized-table refresh; Flink 2.x's default.
    V3,
    /// v4: adds `POST /sessions/:h/scripts` (application mode).
    V4,
}

impl ApiVersion {
    /// The path prefix, `v1`..`v4`.
    pub fn path(self) -> &'static str {
        match self {
            ApiVersion::V1 => "v1",
            ApiVersion::V2 => "v2",
            ApiVersion::V3 => "v3",
            ApiVersion::V4 => "v4",
        }
    }

    /// Parses `V3`, `v3` or `3`.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().trim_start_matches(['v', 'V']) {
            "1" => Some(ApiVersion::V1),
            "2" => Some(ApiVersion::V2),
            "3" => Some(ApiVersion::V3),
            "4" => Some(ApiVersion::V4),
            _ => None,
        }
    }
}

impl fmt::Display for ApiVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.path())
    }
}

/// A session handle the gateway issued (a UUID).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionHandle(String);

/// An operation handle the gateway issued (a UUID).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OperationHandle(String);

macro_rules! handle_impl {
    ($t:ident, $what:literal) => {
        impl $t {
            /// Checks the handle's shape (letters, digits and `-`).
            ///
            /// # Errors
            ///
            /// [`FlinkError::Invalid`] for anything else.
            pub fn new(value: impl Into<String>) -> Result<Self, FlinkError> {
                let value = value.into();
                check_segment($what, &value)?;
                Ok(Self(value))
            }

            /// The handle.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $t {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

handle_impl!(SessionHandle, "session handle");
handle_impl!(OperationHandle, "operation handle");

/// `GET /info`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayInfo {
    /// `Apache Flink`.
    pub product_name: String,
    /// The Flink version, e.g. `2.2.1`.
    pub version: String,
}

/// The body of `POST /sessions`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenSession {
    /// A name for the session (shown in logs).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_name: Option<String>,
    /// Session configuration (`key` to value).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub properties: BTreeMap<String, String>,
}

/// The body of `POST /sessions/:h/statements`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecuteStatement {
    /// One SQL statement.
    pub statement: String,
    /// The gateway's execution timeout, in ms.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_timeout: Option<u64>,
    /// Configuration for this statement only, e.g.
    /// `execution.state-recovery.path` to start from a savepoint.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub execution_config: BTreeMap<String, String>,
}

impl ExecuteStatement {
    /// `statement` with no options.
    pub fn new(statement: impl Into<String>) -> Self {
        Self {
            statement: statement.into(),
            ..Self::default()
        }
    }

    /// Adds one execution option.
    pub fn with_config(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.execution_config.insert(key.into(), value.into());
        self
    }
}

/// The body of `POST /sessions/:h/scripts` (v4, application mode): a script
/// inline or by URI.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeployScript {
    /// The SQL script, inline.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub script: Option<String>,
    /// Where the script is (e.g. `s3://flink/flink/<tenant>/scripts/x.sql`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub script_uri: Option<String>,
    /// Configuration of the application cluster.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub execution_config: BTreeMap<String, String>,
}

/// An operation's state (`GET .../status`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum OperationStatus {
    /// Created, not yet submitted.
    Initialized,
    /// Submitted, waiting for a worker.
    Pending,
    /// Running.
    Running,
    /// Done; results can be fetched.
    Finished,
    /// Cancelled.
    Canceled,
    /// Closed; the handle is gone.
    Closed,
    /// Failed; fetching results returns the error.
    Error,
    /// Ran past its execution timeout.
    Timeout,
}

impl OperationStatus {
    /// Whether the operation can no longer change state.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            OperationStatus::Finished
                | OperationStatus::Canceled
                | OperationStatus::Closed
                | OperationStatus::Error
                | OperationStatus::Timeout
        )
    }
}

/// The row format a result page is fetched in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum RowFormat {
    /// Each field as JSON of its type.
    #[default]
    Json,
    /// Each field as its string form.
    PlainText,
}

impl RowFormat {
    fn as_str(self) -> &'static str {
        match self {
            RowFormat::Json => "JSON",
            RowFormat::PlainText => "PLAIN_TEXT",
        }
    }
}

/// What a result page is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ResultType {
    /// Nothing yet: fetch the same token again later.
    NotReady,
    /// Rows; `nextResultUri` names the next page.
    Payload,
    /// End of stream.
    Eos,
}

/// The change a row carries (Flink's `RowKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RowKind {
    /// A new row.
    Insert,
    /// The old row of an update.
    UpdateBefore,
    /// The new row of an update.
    UpdateAfter,
    /// A retraction.
    Delete,
}

/// A result column.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Column {
    /// The column name.
    pub name: String,
    /// Flink's serialized `LogicalType` (`{"type": "INTEGER", "nullable": true}`).
    #[serde(default)]
    pub logical_type: Value,
    /// The column comment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

/// A result row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Row {
    /// The change this row is.
    pub kind: RowKind,
    /// The fields, in column order.
    #[serde(default)]
    pub fields: Vec<Value>,
}

/// The rows of a page.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultSet {
    /// The columns (repeated on every page).
    #[serde(default)]
    pub columns: Vec<Column>,
    /// `JSON` or `PLAIN_TEXT`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub row_format: Option<String>,
    /// The rows.
    #[serde(default)]
    pub data: Vec<Row>,
}

/// One page of `GET .../result/:token`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultPage {
    /// `NOT_READY`, `PAYLOAD` or `EOS`.
    pub result_type: ResultType,
    /// Whether the statement is a query (its rows are its result).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_query_result: Option<bool>,
    /// The Flink job the statement submitted, if any.
    #[serde(default, rename = "jobID", skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
    /// `SUCCESS` or `SUCCESS_WITH_CONTENT`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_kind: Option<String>,
    /// The rows.
    #[serde(default)]
    pub results: ResultSet,
    /// The next page's path, absent at `EOS`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_result_uri: Option<String>,
}

impl ResultPage {
    /// The token of the next page, read from `nextResultUri`
    /// (`/v3/sessions/<h>/operations/<o>/result/<token>?rowFormat=JSON`).
    pub fn next_token(&self) -> Option<u64> {
        self.next_result_uri.as_deref().and_then(token_of_uri)
    }
}

/// The last path segment of a `nextResultUri`, as a token.
pub fn token_of_uri(uri: &str) -> Option<u64> {
    let path = uri.split(['?', '#']).next().unwrap_or_default();
    path.trim_end_matches('/').rsplit('/').next()?.parse().ok()
}

/// Bounds for [`SqlGatewayClient::fetch_all`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FetchLimits {
    /// Stop after this many rows (the result is then `truncated`).
    pub max_rows: usize,
    /// Give up after this long.
    pub timeout: Duration,
    /// The first wait after `NOT_READY`; doubles up to `max_poll`.
    pub poll: Duration,
    /// The longest wait between `NOT_READY` polls.
    pub max_poll: Duration,
}

impl Default for FetchLimits {
    fn default() -> Self {
        Self {
            max_rows: 10_000,
            timeout: Duration::from_secs(60),
            poll: Duration::from_millis(50),
            max_poll: Duration::from_secs(1),
        }
    }
}

/// Every page of a bounded result, collected.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CollectedResult {
    /// The columns.
    pub columns: Vec<Column>,
    /// The rows, in order.
    pub rows: Vec<Row>,
    /// The Flink job, if the statement submitted one.
    pub job_id: Option<String>,
    /// `SUCCESS` or `SUCCESS_WITH_CONTENT`.
    pub result_kind: Option<String>,
    /// Whether the statement is a query.
    pub is_query_result: Option<bool>,
    /// `max_rows` was reached before `EOS` (a streaming query never ends).
    pub truncated: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionHandleBody {
    session_handle: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OperationHandleBody {
    operation_handle: String,
}

#[derive(Deserialize)]
struct StatusBody {
    status: String,
}

#[derive(Deserialize)]
struct VersionsBody {
    #[serde(default)]
    versions: Vec<String>,
}

#[derive(Deserialize)]
struct PropertiesBody {
    #[serde(default)]
    properties: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct CandidatesBody {
    #[serde(default)]
    candidates: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClusterIdBody {
    #[serde(rename = "clusterID")]
    cluster_id: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ConfigureSessionBody<'a> {
    statement: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    execution_timeout: Option<u64>,
}

#[derive(Serialize)]
struct CompleteStatementBody<'a> {
    statement: &'a str,
    position: u32,
}

/// The SQL Gateway client. Cheap to clone.
#[derive(Debug, Clone)]
pub struct SqlGatewayClient {
    http: Http,
    version: ApiVersion,
}

impl SqlGatewayClient {
    /// A client of the gateway at `base_url` (e.g. `http://sql-gateway:8083`)
    /// speaking `version`.
    ///
    /// # Errors
    ///
    /// [`FlinkError::Invalid`] for a URL that is not `http(s)://`.
    pub fn new(base_url: &str, version: ApiVersion) -> Result<Self, FlinkError> {
        Self::with_client(base_url, version, Http::default_client()?)
    }

    /// [`SqlGatewayClient::new`] with a caller's HTTP client.
    ///
    /// # Errors
    ///
    /// As [`SqlGatewayClient::new`].
    pub fn with_client(
        base_url: &str,
        version: ApiVersion,
        client: reqwest::Client,
    ) -> Result<Self, FlinkError> {
        Ok(Self {
            http: Http::new(base_url, client)?,
            version,
        })
    }

    /// A client that sends `authorization: Bearer <token>` (for reaching
    /// the gateway through loams-gateway).
    pub fn with_bearer(mut self, token: impl Into<String>) -> Self {
        self.http.set_bearer(Some(token.into()));
        self
    }

    /// A client of the same gateway speaking `version`.
    pub fn with_version(mut self, version: ApiVersion) -> Self {
        self.version = version;
        self
    }

    /// The version this client speaks.
    pub fn version(&self) -> ApiVersion {
        self.version
    }

    /// The gateway's base URL.
    pub fn base_url(&self) -> &str {
        self.http.base().as_str()
    }

    /// Asks the gateway which versions it speaks and switches to the
    /// highest of v4, v3 and v2 (D1971). A gateway that only speaks v1 is
    /// refused: this client needs `rowFormat`.
    ///
    /// # Errors
    ///
    /// The call's error, or [`FlinkError::Unsupported`].
    pub async fn negotiate(mut self) -> Result<Self, FlinkError> {
        let offered = self.api_versions().await?;
        let best = offered
            .iter()
            .copied()
            .filter(|v| *v >= ApiVersion::V2)
            .max()
            .ok_or_else(|| {
                FlinkError::Unsupported(format!(
                    "the SQL Gateway speaks {offered:?}; this client needs v2 or later"
                ))
            })?;
        self.version = best;
        Ok(self)
    }

    fn path(&self, rest: &str) -> String {
        format!("{}/{rest}", self.version.path())
    }

    fn require(&self, min: ApiVersion, what: &str) -> Result<(), FlinkError> {
        if self.version >= min {
            Ok(())
        } else {
            Err(FlinkError::Unsupported(format!(
                "{what} needs SQL Gateway API {min} or later; this client speaks {}",
                self.version
            )))
        }
    }

    /// `GET /api_versions`: the versions the gateway speaks (unknown ones
    /// are skipped).
    ///
    /// # Errors
    ///
    /// The call's error.
    pub async fn api_versions(&self) -> Result<Vec<ApiVersion>, FlinkError> {
        let body: VersionsBody = self.http.get("api_versions").await?;
        let mut versions: Vec<ApiVersion> = body
            .versions
            .iter()
            .filter_map(|v| ApiVersion::parse(v.as_str()))
            .collect();
        versions.sort();
        versions.dedup();
        Ok(versions)
    }

    /// `GET /info`.
    ///
    /// # Errors
    ///
    /// The call's error.
    pub async fn info(&self) -> Result<GatewayInfo, FlinkError> {
        self.http.get(&self.path("info")).await
    }

    /// `POST /sessions`.
    ///
    /// # Errors
    ///
    /// The call's error.
    pub async fn open_session(&self, req: &OpenSession) -> Result<SessionHandle, FlinkError> {
        let body: SessionHandleBody = self.http.post(&self.path("sessions"), req).await?;
        SessionHandle::new(body.session_handle)
    }

    /// `GET /sessions/:h`: the session's configuration.
    ///
    /// # Errors
    ///
    /// The call's error (404 for an unknown or expired session).
    pub async fn session_config(
        &self,
        session: &SessionHandle,
    ) -> Result<BTreeMap<String, String>, FlinkError> {
        let body: PropertiesBody = self
            .http
            .get(&self.path(&format!("sessions/{session}")))
            .await?;
        Ok(body.properties)
    }

    /// `POST /sessions/:h/heartbeat`: keeps an idle session open
    /// (`sql-gateway.session.idle-timeout`).
    ///
    /// # Errors
    ///
    /// The call's error.
    pub async fn heartbeat(&self, session: &SessionHandle) -> Result<(), FlinkError> {
        let _: Ignored = self
            .http
            .post(
                &self.path(&format!("sessions/{session}/heartbeat")),
                &serde_json::json!({}),
            )
            .await?;
        Ok(())
    }

    /// `DELETE /sessions/:h`: closes the session and its operations.
    /// Returns the gateway's status string (`CLOSED`).
    ///
    /// # Errors
    ///
    /// The call's error.
    pub async fn close_session(&self, session: &SessionHandle) -> Result<String, FlinkError> {
        let body: StatusBody = self
            .http
            .delete(&self.path(&format!("sessions/{session}")))
            .await?;
        Ok(body.status)
    }

    /// `POST /sessions/:h/configure-session` (v2+): runs a statement that
    /// changes the session only (`SET`, `RESET`, `CREATE CATALOG`, `USE`,
    /// `ADD JAR`, temporary DDL), synchronously.
    ///
    /// # Errors
    ///
    /// [`FlinkError::Unsupported`] below v2, else the call's error.
    pub async fn configure_session(
        &self,
        session: &SessionHandle,
        statement: &str,
        execution_timeout_ms: Option<u64>,
    ) -> Result<(), FlinkError> {
        self.require(ApiVersion::V2, "configure-session")?;
        let _: Ignored = self
            .http
            .post(
                &self.path(&format!("sessions/{session}/configure-session")),
                &ConfigureSessionBody {
                    statement,
                    execution_timeout: execution_timeout_ms,
                },
            )
            .await?;
        Ok(())
    }

    /// `GET /sessions/:h/complete-statement` (v2+): completion candidates
    /// for `statement` at byte `position`. Flink takes the arguments as a
    /// JSON body on the GET.
    ///
    /// # Errors
    ///
    /// [`FlinkError::Unsupported`] below v2, else the call's error.
    pub async fn complete_statement(
        &self,
        session: &SessionHandle,
        statement: &str,
        position: u32,
    ) -> Result<Vec<String>, FlinkError> {
        self.require(ApiVersion::V2, "complete-statement")?;
        let url = self
            .http
            .url(&self.path(&format!("sessions/{session}/complete-statement")))?;
        let body: CandidatesBody = self
            .http
            .call(
                Method::GET,
                url,
                Some(&CompleteStatementBody {
                    statement,
                    position,
                }),
            )
            .await?;
        Ok(body.candidates)
    }

    /// `POST /sessions/:h/statements`: submits one statement; its results
    /// are fetched with [`SqlGatewayClient::fetch_result`].
    ///
    /// # Errors
    ///
    /// The call's error.
    pub async fn execute_statement(
        &self,
        session: &SessionHandle,
        req: &ExecuteStatement,
    ) -> Result<OperationHandle, FlinkError> {
        let body: OperationHandleBody = self
            .http
            .post(&self.path(&format!("sessions/{session}/statements")), req)
            .await?;
        OperationHandle::new(body.operation_handle)
    }

    fn operation_path(&self, session: &SessionHandle, op: &OperationHandle, rest: &str) -> String {
        self.path(&format!("sessions/{session}/operations/{op}/{rest}"))
    }

    /// `GET /sessions/:h/operations/:o/status`.
    ///
    /// # Errors
    ///
    /// The call's error, or [`FlinkError::Decode`] for a status this client
    /// does not know.
    pub async fn operation_status(
        &self,
        session: &SessionHandle,
        op: &OperationHandle,
    ) -> Result<OperationStatus, FlinkError> {
        let path = self.operation_path(session, op, "status");
        let body: StatusBody = self.http.get(&path).await?;
        parse_status(&path, &body.status)
    }

    /// `POST /sessions/:h/operations/:o/cancel`.
    ///
    /// # Errors
    ///
    /// The call's error.
    pub async fn cancel_operation(
        &self,
        session: &SessionHandle,
        op: &OperationHandle,
    ) -> Result<OperationStatus, FlinkError> {
        let path = self.operation_path(session, op, "cancel");
        let body: StatusBody = self.http.post(&path, &serde_json::json!({})).await?;
        parse_status(&path, &body.status)
    }

    /// `DELETE /sessions/:h/operations/:o/close`: cancels if running and
    /// frees the operation's results.
    ///
    /// # Errors
    ///
    /// The call's error.
    pub async fn close_operation(
        &self,
        session: &SessionHandle,
        op: &OperationHandle,
    ) -> Result<OperationStatus, FlinkError> {
        let path = self.operation_path(session, op, "close");
        let body: StatusBody = self.http.delete(&path).await?;
        parse_status(&path, &body.status)
    }

    /// `GET /sessions/:h/operations/:o/result/:token`. Below v2 there is
    /// no `rowFormat` and `format` is ignored.
    ///
    /// # Errors
    ///
    /// The call's error; a statement that failed answers 500 with Flink's
    /// exception in [`FlinkError::Api`].
    pub async fn fetch_result(
        &self,
        session: &SessionHandle,
        op: &OperationHandle,
        token: u64,
        format: RowFormat,
    ) -> Result<ResultPage, FlinkError> {
        let mut url =
            self.http
                .url(&self.operation_path(session, op, &format!("result/{token}")))?;
        if self.version >= ApiVersion::V2 {
            url.query_pairs_mut()
                .append_pair("rowFormat", format.as_str());
        }
        self.http
            .call::<(), ResultPage>(Method::GET, url, None)
            .await
    }

    /// Fetches pages from token 0 until `EOS`, `limits.max_rows` rows or
    /// `limits.timeout`, waiting with backoff on `NOT_READY`. For a
    /// streaming query (which never reaches `EOS`) set `max_rows`; the
    /// result is then `truncated` and the operation is left open (close it
    /// with [`SqlGatewayClient::close_operation`]).
    ///
    /// # Errors
    ///
    /// The first failing fetch, or [`FlinkError::Timeout`].
    pub async fn fetch_all(
        &self,
        session: &SessionHandle,
        op: &OperationHandle,
        limits: FetchLimits,
    ) -> Result<CollectedResult, FlinkError> {
        let deadline = Instant::now() + limits.timeout;
        let mut out = CollectedResult::default();
        let mut token = 0u64;
        let mut wait = limits.poll;
        loop {
            let page = self
                .fetch_result(session, op, token, RowFormat::Json)
                .await?;
            if out.columns.is_empty() && !page.results.columns.is_empty() {
                out.columns = page.results.columns.clone();
            }
            if page.job_id.is_some() {
                out.job_id.clone_from(&page.job_id);
            }
            if page.result_kind.is_some() {
                out.result_kind.clone_from(&page.result_kind);
            }
            if page.is_query_result.is_some() {
                out.is_query_result = page.is_query_result;
            }
            match page.result_type {
                ResultType::Eos => return Ok(out),
                ResultType::NotReady => {
                    if Instant::now() + wait > deadline {
                        return Err(FlinkError::Timeout(format!(
                            "results of operation {op} not ready after {:?}",
                            limits.timeout
                        )));
                    }
                    tokio::time::sleep(wait).await;
                    wait = (wait * 2).min(limits.max_poll);
                    // NOT_READY may still name the same (or a next) token.
                    if let Some(next) = page.next_token() {
                        token = next;
                    }
                }
                ResultType::Payload => {
                    wait = limits.poll;
                    let next = page.next_token();
                    for row in page.results.data {
                        if out.rows.len() >= limits.max_rows {
                            out.truncated = true;
                            return Ok(out);
                        }
                        out.rows.push(row);
                    }
                    match next {
                        Some(next) => token = next,
                        None => return Ok(out),
                    }
                    if Instant::now() > deadline {
                        return Err(FlinkError::Timeout(format!(
                            "results of operation {op} still streaming after {:?}",
                            limits.timeout
                        )));
                    }
                }
            }
        }
    }

    /// Submits `req` and waits for it to finish, returning its results:
    /// for DDL, `INSERT INTO` (whose result is the job id) and bounded
    /// queries. The operation is closed afterwards.
    ///
    /// # Errors
    ///
    /// As [`SqlGatewayClient::execute_statement`] and
    /// [`SqlGatewayClient::fetch_all`].
    pub async fn execute_and_collect(
        &self,
        session: &SessionHandle,
        req: &ExecuteStatement,
        limits: FetchLimits,
    ) -> Result<CollectedResult, FlinkError> {
        let op = self.execute_statement(session, req).await?;
        let result = self.fetch_all(session, &op, limits).await;
        // Best effort: the gateway frees results on session close anyway.
        if let Err(e) = self.close_operation(session, &op).await {
            tracing::debug!(%op, error = %e, "closing the operation failed");
        }
        result
    }

    /// `POST /sessions/:h/scripts` (v4): deploys `req` as an application
    /// cluster (application mode) and returns its cluster id.
    ///
    /// # Errors
    ///
    /// [`FlinkError::Unsupported`] below v4, [`FlinkError::Invalid`] when
    /// neither or both of `script` and `script_uri` are set, else the
    /// call's error.
    pub async fn deploy_script(
        &self,
        session: &SessionHandle,
        req: &DeployScript,
    ) -> Result<String, FlinkError> {
        self.require(ApiVersion::V4, "deploy-script")?;
        if req.script.is_some() == req.script_uri.is_some() {
            return Err(FlinkError::Invalid(
                "deploy-script takes exactly one of script and scriptUri".to_owned(),
            ));
        }
        let body: ClusterIdBody = self
            .http
            .post(&self.path(&format!("sessions/{session}/scripts")), req)
            .await?;
        Ok(body.cluster_id)
    }
}

fn parse_status(path: &str, status: &str) -> Result<OperationStatus, FlinkError> {
    serde_json::from_value(Value::String(status.to_ascii_uppercase())).map_err(|_| {
        FlinkError::Decode {
            url: path.to_owned(),
            message: format!("unknown operation status {status:?}"),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_parse_and_order() {
        assert_eq!(ApiVersion::parse("V3"), Some(ApiVersion::V3));
        assert_eq!(ApiVersion::parse("v4"), Some(ApiVersion::V4));
        assert_eq!(ApiVersion::parse("2"), Some(ApiVersion::V2));
        assert_eq!(ApiVersion::parse("V9"), None);
        assert!(ApiVersion::V4 > ApiVersion::V2);
    }

    #[test]
    fn tokens_come_from_next_result_uri() {
        assert_eq!(
            token_of_uri("/v3/sessions/a/operations/b/result/7?rowFormat=JSON"),
            Some(7)
        );
        assert_eq!(
            token_of_uri("/v1/sessions/a/operations/b/result/0"),
            Some(0)
        );
        assert_eq!(token_of_uri("/v3/sessions/a"), None);
    }

    #[test]
    fn a_result_page_decodes() {
        let page: ResultPage = serde_json::from_value(serde_json::json!({
            "results": {
                "columns": [{"name": "EXPR$0", "logicalType": {"type": "INTEGER", "nullable": false}, "comment": null}],
                "rowFormat": "JSON",
                "data": [{"kind": "INSERT", "fields": [1]}, {"kind": "UPDATE_AFTER", "fields": [2]}]
            },
            "resultType": "PAYLOAD",
            "isQueryResult": true,
            "jobID": "fbf9ad9e7fb4c3c3a8bb3b4e3a1f2d10",
            "resultKind": "SUCCESS_WITH_CONTENT",
            "nextResultUri": "/v3/sessions/s/operations/o/result/1?rowFormat=JSON"
        }))
        .unwrap();
        assert_eq!(page.result_type, ResultType::Payload);
        assert_eq!(page.results.columns[0].name, "EXPR$0");
        assert_eq!(page.results.data[1].kind, RowKind::UpdateAfter);
        assert_eq!(page.next_token(), Some(1));
        assert_eq!(
            page.job_id.as_deref(),
            Some("fbf9ad9e7fb4c3c3a8bb3b4e3a1f2d10")
        );
    }

    #[test]
    fn an_eos_page_without_results_decodes() {
        let page: ResultPage =
            serde_json::from_value(serde_json::json!({"resultType": "EOS", "nextResultUri": null}))
                .unwrap();
        assert_eq!(page.result_type, ResultType::Eos);
        assert!(page.results.data.is_empty());
        assert_eq!(page.next_token(), None);
    }

    #[test]
    fn statuses_parse_case_insensitively() {
        assert_eq!(
            parse_status("p", "finished").unwrap(),
            OperationStatus::Finished
        );
        assert!(parse_status("p", "WEIRD").is_err());
        assert!(OperationStatus::Error.is_terminal());
        assert!(!OperationStatus::Running.is_terminal());
    }

    #[test]
    fn requests_serialize_in_flink_field_names() {
        let open = OpenSession {
            session_name: Some("t1".into()),
            properties: BTreeMap::from([("a".to_owned(), "b".to_owned())]),
        };
        assert_eq!(
            serde_json::to_value(&open).unwrap(),
            serde_json::json!({"sessionName": "t1", "properties": {"a": "b"}})
        );
        let exec = ExecuteStatement::new("SELECT 1")
            .with_config("execution.state-recovery.path", "s3://b/p");
        assert_eq!(
            serde_json::to_value(&exec).unwrap(),
            serde_json::json!({"statement": "SELECT 1", "executionConfig": {"execution.state-recovery.path": "s3://b/p"}})
        );
        let script = DeployScript {
            script_uri: Some("s3://b/x.sql".into()),
            ..DeployScript::default()
        };
        assert_eq!(
            serde_json::to_value(&script).unwrap(),
            serde_json::json!({"scriptUri": "s3://b/x.sql"})
        );
    }

    #[tokio::test]
    async fn version_gates_refuse_before_sending() {
        // Nothing listens on this port; the gate fails before any request.
        let c = SqlGatewayClient::new("http://127.0.0.1:9", ApiVersion::V1).unwrap();
        let s = SessionHandle::new("s").unwrap();
        assert!(matches!(
            c.configure_session(&s, "SET 'a'='b'", None).await,
            Err(FlinkError::Unsupported(_))
        ));
        let c = c.with_version(ApiVersion::V3);
        assert!(matches!(
            c.deploy_script(&s, &DeployScript::default()).await,
            Err(FlinkError::Unsupported(_))
        ));
        let c = c.with_version(ApiVersion::V4);
        assert!(matches!(
            c.deploy_script(&s, &DeployScript::default()).await,
            Err(FlinkError::Invalid(_))
        ));
    }
}
