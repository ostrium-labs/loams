//! The API against an in-memory SQL Gateway.

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicU64, Ordering};

use axum::body::Body;
use axum::http::Request;
use tower::ServiceExt;

use super::backend::{ResultColumn, ResultPage, ResultRow, RowKind};
use super::jobs::MemoryJobStore;
use super::*;
use crate::catalog::IcebergCatalog;

const FLINK_JOB: &str = "0123456789abcdef0123456789abcdef";
const SAVEPOINT: &str = "s3://loams-engines/engines/ns1/eng_x/savepoints/savepoint-1";

/// Records every statement; answers by the statement's first word.
#[derive(Default)]
struct FakeGateway {
    log: StdMutex<Vec<String>>,
    ops: StdMutex<HashMap<String, String>>,
    next: AtomicU64,
    closed_sessions: AtomicU64,
}

impl FakeGateway {
    fn log(&self) -> Vec<String> {
        self.log.lock().map(|l| l.clone()).unwrap_or_default()
    }
}

fn col(name: &str, t: &str) -> ResultColumn {
    ResultColumn {
        name: name.into(),
        data_type: t.into(),
        nullable: true,
    }
}

fn page(
    result_type: ResultType,
    columns: Vec<ResultColumn>,
    rows: Vec<Vec<serde_json::Value>>,
    next: Option<u64>,
) -> ResultPage {
    ResultPage {
        result_type,
        columns,
        rows: rows
            .into_iter()
            .map(|fields| ResultRow {
                kind: RowKind::Insert,
                fields,
            })
            .collect(),
        job_id: None,
        is_query: false,
        next_token: next,
    }
}

#[async_trait]
impl FlinkSqlBackend for FakeGateway {
    async fn open_session(
        &self,
        _name: &str,
        _properties: &BTreeMap<String, String>,
    ) -> Result<SessionHandle, BackendError> {
        Ok(SessionHandle(format!(
            "s{}",
            self.next.fetch_add(1, Ordering::SeqCst)
        )))
    }

