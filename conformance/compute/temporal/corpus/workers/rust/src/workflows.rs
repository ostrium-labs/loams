//! The corpus's Rust workflows: one workflow type per scenario, named as the
//! scenario (`#[run(name = ...)]`), mirroring workers/ts/src/workflows.ts.
//! Signals `go` and `finish`, the query `state` and the update `increment`
//! have the other languages' names. A change here changes the histories:
//! regenerate `histories/rust/` in the same pull request.
//!
//! The Rust SDK has no SideEffect API (like TypeScript), so
//! `side_effect_and_version_marker` records the version marker only.

use std::time::Duration;

use temporalio_common::{RetryPolicy, search_attributes::SearchAttributeKey};
use temporalio_macros::{workflow, workflow_methods};
use temporalio_sdk::{
    ActivityExecutionError, ActivityOptions, ApplicationFailure, CancellableFuture,
    ChildWorkflowOptions, ContinueAsNewOptions, LocalActivityOptions, MemoValue,
    SyncWorkflowContext, TimeoutType, TimerResult, WorkflowContext, WorkflowContextView,
    WorkflowResult, WorkflowTermination, workflows::join_all,
};

use crate::activities::CorpusActivities;

/// The custom keyword attribute the generator registers (as the Go one does).
pub const CORPUS_KEYWORD: SearchAttributeKey<String> = SearchAttributeKey::keyword("CorpusKeyword");

/// One attempt, 30 s start-to-close: the corpus's default activity options.
fn once() -> ActivityOptions {
    ActivityOptions::with_start_to_close_timeout(Duration::from_secs(30))
        .retry_policy(RetryPolicy::builder().maximum_attempts(1).build())
        .build()
}

/// `TIMEOUT_TYPE_<KIND>` for an activity that timed out (the other
/// languages' generators record the same string), else what happened.
pub fn timeout_kind(result: Result<(), ActivityExecutionError>) -> String {
    match result {
        Ok(()) => "no timeout".to_owned(),
        Err(err) => match err.as_timeout() {
            Some(t) => format!("TIMEOUT_TYPE_{}", timeout_name(t.timeout_type())),
            None => format!("not a timeout: {err}"),
        },
    }
}

pub fn timeout_name(t: TimeoutType) -> &'static str {
    match t {
        TimeoutType::StartToClose => "START_TO_CLOSE",
        TimeoutType::ScheduleToStart => "SCHEDULE_TO_START",
        TimeoutType::ScheduleToClose => "SCHEDULE_TO_CLOSE",
        TimeoutType::Heartbeat => "HEARTBEAT",
        TimeoutType::Unspecified => "UNSPECIFIED",
        _ => "UNKNOWN",
    }
}

fn not_reached(what: &str) -> WorkflowTermination {
    ApplicationFailure::non_retryable(what.to_owned()).into()
}

// --- DX1 ----------------------------------------------------------------------

#[workflow]
#[derive(Default)]
pub struct Hello;

#[workflow_methods]
impl Hello {
    #[run(name = "hello")]
    pub async fn run(ctx: &mut WorkflowContext<Self>, name: String) -> WorkflowResult<String> {
        Ok(ctx
            .execute_activity(CorpusActivities::greet, name, once())
            .await?)
    }
}

#[workflow]
#[derive(Default)]
pub struct TwoActivitiesSequential;

#[workflow_methods]
impl TwoActivitiesSequential {
    #[run(name = "two_activities_sequential")]
    pub async fn run(ctx: &mut WorkflowContext<Self>) -> WorkflowResult<String> {
        let a = ctx
            .execute_activity(CorpusActivities::greet, "one".to_owned(), once())
            .await?;
        let b = ctx
            .execute_activity(CorpusActivities::greet, "two".to_owned(), once())
            .await?;
        Ok(format!("{a}, {b}"))
    }
}

#[workflow]
#[derive(Default)]
pub struct ActivityParallelFanout10;

