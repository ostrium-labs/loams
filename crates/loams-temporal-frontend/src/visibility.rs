//! Visibility (D2074-D2076): `ListWorkflowExecutions`,
//! `ScanWorkflowExecutions`, `CountWorkflowExecutions` and
//! `GetSearchAttributes`, and the search-attribute registry the
//! `OperatorService` changes ([`crate::operator`]).
//!
//! Search attributes are stored decoded: a `json/plain` payload's JSON value
//! goes into the visibility row's `search_attrs` under its key (other
//! encodings are not indexable and are skipped). Answers encode them back as
//! `json/plain` payloads with the attribute's `type` metadata.
//!
//! Page tokens are the offset of the next page (8 bytes, big-endian). Runs
//! that start or close between pages can shift a page; Temporal's SQL
//! visibility has the same property.

use std::collections::BTreeMap;
use std::sync::Arc;

use bytes::Bytes;
use loams_temporal_proto::temporal::api::common::v1::{
    Memo, Payload, SearchAttributes, WorkflowExecution, WorkflowType,
};
use loams_temporal_proto::temporal::api::workflow::v1::WorkflowExecutionInfo;
use loams_temporal_proto::temporal::api::workflowservice::v1::count_workflow_executions_response::AggregationGroup;
use loams_temporal_proto::temporal::api::workflowservice::v1::*;
use loams_temporal_schedule::ScheduleRecord;
use loams_temporal_schedule::spec::{ms_duration, ms_ts};
use loams_temporal_store::{NamespaceRecord, RunKey, VisibilityRow};
use loams_temporal_visibility::filter::{status_name, system_attributes};
use loams_temporal_visibility::{
    QueryError, Record, Resolved, ValueType, VisibilityError, VisibilityStore, compile,
};
use prost::Message as _;
use tonic::Status;

use crate::auth::Principal;
use crate::schedules::{ScheduleFilter, json_payload};
use crate::service::Frontend;

/// The default and largest page of a list.
const DEFAULT_PAGE: usize = 1000;
const MAX_PAGE: usize = 1000;

pub(crate) fn query_status(e: QueryError) -> Status {
    Status::invalid_argument(e.to_string())
}

pub(crate) fn visibility_status(e: VisibilityError) -> Status {
    match e {
        VisibilityError::InvalidArgument(m) => Status::invalid_argument(m),
        VisibilityError::Conflict(n) => Status::already_exists(format!(
            "search attribute {n} already exists with another type"
        )),
        VisibilityError::NotFound(n) => {
            Status::not_found(format!("search attribute {n} not found"))
        }
        VisibilityError::Unavailable(m) => Status::unavailable(m),
        other => Status::internal(other.to_string()),
    }
}

/// A payload's JSON value, if it is `json/plain`.
pub fn payload_json(p: &Payload) -> Option<serde_json::Value> {
    let encoding = p.metadata.get("encoding")?;
    if encoding.as_ref() != b"json/plain" {
        return None;
    }
    serde_json::from_slice(&p.data).ok()
}

/// Search attributes as the visibility row stores them.
pub fn search_attrs_json(sa: Option<&SearchAttributes>) -> serde_json::Value {
    let mut out = serde_json::Map::new();
    if let Some(sa) = sa {
        for (k, p) in &sa.indexed_fields {
            if let Some(v) = payload_json(p) {
                out.insert(k.clone(), v);
            }
        }
    }
    serde_json::Value::Object(out)
}

/// The JSON of a visibility row's search attributes as payloads, typed from
/// the registry (an unregistered key is sent as Keyword).
pub fn search_attrs_payloads(
    attrs: &serde_json::Map<String, serde_json::Value>,
    types: &BTreeMap<String, ValueType>,
) -> Option<SearchAttributes> {
    if attrs.is_empty() {
        return None;
    }
    Some(SearchAttributes {
        indexed_fields: attrs
            .iter()
            .map(|(k, v)| {
                let t = types.get(k).copied().unwrap_or(ValueType::Keyword);
                (k.clone(), json_payload(v, t.name()))
            })
            .collect(),
    })
}

