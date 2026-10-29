//! The generator: a Rust SDK worker for every corpus workflow on the queue
//! `corpus-rust`, plus a starter that drives each scenario (signals,
//! queries, cancels, terminates, updates) with the fixed workflow ID
//! `corpus-rust-<scenario>`, as workers/ts/src/generate.ts does. The Go
//! exporter (`corpus-go export -prefix corpus-rust-`) writes the histories
//! afterwards, so every language's files come from one exporter.

use std::{path::Path, time::Duration};

use anyhow::{Context, Result, anyhow, bail};
use serde::Serialize;
use temporalio_client::{
    Client, ClientOptions, Connection, ConnectionOptions, RegisterNamespaceOptions,
    UntypedWorkflow, Url, WorkflowCancelOptions, WorkflowDescribeOptions,
    WorkflowExecuteUpdateOptions, WorkflowGetResultOptions, WorkflowIdConflictPolicy,
    WorkflowIdReusePolicy, WorkflowQueryOptions, WorkflowSignalOptions, WorkflowStartOptions,
    WorkflowTerminateOptions,
    errors::{WorkflowGetResultError, WorkflowStartError, WorkflowUpdateError},
    grpc::{OperatorService as _, WorkflowService as _},
    tonic,
};
use temporalio_common::{
    MemoValues,
    protos::temporal::api::{
        enums::v1::IndexedValueType, operatorservice::v1::AddSearchAttributesRequest,
        workflowservice::v1::RegisterNamespaceRequest,
    },
};
use temporalio_sdk::{
    Runtime, Worker, WorkerOptions,
    runtime::worker_tuner::{FixedSizeSlotSupplier, TunerHolder},
};

use crate::{
    activities::CorpusActivities,
    register_corpus_workflows,
    scenarios::{QUEUE, pick, workflow_id},
    workflows::*,
};

/// What a driver saw, written to the observations file (generate-rust.json).
#[derive(Debug, Default, Serialize)]
pub struct Observation {
    pub scenario: String,
    pub workflow_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    pub expected: Vec<String>,
}

/// The target URL for `host:port` (plain HTTP/2; the gateway's compose
/// profile and the upstream dev server have no TLS) or a full URL.
pub fn target(address: &str) -> String {
    if address.contains("://") {
        address.to_owned()
    } else {
        format!("http://{address}")
    }
}

/// A namespace-bound client. TEMPORAL_API_KEY, when set, goes as
/// `authorization: Bearer <key>` without turning TLS on (TLS is only used
/// when `tls_options` is set), as the Go and TS legs send it.
pub async fn connect(address: &str, namespace: &str) -> Result<Client> {
    let api_key = std::env::var("TEMPORAL_API_KEY")
        .ok()
        .filter(|k| !k.is_empty());
    let opts = ConnectionOptions::new(Url::parse(&target(address)).context("address")?)
        .identity(format!("corpus-rust@{}", std::process::id()))
        .maybe_api_key(api_key)
        .build();
    let connection = Connection::connect(opts)
        .await
        .with_context(|| format!("connect {address}"))?;
    Ok(Client::new(
        connection,
        ClientOptions::new(namespace.to_owned()).build(),
    )?)
}

/// Registers the namespace (72 h retention, as the Go generator) and,
/// optionally, the keyword attribute `CorpusKeyword`; both idempotent.
pub async fn ensure_namespace(
    client: &Client,
    namespace: &str,
    search_attribute: bool,
) -> Result<()> {
    let req: RegisterNamespaceRequest = RegisterNamespaceOptions::builder()
        .namespace(namespace.to_owned())
        .description("Loams history corpus (Rust SDK)".to_owned())
        .workflow_execution_retention_period(Duration::from_secs(72 * 3600))
        .build()
        .into();
    let mut svc = client.connection().workflow_service();
    match svc.register_namespace(tonic::Request::new(req)).await {
        Ok(_) => {}
        Err(s) if s.code() == tonic::Code::AlreadyExists => {}
        Err(s) => bail!("RegisterNamespace {namespace}: {s}"),
    }
    if !search_attribute {
        return Ok(());
    }
    // A new namespace takes a moment to reach every service's cache.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        let mut op = client.connection().operator_service();
        let req = AddSearchAttributesRequest {
            namespace: namespace.to_owned(),
            search_attributes: [("CorpusKeyword".to_owned(), IndexedValueType::Keyword as i32)]
                .into_iter()
                .collect(),
            ..Default::default()
        };
        match op.add_search_attributes(tonic::Request::new(req)).await {
            Ok(_) => break,
            Err(s) if s.code() == tonic::Code::AlreadyExists => break,
            Err(s) if tokio::time::Instant::now() > deadline => bail!("AddSearchAttributes: {s}"),
            Err(_) => tokio::time::sleep(Duration::from_secs(2)).await,
        }
    }
    tokio::time::sleep(Duration::from_secs(3)).await;
    Ok(())
}

