//! D2089: follow-ups (children, external signals) are outbox rows written in
//! the same commit as their events, so a gateway that stops between the
//! commit and the effect loses nothing, and carrying a row out twice
//! changes nothing.

use std::sync::Arc;
use std::time::Duration;

use loams_temporal_frontend::effects::{
    EffectContext, FOLLOW_UP_KIND, FollowUpIntent, FollowUpRow,
};
use loams_temporal_frontend::{
    DevAllowAll, Engine, FrontendConfig, Gateway, Parts, Principal, TokenKeys,
};
use loams_temporal_history::{EventType, HistoryEvent};
use loams_temporal_proto::temporal::api::command::v1::{
    Command, SignalExternalWorkflowExecutionCommandAttributes,
    StartChildWorkflowExecutionCommandAttributes, command,
};
use loams_temporal_proto::temporal::api::common::v1::{WorkflowExecution, WorkflowType};
use loams_temporal_proto::temporal::api::taskqueue::v1::TaskQueue;
use loams_temporal_proto::temporal::api::workflowservice::v1::StartWorkflowExecutionRequest;
use loams_temporal_resonate::FakeDurability;
use loams_temporal_statemachine::Input;
use loams_temporal_store::{MemStore, NamespaceRecord, RunKey, Store};

const SHARDS: u32 = 4;

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_millis() as i64
}

fn ctx() -> EffectContext {
    EffectContext {
        namespace_id: "ns1".into(),
        loams_namespace_id: "lns".into(),
    }
}

fn start_req(workflow_id: &str) -> StartWorkflowExecutionRequest {
    StartWorkflowExecutionRequest {
        namespace: "default".into(),
        workflow_id: workflow_id.into(),
        workflow_type: Some(WorkflowType { name: "t".into() }),
        task_queue: Some(TaskQueue {
            name: "q".into(),
            ..Default::default()
        }),
        request_id: format!("start-{workflow_id}"),
        ..Default::default()
    }
}

fn cmd(a: command::Attributes) -> Command {
    Command {
        attributes: Some(a),
        ..Default::default()
    }
}

async fn store() -> Arc<dyn Store> {
    let store: Arc<dyn Store> = Arc::new(MemStore::new(SHARDS).expect("store"));
    store
        .create_namespace(NamespaceRecord {
            namespace_id: "ns1".into(),
            loams_namespace_id: "lns".into(),
            name: "default".into(),
            retention_secs: 3600,
            state: 1,
            config: serde_json::json!({}),
        })
        .await
        .expect("namespace");
    store
}

/// Before the "crash": a parent whose first workflow task starts a child and
/// signals `other`, committed by an engine whose outbox nobody drains.
async fn commit_without_effects(store: &Arc<dyn Store>) -> (RunKey, RunKey) {
    let engine = Engine::new(store.clone(), Arc::new(now_ms));
    let ctx = ctx();
    let (other_run, _) = engine
        .start(&ctx, "o1".into(), start_req("other"))
        .await
        .expect("other");
    let (parent_run, _) = engine
        .start(&ctx, "p1".into(), start_req("parent"))
        .await
        .expect("parent");
    let parent = RunKey::new("ns1", "parent", parent_run);
    engine
        .execute(
            &parent,
            &ctx,
            Input::WorkflowTaskStarted {
                scheduled_event_id: 2,
                identity: "worker".into(),
                request_id: "poll-1".into(),
                worker_version: None,
            },
        )
        .await
        .expect("task started");
    engine
        .execute(
            &parent,
            &ctx,
            Input::WorkflowTaskCompleted {
                scheduled_event_id: 2,
                attempt: 1,
                identity: "worker".into(),
                commands: vec![
                    cmd(
                        command::Attributes::StartChildWorkflowExecutionCommandAttributes(
                            StartChildWorkflowExecutionCommandAttributes {
                                workflow_id: "child".into(),
                                workflow_type: Some(WorkflowType { name: "c".into() }),
                                ..Default::default()
                            },
                        ),
                    ),
                    cmd(
                        command::Attributes::SignalExternalWorkflowExecutionCommandAttributes(
                            SignalExternalWorkflowExecutionCommandAttributes {
                                execution: Some(WorkflowExecution {
                                    workflow_id: "other".into(),
                                    run_id: String::new(),
                                }),
                                signal_name: "hello".into(),
                                ..Default::default()
                            },
                        ),
                    ),
                ],
                sdk_metadata: None,
                metering_metadata: None,
                worker_version: None,
                force_create_new_workflow_task: false,
                messages: Vec::new(),
                new_run_id: String::new(),
            },
        )
        .await
        .expect("task completed");
    (parent, RunKey::new("ns1", "other", other_run))
}

async fn history(store: &Arc<dyn Store>, key: &RunKey) -> Vec<HistoryEvent> {
    use prost::Message as _;
    store
        .history(key, 1, usize::MAX)
        .await
        .expect("history")
        .into_iter()
        .map(|r| HistoryEvent::decode(r.blob.as_slice()).expect("event"))
        .collect()
}