#[workflow_methods]
impl ActivityParallelFanout10 {
    #[run(name = "activity_parallel_fanout_10")]
    pub async fn run(ctx: &mut WorkflowContext<Self>) -> WorkflowResult<Vec<String>> {
        let c: &WorkflowContext<Self> = ctx;
        let calls: Vec<_> = (0..10)
            .map(move |i| c.execute_activity(CorpusActivities::greet, format!("n{i}"), once()))
            .collect();
        let results: Result<Vec<String>, ActivityExecutionError> =
            join_all(calls).await.into_iter().collect();
        Ok(results?)
    }
}

#[workflow]
#[derive(Default)]
pub struct SignalThenComplete {
    value: Option<String>,
}

#[workflow_methods]
impl SignalThenComplete {
    #[run(name = "signal_then_complete")]
    pub async fn run(ctx: &mut WorkflowContext<Self>) -> WorkflowResult<String> {
        ctx.wait_condition(|s| s.value.is_some()).await?;
        Ok(format!(
            "got {}",
            ctx.state(|s| s.value.clone().unwrap_or_default())
        ))
    }

    #[signal(name = "go")]
    pub fn go(&mut self, _ctx: &mut SyncWorkflowContext<Self>, value: String) {
        self.value = Some(value);
    }
}

#[workflow]
pub struct QueryWhileRunning {
    state: String,
    done: bool,
}

impl Default for QueryWhileRunning {
    fn default() -> Self {
        Self {
            state: "waiting".to_owned(),
            done: false,
        }
    }
}

#[workflow_methods]
impl QueryWhileRunning {
    #[run(name = "query_while_running")]
    pub async fn run(ctx: &mut WorkflowContext<Self>) -> WorkflowResult<String> {
        ctx.wait_condition(|s| s.done).await?;
        ctx.state_mut(|s| s.state = "done".to_owned());
        Ok(ctx.state(|s| s.state.clone()))
    }

    #[query(name = "state")]
    pub fn state(&self, _ctx: &WorkflowContextView) -> String {
        self.state.clone()
    }

    #[signal(name = "finish")]
    pub fn finish(&mut self, _ctx: &mut SyncWorkflowContext<Self>) {
        self.done = true;
    }
}

#[workflow]
#[derive(Default)]
pub struct CancelRequested;

#[workflow_methods]
impl CancelRequested {
    /// Sleeps an hour; the timer inherits the workflow's cancellation, so a
    /// cancel request cancels it and the run closes as cancelled.
    #[run(name = "cancel_requested")]
    pub async fn run(ctx: &mut WorkflowContext<Self>) -> WorkflowResult<()> {
        match ctx.timer(Duration::from_secs(3600)).await {
            TimerResult::Cancelled => Err(WorkflowTermination::cancelled()),
            _ => Ok(()),
        }
    }
}

#[workflow]
#[derive(Default)]
pub struct Terminated {
    done: bool,
}

#[workflow_methods]
impl Terminated {
    #[run(name = "terminated")]
    pub async fn run(ctx: &mut WorkflowContext<Self>) -> WorkflowResult<()> {
        ctx.wait_condition(|s| s.done).await?;
        Ok(())
    }

    #[signal(name = "finish")]
    pub fn finish(&mut self, _ctx: &mut SyncWorkflowContext<Self>) {
        self.done = true;
    }
}

#[workflow]
#[derive(Default)]
pub struct WorkflowFailedApplicationError;

#[workflow_methods]
impl WorkflowFailedApplicationError {
    #[run(name = "workflow_failed_application_error")]
    pub async fn run(_ctx: &mut WorkflowContext<Self>) -> WorkflowResult<()> {
        Err(ApplicationFailure::builder("boom")
            .type_name("CorpusBoom".to_owned())
            .non_retryable(true)
            .details("detail".to_owned())
            .build()
            .into())
    }
}

#[workflow]
#[derive(Default)]
pub struct ActivityHeartbeatDetails;

#[workflow_methods]
impl ActivityHeartbeatDetails {
    #[run(name = "activity_heartbeat_details")]
    pub async fn run(ctx: &mut WorkflowContext<Self>) -> WorkflowResult<i64> {
        let opts = ActivityOptions::with_start_to_close_timeout(Duration::from_secs(30))
            .heartbeat_timeout(Duration::from_secs(10))
            .retry_policy(RetryPolicy::builder().maximum_attempts(1).build())
            .build();
        Ok(ctx
            .execute_activity(CorpusActivities::heartbeat_thrice, (), opts)
            .await?)
    }
}

