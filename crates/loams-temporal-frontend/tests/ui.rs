//! What the Temporal Web UI and the `temporal` CLI read (D2050–D2053):
//! `GetClusterInfo`, the reverse history, `OperatorService`'s search
//! attributes, the Nexus answers, and the UI's read-only listener.

mod common;

use loams_temporal_frontend::Principal;
use loams_temporal_frontend::capabilities::{MATRIX_URL, NEXUS_UNSUPPORTED};
use loams_temporal_frontend::ui::{READ_ONLY_MESSAGE, UiListener};
use loams_temporal_proto::temporal::api::common::v1::{WorkflowExecution, WorkflowType};
use loams_temporal_proto::temporal::api::enums::v1::IndexedValueType;
use loams_temporal_proto::temporal::api::operatorservice::v1::operator_service_client::OperatorServiceClient;
use loams_temporal_proto::temporal::api::operatorservice::v1::{
    ListClustersRequest, ListNexusEndpointsRequest, ListSearchAttributesRequest,
};
use loams_temporal_proto::temporal::api::taskqueue::v1::TaskQueue;
use loams_temporal_proto::temporal::api::workflowservice::v1::workflow_service_client::WorkflowServiceClient;
use loams_temporal_proto::temporal::api::workflowservice::v1::*;
use tonic::transport::Channel;

/// Serves `gw`'s services on a second loopback port: as the UI listener
/// (principal `lns`, read-only) when `ui`, otherwise plainly (operator
/// service included, as `standalone` mounts it).
async fn serve(gw: &common::TestGateway, ui: bool) -> Channel {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let principal = Principal {
        loams_namespace_id: "lns".into(),
        admin: false,
    };
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
    if ui {
        let router = tonic::transport::Server::builder()
            .add_service(UiListener::new(
                gw.gateway.service(),
                principal.clone(),
                true,
            ))
            .add_service(UiListener::new(
                gw.gateway.operator_service(),
                principal,
                true,
            ));
        tokio::spawn(async move { router.serve_with_incoming(incoming).await.expect("serve") });
    } else {
        let router = tonic::transport::Server::builder()
            .add_service(gw.gateway.service())
            .add_service(gw.gateway.operator_service());
        tokio::spawn(async move { router.serve_with_incoming(incoming).await.expect("serve") });
    }
    Channel::from_shared(format!("http://{addr}"))
        .expect("uri")
        .connect()
        .await
        .expect("connect")
}

