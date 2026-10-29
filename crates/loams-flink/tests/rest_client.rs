//! The JobManager REST client against a mock JobManager (httpmock).

use std::time::Duration;

use httpmock::Method::{GET, PATCH, POST};
use httpmock::MockServer;
use loams_flink::FlinkError;
use loams_flink::FlinkRestClient;
use loams_flink::rest::{
    CheckpointOutcome, CheckpointType, JobId, JobState, SavepointFormat, SavepointOutcome,
    SavepointRequest, StopRequest,
};
use serde_json::json;

const J: &str = "fbf9ad9e7fb4c3c3a8bb3b4e3a1f2d10";

fn job() -> JobId {
    JobId::new(J).expect("a valid job id")
}

#[tokio::test]
async fn overview_and_jobs() {
    let server = MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method(GET).path("/overview");
            then.status(200).json_body(json!({
                "taskmanagers": 2, "slots-total": 4, "slots-available": 3,
                "jobs-running": 1, "jobs-finished": 0, "jobs-cancelled": 0, "jobs-failed": 0,
                "flink-version": "2.2.1", "flink-commit": "abc"
            }));
        })
        .await;
    server
        .mock_async(|when, then| {
            when.method(GET).path("/jobs/overview");
            then.status(200).json_body(json!({"jobs": [{
                "jid": J, "name": "orders", "state": "RUNNING",
                "start-time": 1, "end-time": -1, "duration": 5, "last-modification": 2,
                "tasks": {"running": 2}
            }]}));
        })
        .await;
    server
        .mock_async(|when, then| {
            when.method(GET).path(format!("/jobs/{J}"));
            then.status(200).json_body(json!({
                "jid": J, "name": "orders", "isStoppable": false, "state": "RUNNING",
                "job-type": "STREAMING", "start-time": 1, "end-time": -1, "duration": 5,
                "maxParallelism": -1, "now": 6,
                "vertices": [{"id": "v1", "name": "Source", "parallelism": 2, "status": "RUNNING",
                              "maxParallelism": 128, "start-time": 1, "end-time": -1, "duration": 5}],
                "plan": {}
            }));
        })
        .await;
    let c = FlinkRestClient::new(&server.base_url()).unwrap();
    let o = c.overview().await.unwrap();
    assert_eq!(
        (o.taskmanagers, o.slots_total, o.flink_version.as_str()),
        (2, 4, "2.2.1")
    );
    let jobs = c.list_jobs().await.unwrap();
    assert_eq!(jobs[0].state, JobState::Running);
    assert_eq!(jobs[0].jid, job());
    let d = c.job(&job()).await.unwrap();
    assert_eq!(d.vertices[0].parallelism, 2);
    assert!(d.extra.contains_key("plan"));
}

#[tokio::test]
async fn cancel_is_a_patch_with_mode_cancel() {
    let server = MockServer::start_async().await;
    let m = server
        .mock_async(|when, then| {
            when.method(PATCH)
                .path(format!("/jobs/{J}"))
                .query_param("mode", "cancel");
            then.status(202).body("");
        })
        .await;
    FlinkRestClient::new(&server.base_url())
        .unwrap()
        .cancel_job(&job())
        .await
        .unwrap();
    m.assert_async().await;
}