// --- DX2 ----------------------------------------------------------------------

#[workflow]
#[derive(Default)]
pub struct TimerShort;

#[workflow_methods]
impl TimerShort {
    #[run(name = "timer_short")]
    pub async fn run(ctx: &mut WorkflowContext<Self>) -> WorkflowResult<()> {
        ctx.timer(Duration::from_secs(1)).await;
        Ok(())
    }
}

#[workflow]
#[derive(Default)]
pub struct TimerCancelled;

#[workflow_methods]
impl TimerCancelled {
    /// An hour's timer, a 100 ms one, then the long one is cancelled.
    #[run(name = "timer_cancelled")]
    pub async fn run(ctx: &mut WorkflowContext<Self>) -> WorkflowResult<String> {
        let c: &WorkflowContext<Self> = ctx;
        let long = c.timer(Duration::from_secs(3600));
        c.timer(Duration::from_millis(100)).await;
        long.cancel();
        match long.await {
            TimerResult::Cancelled => Ok("cancelled".to_owned()),
            _ => Err(not_reached("the long timer was not cancelled")),
        }
    }
}

#[workflow]
#[derive(Default)]
pub struct ActivityRetryBackoff3Attempts;

#[workflow_methods]
impl ActivityRetryBackoff3Attempts {
    #[run(name = "activity_retry_backoff_3_attempts")]
    pub async fn run(ctx: &mut WorkflowContext<Self>) -> WorkflowResult<i64> {
        let opts = ActivityOptions::with_start_to_close_timeout(Duration::from_secs(10))
            .retry_policy(
                RetryPolicy::builder()
                    .initial_interval(Duration::from_secs(1))
                    .backoff_coefficient(2.0)
                    .maximum_attempts(3)
                    .build(),
            )
            .build();
        Ok(ctx
            .execute_activity(CorpusActivities::fail_twice, (), opts)
            .await?)
    }
}

#[workflow]
#[derive(Default)]
pub struct ActivityStartToCloseTimeout;

#[workflow_methods]
impl ActivityStartToCloseTimeout {
    #[run(name = "activity_start_to_close_timeout")]
    pub async fn run(ctx: &mut WorkflowContext<Self>) -> WorkflowResult<String> {
        let opts = ActivityOptions::with_start_to_close_timeout(Duration::from_secs(1))
            .retry_policy(RetryPolicy::builder().maximum_attempts(1).build())
            .build();
        Ok(timeout_kind(
            ctx.execute_activity(CorpusActivities::sleep_for, 5000u64, opts)
                .await,
        ))
    }
}

#[workflow]
#[derive(Default)]
pub struct ActivityScheduleToStartTimeout;

#[workflow_methods]
impl ActivityScheduleToStartTimeout {
    /// On a task queue no worker polls, so the schedule-to-start timeout fires.
    #[run(name = "activity_schedule_to_start_timeout")]
    pub async fn run(ctx: &mut WorkflowContext<Self>) -> WorkflowResult<String> {
        let opts = ActivityOptions::with_start_to_close_timeout(Duration::from_secs(1))
            .task_queue("corpus-no-worker".to_owned())
            .schedule_to_start_timeout(Duration::from_secs(1))
            .retry_policy(RetryPolicy::builder().maximum_attempts(1).build())
            .build();
        let r = ctx
            .execute_activity(CorpusActivities::greet, "nobody".to_owned(), opts)
            .await;
        Ok(timeout_kind(r.map(|_| ())))
    }
}

#[workflow]
#[derive(Default)]
pub struct ActivityHeartbeatTimeout;

#[workflow_methods]
impl ActivityHeartbeatTimeout {
    #[run(name = "activity_heartbeat_timeout")]
    pub async fn run(ctx: &mut WorkflowContext<Self>) -> WorkflowResult<String> {
        let opts = ActivityOptions::with_start_to_close_timeout(Duration::from_secs(30))
            .heartbeat_timeout(Duration::from_secs(1))
            .retry_policy(RetryPolicy::builder().maximum_attempts(1).build())
            .build();
        Ok(timeout_kind(
            ctx.execute_activity(CorpusActivities::sleep_for, 5000u64, opts)
                .await,
        ))
    }
}

