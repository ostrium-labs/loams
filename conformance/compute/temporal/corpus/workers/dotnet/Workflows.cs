// The corpus's .NET workflows (TSDK-dotnet): the TypeScript scenarios, one
// class per scenario, each registered under its scenario name, so the
// generator and the replayer agree on it. The Go-only scenarios
// (workflow_task_failure_then_fix, schedule_triggered,
// reset_to_first_workflow_task) are left out, as in TypeScript. A change here
// changes the histories: regenerate histories/dotnet/ in the same PR.
using Temporalio.Common;
using Temporalio.Exceptions;
using Temporalio.Workflows;

namespace Loams.Corpus;

internal static class Opts
{
    public const string SearchAttribute = "CorpusKeyword";

    /// <summary>One attempt, 30 s start-to-close (TypeScript's `once`).</summary>
    public static ActivityOptions Once() => new()
    {
        StartToCloseTimeout = TimeSpan.FromSeconds(30),
        RetryPolicy = new RetryPolicy { MaximumAttempts = 1 },
    };

    public static readonly object?[] NoArgs = Array.Empty<object?>();

    public static Task<string> Greet(string name, ActivityOptions? options = null) =>
        Workflow.ExecuteActivityAsync<string>("greet", new object?[] { name }, options ?? Once());

    public static string TimeoutKind(Exception e) =>
        e is ActivityFailureException { InnerException: TimeoutFailureException t }
            ? $"TIMEOUT_TYPE_{t.TimeoutType}"
            : $"not a timeout: {e.Message}";

    /// <summary>The "increment" update's validator: a negative increment is refused.</summary>
    public static void ValidateIncrement(int by)
    {
        if (by < 0)
        {
            throw new ApplicationFailureException("increment must not be negative", "CorpusInvalid");
        }
    }
}

// --- DX1 ----------------------------------------------------------------------

[Workflow("hello")]
public class Hello
{
    [WorkflowRun]
    public async Task<string> RunAsync(string name) => await Opts.Greet(name);
}

[Workflow("two_activities_sequential")]
public class TwoActivitiesSequential
{
    [WorkflowRun]
    public async Task<string> RunAsync()
    {
        var a = await Opts.Greet("one");
        var b = await Opts.Greet("two");
        return $"{a}, {b}";
    }
}

[Workflow("activity_parallel_fanout_10")]
public class ActivityParallelFanout10
{
    [WorkflowRun]
    public async Task<string[]> RunAsync()
    {
        var all = Enumerable.Range(0, 10).Select(i => Opts.Greet($"n{i}")).ToList();
        return await Workflow.WhenAllAsync(all);
    }
}

[Workflow("signal_then_complete")]
public class SignalThenComplete
{
    private string? value;

    [WorkflowRun]
    public async Task<string> RunAsync()
    {
        await Workflow.WaitConditionAsync(() => value != null);
        return $"got {value}";
    }

    [WorkflowSignal("go")]
    public Task GoAsync(string s)
    {
        value = s;
        return Task.CompletedTask;
    }
}

[Workflow("query_while_running")]
public class QueryWhileRunning
{
    private string state = "waiting";
    private bool done;

    [WorkflowRun]
    public async Task<string> RunAsync()
    {
        await Workflow.WaitConditionAsync(() => done);
        state = "done";
        return state;
    }

    [WorkflowQuery("state")]
    public string State() => state;

    [WorkflowSignal("finish")]
    public Task FinishAsync()
    {
        done = true;
        return Task.CompletedTask;
    }
}

[Workflow("cancel_requested")]
public class CancelRequested
{
    [WorkflowRun]
    public async Task RunAsync() => await Workflow.DelayAsync(TimeSpan.FromHours(1));
}

[Workflow("terminated")]
public class Terminated
{
    private bool done;

    [WorkflowRun]
    public async Task RunAsync() => await Workflow.WaitConditionAsync(() => done);

    [WorkflowSignal("finish")]
    public Task FinishAsync()
    {
        done = true;
        return Task.CompletedTask;
    }
}

[Workflow("workflow_failed_application_error")]
public class WorkflowFailedApplicationError
{
    [WorkflowRun]
    public async Task RunAsync() =>
        throw new ApplicationFailureException("boom", "CorpusBoom", true, new object?[] { "detail" });
}

[Workflow("activity_heartbeat_details")]
public class ActivityHeartbeatDetails
{
    [WorkflowRun]
    public async Task<int> RunAsync()
    {
        var o = Opts.Once();
        o.HeartbeatTimeout = TimeSpan.FromSeconds(10);
        return await Workflow.ExecuteActivityAsync<int>("heartbeat_thrice", Opts.NoArgs, o);
    }
}

// --- DX2 ----------------------------------------------------------------------

[Workflow("timer_short")]
public class TimerShort
{
    [WorkflowRun]
    public async Task RunAsync() => await Workflow.DelayAsync(TimeSpan.FromSeconds(1));
}

