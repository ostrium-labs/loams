//! [`FlinkSqlBackend`] for FL-a's [`SqlGatewayClient`], so the API drives
//! the same SQL Gateway client as the controller (engines-81.md, FL-a).

use std::collections::BTreeMap;

use async_trait::async_trait;

use super::backend::{
    BackendError, FlinkSqlBackend, OperationHandle, ResultColumn, ResultPage, ResultRow,
    ResultType, RowKind, SessionHandle, summarize_errors,
};
use crate::error::FlinkError;
use crate::sql_gateway::{self as sg, ExecuteStatement, OpenSession, RowFormat, SqlGatewayClient};

impl From<FlinkError> for BackendError {
    fn from(e: FlinkError) -> Self {
        match e {
            FlinkError::Transport { .. } | FlinkError::Timeout(_) => {
                BackendError::Unavailable(e.to_string())
            }
            FlinkError::Api { status: 404, .. } => BackendError::NotFound,
            // A failed statement comes back as a 500 on fetch (or a 400 on
            // execute for a parse error), with the cause in `errors`.
            FlinkError::Api {
                status: 400 | 500,
                ref errors,
                ..
            } => BackendError::Statement(summarize_errors(errors)),
            FlinkError::Api {
                status, message, ..
            } => BackendError::Gateway { status, message },
            FlinkError::Failed(m) => BackendError::Statement(m),
            FlinkError::Decode { .. } | FlinkError::Unsupported(_) | FlinkError::Invalid(_) => {
                BackendError::Protocol(e.to_string())
            }
        }
    }
}

fn session(h: &SessionHandle) -> Result<sg::SessionHandle, BackendError> {
    sg::SessionHandle::new(h.0.clone()).map_err(BackendError::from)
}

fn operation(h: &OperationHandle) -> Result<sg::OperationHandle, BackendError> {
    sg::OperationHandle::new(h.0.clone()).map_err(BackendError::from)
}

/// The SQL Gateway's page as the API's.
pub fn convert_page(p: sg::ResultPage) -> ResultPage {
    let next_token = match p.result_type {
        sg::ResultType::Eos => None,
        _ => p.next_token(),
    };
    let result_type = match p.result_type {
        sg::ResultType::NotReady => ResultType::NotReady,
        sg::ResultType::Payload => ResultType::Payload,
        sg::ResultType::Eos => ResultType::Eos,
    };
    let columns = p
        .results
        .columns
        .into_iter()
        .map(|c| ResultColumn {
            name: c.name,
            data_type: c
                .logical_type
                .get("type")
                .and_then(|t| t.as_str())
                .unwrap_or_default()
                .to_string(),
            nullable: c
                .logical_type
                .get("nullable")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true),
        })
        .collect();
    let rows = p
        .results
        .data
        .into_iter()
        .map(|r| ResultRow {
            kind: match r.kind {
                sg::RowKind::Insert => RowKind::Insert,
                sg::RowKind::UpdateBefore => RowKind::UpdateBefore,
                sg::RowKind::UpdateAfter => RowKind::UpdateAfter,
                sg::RowKind::Delete => RowKind::Delete,
            },
            fields: r.fields,
        })
        .collect();
    ResultPage {
        result_type,
        columns,
        rows,
        job_id: p.job_id,
        is_query: p.is_query_result.unwrap_or(false),
        next_token,
    }
}

#[async_trait]
impl FlinkSqlBackend for SqlGatewayClient {
    async fn open_session(
        &self,
        name: &str,
        properties: &BTreeMap<String, String>,
    ) -> Result<SessionHandle, BackendError> {
        let h = SqlGatewayClient::open_session(
            self,
            &OpenSession {
                session_name: Some(name.to_string()),
                properties: properties.clone(),
            },
        )
        .await?;
        Ok(SessionHandle(h.as_str().to_string()))
    }

    async fn close_session(&self, s: &SessionHandle) -> Result<(), BackendError> {
        SqlGatewayClient::close_session(self, &session(s)?).await?;
        Ok(())
    }

    async fn heartbeat(&self, s: &SessionHandle) -> Result<(), BackendError> {
        SqlGatewayClient::heartbeat(self, &session(s)?).await?;
        Ok(())
    }

    async fn execute(
        &self,
        s: &SessionHandle,
        statement: &str,
    ) -> Result<OperationHandle, BackendError> {
        let op = self
            .execute_statement(&session(s)?, &ExecuteStatement::new(statement))
            .await?;
        Ok(OperationHandle(op.as_str().to_string()))
    }

    async fn fetch(
        &self,
        s: &SessionHandle,
        op: &OperationHandle,
        token: u64,
    ) -> Result<ResultPage, BackendError> {
        let p = self
            .fetch_result(&session(s)?, &operation(op)?, token, RowFormat::Json)
            .await?;
        Ok(convert_page(p))
    }

    async fn cancel(&self, s: &SessionHandle, op: &OperationHandle) -> Result<(), BackendError> {
        self.cancel_operation(&session(s)?, &operation(op)?).await?;
        Ok(())
    }

    async fn close_operation(
        &self,
        s: &SessionHandle,
        op: &OperationHandle,
    ) -> Result<(), BackendError> {
        SqlGatewayClient::close_operation(self, &session(s)?, &operation(op)?).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_a_gateway_page() {
        let p: sg::ResultPage = serde_json::from_value(serde_json::json!({
            "resultType": "PAYLOAD",
            "isQueryResult": true,
            "jobID": "a1",
            "resultKind": "SUCCESS_WITH_CONTENT",
            "results": {
                "columns": [{"name": "n", "logicalType": {"type": "BIGINT", "nullable": false}}],
                "rowFormat": "JSON",
                "data": [{"kind": "UPDATE_AFTER", "fields": [2]}]
            },
            "nextResultUri": "/v3/sessions/s/operations/o/result/4?rowFormat=JSON"
        }))
        .unwrap_or_else(|e| panic!("{e}"));
        let page = convert_page(p);
        assert_eq!(page.result_type, ResultType::Payload);
        assert_eq!(page.columns[0].data_type, "BIGINT");
        assert!(!page.columns[0].nullable);
        assert_eq!(page.rows[0].kind, RowKind::UpdateAfter);
        assert_eq!(page.job_id.as_deref(), Some("a1"));
        assert!(page.is_query);
        assert_eq!(page.next_token, Some(4));
    }
}