#[workflow]
#[derive(Default)]
pub struct SignalWithStart {
    value: Option<String>,
}

#[workflow_methods]
impl SignalWithStart {
    #[run(name = "signal_with_start")]
    pub async fn run(ctx: &mut WorkflowContext<Self>) -> WorkflowResult<String> {
        ctx.wait_condition(|s| s.value.is_some()).await?;
        Ok(format!(
            "started with {}",
            ctx.state(|s| s.value.clone().unwrap_or_default())
        ))
    }

    #[signal(name = "go")]
    pub fn go(&mut self, _ctx: &mut SyncWorkflowContext<Self>, value: String) {
        self.value = Some(value);
    }
}

#[workflow]
#[derive(Default)]
pub struct ChildGreeter;

#[workflow_methods]
impl ChildGreeter {
    #[run(name = "child_greeter")]
    pub async fn run(ctx: &mut WorkflowContext<Self>, name: String) -> WorkflowResult<String> {
        Ok(ctx
            .execute_activity(CorpusActivities::greet, name, once())
            .await?)
    }
}

#[workflow]
#[derive(Default)]
pub struct ChildSleeper;

#[workflow_methods]
impl ChildSleeper {
    #[run(name = "child_sleeper")]
    pub async fn run(ctx: &mut WorkflowContext<Self>) -> WorkflowResult<()> {
        match ctx.timer(Duration::from_secs(3600)).await {
            TimerResult::Cancelled => Err(WorkflowTermination::cancelled()),
            _ => Ok(()),
        }
    }
}

#[workflow]
#[derive(Default)]
pub struct ChildWorkflow;

#[workflow_methods]
impl ChildWorkflow {
    #[run(name = "child_workflow")]
    pub async fn run(ctx: &mut WorkflowContext<Self>) -> WorkflowResult<String> {
        let id = format!("{}-child", ctx.workflow_id());
        let started = ctx
            .start_child_workflow(
                ChildGreeter::run,
                "child".to_owned(),
                ChildWorkflowOptions::workflow_id(id),
            )
            .await?;
        Ok(started.result().await?)
    }
}

#[workflow]
#[derive(Default)]
pub struct ChildWorkflowCancelled;

#[workflow_methods]
impl ChildWorkflowCancelled {
    #[run(name = "child_workflow_cancelled")]
    pub async fn run(ctx: &mut WorkflowContext<Self>) -> WorkflowResult<String> {
        let id = format!("{}-child", ctx.workflow_id());
        let started = ctx
            .start_child_workflow(ChildSleeper::run, (), ChildWorkflowOptions::workflow_id(id))
            .await?;
        started.cancel("corpus cancels the child".to_owned());
        match started.result().await {
            Err(err) if err.as_cancelled().is_some() => Ok("child cancelled".to_owned()),
            Err(err) => Err(err.into()),
            Ok(()) => Err(not_reached("the child was not cancelled")),
        }
    }
}

#[workflow]
#[derive(Default)]
pub struct ContinueAsNew3;

#[workflow_methods]
impl ContinueAsNew3 {
    #[run(name = "continue_as_new_3")]
    pub async fn run(ctx: &mut WorkflowContext<Self>, n: i64) -> WorkflowResult<i64> {
        if n < 3 {
            ctx.continue_as_new(n + 1, ContinueAsNewOptions::default())?;
        }
        Ok(n)
    }
}

#[workflow]
#[derive(Default)]
pub struct IdReuseAllowDuplicateFailedOnly;

#[workflow_methods]
impl IdReuseAllowDuplicateFailedOnly {
    #[run(name = "id_reuse_allow_duplicate_failed_only")]
    pub async fn run(_ctx: &mut WorkflowContext<Self>, fail: bool) -> WorkflowResult<String> {
        if fail {
            return Err(ApplicationFailure::builder("first run fails")
                .type_name("CorpusFirstRun".to_owned())
                .non_retryable(true)
                .build()
                .into());
        }
        Ok("second run".to_owned())
    }
}