[Workflow("timer_cancelled")]
public class TimerCancelled
{
    [WorkflowRun]
    public async Task<string> RunAsync()
    {
        using var cts = CancellationTokenSource.CreateLinkedTokenSource(Workflow.CancellationToken);
        var longTimer = Workflow.DelayAsync(TimeSpan.FromHours(1), cts.Token);
        await Workflow.DelayAsync(TimeSpan.FromMilliseconds(100));
        cts.Cancel();
        try
        {
            await longTimer;
        }
        catch (Exception e) when (TemporalException.IsCanceledException(e))
        {
            return "cancelled";
        }
        throw new ApplicationFailureException("the long timer was not cancelled", "CorpusBug");
    }
}

[Workflow("activity_retry_backoff_3_attempts")]
public class ActivityRetryBackoff3Attempts
{
    [WorkflowRun]
    public async Task<int> RunAsync() =>
        await Workflow.ExecuteActivityAsync<int>("fail_twice", Opts.NoArgs, new ActivityOptions
        {
            StartToCloseTimeout = TimeSpan.FromSeconds(10),
            RetryPolicy = new RetryPolicy
            {
                InitialInterval = TimeSpan.FromSeconds(1),
                BackoffCoefficient = 2,
                MaximumAttempts = 3,
            },
        });
}

[Workflow("activity_start_to_close_timeout")]
public class ActivityStartToCloseTimeout
{
    [WorkflowRun]
    public async Task<string> RunAsync()
    {
        try
        {
            await Workflow.ExecuteActivityAsync("sleep_for", new object?[] { 5000 }, new ActivityOptions
            {
                StartToCloseTimeout = TimeSpan.FromSeconds(1),
                RetryPolicy = new RetryPolicy { MaximumAttempts = 1 },
            });
            return "no timeout";
        }
        catch (ActivityFailureException e)
        {
            return Opts.TimeoutKind(e);
        }
    }
}

[Workflow("activity_schedule_to_start_timeout")]
public class ActivityScheduleToStartTimeout
{
    [WorkflowRun]
    public async Task<string> RunAsync()
    {
        try
        {
            await Opts.Greet("nobody", new ActivityOptions
            {
                TaskQueue = "corpus-no-worker",
                ScheduleToStartTimeout = TimeSpan.FromSeconds(1),
                StartToCloseTimeout = TimeSpan.FromSeconds(1),
                RetryPolicy = new RetryPolicy { MaximumAttempts = 1 },
            });
            return "no timeout";
        }
        catch (ActivityFailureException e)
        {
            return Opts.TimeoutKind(e);
        }
    }
}

[Workflow("activity_heartbeat_timeout")]
public class ActivityHeartbeatTimeout
{
    [WorkflowRun]
    public async Task<string> RunAsync()
    {
        try
        {
            await Workflow.ExecuteActivityAsync("sleep_for", new object?[] { 5000 }, new ActivityOptions
            {
                StartToCloseTimeout = TimeSpan.FromSeconds(30),
                HeartbeatTimeout = TimeSpan.FromSeconds(1),
                RetryPolicy = new RetryPolicy { MaximumAttempts = 1 },
            });
            return "no timeout";
        }
        catch (ActivityFailureException e)
        {
            return Opts.TimeoutKind(e);
        }
    }
}

[Workflow("signal_with_start")]
public class SignalWithStart
{
    private string? value;

    [WorkflowRun]
    public async Task<string> RunAsync()
    {
        await Workflow.WaitConditionAsync(() => value != null);
        return $"started with {value}";
    }

    [WorkflowSignal("go")]
    public Task GoAsync(string s)
    {
        value = s;
        return Task.CompletedTask;
    }
}

[Workflow("child_greeter")]
public class ChildGreeter
{
    [WorkflowRun]
    public async Task<string> RunAsync(string name) => await Opts.Greet(name);
}

[Workflow("child_sleeper")]
public class ChildSleeper
{
    [WorkflowRun]
    public async Task RunAsync() => await Workflow.DelayAsync(TimeSpan.FromHours(1));
}

[Workflow("child_workflow")]
public class ChildWorkflow
{
    [WorkflowRun]
    public async Task<string> RunAsync() =>
        await Workflow.ExecuteChildWorkflowAsync<string>(
            "child_greeter",
            new object?[] { "child" },
            new ChildWorkflowOptions { Id = $"{Workflow.Info.WorkflowId}-child" });
}