fn start_req(id: &str) -> StartWorkflowExecutionRequest {
    StartWorkflowExecutionRequest {
        namespace: "default".into(),
        workflow_id: id.into(),
        workflow_type: Some(WorkflowType { name: "t".into() }),
        task_queue: Some(TaskQueue {
            name: "q".into(),
            ..Default::default()
        }),
        request_id: format!("r-{id}"),
        ..Default::default()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cluster_info_names_the_gateway_not_a_temporal_server() {
    let gw = common::start().await;
    let info = gw
        .client
        .clone()
        .get_cluster_info(GetClusterInfoRequest {})
        .await
        .expect("cluster info")
        .into_inner();
    assert!(
        info.server_version.starts_with("loams-temporal "),
        "{}",
        info.server_version
    );
    assert_eq!(info.cluster_name, info.cluster_id);
    assert!(info.history_shard_count > 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reverse_history_pages_are_the_history_newest_first() {
    let gw = common::start().await;
    let mut c = gw.client.clone();
    let run = c
        .start_workflow_execution(start_req("wrev"))
        .await
        .expect("start")
        .into_inner();
    let execution = Some(WorkflowExecution {
        workflow_id: "wrev".into(),
        run_id: run.run_id,
    });
    for i in 0..3 {
        c.signal_workflow_execution(SignalWorkflowExecutionRequest {
            namespace: "default".into(),
            workflow_execution: execution.clone(),
            signal_name: format!("s{i}"),
            ..Default::default()
        })
        .await
        .expect("signal");
    }
    c.terminate_workflow_execution(TerminateWorkflowExecutionRequest {
        namespace: "default".into(),
        workflow_execution: execution.clone(),
        reason: "done".into(),
        ..Default::default()
    })
    .await
    .expect("terminate");
    let forward: Vec<i64> = c
        .get_workflow_execution_history(GetWorkflowExecutionHistoryRequest {
            namespace: "default".into(),
            execution: execution.clone(),
            ..Default::default()
        })
        .await
        .expect("history")
        .into_inner()
        .history
        .expect("history")
        .events
        .iter()
        .map(|e| e.event_id)
        .collect();
    assert!(forward.len() >= 5, "{forward:?}");

    let mut backward = Vec::new();
    let mut token = Vec::new();
    let mut pages = 0;
    loop {
        pages += 1;
        let r = c
            .get_workflow_execution_history_reverse(GetWorkflowExecutionHistoryReverseRequest {
                namespace: "default".into(),
                execution: execution.clone(),
                maximum_page_size: 2,
                next_page_token: token.into(),
            })
            .await
            .expect("reverse")
            .into_inner();
        let events = r.history.expect("history").events;
        assert!(events.len() <= 2);
        backward.extend(events.iter().map(|e| e.event_id));
        if r.next_page_token.is_empty() {
            break;
        }
        token = r.next_page_token.to_vec();
        assert!(pages < 100, "the reverse pages do not end");
    }
    let mut expected = forward.clone();
    expected.reverse();
    assert_eq!(backward, expected);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn operator_lists_system_search_attributes_and_refuses_nexus() {
    let gw = common::start().await;
    let channel = serve(&gw, false).await;
    let mut op = OperatorServiceClient::new(channel);
    let attrs = op
        .list_search_attributes(ListSearchAttributesRequest {
            namespace: "default".into(),
        })
        .await
        .expect("search attributes")
        .into_inner();
    assert!(attrs.custom_attributes.is_empty());
    assert_eq!(
        attrs.system_attributes.get("WorkflowId"),
        Some(&(IndexedValueType::Keyword as i32))
    );
    assert_eq!(
        attrs.system_attributes.get("StartTime"),
        Some(&(IndexedValueType::Datetime as i32))
    );
    let e = op
        .list_search_attributes(ListSearchAttributesRequest {
            namespace: "nope".into(),
        })
        .await
        .expect_err("unknown namespace");
    assert_eq!(e.code(), tonic::Code::NotFound);

    let e = op
        .list_nexus_endpoints(ListNexusEndpointsRequest::default())
        .await
        .expect_err("no Nexus");
    assert_eq!(e.code(), tonic::Code::Unimplemented);
    assert!(e.message().contains(NEXUS_UNSUPPORTED), "{}", e.message());

    let e = op
        .list_clusters(ListClustersRequest::default())
        .await
        .expect_err("not implemented");
    assert_eq!(e.code(), tonic::Code::Unimplemented);
    assert!(e.message().contains(MATRIX_URL), "{}", e.message());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_ui_listener_reads_without_a_key_and_refuses_writes() {
    let gw = common::start().await;
    let run = gw
        .client
        .clone()
        .start_workflow_execution(start_req("wui"))
        .await
        .expect("start")
        .into_inner();
    let channel = serve(&gw, true).await;
    let mut ui = WorkflowServiceClient::new(channel.clone());
    let execution = Some(WorkflowExecution {
        workflow_id: "wui".into(),
        run_id: run.run_id,
    });

    let namespaces = ui
        .list_namespaces(ListNamespacesRequest::default())
        .await
        .expect("namespaces")
        .into_inner();
    assert!(namespaces.namespaces.iter().any(|n| {
        n.namespace_info
            .as_ref()
            .is_some_and(|i| i.name == "default")
    }));
    ui.describe_workflow_execution(DescribeWorkflowExecutionRequest {
        namespace: "default".into(),
        execution: execution.clone(),
        ..Default::default()
    })
    .await
    .expect("describe");
    ui.get_cluster_info(GetClusterInfoRequest {})
        .await
        .expect("cluster info");
    OperatorServiceClient::new(channel)
        .list_search_attributes(ListSearchAttributesRequest {
            namespace: "default".into(),
        })
        .await
        .expect("search attributes");

    for e in [
        ui.terminate_workflow_execution(TerminateWorkflowExecutionRequest {
            namespace: "default".into(),
            workflow_execution: execution.clone(),
            ..Default::default()
        })
        .await
        .map(|_| ())
        .expect_err("terminate is a write"),
        ui.start_workflow_execution(start_req("wui2"))
            .await
            .map(|_| ())
            .expect_err("start is a write"),
        ui.register_namespace(RegisterNamespaceRequest {
            namespace: "other".into(),
            ..Default::default()
        })
        .await
        .map(|_| ())
        .expect_err("register is a write"),
    ] {
        assert_eq!(e.code(), tonic::Code::PermissionDenied);
        assert_eq!(e.message(), READ_ONLY_MESSAGE);
    }
}