fn opts(id: &str) -> WorkflowStartOptions {
    WorkflowStartOptions::new(QUEUE.to_owned(), id.to_owned()).build()
}

/// Waits until the run's first workflow task completed, so a signal, query,
/// cancel or update lands on a running workflow (as the Go and TS drivers).
async fn wait_started(client: &Client, id: &str) -> Result<()> {
    let h = client.get_workflow_handle::<UntypedWorkflow>(id.to_owned());
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        if let Ok(d) = h.describe(WorkflowDescribeOptions::default()).await
            && d.history_length() >= 4
            && d.raw().pending_workflow_task.is_none()
        {
            return Ok(());
        }
        if tokio::time::Instant::now() > deadline {
            bail!("{id}: the first workflow task did not complete within 60 s");
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Awaits a handle's result and records it as JSON.
macro_rules! settle {
    ($o:ident, $h:expr) => {{
        let v = $h.get_result(WorkflowGetResultOptions::default()).await?;
        $o.result = Some(serde_json::to_string(&v)?);
    }};
}

/// Starts a workflow with `input` and records its result.
macro_rules! simple {
    ($o:ident, $client:ident, $id:ident, $run:expr, $input:expr) => {{
        let h = $client.start_workflow($run, $input, opts(&$id)).await?;
        settle!($o, h);
    }};
}

/// update_accepted_completed / update_rejected: the update `increment` by
/// 2 (accepted) or -1 (the validator refuses it), then `finish`.
macro_rules! update_case {
    ($o:ident, $client:ident, $id:ident, $wf:ident, $by:expr) => {{
        let by: i64 = $by;
        let h = $client.start_workflow($wf::run, (), opts(&$id)).await?;
        wait_started($client, &$id).await?;
        let update = WorkflowExecuteUpdateOptions::builder()
            .update_id(format!("{}-update", $id))
            .build();
        match h.execute_update($wf::increment, by, update).await {
            Ok(total) if by >= 0 => $o.expected.push(format!("update returned {total}")),
            Ok(total) => {
                bail!("the validator should reject a negative increment (returned {total})")
            }
            Err(WorkflowUpdateError::Failed(f)) if by < 0 => {
                $o.expected.push(format!("update rejected: {}", f.message))
            }
            Err(e) => return Err(anyhow!(e).context("execute_update")),
        }
        h.signal($wf::finish, (), WorkflowSignalOptions::default())
            .await?;
        settle!($o, h);
    }};
}

/// Drives one scenario.
pub async fn drive(client: &Client, name: &str) -> Result<Observation> {
    let id = workflow_id(name);
    let mut o = Observation {
        scenario: name.to_owned(),
        workflow_id: id.clone(),
        ..Default::default()
    };
    match name {
        "hello" => simple!(o, client, id, Hello::run, "corpus".to_owned()),
        "two_activities_sequential" => simple!(o, client, id, TwoActivitiesSequential::run, ()),
        "activity_parallel_fanout_10" => simple!(o, client, id, ActivityParallelFanout10::run, ()),
        "signal_then_complete" => {
            let h = client
                .start_workflow(SignalThenComplete::run, (), opts(&id))
                .await?;
            wait_started(client, &id).await?;
            h.signal(
                SignalThenComplete::go,
                "s".to_owned(),
                WorkflowSignalOptions::default(),
            )
            .await?;
            settle!(o, h);
        }
        "query_while_running" => {
            let h = client
                .start_workflow(QueryWhileRunning::run, (), opts(&id))
                .await?;
            wait_started(client, &id).await?;
            let state = h
                .query(
                    QueryWhileRunning::state,
                    (),
                    WorkflowQueryOptions::default(),
                )
                .await?;
            if state != "waiting" {
                bail!("query answered {state:?}");
            }
            o.expected.push("query state=waiting".to_owned());
            h.signal(
                QueryWhileRunning::finish,
                (),
                WorkflowSignalOptions::default(),
            )
            .await?;
            settle!(o, h);
        }
        "cancel_requested" => {
            let h = client
                .start_workflow(CancelRequested::run, (), opts(&id))
                .await?;
            wait_started(client, &id).await?;
            h.cancel(WorkflowCancelOptions::default()).await?;
            match h.get_result(WorkflowGetResultOptions::default()).await {
                Err(e @ WorkflowGetResultError::Cancelled { .. }) => o.expected.push(e.to_string()),
                Err(e) => bail!("want a cancelled workflow, got: {e}"),
                Ok(_) => bail!("the workflow completed instead of closing as cancelled"),
            }
        }
        "terminated" => {
            let h = client
                .start_workflow(Terminated::run, (), opts(&id))
                .await?;
            wait_started(client, &id).await?;
            h.terminate(
                WorkflowTerminateOptions::builder()
                    .reason("corpus terminates it")
                    .build(),
            )
            .await?;
            match h.get_result(WorkflowGetResultOptions::default()).await {
                Err(e @ WorkflowGetResultError::Terminated { .. }) => {
                    o.expected.push(e.to_string())
                }
                Err(e) => bail!("want a terminated workflow, got: {e}"),
                Ok(_) => bail!("the workflow completed instead of closing as terminated"),
            }
        }
        "workflow_failed_application_error" => {
            let h = client
                .start_workflow(WorkflowFailedApplicationError::run, (), opts(&id))
                .await?;
            match h.get_result(WorkflowGetResultOptions::default()).await {
                Err(e @ WorkflowGetResultError::Failed(_)) => o.expected.push(e.to_string()),
                Err(e) => bail!("want an application error, got: {e}"),
                Ok(_) => bail!("want an application error, the workflow completed"),
            }
        }
        "activity_heartbeat_details" => simple!(o, client, id, ActivityHeartbeatDetails::run, ()),
        "timer_short" => simple!(o, client, id, TimerShort::run, ()),
        "timer_cancelled" => simple!(o, client, id, TimerCancelled::run, ()),
        "activity_retry_backoff_3_attempts" => {
            simple!(o, client, id, ActivityRetryBackoff3Attempts::run, ())
        }
        "activity_start_to_close_timeout" => {
            simple!(o, client, id, ActivityStartToCloseTimeout::run, ())
        }
        "activity_schedule_to_start_timeout" => {
            simple!(o, client, id, ActivityScheduleToStartTimeout::run, ())
        }
        "activity_heartbeat_timeout" => simple!(o, client, id, ActivityHeartbeatTimeout::run, ()),
        "signal_with_start" => {
            let h = client
                .signal_with_start_workflow(
                    SignalWithStart::run,
                    (),
                    SignalWithStart::go,
                    "s".to_owned(),
                    opts(&id),
                )
                .await?;
            settle!(o, h);
        }
        "child_workflow" => simple!(o, client, id, ChildWorkflow::run, ()),
        "child_workflow_cancelled" => simple!(o, client, id, ChildWorkflowCancelled::run, ()),
        "continue_as_new_3" => simple!(o, client, id, ContinueAsNew3::run, 0i64),
        "id_reuse_allow_duplicate_failed_only" => {
            let policy = || {
                WorkflowStartOptions::new(QUEUE.to_owned(), id.clone())
                    .id_reuse_policy(WorkflowIdReusePolicy::AllowDuplicateFailedOnly)
                    .build()
            };
            let first = client
                .start_workflow(IdReuseAllowDuplicateFailedOnly::run, true, policy())
                .await?;
            if first
                .get_result(WorkflowGetResultOptions::default())
                .await
                .is_ok()
            {
                bail!("the first run should fail");
            }
            let second = client
                .start_workflow(IdReuseAllowDuplicateFailedOnly::run, false, policy())
                .await?;
            o.expected
                .push("the first run failed; the second started with the same ID".to_owned());
            settle!(o, second);
        }
        "id_conflict_fail" => {
            let policy = || {
                WorkflowStartOptions::new(QUEUE.to_owned(), id.clone())
                    .id_conflict_policy(WorkflowIdConflictPolicy::Fail)
                    .build()
            };
            let h = client
                .start_workflow(IdConflictFail::run, (), policy())
                .await?;
            match client
                .start_workflow(IdConflictFail::run, (), policy())
                .await
            {
                Err(WorkflowStartError::AlreadyStarted { .. }) => o.expected.push(
                    "the second start was refused: WorkflowExecutionAlreadyStarted".to_owned(),
                ),
                Err(e) => bail!("want WorkflowExecutionAlreadyStarted, got: {e}"),
                Ok(_) => bail!("the second start should be refused"),
            }
            h.signal(IdConflictFail::finish, (), WorkflowSignalOptions::default())
                .await?;
            settle!(o, h);
        }
        "local_activity" => simple!(o, client, id, LocalActivity::run, ()),
        "side_effect_and_version_marker" => {
            simple!(o, client, id, SideEffectAndVersionMarker::run, ())
        }
        "search_attributes_upsert" => simple!(o, client, id, SearchAttributesUpsert::run, ()),
        "memo" => {
            let mut memo = MemoValues::new();
            memo.insert("initial", "set at start".to_owned());
            let start = WorkflowStartOptions::new(QUEUE.to_owned(), id.clone())
                .memo(memo)
                .build();
            let h = client.start_workflow(MemoUpsert::run, (), start).await?;
            settle!(o, h);
        }
        "large_payload_1mb" => simple!(o, client, id, LargePayload1mb::run, ()),
        "update_accepted_completed" => update_case!(o, client, id, UpdateAcceptedCompleted, 2),
        "update_rejected" => update_case!(o, client, id, UpdateRejected, -1),
        other => bail!("no driver for scenario {other:?}"),
    }
    Ok(o)
}

/// One activity at a time (as the Go worker), so the fan-out's activities
/// complete in the order they were scheduled and regenerating is stable.
fn tuner() -> TunerHolder {
    TunerHolder::builder()
        .workflow_task_slot_supplier(FixedSizeSlotSupplier::new(10))
        .activity_task_slot_supplier(FixedSizeSlotSupplier::new(1))
        .local_activity_task_slot_supplier(FixedSizeSlotSupplier::new(10))
        .nexus_task_slot_supplier(FixedSizeSlotSupplier::new(1))
        .build()
}

/// `generate`: runs the worker and drives the scenarios, then writes the
/// observations to `out`.
pub async fn generate(
    address: &str,
    namespace: &str,
    out: &Path,
    only: Option<&str>,
    register: bool,
) -> Result<()> {
    let list = pick(only).map_err(anyhow::Error::msg)?;
    let client = connect(address, namespace).await?;
    if register {
        ensure_namespace(&client, namespace, true).await?;
    }
    let runtime = Runtime::from_current_tokio(Default::default())?;
    let mut worker_opts = WorkerOptions::new(QUEUE.to_owned())
        .register_activities(CorpusActivities)
        .tuner(tuner())
        .build();
    register_corpus_workflows!(worker_opts);
    let mut worker = Worker::new(&runtime, client.clone(), worker_opts)?;
    let shutdown = worker.shutdown_handle();

    let drive_all = async {
        let mut observations = Vec::new();
        for name in &list {
            let o = tokio::time::timeout(Duration::from_secs(180), drive(&client, name))
                .await
                .map_err(|_| anyhow!("{name}: no result within 3 minutes"))?
                .with_context(|| format!("scenario {name}"))?;
            println!("corpus: {name}: {}", o.result.as_deref().unwrap_or(""));
            observations.push(o);
        }
        Ok::<_, anyhow::Error>(observations)
    };
    let run = worker.run();
    tokio::pin!(run);
    tokio::pin!(drive_all);
    let observations = tokio::select! {
        r = &mut run => match r {
            Ok(()) => Err(anyhow!("the worker stopped before the scenarios finished")),
            Err(e) => Err(anyhow!(e).context("the worker failed before the scenarios finished")),
        },
        o = &mut drive_all => o,
    };
    shutdown();
    let observations = match observations {
        Ok(o) => {
            run.await.context("worker shutdown")?;
            o
        }
        Err(e) => return Err(e),
    };

    let doc = serde_json::json!({
        "namespace": namespace,
        "language": "rust",
        "sdk": "temporalio-sdk 1.1.0 (temporalio-sdk-core 0.10.0)",
        "scenarios": observations,
    });
    std::fs::write(out, serde_json::to_string_pretty(&doc)? + "\n")
        .with_context(|| format!("write {}", out.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_adds_a_plain_scheme() {
        assert_eq!(target("127.0.0.1:27233"), "http://127.0.0.1:27233");
        assert_eq!(target("loams:7233"), "http://loams:7233");
        assert_eq!(target("https://x:7233"), "https://x:7233");
    }

    #[test]
    fn observations_serialise_like_the_other_languages() {
        let o = Observation {
            scenario: "hello".into(),
            workflow_id: "corpus-rust-hello".into(),
            result: None,
            expected: vec![],
        };
        let v = serde_json::to_value(&o).unwrap();
        assert_eq!(v["scenario"], "hello");
        assert!(v.get("result").is_none());
    }
}
