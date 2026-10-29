//! Visibility on the in-process gateway (D2074-D2076): the memory store fed
//! from commits, the query language, counts and the search-attribute
//! registry.

mod common;

use std::collections::HashMap;

use bytes::Bytes;
use loams_temporal_proto::temporal::api::common::v1::{
    Payload, SearchAttributes, WorkflowExecution, WorkflowType,
};
use loams_temporal_proto::temporal::api::enums::v1::IndexedValueType;
use loams_temporal_proto::temporal::api::operatorservice::v1::operator_service_client::OperatorServiceClient;
use loams_temporal_proto::temporal::api::operatorservice::v1::{
    AddSearchAttributesRequest, ListSearchAttributesRequest, RemoveSearchAttributesRequest,
};
use loams_temporal_proto::temporal::api::taskqueue::v1::TaskQueue;
use loams_temporal_proto::temporal::api::workflowservice::v1::workflow_service_client::WorkflowServiceClient;
use loams_temporal_proto::temporal::api::workflowservice::v1::*;

fn keyword(v: &str) -> Payload {
    let mut p = Payload {
        data: Bytes::from(format!("{v:?}")),
        ..Default::default()
    };
    p.metadata
        .insert("encoding".into(), Bytes::from_static(b"json/plain"));
    p.metadata
        .insert("type".into(), Bytes::from_static(b"Keyword"));
    p
}

fn start(id: &str, wf_type: &str, customer: &str) -> StartWorkflowExecutionRequest {
    StartWorkflowExecutionRequest {
        namespace: "default".into(),
        workflow_id: id.into(),
        workflow_type: Some(WorkflowType {
            name: wf_type.into(),
        }),
        task_queue: Some(TaskQueue {
            name: "q".into(),
            ..Default::default()
        }),
        request_id: format!("req-{id}"),
        search_attributes: Some(SearchAttributes {
            indexed_fields: HashMap::from([("CustomerId".to_string(), keyword(customer))]),
        }),
        ..Default::default()
    }
}