fn count(events: &[HistoryEvent], t: EventType) -> usize {
    events.iter().filter(|e| e.event_type == t as i32).count()
}

async fn gateway(store: Arc<dyn Store>) -> (Gateway, tokio::sync::watch::Sender<bool>) {
    let (stop, rx) = tokio::sync::watch::channel(false);
    let gw = Gateway::start(
        Parts {
            store,
            durability: Arc::new(FakeDurability::new()),
            auth: Arc::new(DevAllowAll(Principal {
                loams_namespace_id: "lns".into(),
                admin: true,
            })),
            tokens: TokenKeys::new(vec![("k1".into(), vec![7; 32])]),
            pid: "gw-restarted".into(),
            shards: SHARDS,
            lease: Duration::from_secs(60),
            config: FrontendConfig::default(),
        },
        rx,
    )
    .await
    .expect("gateway");
    (gw, stop)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn follow_ups_survive_a_crash_between_commit_and_effect() {
    let store = store().await;
    let (parent, other) = commit_without_effects(&store).await;

    // The intents are in the outbox, committed with the events; a relay
    // that claimed them and then died leaves them claimed until the lease
    // runs out.
    let shards: Vec<u32> = (0..SHARDS).collect();
    let claimed = store
        .claim_outbox(&shards, 100, Duration::from_millis(1), "gw-crashed")
        .await
        .expect("claim");
    let follow_ups: Vec<_> = claimed
        .iter()
        .filter(|r| r.kind == FOLLOW_UP_KIND)
        .collect();
    assert_eq!(follow_ups.len(), 2, "a child and a signal: {claimed:?}");
    assert!(
        store
            .current_run("ns1", "child")
            .await
            .expect("current")
            .is_none(),
        "nothing was carried out before the crash"
    );
    tokio::time::sleep(Duration::from_millis(20)).await;

    // The restarted gateway drains them.
    let (gw, _stop) = gateway(store.clone()).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let p = history(&store, &parent).await;
        let o = history(&store, &other).await;
        if count(&p, EventType::ChildWorkflowExecutionStarted) == 1
            && count(&p, EventType::ExternalWorkflowExecutionSignaled) == 1
            && count(&o, EventType::WorkflowExecutionSignaled) == 1
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "follow-ups not carried out after the restart: parent {:?}",
            p.iter().map(|e| e.event_type).collect::<Vec<_>>()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let child = store
        .current_run("ns1", "child")
        .await
        .expect("current")
        .expect("the child started");

    // The effects happened but the rows were never marked: carrying them
    // out again changes nothing.
    let p = history(&store, &parent).await;
    let initiated = |t: EventType| {
        p.iter()
            .find(|e| e.event_type == t as i32)
            .map(|e| e.event_id)
            .expect("initiated")
    };
    let child_initiated = initiated(EventType::StartChildWorkflowExecutionInitiated);
    let signal_initiated = initiated(EventType::SignalExternalWorkflowExecutionInitiated);
    for intent in [
        FollowUpIntent::StartChild {
            initiated_event_id: child_initiated,
        },
        FollowUpIntent::SignalExternal {
            initiated_event_id: signal_initiated,
        },
    ] {
        let row = FollowUpRow {
            namespace_id: "ns1".into(),
            loams_namespace_id: "lns".into(),
            workflow_id: parent.workflow_id.clone(),
            run_id: parent.run_id.clone(),
            intent,
        };
        gw.frontend.run_follow_up(&row).await.expect("redo");
        gw.frontend.run_follow_up(&row).await.expect("redo again");
    }
    let p = history(&store, &parent).await;
    let o = history(&store, &other).await;
    assert_eq!(count(&p, EventType::ChildWorkflowExecutionStarted), 1);
    assert_eq!(count(&p, EventType::ExternalWorkflowExecutionSignaled), 1);
    assert_eq!(count(&o, EventType::WorkflowExecutionSignaled), 1);
    let again = store
        .current_run("ns1", "child")
        .await
        .expect("current")
        .expect("child");
    assert_eq!(again.run_id, child.run_id, "no second child run");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_repeated_signal_request_is_written_once() {
    let store = store().await;
    let engine = Engine::new(store.clone(), Arc::new(now_ms));
    let ctx = ctx();
    let (run, _) = engine
        .start(&ctx, "r1".into(), start_req("w"))
        .await
        .expect("start");
    let key = RunKey::new("ns1", "w", run);
    for _ in 0..2 {
        engine
            .execute(
                &key,
                &ctx,
                Input::Signal {
                    name: "s".into(),
                    input: None,
                    identity: "c".into(),
                    header: None,
                    request_id: "same".into(),
                },
            )
            .await
            .expect("signal");
    }
    assert_eq!(
        count(
            &history(&store, &key).await,
            EventType::WorkflowExecutionSignaled
        ),
        1
    );
}