#[tokio::test]
async fn stop_with_savepoint_then_wait() {
    let server = MockServer::start_async().await;
    let stop = server
        .mock_async(|when, then| {
            when.method(POST)
                .path(format!("/jobs/{J}/stop"))
                .json_body(json!({
                    "targetDirectory": "s3://flink/flink/t1/savepoints",
                    "drain": false,
                    "formatType": "CANONICAL"
                }));
            then.status(202).json_body(json!({"request-id": "trig-1"}));
        })
        .await;
    server
        .mock_async(|when, then| {
            when.method(GET)
                .path(format!("/jobs/{J}/savepoints/trig-1"));
            then.status(200).json_body(json!({
                "status": {"id": "COMPLETED"},
                "operation": {"location": "s3://flink/flink/t1/savepoints/savepoint-fbf9ad-1"}
            }));
        })
        .await;
    let c = FlinkRestClient::new(&server.base_url()).unwrap();
    let trigger = c
        .stop_with_savepoint(
            &job(),
            &StopRequest {
                target_directory: Some("s3://flink/flink/t1/savepoints".into()),
                drain: false,
                format_type: SavepointFormat::Canonical,
                trigger_id: None,
            },
        )
        .await
        .unwrap();
    let location = c
        .wait_savepoint(
            &job(),
            &trigger,
            Duration::from_millis(1),
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    assert_eq!(
        location,
        "s3://flink/flink/t1/savepoints/savepoint-fbf9ad-1"
    );
    stop.assert_async().await;
}

#[tokio::test]
async fn a_failed_savepoint_reports_its_cause() {
    let server = MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method(POST)
                .path(format!("/jobs/{J}/savepoints"))
                .json_body(json!({
                    "cancel-job": false,
                    "formatType": "NATIVE"
                }));
            then.status(202).json_body(json!({"request-id": "trig-2"}));
        })
        .await;
    server
        .mock_async(|when, then| {
            when.method(GET).path(format!("/jobs/{J}/savepoints/trig-2"));
            then.status(200).json_body(json!({
                "status": {"id": "COMPLETED"},
                "operation": {"failure-cause": {"class": "java.io.IOException", "serialized-throwable": "bucket not found"}}
            }));
        })
        .await;
    let c = FlinkRestClient::new(&server.base_url()).unwrap();
    let trigger = c
        .trigger_savepoint(
            &job(),
            &SavepointRequest {
                target_directory: None,
                cancel_job: false,
                format_type: SavepointFormat::Native,
                trigger_id: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        c.savepoint_status(&job(), &trigger).await.unwrap(),
        SavepointOutcome::Failed {
            cause: "bucket not found".into()
        }
    );
    let err = c
        .wait_savepoint(
            &job(),
            &trigger,
            Duration::from_millis(1),
            Duration::from_secs(1),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, FlinkError::Failed(_)), "{err:?}");
}

#[tokio::test]
async fn checkpoints_trigger_status_and_statistics() {
    let server = MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method(POST)
                .path(format!("/jobs/{J}/checkpoints"))
                .json_body(json!({"checkpointType": "FULL"}));
            then.status(202).json_body(json!({"request-id": "cp-1"}));
        })
        .await;
    server
        .mock_async(|when, then| {
            when.method(GET).path(format!("/jobs/{J}/checkpoints/cp-1"));
            then.status(200).json_body(json!({
                "status": {"id": "COMPLETED"},
                "operation": {"checkpointId": 42}
            }));
        })
        .await;
    server
        .mock_async(|when, then| {
            when.method(GET).path(format!("/jobs/{J}/checkpoints"));
            then.status(200).json_body(json!({
                "counts": {"restored": 1, "total": 43, "in_progress": 0, "completed": 42, "failed": 1},
                "summary": {"state_size": {"min": 1, "max": 2, "avg": 1}},
                "latest": {
                    "completed": {"@class": "completed", "id": 42, "status": "COMPLETED", "is_savepoint": false,
                                  "trigger_timestamp": 10, "latest_ack_timestamp": 20, "checkpointed_size": 5,
                                  "state_size": 100, "end_to_end_duration": 10,
                                  "external_path": "s3://flink/flink/t1/checkpoints/fbf9/chk-42",
                                  "checkpoint_type": "CHECKPOINT"},
                    "savepoint": null,
                    "failed": {"@class": "failed", "id": 41, "status": "FAILED", "failure_message": "timeout"},
                    "restored": {"id": 7, "restore_timestamp": 3, "is_savepoint": true,
                                 "external_path": "s3://flink/flink/t1/savepoints/savepoint-1"}
                },
                "history": []
            }));
        })
        .await;
    let c = FlinkRestClient::new(&server.base_url()).unwrap();
    let trigger = c
        .trigger_checkpoint(&job(), CheckpointType::Full, None)
        .await
        .unwrap();
    assert_eq!(
        c.checkpoint_status(&job(), &trigger).await.unwrap(),
        CheckpointOutcome::Completed { id: 42 }
    );
    let stats = c.checkpoints(&job()).await.unwrap();
    assert_eq!(stats.counts.completed, 42);
    assert_eq!(
        stats.restore_path(),
        Some("s3://flink/flink/t1/checkpoints/fbf9/chk-42")
    );
    assert_eq!(
        stats.latest.failed.unwrap().failure_message.as_deref(),
        Some("timeout")
    );
}

#[tokio::test]
async fn unknown_jobs_are_not_found() {
    let server = MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method(GET).path(format!("/jobs/{J}"));
            then.status(404)
                .json_body(json!({"errors": ["Job could not be found."]}));
        })
        .await;
    let err = FlinkRestClient::new(&server.base_url())
        .unwrap()
        .job(&job())
        .await
        .unwrap_err();
    assert!(err.is_not_found());
}
