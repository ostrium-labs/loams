//! Running statements through a [`FlinkSqlBackend`]: to the end of their
//! results, and in a session opened with the bootstrap script (D1981).

use std::collections::BTreeMap;
use std::time::Duration;

use tokio::time::Instant;

use super::backend::{
    BackendError, FlinkSqlBackend, ResultColumn, ResultRow, ResultType, SessionHandle,
};
use super::{ApiError, FlinkTarget, SecretResolver};
use crate::catalog::{RenderCtx, SessionSpec};

/// How long and how much a statement may run before it is cut off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunLimits {
    pub max_rows: usize,
    pub wait: Duration,
    pub poll_min: Duration,
    pub poll_max: Duration,
}

impl Default for RunLimits {
    fn default() -> Self {
        Self {
            max_rows: 10_000,
            // STOP JOB … WITH SAVEPOINT waits for the savepoint.
            wait: Duration::from_secs(600),
            poll_min: Duration::from_millis(100),
            poll_max: Duration::from_secs(2),
        }
    }
}

/// A statement's results, collected.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Collected {
    pub columns: Vec<ResultColumn>,
    pub rows: Vec<ResultRow>,
    pub job_id: Option<String>,
    pub is_query: bool,
    /// Cut off by `max_rows` or `wait`.
    pub truncated: bool,
}

impl Collected {
    /// The first row's first field as text (`STOP JOB`'s savepoint path,
    /// an `INSERT`'s job id).
    pub fn first_text(&self) -> Option<String> {
        let v = self.rows.first()?.fields.first()?;
        match v {
            serde_json::Value::String(s) => Some(s.clone()),
            serde_json::Value::Null => None,
            other => Some(other.to_string()),
        }
    }

    /// The value of column `name` (case-insensitive) in `row`.
    pub fn field<'a>(&self, row: &'a ResultRow, name: &str) -> Option<&'a serde_json::Value> {
        let i = self
            .columns
            .iter()
            .position(|c| c.name.eq_ignore_ascii_case(name))?;
        row.fields.get(i)
    }
}

/// Execute `sql` and collect its results; the operation is closed after.
pub async fn run_to_end(
    backend: &dyn FlinkSqlBackend,
    session: &SessionHandle,
    sql: &str,
    limits: &RunLimits,
) -> Result<Collected, BackendError> {
    let op = backend.execute(session, sql).await?;
    let deadline = Instant::now() + limits.wait;
    let mut out = Collected::default();
    let mut token = 0u64;
    let mut delay = limits.poll_min;
    let result = loop {
        let page = match backend.fetch(session, &op, token).await {
            Ok(p) => p,
            Err(e) => break Err(e),
        };
        if out.job_id.is_none() {
            out.job_id.clone_from(&page.job_id);
        }
        if out.columns.is_empty() && !page.columns.is_empty() {
            out.columns = page.columns.clone();
        }
        out.is_query |= page.is_query;
        match page.result_type {
            ResultType::Eos => break Ok(()),
            ResultType::NotReady => {
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(limits.poll_max);
            }
            ResultType::Payload => {
                delay = limits.poll_min;
                out.rows.extend(page.rows);
                if out.rows.len() >= limits.max_rows {
                    out.rows.truncate(limits.max_rows);
                    out.truncated = true;
                    let _ = backend.cancel(session, &op).await;
                    break Ok(());
                }
            }
        }
        match page.next_token {
            Some(t) => token = t,
            None => break Ok(()),
        }
        if Instant::now() >= deadline {
            out.truncated = true;
            let _ = backend.cancel(session, &op).await;
            break Ok(());
        }
    };
    let _ = backend.close_operation(session, &op).await;
    result.map(|()| out)
}

/// The first line of a statement, for messages (never the whole text,
/// which may be long).
pub(crate) fn headline(sql: &str) -> String {
    let line = sql.lines().next().unwrap_or("").trim();
    if line.chars().count() > 80 {
        let cut: String = line.chars().take(80).collect();
        format!("{cut}…")
    } else {
        line.to_string()
    }
}

/// Open a session on `target` and run the bootstrap of `target.session`
/// merged with `extra`, then each of `setup`. On failure the session is
/// closed and the error names the statement (redacted).
pub async fn open_bootstrapped(
    target: &FlinkTarget,
    secrets: &dyn SecretResolver,
    namespace: &str,
    session_name: &str,
    extra: &SessionSpec,
    setup: &[String],
    limits: &RunLimits,
) -> Result<SessionHandle, ApiError> {
    let spec = target.session.merged(extra);
    let script = spec.script()?;
    let refs = script.secret_refs();
    let values = if refs.is_empty() {
        BTreeMap::new()
    } else {
        secrets.resolve(namespace, &refs).await?
    };
    let resolved = script.render(&RenderCtx::resolved(&values))?;
    let redacted = script.render(&RenderCtx::redacted())?;
    let backend = target.backend.as_ref();
    let session = backend
        .open_session(session_name, &BTreeMap::new())
        .await
        .map_err(ApiError::from)?;
    let ddl_limits = RunLimits {
        max_rows: 100,
        ..*limits
    };
    let steps = resolved
        .iter()
        .zip(redacted.iter())
        .map(|(sql, shown)| (sql.as_str(), shown.as_str()))
        .chain(setup.iter().map(|s| (s.as_str(), s.as_str())));
    for (i, (sql, shown)) in steps.enumerate() {
        if let Err(e) = run_to_end(backend, &session, sql, &ddl_limits).await {
            let _ = backend.close_session(&session).await;
            return Err(ApiError::statement(format!(
                "session setup statement {} ({}) failed: {e}",
                i + 1,
                headline(shown)
            )));
        }
    }
    Ok(session)
}
