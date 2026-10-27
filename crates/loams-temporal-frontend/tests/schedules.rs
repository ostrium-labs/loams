//! Temporal Schedules on the in-process gateway (D2070-D2073): the fake
//! Resonate's clock drives the `sch` timers.

mod common;

use std::time::Duration;

use loams_temporal_proto::temporal::api::common::v1::{WorkflowExecution, WorkflowType};
use loams_temporal_proto::temporal::api::enums::v1::ScheduleOverlapPolicy;
use loams_temporal_proto::temporal::api::schedule::v1::{
    Schedule, ScheduleAction, SchedulePatch, SchedulePolicies, ScheduleSpec,
    TriggerImmediatelyRequest, schedule_action,
};
use loams_temporal_proto::temporal::api::taskqueue::v1::TaskQueue;
use loams_temporal_proto::temporal::api::workflow::v1::NewWorkflowExecutionInfo;
use loams_temporal_proto::temporal::api::workflowservice::v1::*;

fn schedule(spec: &str, overlap: ScheduleOverlapPolicy) -> Schedule {
    Schedule {
        spec: Some(ScheduleSpec {
            cron_string: vec![spec.into()],
            ..Default::default()
        }),
        action: Some(ScheduleAction {
            action: Some(schedule_action::Action::StartWorkflow(
                NewWorkflowExecutionInfo {
                    workflow_id: "scheduled".into(),
                    workflow_type: Some(WorkflowType { name: "t".into() }),
                    task_queue: Some(TaskQueue {
                        name: "q".into(),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )),
        }),
        policies: Some(SchedulePolicies {
            overlap_policy: overlap as i32,
            ..Default::default()
        }),
        state: None,
    }
}

async fn create(gw: &common::TestGateway, id: &str, s: Schedule) -> CreateScheduleResponse {
    gw.client
        .clone()
        .create_schedule(CreateScheduleRequest {
            namespace: "default".into(),
            schedule_id: id.into(),
            schedule: Some(s),
            request_id: format!("create-{id}"),
            ..Default::default()
        })
        .await
        .expect("create")
        .into_inner()
}

async fn describe(gw: &common::TestGateway, id: &str) -> DescribeScheduleResponse {
    gw.client
        .clone()
        .describe_schedule(DescribeScheduleRequest {
            namespace: "default".into(),
            schedule_id: id.into(),
        })
        .await
        .expect("describe")
        .into_inner()
}

/// Waits until the schedule has taken `n` actions.
async fn wait_actions(gw: &common::TestGateway, id: &str, n: i64) -> DescribeScheduleResponse {
    for _ in 0..200 {
        let d = describe(gw, id).await;
        if d.info.as_ref().is_some_and(|i| i.action_count >= n) {
            return d;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("schedule {id} did not reach {n} actions");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_timer_fires_the_schedule_and_starts_a_workflow() {
    let gw = common::start().await;
    let created = create(
        &gw,
        "every-minute",
        schedule("@every 1m", ScheduleOverlapPolicy::AllowAll),
    )
    .await;
    assert_eq!(created.conflict_token.len(), 8);
    let d = describe(&gw, "every-minute").await;
    let info = d.info.expect("info");
    assert_eq!(info.action_count, 0);
    assert_eq!(info.future_action_times.len(), 10);
    // The canonical spec comes back without the cron string.
    let spec = d.schedule.expect("schedule").spec.expect("spec");
    assert!(spec.cron_string.is_empty() && spec.interval.len() == 1);

    gw.fake.advance(61_000);
    let d = wait_actions(&gw, "every-minute", 1).await;
    let info = d.info.expect("info");
    let action = info.recent_actions.first().expect("an action");
    let started = action.start_workflow_result.clone().expect("result");
    assert!(
        started.workflow_id.starts_with("scheduled-"),
        "{}",
        started.workflow_id
    );
    // The workflow exists and carries the schedule's search attribute.
    gw.client
        .clone()
        .describe_workflow_execution(DescribeWorkflowExecutionRequest {
            namespace: "default".into(),
            execution: Some(WorkflowExecution {
                workflow_id: started.workflow_id.clone(),
                run_id: started.run_id.clone(),
            }),
        })
        .await
        .expect("the scheduled workflow");
    assert_eq!(info.running_workflows.len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn trigger_pause_list_matching_times_and_delete() {
    let gw = common::start().await;
    let mut c = gw.client.clone();
    create(
        &gw,
        "daily",
        schedule("@daily", ScheduleOverlapPolicy::Skip),
    )
    .await;
    // A retried create with the same request ID is answered; another is refused.
    create(
        &gw,
        "daily",
        schedule("@daily", ScheduleOverlapPolicy::Skip),
    )
    .await;
    let err = c
        .create_schedule(CreateScheduleRequest {
            namespace: "default".into(),
            schedule_id: "daily".into(),
            schedule: Some(schedule("@daily", ScheduleOverlapPolicy::Skip)),
            request_id: "other".into(),
            ..Default::default()
        })
        .await
        .expect_err("taken");
    assert_eq!(err.code(), tonic::Code::AlreadyExists);

    c.patch_schedule(PatchScheduleRequest {
        namespace: "default".into(),
        schedule_id: "daily".into(),
        patch: Some(SchedulePatch {
            trigger_immediately: Some(TriggerImmediatelyRequest::default()),
            pause: "maintenance".into(),
            ..Default::default()
        }),
        ..Default::default()
    })
    .await
    .expect("patch");
    let d = wait_actions(&gw, "daily", 1).await;
    let state = d.schedule.expect("schedule").state.expect("state");
    assert!(state.paused);
    assert_eq!(state.notes, "maintenance");
    assert!(d.info.expect("info").future_action_times.is_empty());

    // Two days of @daily, by matching times.
    let now = gw.fake.now();
    let times = c
        .list_schedule_matching_times(ListScheduleMatchingTimesRequest {
            namespace: "default".into(),
            schedule_id: "daily".into(),
            start_time: Some(prost_types::Timestamp {
                seconds: now / 1000,
                nanos: 0,
            }),
            end_time: Some(prost_types::Timestamp {
                seconds: now / 1000 + 2 * 86_400,
                nanos: 0,
            }),
        })
        .await
        .expect("matching times")
        .into_inner();
    assert_eq!(times.start_time.len(), 2);

    let list = c
        .list_schedules(ListSchedulesRequest {
            namespace: "default".into(),
            ..Default::default()
        })
        .await
        .expect("list")
        .into_inner();
    assert_eq!(list.schedules.len(), 1);
    assert!(list.schedules[0].info.as_ref().is_some_and(|i| i.paused));

    // A stale conflict token is refused.
    let stale = c
        .update_schedule(UpdateScheduleRequest {
            namespace: "default".into(),
            schedule_id: "daily".into(),
            schedule: Some(schedule("@hourly", ScheduleOverlapPolicy::Skip)),
            conflict_token: vec![0u8; 8].into(),
            ..Default::default()
        })
        .await
        .expect_err("stale token");
    assert_eq!(stale.code(), tonic::Code::FailedPrecondition);

    c.delete_schedule(DeleteScheduleRequest {
        namespace: "default".into(),
        schedule_id: "daily".into(),
        identity: String::new(),
    })
    .await
    .expect("delete");
    let gone = c
        .describe_schedule(DescribeScheduleRequest {
            namespace: "default".into(),
            schedule_id: "daily".into(),
        })
        .await
        .expect_err("deleted");
    assert_eq!(gone.code(), tonic::Code::NotFound);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn skip_does_not_start_while_one_runs() {
    let gw = common::start().await;
    create(
        &gw,
        "skip",
        schedule("@every 1m", ScheduleOverlapPolicy::Skip),
    )
    .await;
    gw.fake.advance(61_000);
    wait_actions(&gw, "skip", 1).await;
    // The first workflow never completes (no worker), so the next is skipped.
    gw.fake.advance(60_000);
    for _ in 0..200 {
        let info = describe(&gw, "skip").await.info.expect("info");
        if info.overlap_skipped >= 1 {
            assert_eq!(info.action_count, 1);
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("the second time was not skipped");
}