/// A visibility row as a [`Record`].
pub fn record(key: &RunKey, row: &VisibilityRow) -> Record {
    Record {
        workflow_id: key.workflow_id.clone(),
        run_id: key.run_id.clone(),
        workflow_type: row.workflow_type.clone(),
        status: row.status,
        task_queue: row.task_queue.clone(),
        start_ms: row.start_time_ms,
        close_ms: row.close_time_ms,
        execution_ms: row.execution_time_ms,
        search_attrs: row.search_attrs.as_object().cloned().unwrap_or_default(),
        memo: row.memo.clone(),
    }
}

fn info(r: &Record, types: &BTreeMap<String, ValueType>) -> WorkflowExecutionInfo {
    WorkflowExecutionInfo {
        execution: Some(WorkflowExecution {
            workflow_id: r.workflow_id.clone(),
            run_id: r.run_id.clone(),
        }),
        r#type: Some(WorkflowType {
            name: r.workflow_type.clone(),
        }),
        start_time: Some(ms_ts(r.start_ms)),
        close_time: r.close_ms.map(ms_ts),
        status: r.status,
        execution_time: Some(ms_ts(r.execution_ms)),
        memo: r.memo.as_deref().and_then(|m| Memo::decode(m).ok()),
        search_attributes: search_attrs_payloads(&r.search_attrs, types),
        task_queue: r.task_queue.clone(),
        execution_duration: r.close_ms.map(|c| ms_duration(c - r.start_ms)),
        ..Default::default()
    }
}

fn page_size(n: i32) -> usize {
    if n > 0 {
        (n as usize).min(MAX_PAGE)
    } else {
        DEFAULT_PAGE
    }
}

fn offset(token: &[u8]) -> Result<usize, Status> {
    match token.len() {
        0 => Ok(0),
        8 => {
            let mut b = [0u8; 8];
            b.copy_from_slice(token);
            usize::try_from(u64::from_be_bytes(b))
                .map_err(|_| Status::invalid_argument("invalid next_page_token"))
        }
        _ => Err(Status::invalid_argument("invalid next_page_token")),
    }
}

/// The types of every attribute a namespace can query.
fn all_types(custom: &BTreeMap<String, ValueType>) -> BTreeMap<String, ValueType> {
    let mut t = system_attributes();
    t.extend(custom.iter().map(|(k, v)| (k.clone(), *v)));
    t
}

/// A schedule as a [`Record`] for ListSchedules' query: its ID as the
/// workflow ID, its action's workflow type, `TemporalSchedulePaused`, and
/// its own search attributes.
pub(crate) fn schedule_record(rec: &ScheduleRecord) -> Record {
    let mut attrs = match search_attrs_json(rec.search_attributes.as_ref()) {
        serde_json::Value::Object(m) => m,
        _ => serde_json::Map::new(),
    };
    attrs.insert(
        "TemporalSchedulePaused".into(),
        serde_json::Value::Bool(rec.paused()),
    );
    let workflow_type = match rec.schedule.action.as_ref().and_then(|a| a.action.as_ref()) {
        Some(loams_temporal_proto::temporal::api::schedule::v1::schedule_action::Action::StartWorkflow(i)) => {
            i.workflow_type.as_ref().map(|t| t.name.clone()).unwrap_or_default()
        }
        None => String::new(),
    };
    Record {
        workflow_id: rec.schedule_id.clone(),
        run_id: rec.generation.clone(),
        workflow_type,
        status: 1,
        start_ms: rec.runtime.create_time_ms,
        execution_ms: rec.runtime.create_time_ms,
        search_attrs: attrs,
        ..Default::default()
    }
}

impl Frontend {
    /// With the memory visibility store, feeds it every committed row, in
    /// commit order (Postgres reads the rows the commits write).
    pub(crate) fn attach_visibility(&self) {
        if let Some(m) = self.visibility_feed.clone() {
            self.engine
                .set_visibility_tap(Arc::new(move |key: &RunKey, row: &VisibilityRow| {
                    m.put(&key.namespace_id, record(key, row))
                }));
        }
    }

    async fn resolved(
        &self,
        ns: &NamespaceRecord,
        query: &str,
    ) -> Result<(Resolved, BTreeMap<String, ValueType>), Status> {
        let custom = self
            .visibility
            .custom_attributes(&ns.namespace_id)
            .await
            .map_err(visibility_status)?;
        let r = compile(query, &custom).map_err(query_status)?;
        Ok((r, all_types(&custom)))
    }

