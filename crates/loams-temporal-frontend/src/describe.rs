//! TMP-api group C (D2086): activity answers by ID, the reverse history,
//! `DescribeTaskQueue`, `GetClusterInfo`, `ListTaskQueuePartitions` and
//! `ShutdownWorker`.

use std::collections::HashMap;

use loams_temporal_history::time::ts;
use loams_temporal_matching::{QueueKey, TaskKind as QueueKind};
use loams_temporal_proto::loams::temporal::internal::v1::{TaskKind, TaskToken};
use loams_temporal_proto::temporal::api::common::v1::WorkflowExecution;
use loams_temporal_proto::temporal::api::deployment::v1::WorkerDeploymentOptions;
use loams_temporal_proto::temporal::api::enums::v1::TaskQueueType;
use loams_temporal_proto::temporal::api::history::v1::History;
use loams_temporal_proto::temporal::api::taskqueue::v1::{
    PollerInfo, TaskQueuePartitionMetadata, TaskQueueStats,
};
use loams_temporal_proto::temporal::api::workflowservice::v1::{
    DescribeTaskQueueRequest, DescribeTaskQueueResponse, GetClusterInfoResponse,
    GetWorkflowExecutionHistoryReverseRequest, GetWorkflowExecutionHistoryReverseResponse,
    ListTaskQueuePartitionsRequest, ListTaskQueuePartitionsResponse,
};
use loams_temporal_statemachine::{Input, Reply};
use tonic::Status;

use crate::auth::Principal;
use crate::service::Frontend;

/// The cluster name `GetClusterInfo` reports.
pub const CLUSTER_NAME: &str = "loams";

/// Which run and activity a `…ById` request names.
#[derive(Debug, Clone)]
pub(crate) struct ActivityRef<'a> {
    pub namespace: &'a str,
    pub workflow_id: &'a str,
    pub run_id: &'a str,
    pub activity_id: &'a str,
}

/// The matcher queue of a `DescribeTaskQueue` type (workflow when unset).
pub(crate) fn queue_kind(task_queue_type: i32) -> Result<QueueKind, Status> {
    match TaskQueueType::try_from(task_queue_type).unwrap_or_default() {
        TaskQueueType::Unspecified | TaskQueueType::Workflow => Ok(QueueKind::Workflow),
        TaskQueueType::Activity => Ok(QueueKind::Activity),
        other => Err(Status::invalid_argument(format!(
            "task queue type {} is not supported",
            other.as_str_name()
        ))),
    }
}

/// A reverse-history page: the event IDs of the page ending at `last`
/// (inclusive), at most `page` of them, and the next page's last ID (0 when
/// this page reaches event 1).
pub(crate) fn reverse_page(last: i64, page: usize) -> (i64, i64, i64) {
    let page = page.max(1) as i64;
    let first = (last - page + 1).max(1);
    (first, last, first - 1)
}

impl Frontend {
    /// Applies `make(scheduled_event_id, attempt)` to the activity a
    /// `…ById` request names (its current attempt).
    pub(crate) async fn activity_by_id(
        &self,
        p: &Principal,
        a: ActivityRef<'_>,
        make: impl FnOnce(i64, i32) -> Input,
    ) -> Result<Reply, Status> {
        if a.activity_id.is_empty() {
            return Err(Status::invalid_argument(
                "ActivityID is not set on request.",
            ));
        }
        let ns = self.namespace(p, a.namespace).await?;
        let key = self
            .run_key(
                &ns,
                Some(&WorkflowExecution {
                    workflow_id: a.workflow_id.to_string(),
                    run_id: a.run_id.to_string(),
                }),
            )
            .await?;
        let state = self.engine.load(&key).await?.state;
        let act = state
            .activities
            .iter()
            .find(|x| x.activity_id == a.activity_id)
            .ok_or_else(|| {
                Status::not_found(format!(
                    "Activity task not found (activity ID {}).",
                    a.activity_id
                ))
            })?;
        let (scheduled_event_id, attempt) = (act.scheduled_event_id, act.attempt);
        let t = self
            .engine
            .execute(&key, &Self::ctx(&ns), make(scheduled_event_id, attempt))
            .await?;
        if !matches!(t.reply, Reply::Heartbeat { .. }) {
            let token = TaskToken {
                namespace_id: key.namespace_id.clone(),
                workflow_id: key.workflow_id.clone(),
                run_id: key.run_id.clone(),
                scheduled_event_id,
                attempt,
                kind: TaskKind::Activity as i32,
                ..Default::default()
            };
            self.matcher.finished(&Self::promise_id(&ns, &key, &token));
        }
        Ok(t.reply)
    }