#[workflow]
#[derive(Default)]
pub struct IdConflictFail {
    done: bool,
}

#[workflow_methods]
impl IdConflictFail {
    #[run(name = "id_conflict_fail")]
    pub async fn run(ctx: &mut WorkflowContext<Self>) -> WorkflowResult<()> {
        ctx.wait_condition(|s| s.done).await?;
        Ok(())
    }

    #[signal(name = "finish")]
    pub fn finish(&mut self, _ctx: &mut SyncWorkflowContext<Self>) {
        self.done = true;
    }
}

#[workflow]
#[derive(Default)]
pub struct LocalActivity;

#[workflow_methods]
impl LocalActivity {
    #[run(name = "local_activity")]
    pub async fn run(ctx: &mut WorkflowContext<Self>) -> WorkflowResult<String> {
        let opts = LocalActivityOptions::builder()
            .start_to_close_timeout(Duration::from_secs(10))
            .build();
        Ok(ctx
            .execute_local_activity(CorpusActivities::greet, "local".to_owned(), opts)
            .await?)
    }
}

#[workflow]
#[derive(Default)]
pub struct SideEffectAndVersionMarker;

#[workflow_methods]
impl SideEffectAndVersionMarker {
    #[run(name = "side_effect_and_version_marker")]
    pub async fn run(ctx: &mut WorkflowContext<Self>) -> WorkflowResult<String> {
        let v = if ctx.patched("corpus-change") { 1 } else { 0 };
        Ok(format!("version {v}"))
    }
}

// --- DX3 ----------------------------------------------------------------------

#[workflow]
#[derive(Default)]
pub struct SearchAttributesUpsert;

#[workflow_methods]
impl SearchAttributesUpsert {
    #[run(name = "search_attributes_upsert")]
    pub async fn run(ctx: &mut WorkflowContext<Self>) -> WorkflowResult<()> {
        ctx.upsert_search_attributes([CORPUS_KEYWORD.value_set("first".to_owned())]);
        ctx.timer(Duration::from_millis(100)).await;
        ctx.upsert_search_attributes([CORPUS_KEYWORD.value_set("second".to_owned())]);
        Ok(())
    }
}

#[workflow]
#[derive(Default)]
pub struct MemoUpsert;

#[workflow_methods]
impl MemoUpsert {
    #[run(name = "memo")]
    pub async fn run(ctx: &mut WorkflowContext<Self>) -> WorkflowResult<()> {
        ctx.upsert_memo([
            ("step", Some(MemoValue::new("upserted".to_owned()))),
            ("n", Some(MemoValue::new(2i64))),
        ])?;
        Ok(())
    }
}

#[workflow]
#[derive(Default)]
pub struct LargePayload1mb;

#[workflow_methods]
impl LargePayload1mb {
    #[run(name = "large_payload_1mb")]
    pub async fn run(ctx: &mut WorkflowContext<Self>) -> WorkflowResult<i64> {
        let s = ctx
            .execute_activity(CorpusActivities::large, 1u64 << 20, once())
            .await?;
        Ok(i64::try_from(s.len()).unwrap_or(i64::MAX))
    }
}

// --- DX4 ----------------------------------------------------------------------

/// The validator both update scenarios share: a negative increment is refused.
pub fn validate_increment(by: i64) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if by < 0 {
        Err("increment must not be negative".into())
    } else {
        Ok(())
    }
}

#[workflow]
#[derive(Default)]
pub struct UpdateAcceptedCompleted {
    total: i64,
    done: bool,
}

#[workflow_methods]
impl UpdateAcceptedCompleted {
    #[run(name = "update_accepted_completed")]
    pub async fn run(ctx: &mut WorkflowContext<Self>) -> WorkflowResult<String> {
        ctx.wait_condition(|s| s.done).await?;
        Ok(format!("total {}", ctx.state(|s| s.total)))
    }

    #[update_validator(increment)]
    pub fn validate_increment(
        &self,
        _ctx: &WorkflowContextView,
        by: &i64,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        validate_increment(*by)
    }

    #[update(name = "increment")]
    pub fn increment(&mut self, _ctx: &mut SyncWorkflowContext<Self>, by: i64) -> i64 {
        self.total += by;
        self.total
    }