async fn list(
    c: &mut WorkflowServiceClient<tonic::transport::Channel>,
    query: &str,
) -> Vec<String> {
    let mut ids: Vec<String> = c
        .list_workflow_executions(ListWorkflowExecutionsRequest {
            namespace: "default".into(),
            query: query.into(),
            ..Default::default()
        })
        .await
        .expect("list")
        .into_inner()
        .executions
        .into_iter()
        .map(|e| e.execution.expect("execution").workflow_id)
        .collect();
    ids.sort();
    ids
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_count_and_registry() {
    let gw = common::start().await;
    let mut c = gw.client.clone();
    let mut op = OperatorServiceClient::new(gw.channel.clone());

    // Unregistered: a query on it is refused.
    let e = c
        .list_workflow_executions(ListWorkflowExecutionsRequest {
            namespace: "default".into(),
            query: "CustomerId = 'c1'".into(),
            ..Default::default()
        })
        .await
        .expect_err("unknown attribute");
    assert_eq!(e.code(), tonic::Code::InvalidArgument);

    op.add_search_attributes(AddSearchAttributesRequest {
        namespace: "default".into(),
        search_attributes: HashMap::from([(
            "CustomerId".to_string(),
            IndexedValueType::Keyword as i32,
        )]),
    })
    .await
    .expect("add");
    let listed = op
        .list_search_attributes(ListSearchAttributesRequest {
            namespace: "default".into(),
        })
        .await
        .expect("list attributes")
        .into_inner();
    assert_eq!(
        listed.custom_attributes.get("CustomerId"),
        Some(&(IndexedValueType::Keyword as i32))
    );

    for (id, t, cust) in [
        ("o1", "order", "c1"),
        ("o2", "order", "c2"),
        ("p1", "payment", "c1"),
    ] {
        c.start_workflow_execution(start(id, t, cust))
            .await
            .expect("start");
    }
    c.terminate_workflow_execution(TerminateWorkflowExecutionRequest {
        namespace: "default".into(),
        workflow_execution: Some(WorkflowExecution {
            workflow_id: "o2".into(),
            run_id: String::new(),
        }),
        reason: "test".into(),
        ..Default::default()
    })
    .await
    .expect("terminate");

    assert_eq!(list(&mut c, "").await, ["o1", "o2", "p1"]);
    assert_eq!(list(&mut c, "CustomerId = 'c1'").await, ["o1", "p1"]);
    assert_eq!(
        list(
            &mut c,
            "WorkflowType = 'order' AND ExecutionStatus = 'Running'"
        )
        .await,
        ["o1"]
    );
    assert_eq!(list(&mut c, "ExecutionStatus = 'Terminated'").await, ["o2"]);
    assert_eq!(
        list(&mut c, "WorkflowId STARTS_WITH 'o'").await,
        ["o1", "o2"]
    );

    // The search attributes come back on the listing.
    let one = c
        .list_workflow_executions(ListWorkflowExecutionsRequest {
            namespace: "default".into(),
            query: "WorkflowId = 'p1'".into(),
            ..Default::default()
        })
        .await
        .expect("list")
        .into_inner();
    let sa = one.executions[0]
        .search_attributes
        .clone()
        .expect("search attributes");
    assert_eq!(sa.indexed_fields["CustomerId"].data.as_ref(), b"\"c1\"");

    // Pages.
    let first = c
        .list_workflow_executions(ListWorkflowExecutionsRequest {
            namespace: "default".into(),
            page_size: 2,
            ..Default::default()
        })
        .await
        .expect("page 1")
        .into_inner();
    assert_eq!(first.executions.len(), 2);
    assert!(!first.next_page_token.is_empty());
    let second = c
        .scan_workflow_executions(ScanWorkflowExecutionsRequest {
            namespace: "default".into(),
            page_size: 2,
            next_page_token: first.next_page_token,
            ..Default::default()
        })
        .await
        .expect("page 2")
        .into_inner();
    assert_eq!(second.executions.len(), 1);
    assert!(second.next_page_token.is_empty());

    let count = c
        .count_workflow_executions(CountWorkflowExecutionsRequest {
            namespace: "default".into(),
            query: "CustomerId = 'c1'".into(),
        })
        .await
        .expect("count")
        .into_inner();
    assert_eq!(count.count, 2);
    let grouped = c
        .count_workflow_executions(CountWorkflowExecutionsRequest {
            namespace: "default".into(),
            query: "GROUP BY ExecutionStatus".into(),
        })
        .await
        .expect("grouped count")
        .into_inner();
    assert_eq!(grouped.count, 3);
    let groups: HashMap<String, i64> = grouped
        .groups
        .into_iter()
        .map(|g| {
            (
                String::from_utf8(g.group_values[0].data.to_vec()).expect("utf-8"),
                g.count,
            )
        })
        .collect();
    assert_eq!(groups.get("\"Running\""), Some(&2));
    assert_eq!(groups.get("\"Terminated\""), Some(&1));

    let keys = c
        .get_search_attributes(GetSearchAttributesRequest {})
        .await
        .expect("keys")
        .into_inner()
        .keys;
    assert!(keys.contains_key("CustomerId") && keys.contains_key("WorkflowType"));

    // Bad queries are INVALID_ARGUMENT.
    for bad in [
        "WorkflowId =",
        "HistoryLength > 1",
        "ExecutionStatus = 'Sleeping'",
    ] {
        let e = c
            .list_workflow_executions(ListWorkflowExecutionsRequest {
                namespace: "default".into(),
                query: bad.into(),
                ..Default::default()
            })
            .await
            .expect_err(bad);
        assert_eq!(e.code(), tonic::Code::InvalidArgument, "{bad}");
    }

    op.remove_search_attributes(RemoveSearchAttributesRequest {
        namespace: "default".into(),
        search_attributes: vec!["CustomerId".into()],
    })
    .await
    .expect("remove");
    let e = op
        .remove_search_attributes(RemoveSearchAttributesRequest {
            namespace: "default".into(),
            search_attributes: vec!["CustomerId".into()],
        })
        .await
        .expect_err("already removed");
    assert_eq!(e.code(), tonic::Code::NotFound);
}
