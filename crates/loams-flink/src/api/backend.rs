//! The seam between the API and a Flink SQL Gateway (D1988).
//!
//! [`FlinkSqlBackend`] is the SQL Gateway's REST surface (v3):
//! sessions, statements as operations, paged results. FL-a's
//! [`crate::sql_gateway::SqlGatewayClient`] implements it
//! ([`super::sql_gateway_adapter`]); tests use an in-memory fake.

use std::collections::BTreeMap;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// A SQL Gateway session.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionHandle(pub String);

/// A statement running in a session.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OperationHandle(pub String);

/// Whether a page has data, is not ready yet, or ends the results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ResultType {
    NotReady,
    Payload,
    Eos,
}

/// A result column.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResultColumn {
    pub name: String,
    /// The logical type's root (`VARCHAR`, `BIGINT`, `TIMESTAMP_WITH_LOCAL_TIME_ZONE`).
    #[serde(rename = "type")]
    pub data_type: String,
    #[serde(default)]
    pub nullable: bool,
}

/// A changelog row's kind, spelled as Flink prints it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RowKind {
    #[serde(rename = "+I")]
    Insert,
    #[serde(rename = "-U")]
    UpdateBefore,
    #[serde(rename = "+U")]
    UpdateAfter,
    #[serde(rename = "-D")]
    Delete,
}

impl RowKind {
    /// From the SQL Gateway's JSON (`INSERT`, `UPDATE_BEFORE`, …).
    pub fn from_gateway(kind: &str) -> Option<Self> {
        Some(match kind {
            "INSERT" => Self::Insert,
            "UPDATE_BEFORE" => Self::UpdateBefore,
            "UPDATE_AFTER" => Self::UpdateAfter,
            "DELETE" => Self::Delete,
            _ => return None,
        })
    }

    pub fn short(self) -> &'static str {
        match self {
            Self::Insert => "+I",
            Self::UpdateBefore => "-U",
            Self::UpdateAfter => "+U",
            Self::Delete => "-D",
        }
    }
}

/// A row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResultRow {
    pub kind: RowKind,
    pub fields: Vec<serde_json::Value>,
}

/// One page of results.
#[derive(Debug, Clone, PartialEq)]
pub struct ResultPage {
    pub result_type: ResultType,
    pub columns: Vec<ResultColumn>,
    pub rows: Vec<ResultRow>,
    /// The Flink job the statement submitted (`INSERT`, a streaming
    /// `SELECT`).
    pub job_id: Option<String>,
    /// Whether the statement is a query (rows are its results) rather than
    /// DDL or a job submission.
    pub is_query: bool,
    /// The token of the next page; `None` after the last.
    pub next_token: Option<u64>,
}

/// What can go wrong talking to the gateway.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BackendError {
    #[error("the Flink SQL Gateway is unreachable: {0}")]
    Unavailable(String),
    #[error("the Flink SQL Gateway has no such session or operation")]
    NotFound,
    /// The statement itself failed (parse, validation, runtime). The message
    /// is the root cause, not the stack trace.
    #[error("{0}")]
    Statement(String),
    #[error("the Flink SQL Gateway answered {status}: {message}")]
    Gateway { status: u16, message: String },
    #[error("unexpected answer from the Flink SQL Gateway: {0}")]
    Protocol(String),
}

/// The SQL Gateway, as the API uses it.
#[async_trait]
pub trait FlinkSqlBackend: Send + Sync + 'static {
    async fn open_session(
        &self,
        name: &str,
        properties: &BTreeMap<String, String>,
    ) -> Result<SessionHandle, BackendError>;
    async fn close_session(&self, session: &SessionHandle) -> Result<(), BackendError>;
    async fn heartbeat(&self, session: &SessionHandle) -> Result<(), BackendError>;
    async fn execute(
        &self,
        session: &SessionHandle,
        statement: &str,
    ) -> Result<OperationHandle, BackendError>;
    async fn fetch(
        &self,
        session: &SessionHandle,
        op: &OperationHandle,
        token: u64,
    ) -> Result<ResultPage, BackendError>;
    async fn cancel(
        &self,
        session: &SessionHandle,
        op: &OperationHandle,
    ) -> Result<(), BackendError>;
    async fn close_operation(
        &self,
        session: &SessionHandle,
        op: &OperationHandle,
    ) -> Result<(), BackendError>;
}

/// The root cause in the gateway's `errors` list: Flink sends a generic
/// first entry and a stack trace; the last `Caused by:` line is what a
/// person needs.
pub fn summarize_errors(errors: &[String]) -> String {
    let mut best: Option<&str> = None;
    for e in errors {
        for line in e.lines() {
            let line = line.trim();
            if let Some(cause) = line.strip_prefix("Caused by: ") {
                best = Some(cause);
            }
        }
    }
    if let Some(b) = best {
        return b.to_string();
    }
    errors
        .iter()
        .flat_map(|e| e.lines())
        .map(str::trim)
        .find(|l| !l.is_empty() && *l != "Internal server error.")
        .unwrap_or("the statement failed")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_cause_is_the_last_caused_by() {
        let errors = vec![
            "Internal server error.".to_string(),
            "<Exception on server side:\norg.apache.flink.table.gateway.api.utils.SqlGatewayException: x\n\tat a.b\nCaused by: org.apache.flink.table.api.ValidationException: outer\nCaused by: org.apache.flink.table.api.ValidationException: Object 'nope' not found\n\tat c.d".to_string(),
        ];
        assert_eq!(
            summarize_errors(&errors),
            "org.apache.flink.table.api.ValidationException: Object 'nope' not found"
        );
        assert_eq!(
            summarize_errors(&["Internal server error.".into(), "Session not found".into()]),
            "Session not found"
        );
        assert_eq!(summarize_errors(&[]), "the statement failed");
    }

    #[test]
    fn row_kinds() {
        assert_eq!(
            RowKind::from_gateway("UPDATE_AFTER"),
            Some(RowKind::UpdateAfter)
        );
        assert_eq!(RowKind::from_gateway("x"), None);
        assert_eq!(
            serde_json::to_string(&RowKind::Delete).unwrap_or_default(),
            "\"-D\""
        );
    }
}
