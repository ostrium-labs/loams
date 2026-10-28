// The .NET scenarios in plan order: scenarios.json's non-Go-only ones
// (manifest.py check reads the ("name", "DXn") pairs below), and every
// workflow type the worker and the replayer register.
namespace Loams.Corpus;

public static class Scenarios
{
    public static readonly (string Name, string Phase)[] All =
    {
        ("hello", "DX1"),
        ("two_activities_sequential", "DX1"),
        ("activity_parallel_fanout_10", "DX1"),
        ("signal_then_complete", "DX1"),
        ("query_while_running", "DX1"),
        ("cancel_requested", "DX1"),
        ("terminated", "DX1"),
        ("workflow_failed_application_error", "DX1"),
        ("activity_heartbeat_details", "DX1"),
        ("timer_short", "DX2"),
        ("timer_cancelled", "DX2"),
        ("activity_retry_backoff_3_attempts", "DX2"),
        ("activity_start_to_close_timeout", "DX2"),
        ("activity_schedule_to_start_timeout", "DX2"),
        ("activity_heartbeat_timeout", "DX2"),
        ("signal_with_start", "DX2"),
        ("child_workflow", "DX2"),
        ("child_workflow_cancelled", "DX2"),
        ("continue_as_new_3", "DX2"),
        ("id_reuse_allow_duplicate_failed_only", "DX2"),
        ("id_conflict_fail", "DX2"),
        ("local_activity", "DX2"),
        ("side_effect_and_version_marker", "DX2"),
        ("search_attributes_upsert", "DX3"),
        ("memo", "DX3"),
        ("large_payload_1mb", "DX3"),
        ("update_accepted_completed", "DX4"),
        ("update_rejected", "DX4"),
    };

    /// <summary>Every workflow class: the scenarios' and the two children's.</summary>
    public static readonly Type[] Workflows =
    {
        typeof(Hello),
        typeof(TwoActivitiesSequential),
        typeof(ActivityParallelFanout10),
        typeof(SignalThenComplete),
        typeof(QueryWhileRunning),
        typeof(CancelRequested),
        typeof(Terminated),
        typeof(WorkflowFailedApplicationError),
        typeof(ActivityHeartbeatDetails),
        typeof(TimerShort),
        typeof(TimerCancelled),
        typeof(ActivityRetryBackoff3Attempts),
        typeof(ActivityStartToCloseTimeout),
        typeof(ActivityScheduleToStartTimeout),
        typeof(ActivityHeartbeatTimeout),
        typeof(SignalWithStart),
        typeof(ChildWorkflow),
        typeof(ChildWorkflowCancelled),
        typeof(ContinueAsNew3),
        typeof(IdReuseAllowDuplicateFailedOnly),
        typeof(IdConflictFail),
        typeof(LocalActivity),
        typeof(SideEffectAndVersionMarker),
        typeof(SearchAttributesUpsert),
        typeof(Memo),
        typeof(LargePayload1Mb),
        typeof(UpdateAcceptedCompleted),
        typeof(UpdateRejected),
        typeof(ChildGreeter),
        typeof(ChildSleeper),
    };
}