[Workflow("child_workflow_cancelled")]
public class ChildWorkflowCancelled
{
    [WorkflowRun]
    public async Task<string> RunAsync()
    {
        using var cts = CancellationTokenSource.CreateLinkedTokenSource(Workflow.CancellationToken);
        var child = await Workflow.StartChildWorkflowAsync(
            "child_sleeper",
            Opts.NoArgs,
            new ChildWorkflowOptions { Id = $"{Workflow.Info.WorkflowId}-child", CancellationToken = cts.Token });
        cts.Cancel();
        try
        {
            await child.GetResultAsync();
        }
        catch (Exception e) when (TemporalException.IsCanceledException(e))
        {
            return "child cancelled";
        }
        throw new ApplicationFailureException("the child was not cancelled", "CorpusBug");
    }
}

[Workflow("continue_as_new_3")]
public class ContinueAsNew3
{
    [WorkflowRun]
    public async Task<int> RunAsync(int n)
    {
        if (n < 3)
        {
            throw Workflow.CreateContinueAsNewException("continue_as_new_3", new object?[] { n + 1 });
        }
        return n;
    }
}

[Workflow("id_reuse_allow_duplicate_failed_only")]
public class IdReuseAllowDuplicateFailedOnly
{
    [WorkflowRun]
    public async Task<string> RunAsync(bool fail)
    {
        if (fail)
        {
            throw new ApplicationFailureException("first run fails", "CorpusFirstRun", true);
        }
        return "second run";
    }
}

[Workflow("id_conflict_fail")]
public class IdConflictFail
{
    private bool done;

    [WorkflowRun]
    public async Task RunAsync() => await Workflow.WaitConditionAsync(() => done);

    [WorkflowSignal("finish")]
    public Task FinishAsync()
    {
        done = true;
        return Task.CompletedTask;
    }
}

[Workflow("local_activity")]
public class LocalActivity
{
    [WorkflowRun]
    public async Task<string> RunAsync() =>
        await Workflow.ExecuteLocalActivityAsync<string>(
            "greet",
            new object?[] { "local" },
            new LocalActivityOptions { StartToCloseTimeout = TimeSpan.FromSeconds(10) });
}

[Workflow("side_effect_and_version_marker")]
public class SideEffectAndVersionMarker
{
    [WorkflowRun]
    public async Task<string> RunAsync()
    {
        // .NET has no SideEffect (workflow code is deterministic by
        // construction, as in TypeScript); Patched writes the version marker.
        var v = Workflow.Patched("corpus-change") ? 1 : 0;
        return $"version {v}";
    }
}

// --- DX3 ----------------------------------------------------------------------

[Workflow("search_attributes_upsert")]
public class SearchAttributesUpsert
{
    private static readonly SearchAttributeKey<string> Key = SearchAttributeKey.CreateKeyword(Opts.SearchAttribute);

    [WorkflowRun]
    public async Task RunAsync()
    {
        Workflow.UpsertTypedSearchAttributes(Key.ValueSet("first"));
        await Workflow.DelayAsync(TimeSpan.FromMilliseconds(100));
        Workflow.UpsertTypedSearchAttributes(Key.ValueSet("second"));
    }
}

[Workflow("memo")]
public class Memo
{
    [WorkflowRun]
    public async Task RunAsync() =>
        Workflow.UpsertMemo(MemoUpdate.ValueSet("step", "upserted"), MemoUpdate.ValueSet("n", 2));
}

[Workflow("large_payload_1mb")]
public class LargePayload1Mb
{
    [WorkflowRun]
    public async Task<int> RunAsync() =>
        (await Workflow.ExecuteActivityAsync<string>("large", new object?[] { 1 << 20 }, Opts.Once())).Length;
}

// --- DX4 ----------------------------------------------------------------------

// The two update scenarios share one body; each class repeats its handlers
// (handlers are declared on the workflow class itself).
[Workflow("update_accepted_completed")]
public class UpdateAcceptedCompleted
{
    private int total;
    private bool done;

    [WorkflowRun]
    public async Task<string> RunAsync()
    {
        await Workflow.WaitConditionAsync(() => done);
        return $"total {total}";
    }

    [WorkflowUpdate("increment")]
    public async Task<int> IncrementAsync(int by)
    {
        total += by;
        return total;
    }

    [WorkflowUpdateValidator(nameof(IncrementAsync))]
    public void ValidateIncrement(int by) => Opts.ValidateIncrement(by);

    [WorkflowSignal("finish")]
    public Task FinishAsync()
    {
        done = true;
        return Task.CompletedTask;
    }
}

[Workflow("update_rejected")]
public class UpdateRejected
{
    private int total;
    private bool done;

    [WorkflowRun]
    public async Task<string> RunAsync()
    {
        await Workflow.WaitConditionAsync(() => done);
        return $"total {total}";
    }

    [WorkflowUpdate("increment")]
    public async Task<int> IncrementAsync(int by)
    {
        total += by;
        return total;
    }

    [WorkflowUpdateValidator(nameof(IncrementAsync))]
    public void ValidateIncrement(int by) => Opts.ValidateIncrement(by);

    [WorkflowSignal("finish")]
    public Task FinishAsync()
    {
        done = true;
        return Task.CompletedTask;
    }
}