    async fn close_session(&self, _session: &SessionHandle) -> Result<(), BackendError> {
        self.closed_sessions.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn heartbeat(&self, _session: &SessionHandle) -> Result<(), BackendError> {
        Ok(())
    }

    async fn execute(
        &self,
        _session: &SessionHandle,
        statement: &str,
    ) -> Result<OperationHandle, BackendError> {
        if let Ok(mut l) = self.log.lock() {
            l.push(statement.to_string());
        }
        let op = format!("op{}", self.next.fetch_add(1, Ordering::SeqCst));
        if let Ok(mut ops) = self.ops.lock() {
            ops.insert(op.clone(), statement.to_string());
        }
        Ok(OperationHandle(op))
    }

    async fn fetch(
        &self,
        _session: &SessionHandle,
        op: &OperationHandle,
        token: u64,
    ) -> Result<ResultPage, BackendError> {
        let stmt = self
            .ops
            .lock()
            .ok()
            .and_then(|ops| ops.get(&op.0).cloned())
            .ok_or(BackendError::NotFound)?;
        let head = stmt
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_ascii_uppercase();
        if head == "FAIL" || stmt.contains("FAIL_ME") {
            return Err(BackendError::Statement("boom".into()));
        }
        Ok(match (head.as_str(), token) {
            ("SELECT", 0) => {
                let mut p = page(
                    ResultType::Payload,
                    vec![col("n", "BIGINT")],
                    vec![vec![json!(1)], vec![json!(2)]],
                    Some(1),
                );
                p.is_query = true;
                p
            }
            ("SELECT", 1) => {
                let mut p = page(ResultType::Payload, vec![], vec![vec![json!(3)]], Some(2));
                p.is_query = true;
                p
            }
            ("INSERT", _) => {
                let mut p = page(
                    ResultType::Payload,
                    vec![col("job id", "VARCHAR")],
                    vec![vec![json!(FLINK_JOB)]],
                    None,
                );
                p.job_id = Some(FLINK_JOB.into());
                p
            }
            ("STOP", 0) => page(ResultType::NotReady, vec![], vec![], Some(1)),
            ("STOP", 1) => page(
                ResultType::Payload,
                vec![col("savepoint path", "VARCHAR")],
                vec![vec![json!(SAVEPOINT)]],
                Some(2),
            ),
            ("SHOW", 0) => page(
                ResultType::Payload,
                vec![
                    col("job id", "VARCHAR"),
                    col("job name", "VARCHAR"),
                    col("status", "VARCHAR"),
                ],
                vec![vec![json!(FLINK_JOB), json!("loams:x"), json!("RUNNING")]],
                Some(1),
            ),
            _ => page(ResultType::Eos, vec![], vec![], None),
        })
    }

    async fn cancel(
        &self,
        _session: &SessionHandle,
        _op: &OperationHandle,
    ) -> Result<(), BackendError> {
        Ok(())
    }

    async fn close_operation(
        &self,
        _session: &SessionHandle,
        _op: &OperationHandle,
    ) -> Result<(), BackendError> {
        Ok(())
    }
}

struct TestAuth;

#[async_trait]
impl Authorizer for TestAuth {
    async fn authorize(
        &self,
        headers: &HeaderMap,
        _namespace: &str,
        need: Need,
    ) -> Result<Principal, ApiError> {
        match headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
        {
            Some("Bearer operator") => Ok(Principal {
                id: "user:op".into(),
            }),
            Some("Bearer viewer") if need == Need::View => Ok(Principal {
                id: "user:view".into(),
            }),
            Some("Bearer viewer") => Err(ApiError::forbidden("viewer")),
            _ => Err(ApiError::unauthenticated("no token")),
        }
    }
}

fn setup() -> (ApiState, Arc<FakeGateway>) {
    let fake = Arc::new(FakeGateway::default());
    let mut iceberg = IcebergCatalog::for_namespace("loams", "ns1");
    iceberg.storage.endpoint = Some("http://rustfs:9000".into());
    let target = Arc::new(FlinkTarget {
        backend: fake.clone(),
        session: SessionSpec {
            savepoint_dir: Some("s3://loams-engines/engines/ns1/eng_x/savepoints".into()),
            iceberg: Some(iceberg),
            ..SessionSpec::default()
        },
    });
    let mut instances = HashMap::new();
    instances.insert(("ns1".to_string(), "eng_x".to_string()), target);
    let mut limits = ApiLimits::default();
    limits.run.poll_min = Duration::from_millis(1);
    limits.run.poll_max = Duration::from_millis(2);
    let state = ApiState::new(
        Arc::new(TestAuth),
        Arc::new(StaticInstances(instances)),
        Arc::new(StaticSecrets::default()),
        Arc::new(MemoryJobStore::default()),
    )
    .with_limits(limits);
    (state, fake)
}

const BASE: &str = "/v1/namespaces/ns1/engines/eng_x/flink";

async fn call(
    state: &ApiState,
    method: &str,
    path: &str,
    token: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, String) {
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .header(header::AUTHORIZATION, format!("Bearer {token}"));
    let body = match body {
        Some(v) => {
            req = req.header(header::CONTENT_TYPE, "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let req = req.body(body).unwrap_or_else(|e| panic!("{e}"));
    let resp = router(state.clone())
        .oneshot(req)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

fn parse(body: &str) -> serde_json::Value {
    serde_json::from_str(body).unwrap_or_else(|e| panic!("{e}: {body}"))
}

#[tokio::test]
async fn auth_is_required_and_viewers_cannot_operate() {
    let (st, _) = setup();
    let (s, b) = call(&st, "GET", &format!("{BASE}/jobs"), "nobody", None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "{b}");
    assert_eq!(parse(&b)["error"], "unauthenticated");
    let (s, _) = call(&st, "GET", &format!("{BASE}/jobs"), "viewer", None).await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = call(&st, "POST", &format!("{BASE}/sessions"), "viewer", None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = call(
        &st,
        "GET",
        "/v1/namespaces/ns1/engines/eng_other/flink/bootstrap",
        "viewer",
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn bootstrap_is_redacted_and_names_the_lake_catalog() {
    let (st, _) = setup();
    let (s, b) = call(&st, "GET", &format!("{BASE}/bootstrap"), "viewer", None).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    let script = parse(&b)["script"].as_str().unwrap_or_default().to_string();
    assert!(
        script.contains("CREATE CATALOG IF NOT EXISTS `loams`"),
        "{script}"
    );
    assert!(script.contains("'base-namespace' = 'ns_ns1'"), "{script}");
}

#[tokio::test]
async fn session_statement_and_streamed_results() {
    let (st, fake) = setup();
    let (s, b) = call(&st, "POST", &format!("{BASE}/sessions"), "operator", None).await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    let session = parse(&b)["session"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let log = fake.log();
    assert!(
        log[0].starts_with("SET 'execution.runtime-mode'"),
        "{log:?}"
    );
    assert!(
        log.iter()
            .any(|l| l.starts_with("CREATE CATALOG IF NOT EXISTS `loams`"))
    );

    let (s, b) = call(
        &st,
        "POST",
        &format!("{BASE}/sessions/{session}/statements"),
        "operator",
        Some(json!({ "statement": "SELECT n FROM t;" })),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    let stmt = parse(&b)["statement"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let (s, b) = call(
        &st,
        "GET",
        &format!("{BASE}/sessions/{session}/statements/{stmt}/results?max_rows=10"),
        "operator",
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    let lines: Vec<serde_json::Value> = b.lines().map(parse).collect();
    assert_eq!(lines[0]["type"], "columns");
    assert_eq!(lines[1]["type"], "rows");
    assert_eq!(lines[1]["rows"][0]["kind"], "+I");
    let end = lines.last().cloned().unwrap_or_default();
    assert_eq!(end["type"], "end");
    assert_eq!(end["rows"], 3);
    assert_eq!(end["truncated"], false);

    // Another principal cannot see the session.
    let (s, _) = call(
        &st,
        "DELETE",
        &format!("{BASE}/sessions/{session}"),
        "viewer",
        None,
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = call(
        &st,
        "DELETE",
        &format!("{BASE}/sessions/{session}"),
        "operator",
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn results_are_cut_at_max_rows() {
    let (st, _) = setup();
    let (_, b) = call(&st, "POST", &format!("{BASE}/sessions"), "operator", None).await;
    let session = parse(&b)["session"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let (_, b) = call(
        &st,
        "POST",
        &format!("{BASE}/sessions/{session}/statements"),
        "operator",
        Some(json!({ "statement": "SELECT n FROM t" })),
    )
    .await;
    let stmt = parse(&b)["statement"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let (_, b) = call(
        &st,
        "GET",
        &format!("{BASE}/sessions/{session}/statements/{stmt}/results?max_rows=1"),
        "operator",
        None,
    )
    .await;
    let end: serde_json::Value = b.lines().last().map(parse).unwrap_or_default();
    assert_eq!(end["rows"], 1, "{b}");
    assert_eq!(end["truncated"], true, "{b}");
}

#[tokio::test]
async fn protected_settings_are_refused() {
    let (st, _) = setup();
    let (s, b) = call(
        &st,
        "POST",
        &format!("{BASE}/sql"),
        "operator",
        Some(json!({ "sql": "SET 'execution.state-recovery.path' = 's3://x'" })),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{b}");
}

#[tokio::test]
async fn one_shot_sql_returns_the_last_result_and_failures() {
    let (st, _) = setup();
    let (s, b) = call(
        &st,
        "POST",
        &format!("{BASE}/sql"),
        "operator",
        Some(json!({ "sql": "CREATE TABLE x (a INT); SELECT n FROM x;" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    let r = parse(&b);
    assert_eq!(r["row_count"], 3);
    assert_eq!(r["columns"][0]["name"], "n");
    let (s, b) = call(
        &st,
        "POST",
        &format!("{BASE}/sql"),
        "operator",
        Some(json!({ "sql": "FAIL now" })),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{b}");
    assert_eq!(parse(&b)["message"], "boom");
}

#[tokio::test]
async fn job_lifecycle_start_stop_resume() {
    let (st, fake) = setup();
    let (s, b) = call(
        &st,
        "POST",
        &format!("{BASE}/jobs"),
        "operator",
        Some(json!({
            "name": "clicks per minute",
            "statement": "INSERT INTO loams.analytics.clicks_per_minute SELECT 1",
            "setup": ["CREATE DATABASE IF NOT EXISTS loams.analytics"],
        })),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    let job = parse(&b);
    assert_eq!(job["state"], "draft");
    let id = job["id"].as_str().unwrap_or_default().to_string();

    let (s, b) = call(
        &st,
        "POST",
        &format!("{BASE}/jobs/{id}/start"),
        "operator",
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    let job = parse(&b);
    assert_eq!(job["state"], "running", "{b}");
    assert_eq!(job["flink_job_id"], FLINK_JOB);
    let log = fake.log();
    assert!(
        log.iter()
            .any(|l| l == "CREATE DATABASE IF NOT EXISTS loams.analytics")
    );
    assert!(
        log.iter()
            .any(|l| l.starts_with("SET 'pipeline.name' = 'loams:fj_"))
    );

    // A second start while running is a conflict.
    let (s, _) = call(
        &st,
        "POST",
        &format!("{BASE}/jobs/{id}/start"),
        "operator",
        None,
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT);
    // Refresh sees it running.
    let (s, b) = call(&st, "GET", &format!("{BASE}/jobs/{id}"), "viewer", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(parse(&b)["state"], "running");

    let (s, b) = call(
        &st,
        "POST",
        &format!("{BASE}/jobs/{id}/stop"),
        "operator",
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    let job = parse(&b);
    assert_eq!(job["state"], "stopped", "{b}");
    assert_eq!(job["savepoint"], SAVEPOINT);
    assert!(
        fake.log()
            .iter()
            .any(|l| l == &format!("STOP JOB '{FLINK_JOB}' WITH SAVEPOINT"))
    );

    let (s, b) = call(
        &st,
        "POST",
        &format!("{BASE}/jobs/{id}/resume"),
        "operator",
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(parse(&b)["state"], "running");
    assert!(
        fake.log()
            .iter()
            .any(|l| l == &format!("SET 'execution.state-recovery.path' = '{SAVEPOINT}'"))
    );

    let (s, _) = call(
        &st,
        "DELETE",
        &format!("{BASE}/jobs/{id}"),
        "operator",
        None,
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT, "running jobs are not deleted");
}

#[tokio::test]
async fn a_failed_submit_marks_the_job_failed() {
    let (st, _) = setup();
    let (_, b) = call(
        &st,
        "POST",
        &format!("{BASE}/jobs"),
        "operator",
        Some(json!({
            "name": "broken",
            "statement": "INSERT INTO x SELECT 1",
            "setup": ["CREATE TABLE FAIL_ME (a INT)"],
        })),
    )
    .await;
    let id = parse(&b)["id"].as_str().unwrap_or_default().to_string();
    let (s, b) = call(
        &st,
        "POST",
        &format!("{BASE}/jobs/{id}/start"),
        "operator",
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    let job = parse(&b);
    assert_eq!(job["state"], "failed");
    assert!(
        job["message"]
            .as_str()
            .unwrap_or_default()
            .contains("CREATE TABLE FAIL_ME"),
        "{b}"
    );
}