    pub(crate) async fn list_page(
        &self,
        p: &Principal,
        namespace: &str,
        query: &str,
        size: i32,
        token: &[u8],
    ) -> Result<(Vec<WorkflowExecutionInfo>, Bytes), Status> {
        let ns = self.namespace(p, namespace).await?;
        let (q, types) = self.resolved(&ns, query).await?;
        if q.group_by_status {
            return Err(Status::invalid_argument(
                "GROUP BY is only allowed in CountWorkflowExecutions",
            ));
        }
        let size = page_size(size);
        let from = offset(token)?;
        // One more than a page tells whether there is a next one.
        let mut rows = self
            .visibility
            .list(&ns.namespace_id, &q, size + 1, from)
            .await
            .map_err(visibility_status)?;
        let more = rows.len() > size;
        rows.truncate(size);
        let next = if more {
            Bytes::copy_from_slice(&((from + size) as u64).to_be_bytes())
        } else {
            Bytes::new()
        };
        Ok((rows.iter().map(|r| info(r, &types)).collect(), next))
    }

    pub(crate) async fn list_workflow_executions(
        &self,
        p: &Principal,
        r: ListWorkflowExecutionsRequest,
    ) -> Result<ListWorkflowExecutionsResponse, Status> {
        let (executions, next_page_token) = self
            .list_page(p, &r.namespace, &r.query, r.page_size, &r.next_page_token)
            .await?;
        Ok(ListWorkflowExecutionsResponse {
            executions,
            next_page_token,
        })
    }

    pub(crate) async fn scan_workflow_executions(
        &self,
        p: &Principal,
        r: ScanWorkflowExecutionsRequest,
    ) -> Result<ScanWorkflowExecutionsResponse, Status> {
        // Scan is List without an order guarantee; the same pages serve it.
        let (executions, next_page_token) = self
            .list_page(p, &r.namespace, &r.query, r.page_size, &r.next_page_token)
            .await?;
        Ok(ScanWorkflowExecutionsResponse {
            executions,
            next_page_token,
        })
    }

    pub(crate) async fn count_workflow_executions(
        &self,
        p: &Principal,
        r: CountWorkflowExecutionsRequest,
    ) -> Result<CountWorkflowExecutionsResponse, Status> {
        let ns = self.namespace(p, &r.namespace).await?;
        let (q, _) = self.resolved(&ns, &r.query).await?;
        if !q.order_by.is_empty() {
            return Err(Status::invalid_argument(
                "ORDER BY is not allowed in CountWorkflowExecutions",
            ));
        }
        if !q.group_by_status {
            let count = self
                .visibility
                .count(&ns.namespace_id, &q)
                .await
                .map_err(visibility_status)?;
            return Ok(CountWorkflowExecutionsResponse {
                count,
                groups: Vec::new(),
            });
        }
        let by = self
            .visibility
            .count_by_status(&ns.namespace_id, &q)
            .await
            .map_err(visibility_status)?;
        Ok(CountWorkflowExecutionsResponse {
            count: by.iter().map(|(_, n)| n).sum(),
            groups: by
                .into_iter()
                .map(|(status, count)| AggregationGroup {
                    group_values: vec![json_payload(
                        &serde_json::Value::String(status_name(status).to_string()),
                        "Keyword",
                    )],
                    count,
                })
                .collect(),
        })
    }

    pub(crate) async fn get_search_attributes(
        &self,
        p: &Principal,
    ) -> Result<GetSearchAttributesResponse, Status> {
        // No namespace in the request: the system attributes and the custom
        // ones of every namespace of the caller.
        let mut keys: BTreeMap<String, ValueType> = system_attributes();
        let namespaces = self
            .store()
            .list_namespaces(&p.loams_namespace_id)
            .await
            .map_err(crate::engine::store_status)?;
        for ns in namespaces {
            let custom = self
                .visibility
                .custom_attributes(&ns.namespace_id)
                .await
                .map_err(visibility_status)?;
            keys.extend(custom);
        }
        Ok(GetSearchAttributesResponse {
            keys: keys.into_iter().map(|(k, t)| (k, t.to_i32())).collect(),
        })
    }