    /// `GetWorkflowExecutionHistoryReverse`: newest events first, paged
    /// backwards (the page token is the next page's last event ID).
    pub(crate) async fn history_reverse(
        &self,
        p: &Principal,
        r: GetWorkflowExecutionHistoryReverseRequest,
    ) -> Result<GetWorkflowExecutionHistoryReverseResponse, Status> {
        let ns = self.namespace(p, &r.namespace).await?;
        let key = self.run_key(&ns, r.execution.as_ref()).await?;
        let last = if r.next_page_token.len() == 8 {
            i64::from_be_bytes(r.next_page_token[..8].try_into().expect("8 bytes"))
        } else {
            self.engine.load(&key).await?.state.next_event_id - 1
        };
        let page = match r.maximum_page_size {
            n if n > 0 => n as usize,
            _ => self.config.history_page_size,
        };
        let (first, last, next) = reverse_page(last, page);
        let mut events = if last >= 1 {
            self.engine
                .history(&key, first, (last - first + 1) as usize)
                .await?
        } else {
            Vec::new()
        };
        events.reverse();
        Ok(GetWorkflowExecutionHistoryReverseResponse {
            history: Some(History { events }),
            next_page_token: if next >= 1 {
                next.to_be_bytes().to_vec().into()
            } else {
                Default::default()
            },
        })
    }

    /// `DescribeTaskQueue`: the pollers this gateway has seen (one replica
    /// owns every queue in DX1, X1.R8) and, when asked, the backlog.
    pub(crate) async fn describe_task_queue(
        &self,
        p: &Principal,
        r: DescribeTaskQueueRequest,
    ) -> Result<DescribeTaskQueueResponse, Status> {
        let ns = self.namespace(p, &r.namespace).await?;
        let name = r
            .task_queue
            .as_ref()
            .map(|t| t.name.clone())
            .filter(|n| !n.is_empty())
            .ok_or_else(|| Status::invalid_argument("TaskQueue is not set on request."))?;
        let key = QueueKey {
            namespace_id: ns.namespace_id.clone(),
            task_queue: name,
            kind: queue_kind(r.task_queue_type)?,
        };
        let pollers = self
            .matcher
            .pollers(&key, self.engine.now_ms())
            .into_iter()
            .map(|p| PollerInfo {
                last_access_time: Some(ts(p.last_access_ms)),
                identity: p.identity,
                rate_per_second: 0.0,
                deployment_options: (!p.build_id.is_empty()).then(|| WorkerDeploymentOptions {
                    build_id: p.build_id,
                    ..Default::default()
                }),
                ..Default::default()
            })
            .collect();
        let stats = r.report_stats.then(|| TaskQueueStats {
            approximate_backlog_count: self.matcher.backlog(&key) as i64,
            ..Default::default()
        });
        Ok(DescribeTaskQueueResponse {
            pollers,
            stats,
            ..Default::default()
        })
    }

    /// `GetClusterInfo`.
    pub(crate) fn cluster_info(&self) -> GetClusterInfoResponse {
        GetClusterInfoResponse {
            supported_clients: HashMap::new(),
            server_version: self.config.server_version.clone(),
            cluster_id: CLUSTER_NAME.to_string(),
            cluster_name: CLUSTER_NAME.to_string(),
            // The store's real shard count and kind when the standalone
            // runner set them (D2051), Temporal's defaults otherwise.
            history_shard_count: if self.config.history_shard_count > 0 {
                self.config.history_shard_count
            } else {
                loams_temporal_store::DEFAULT_SHARDS as i32
            },
            persistence_store: if self.config.persistence_store.is_empty() {
                "postgres".to_string()
            } else {
                self.config.persistence_store.clone()
            },
            visibility_store: "postgres".to_string(),
            initial_failover_version: 1,
            failover_version_increment: 10,
            ..Default::default()
        }
    }

    /// `ListTaskQueuePartitions`: one partition per queue, owned by this
    /// gateway (X1.R8).
    pub(crate) async fn task_queue_partitions(
        &self,
        p: &Principal,
        r: ListTaskQueuePartitionsRequest,
    ) -> Result<ListTaskQueuePartitionsResponse, Status> {
        self.namespace(p, &r.namespace).await?;
        let name = r
            .task_queue
            .map(|t| t.name)
            .filter(|n| !n.is_empty())
            .ok_or_else(|| Status::invalid_argument("TaskQueue is not set on request."))?;
        let one = || {
            vec![TaskQueuePartitionMetadata {
                key: name.clone(),
                owner_host_name: CLUSTER_NAME.to_string(),
            }]
        };
        Ok(ListTaskQueuePartitionsResponse {
            activity_task_queue_partitions: one(),
            workflow_task_queue_partitions: one(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reverse_pages_walk_back_to_event_one() {
        assert_eq!(reverse_page(10, 4), (7, 10, 6));
        assert_eq!(reverse_page(6, 4), (3, 6, 2));
        assert_eq!(reverse_page(2, 4), (1, 2, 0));
        assert_eq!(reverse_page(5, 0), (5, 5, 4));
    }

    #[test]
    fn task_queue_types_map_to_queues() {
        assert_eq!(queue_kind(0).expect("unset"), QueueKind::Workflow);
        assert_eq!(
            queue_kind(TaskQueueType::Activity as i32).expect("activity"),
            QueueKind::Activity
        );
        assert!(queue_kind(TaskQueueType::Nexus as i32).is_err());
    }
}
