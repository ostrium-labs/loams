//! `ListOpenWorkflowExecutions` and `ListClosedWorkflowExecutions`, the
//! older listing methods, served from the store's keyset-paged run listing
//! (`Store::list_runs`, #1442), never from a bulk read.
//!
//! The filters map onto the listing's own: a workflow ID or a workflow
//! type, and open or closed. A start-time window and a closed-status filter
//! are applied to each page as it is read, so a page can come back shorter
//! than asked; the token still moves on.

use loams_temporal_history::time::ts;
use loams_temporal_proto::temporal::api::common::v1::{WorkflowExecution, WorkflowType};
use loams_temporal_proto::temporal::api::filter::v1::StartTimeFilter;
use loams_temporal_proto::temporal::api::workflow::v1::WorkflowExecutionInfo;
use loams_temporal_proto::temporal::api::workflowservice::v1::{
    ListClosedWorkflowExecutionsRequest, ListClosedWorkflowExecutionsResponse,
    ListOpenWorkflowExecutionsRequest, ListOpenWorkflowExecutionsResponse,
    list_closed_workflow_executions_request, list_open_workflow_executions_request,
};
use loams_temporal_store::{ListRuns, ListedRun, MAX_PAGE_SIZE, RunFilter};
use tonic::Status;

use crate::auth::Principal;
use crate::engine::store_status;
use crate::service::Frontend;

/// The page size when the request names none.
pub const DEFAULT_LIST_PAGE: usize = 100;

pub(crate) fn list_page_size(n: i32) -> usize {
    match n {
        n if n > 0 => (n as usize).min(MAX_PAGE_SIZE),
        _ => DEFAULT_LIST_PAGE,
    }
}

/// Whether a start time falls in the request's window (an unset bound is
/// open).
pub(crate) fn in_window(f: Option<&StartTimeFilter>, start_ms: i64) -> bool {
    let Some(f) = f else { return true };
    let ms = |t: &prost_types::Timestamp| loams_temporal_history::time::ms(t);
    let after = f.earliest_time.as_ref().is_none_or(|t| start_ms >= ms(t));
    let before = f
        .latest_time
        .as_ref()
        .filter(|t| t.seconds != 0 || t.nanos != 0)
        .is_none_or(|t| start_ms <= ms(t));
    after && before
}

/// A listed run as the SDKs read it.
pub(crate) fn execution_info(r: &ListedRun) -> WorkflowExecutionInfo {
    let v = &r.visibility;
    WorkflowExecutionInfo {
        execution: Some(WorkflowExecution {
            workflow_id: r.key.workflow_id.clone(),
            run_id: r.key.run_id.clone(),
        }),
        r#type: Some(WorkflowType {
            name: v.workflow_type.clone(),
        }),
        start_time: Some(ts(v.start_time_ms)),
        close_time: v.close_time_ms.map(ts),
        status: v.status,
        execution_time: Some(ts(v.execution_time_ms)),
        task_queue: v.task_queue.clone(),
        ..Default::default()
    }
}

impl Frontend {
    async fn listed(
        &self,
        p: &Principal,
        namespace: &str,
        filter: RunFilter,
        page_size: i32,
        token: &[u8],
    ) -> Result<(Vec<ListedRun>, Vec<u8>), Status> {
        let ns = self.namespace(p, namespace).await?;
        let page = self
            .store()
            .list_runs(&ListRuns {
                namespace_id: ns.namespace_id.clone(),
                filter,
                page_size: list_page_size(page_size),
                page_token: token.to_vec(),
            })
            .await
            .map_err(store_status)?;
        Ok((page.runs, page.next_page_token))
    }

    /// `ListOpenWorkflowExecutions`.
    pub(crate) async fn list_open(
        &self,
        p: &Principal,
        r: ListOpenWorkflowExecutionsRequest,
    ) -> Result<ListOpenWorkflowExecutionsResponse, Status> {
        use list_open_workflow_executions_request::Filters;
        let mut filter = RunFilter {
            open: Some(true),
            ..Default::default()
        };
        match &r.filters {
            Some(Filters::ExecutionFilter(e)) => filter.workflow_id = Some(e.workflow_id.clone()),
            Some(Filters::TypeFilter(t)) => filter.workflow_type = Some(t.name.clone()),
            None => {}
        }
        let (runs, next) = self
            .listed(
                p,
                &r.namespace,
                filter,
                r.maximum_page_size,
                &r.next_page_token,
            )
            .await?;
        Ok(ListOpenWorkflowExecutionsResponse {
            executions: runs
                .iter()
                .filter(|x| in_window(r.start_time_filter.as_ref(), x.visibility.start_time_ms))
                .map(execution_info)
                .collect(),
            next_page_token: next.into(),
        })
    }

    /// `ListClosedWorkflowExecutions`.
    pub(crate) async fn list_closed(
        &self,
        p: &Principal,
        r: ListClosedWorkflowExecutionsRequest,
    ) -> Result<ListClosedWorkflowExecutionsResponse, Status> {
        use list_closed_workflow_executions_request::Filters;
        let mut filter = RunFilter {
            open: Some(false),
            ..Default::default()
        };
        let mut status = None;
        match &r.filters {
            Some(Filters::ExecutionFilter(e)) => filter.workflow_id = Some(e.workflow_id.clone()),
            Some(Filters::TypeFilter(t)) => filter.workflow_type = Some(t.name.clone()),
            Some(Filters::StatusFilter(s)) => status = Some(s.status),
            None => {}
        }
        let (runs, next) = self
            .listed(
                p,
                &r.namespace,
                filter,
                r.maximum_page_size,
                &r.next_page_token,
            )
            .await?;
        Ok(ListClosedWorkflowExecutionsResponse {
            executions: runs
                .iter()
                .filter(|x| in_window(r.start_time_filter.as_ref(), x.visibility.start_time_ms))
                .filter(|x| status.is_none_or(|s| x.visibility.status == s))
                .map(execution_info)
                .collect(),
            next_page_token: next.into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_sizes_default_and_cap() {
        assert_eq!(list_page_size(0), DEFAULT_LIST_PAGE);
        assert_eq!(list_page_size(7), 7);
        assert_eq!(list_page_size(i32::MAX), MAX_PAGE_SIZE);
    }

    #[test]
    fn start_windows_are_inclusive_and_open_when_unset() {
        let f = StartTimeFilter {
            earliest_time: Some(ts(1_000)),
            latest_time: Some(ts(2_000)),
        };
        assert!(in_window(Some(&f), 1_000));
        assert!(in_window(Some(&f), 2_000));
        assert!(!in_window(Some(&f), 999));
        assert!(!in_window(Some(&f), 2_001));
        assert!(in_window(None, 5));
        let open_end = StartTimeFilter {
            earliest_time: Some(ts(1_000)),
            latest_time: None,
        };
        assert!(in_window(Some(&open_end), 9_999_999));
    }
}