    #[signal(name = "finish")]
    pub fn finish(&mut self, _ctx: &mut SyncWorkflowContext<Self>) {
        self.done = true;
    }
}

#[workflow]
#[derive(Default)]
pub struct UpdateRejected {
    total: i64,
    done: bool,
}

#[workflow_methods]
impl UpdateRejected {
    #[run(name = "update_rejected")]
    pub async fn run(ctx: &mut WorkflowContext<Self>) -> WorkflowResult<String> {
        ctx.wait_condition(|s| s.done).await?;
        Ok(format!("total {}", ctx.state(|s| s.total)))
    }

    #[update_validator(increment)]
    pub fn validate_increment(
        &self,
        _ctx: &WorkflowContextView,
        by: &i64,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        validate_increment(*by)
    }

    #[update(name = "increment")]
    pub fn increment(&mut self, _ctx: &mut SyncWorkflowContext<Self>, by: i64) -> i64 {
        self.total += by;
        self.total
    }

    #[signal(name = "finish")]
    pub fn finish(&mut self, _ctx: &mut SyncWorkflowContext<Self>) {
        self.done = true;
    }
}

/// Registers every workflow type of this module on a worker's or a
/// replayer's options (both have `register_workflow::<W>() -> Result<..>`).
#[macro_export]
macro_rules! register_corpus_workflows {
    ($opts:expr) => {{
        use $crate::workflows::*;
        $opts.register_workflow::<Hello>()?;
        $opts.register_workflow::<TwoActivitiesSequential>()?;
        $opts.register_workflow::<ActivityParallelFanout10>()?;
        $opts.register_workflow::<SignalThenComplete>()?;
        $opts.register_workflow::<QueryWhileRunning>()?;
        $opts.register_workflow::<CancelRequested>()?;
        $opts.register_workflow::<Terminated>()?;
        $opts.register_workflow::<WorkflowFailedApplicationError>()?;
        $opts.register_workflow::<ActivityHeartbeatDetails>()?;
        $opts.register_workflow::<TimerShort>()?;
        $opts.register_workflow::<TimerCancelled>()?;
        $opts.register_workflow::<ActivityRetryBackoff3Attempts>()?;
        $opts.register_workflow::<ActivityStartToCloseTimeout>()?;
        $opts.register_workflow::<ActivityScheduleToStartTimeout>()?;
        $opts.register_workflow::<ActivityHeartbeatTimeout>()?;
        $opts.register_workflow::<SignalWithStart>()?;
        $opts.register_workflow::<ChildGreeter>()?;
        $opts.register_workflow::<ChildSleeper>()?;
        $opts.register_workflow::<ChildWorkflow>()?;
        $opts.register_workflow::<ChildWorkflowCancelled>()?;
        $opts.register_workflow::<ContinueAsNew3>()?;
        $opts.register_workflow::<IdReuseAllowDuplicateFailedOnly>()?;
        $opts.register_workflow::<IdConflictFail>()?;
        $opts.register_workflow::<LocalActivity>()?;
        $opts.register_workflow::<SideEffectAndVersionMarker>()?;
        $opts.register_workflow::<SearchAttributesUpsert>()?;
        $opts.register_workflow::<MemoUpsert>()?;
        $opts.register_workflow::<LargePayload1mb>()?;
        $opts.register_workflow::<UpdateAcceptedCompleted>()?;
        $opts.register_workflow::<UpdateRejected>()?;
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_names_match_the_proto_enum() {
        assert_eq!(timeout_name(TimeoutType::StartToClose), "START_TO_CLOSE");
        assert_eq!(
            timeout_name(TimeoutType::ScheduleToStart),
            "SCHEDULE_TO_START"
        );
        assert_eq!(timeout_name(TimeoutType::Heartbeat), "HEARTBEAT");
        assert_eq!(timeout_kind(Ok(())), "no timeout");
    }

    #[test]
    fn negative_increments_are_refused() {
        assert!(validate_increment(2).is_ok());
        assert!(validate_increment(0).is_ok());
        assert!(validate_increment(-1).is_err());
    }
}