    /// The custom attributes of a namespace (for the `OperatorService`).
    pub(crate) async fn custom_search_attributes(
        &self,
        ns: &NamespaceRecord,
    ) -> Result<BTreeMap<String, ValueType>, Status> {
        self.visibility
            .custom_attributes(&ns.namespace_id)
            .await
            .map_err(visibility_status)
    }

    pub(crate) async fn add_search_attributes(
        &self,
        ns: &NamespaceRecord,
        attrs: &BTreeMap<String, ValueType>,
    ) -> Result<(), Status> {
        self.visibility
            .add_attributes(&ns.namespace_id, attrs)
            .await
            .map_err(visibility_status)
    }

    pub(crate) async fn remove_search_attributes(
        &self,
        ns: &NamespaceRecord,
        names: &[String],
    ) -> Result<(), Status> {
        self.visibility
            .remove_attributes(&ns.namespace_id, names)
            .await
            .map_err(visibility_status)
    }

    /// ListSchedules' filter: the query evaluated on [`schedule_record`].
    pub(crate) async fn schedule_query(
        &self,
        ns: &NamespaceRecord,
        query: &str,
    ) -> Result<ScheduleFilter, Status> {
        if query.trim().is_empty() {
            return Ok(ScheduleFilter::all());
        }
        let (q, _) = self.resolved(ns, query).await?;
        if q.group_by_status || !q.order_by.is_empty() {
            return Err(Status::invalid_argument(
                "ListSchedules queries take a filter only",
            ));
        }
        Ok(ScheduleFilter::new(move |rec| {
            loams_temporal_visibility::eval::passes(&q, &schedule_record(rec))
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payloads_round_trip_through_json() {
        let mut sa = SearchAttributes::default();
        sa.indexed_fields.insert(
            "CustomerId".into(),
            json_payload(&serde_json::json!("c-1"), "Keyword"),
        );
        sa.indexed_fields.insert(
            "Amount".into(),
            json_payload(&serde_json::json!(12.5), "Double"),
        );
        // Not json/plain: skipped.
        sa.indexed_fields.insert(
            "Raw".into(),
            Payload {
                data: Bytes::from_static(b"\x01"),
                ..Default::default()
            },
        );
        let j = search_attrs_json(Some(&sa));
        assert_eq!(j, serde_json::json!({"CustomerId": "c-1", "Amount": 12.5}));
        let types = BTreeMap::from([("Amount".to_string(), ValueType::Double)]);
        let back = search_attrs_payloads(j.as_object().unwrap(), &types).unwrap();
        assert_eq!(
            payload_json(&back.indexed_fields["Amount"]),
            Some(serde_json::json!(12.5))
        );
        assert_eq!(
            back.indexed_fields["Amount"]
                .metadata
                .get("type")
                .map(|b| b.as_ref()),
            Some(&b"Double"[..])
        );
        assert_eq!(
            back.indexed_fields["CustomerId"]
                .metadata
                .get("type")
                .map(|b| b.as_ref()),
            Some(&b"Keyword"[..])
        );
        assert!(search_attrs_payloads(&serde_json::Map::new(), &types).is_none());
    }

    #[test]
    fn page_tokens() {
        assert_eq!(offset(&[]).unwrap(), 0);
        assert_eq!(offset(&42u64.to_be_bytes()).unwrap(), 42);
        assert!(offset(&[1, 2]).is_err());
        assert_eq!(page_size(0), DEFAULT_PAGE);
        assert_eq!(page_size(5), 5);
        assert_eq!(page_size(1_000_000), MAX_PAGE);
    }

    #[test]
    fn infos_carry_times_and_duration() {
        let r = Record {
            workflow_id: "w".into(),
            run_id: "r".into(),
            workflow_type: "t".into(),
            status: 2,
            start_ms: 1_000,
            close_ms: Some(61_000),
            execution_ms: 1_000,
            ..Default::default()
        };
        let i = info(&r, &BTreeMap::new());
        assert_eq!(i.execution_duration.unwrap().seconds, 60);
        assert_eq!(i.close_time.unwrap().seconds, 61);
        assert!(i.search_attributes.is_none());
    }
}
