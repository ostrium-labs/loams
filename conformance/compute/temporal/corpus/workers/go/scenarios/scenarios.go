// Package scenarios holds the corpus's Go workflows and activities (DX0
// Task 4): one workflow per scenario, registered under the scenario's name,
// so the generator and the replayer agree on what each history runs. A
// change to a workflow here changes the histories it produces: regenerate
// the corpus (generate.sh) in the same PR.
package scenarios

import (
	"context"
	"errors"
	"fmt"
	"strings"
	"sync/atomic"
	"time"

	"go.temporal.io/sdk/activity"
	"go.temporal.io/sdk/temporal"
	"go.temporal.io/sdk/workflow"
)

// Phase is the DX phase whose gateway must replay a scenario (design 74 §14).
type Phase string

const (
	DX1 Phase = "DX1"
	DX2 Phase = "DX2"
	DX3 Phase = "DX3"
	DX4 Phase = "DX4"
)

// Scenario names a workflow and the phase it belongs to.
type Scenario struct {
	Name     string
	Phase    Phase
	Workflow any
	// GoOnly marks the scenarios the TypeScript worker does not have.
	GoOnly bool
}

// All is the corpus in plan order.
var All = []Scenario{
	{"hello", DX1, Hello, false},
	{"two_activities_sequential", DX1, TwoActivitiesSequential, false},
	{"activity_parallel_fanout_10", DX1, ActivityParallelFanout10, false},
	{"signal_then_complete", DX1, SignalThenComplete, false},
	{"query_while_running", DX1, QueryWhileRunning, false},
	{"cancel_requested", DX1, CancelRequested, false},
	{"terminated", DX1, Terminated, false},
	{"workflow_failed_application_error", DX1, WorkflowFailedApplicationError, false},
	{"activity_heartbeat_details", DX1, ActivityHeartbeatDetails, false},

	{"timer_short", DX2, TimerShort, false},
	{"timer_cancelled", DX2, TimerCancelled, false},
	{"activity_retry_backoff_3_attempts", DX2, ActivityRetryBackoff3Attempts, false},
	{"activity_start_to_close_timeout", DX2, ActivityStartToCloseTimeout, false},
	{"activity_schedule_to_start_timeout", DX2, ActivityScheduleToStartTimeout, false},
	{"activity_heartbeat_timeout", DX2, ActivityHeartbeatTimeout, false},
	{"workflow_task_failure_then_fix", DX2, WorkflowTaskFailureThenFix, true},
	{"signal_with_start", DX2, SignalWithStart, false},
	{"child_workflow", DX2, ChildWorkflow, false},
	{"child_workflow_cancelled", DX2, ChildWorkflowCancelled, false},
	{"continue_as_new_3", DX2, ContinueAsNew3, false},
	{"id_reuse_allow_duplicate_failed_only", DX2, IDReuseAllowDuplicateFailedOnly, false},
	{"id_conflict_fail", DX2, IDConflictFail, false},
	{"local_activity", DX2, LocalActivity, false},
	{"side_effect_and_version_marker", DX2, SideEffectAndVersionMarker, false},

	{"search_attributes_upsert", DX3, SearchAttributesUpsert, false},
	{"memo", DX3, Memo, false},
	{"large_payload_1mb", DX3, LargePayload1MB, false},

	{"update_accepted_completed", DX4, UpdateAcceptedCompleted, false},
	{"update_rejected", DX4, UpdateRejected, false},
	{"schedule_triggered", DX4, ScheduleTriggered, true},
	{"reset_to_first_workflow_task", DX4, ResetToFirstWorkflowTask, true},
}

// The child workflows the scenarios start, registered alongside them.
var Children = map[string]any{
	"child_sleeper": ChildSleeper,
	"child_greeter": ChildGreeter,
}

// Activities, registered by name.
var Activities = map[string]any{
	"greet":            Greet,
	"heartbeat_thrice": HeartbeatThrice,
	"fail_twice":       FailTwice,
	"sleep_for":        SleepFor,
	"large":            Large,
}

// Signal, query and update names the generator uses.
const (
	SignalGo        = "go"
	SignalFinish    = "finish"
	QueryState      = "state"
	UpdateIncrement = "increment"
	// The custom search attribute search_attributes_upsert sets.
	SearchAttribute = "CorpusKeyword"
)

// FailWorkflowTasks is how many workflow tasks of WorkflowTaskFailureThenFix
// panic before the "fix": the generator sets it to 1, so exactly one
// WorkflowTaskFailed is recorded and the retried task completes the
// workflow. The replayer leaves it at 0.
var FailWorkflowTasks atomic.Int32

func ao(ctx workflow.Context) workflow.Context {
	return workflow.WithActivityOptions(ctx, workflow.ActivityOptions{
		StartToCloseTimeout: 30 * time.Second,
		RetryPolicy:         &temporal.RetryPolicy{MaximumAttempts: 1},
	})
}

// --- Activities -------------------------------------------------------------

func Greet(_ context.Context, name string) (string, error) { return "hello " + name, nil }

// LocalGreet is local_activity's activity. It is not registered with the
// worker, so its type is the function's name in the generator and the
// replayer alike.
func LocalGreet(ctx context.Context, name string) (string, error) { return Greet(ctx, name) }

func HeartbeatThrice(ctx context.Context) (int, error) {
	for i := 1; i <= 3; i++ {
		activity.RecordHeartbeat(ctx, i)
	}
	return 3, nil
}

func FailTwice(ctx context.Context) (int32, error) {
	attempt := activity.GetInfo(ctx).Attempt
	if attempt < 3 {
		return attempt, temporal.NewApplicationError(fmt.Sprintf("attempt %d fails", attempt), "Transient")
	}
	return attempt, nil
}

func SleepFor(ctx context.Context, d time.Duration) error {
	select {
	case <-time.After(d):
		return nil
	case <-ctx.Done():
		return ctx.Err()
	}
}

func Large(_ context.Context, n int) (string, error) { return strings.Repeat("x", n), nil }

// --- DX1 ----------------------------------------------------------------------

func Hello(ctx workflow.Context, name string) (string, error) {
	var out string
	err := workflow.ExecuteActivity(ao(ctx), "greet", name).Get(ctx, &out)
	return out, err
}

func TwoActivitiesSequential(ctx workflow.Context) (string, error) {
	var a, b string
	if err := workflow.ExecuteActivity(ao(ctx), "greet", "one").Get(ctx, &a); err != nil {
		return "", err
	}
	if err := workflow.ExecuteActivity(ao(ctx), "greet", "two").Get(ctx, &b); err != nil {
		return "", err
	}
	return a + ", " + b, nil
}

func ActivityParallelFanout10(ctx workflow.Context) ([]string, error) {
	futures := make([]workflow.Future, 10)
	for i := range futures {
		futures[i] = workflow.ExecuteActivity(ao(ctx), "greet", fmt.Sprintf("n%d", i))
	}
	out := make([]string, len(futures))
	for i, f := range futures {
		if err := f.Get(ctx, &out[i]); err != nil {
			return nil, err
		}
	}
	return out, nil
}

func SignalThenComplete(ctx workflow.Context) (string, error) {
	var v string
	workflow.GetSignalChannel(ctx, SignalGo).Receive(ctx, &v)
	return "got " + v, nil
}

func QueryWhileRunning(ctx workflow.Context) (string, error) {
	state := "waiting"
	if err := workflow.SetQueryHandler(ctx, QueryState, func() (string, error) { return state, nil }); err != nil {
		return "", err
	}
	workflow.GetSignalChannel(ctx, SignalFinish).Receive(ctx, nil)
	state = "done"
	return state, nil
}

func CancelRequested(ctx workflow.Context) error {
	// Cancelled while it sleeps: the workflow closes as cancelled.
	return workflow.Sleep(ctx, time.Hour)
}

func Terminated(ctx workflow.Context) error {
	workflow.GetSignalChannel(ctx, SignalFinish).Receive(ctx, nil)
	return nil
}

func WorkflowFailedApplicationError(ctx workflow.Context) error {
	return temporal.NewNonRetryableApplicationError("boom", "CorpusBoom", nil, "detail")
}

func ActivityHeartbeatDetails(ctx workflow.Context) (int, error) {
	ctx = workflow.WithActivityOptions(ctx, workflow.ActivityOptions{
		StartToCloseTimeout: 30 * time.Second,
		HeartbeatTimeout:    10 * time.Second,
		RetryPolicy:         &temporal.RetryPolicy{MaximumAttempts: 1},
	})
	var n int
	err := workflow.ExecuteActivity(ctx, "heartbeat_thrice").Get(ctx, &n)
	return n, err
}

// --- DX2 ----------------------------------------------------------------------

func TimerShort(ctx workflow.Context) error { return workflow.Sleep(ctx, time.Second) }

func TimerCancelled(ctx workflow.Context) (string, error) {
	timerCtx, cancel := workflow.WithCancel(ctx)
	long := workflow.NewTimer(timerCtx, time.Hour)
	if err := workflow.Sleep(ctx, 100*time.Millisecond); err != nil {
		return "", err
	}
	cancel()
	if err := long.Get(ctx, nil); !temporal.IsCanceledError(err) {
		return "", fmt.Errorf("the long timer was not cancelled: %v", err)
	}
	return "cancelled", nil
}

func ActivityRetryBackoff3Attempts(ctx workflow.Context) (int32, error) {
	ctx = workflow.WithActivityOptions(ctx, workflow.ActivityOptions{
		StartToCloseTimeout: 10 * time.Second,
		RetryPolicy: &temporal.RetryPolicy{
			InitialInterval:    time.Second,
			BackoffCoefficient: 2,
			MaximumAttempts:    3,
		},
	})
	var attempt int32
	err := workflow.ExecuteActivity(ctx, "fail_twice").Get(ctx, &attempt)
	return attempt, err
}

func timeoutKind(err error) string {
	var te *temporal.TimeoutError
	if errors.As(err, &te) {
		return te.TimeoutType().String()
	}
	return fmt.Sprintf("not a timeout: %v", err)
}

func ActivityStartToCloseTimeout(ctx workflow.Context) (string, error) {
	ctx = workflow.WithActivityOptions(ctx, workflow.ActivityOptions{
		StartToCloseTimeout: time.Second,
		RetryPolicy:         &temporal.RetryPolicy{MaximumAttempts: 1},
	})
	err := workflow.ExecuteActivity(ctx, "sleep_for", 5*time.Second).Get(ctx, nil)
	return timeoutKind(err), nil
}

// NoWorkerQueue has no worker: an activity there times out before it starts.
const NoWorkerQueue = "corpus-no-worker"

func ActivityScheduleToStartTimeout(ctx workflow.Context) (string, error) {
	ctx = workflow.WithActivityOptions(ctx, workflow.ActivityOptions{
		TaskQueue:              NoWorkerQueue,
		ScheduleToStartTimeout: time.Second,
		StartToCloseTimeout:    time.Second,
		RetryPolicy:            &temporal.RetryPolicy{MaximumAttempts: 1},
	})
	err := workflow.ExecuteActivity(ctx, "greet", "nobody").Get(ctx, nil)
	return timeoutKind(err), nil
}

func ActivityHeartbeatTimeout(ctx workflow.Context) (string, error) {
	ctx = workflow.WithActivityOptions(ctx, workflow.ActivityOptions{
		StartToCloseTimeout: 30 * time.Second,
		HeartbeatTimeout:    time.Second,
		RetryPolicy:         &temporal.RetryPolicy{MaximumAttempts: 1},
	})
	err := workflow.ExecuteActivity(ctx, "sleep_for", 5*time.Second).Get(ctx, nil)
	return timeoutKind(err), nil
}

func WorkflowTaskFailureThenFix(ctx workflow.Context) (string, error) {
	// The counter is read outside a side effect on purpose: it is process
	// state, so the first attempt panics (WorkflowTaskFailed) and the retry
	// completes.
	if FailWorkflowTasks.Add(-1) >= 0 {
		panic("corpus: the first workflow task fails")
	}
	var out string
	err := workflow.ExecuteActivity(ao(ctx), "greet", "after the fix").Get(ctx, &out)
	return out, err
}

func SignalWithStart(ctx workflow.Context) (string, error) {
	var v string
	workflow.GetSignalChannel(ctx, SignalGo).Receive(ctx, &v)
	return "started with " + v, nil
}

func ChildGreeter(ctx workflow.Context, name string) (string, error) { return Hello(ctx, name) }

func ChildSleeper(ctx workflow.Context) error { return workflow.Sleep(ctx, time.Hour) }

func ChildWorkflow(ctx workflow.Context) (string, error) {
	cctx := workflow.WithChildOptions(ctx, workflow.ChildWorkflowOptions{
		WorkflowID: workflow.GetInfo(ctx).WorkflowExecution.ID + "-child",
	})
	var out string
	err := workflow.ExecuteChildWorkflow(cctx, "child_greeter", "child").Get(ctx, &out)
	return out, err
}

func ChildWorkflowCancelled(ctx workflow.Context) (string, error) {
	childCtx, cancel := workflow.WithCancel(ctx)
	childCtx = workflow.WithChildOptions(childCtx, workflow.ChildWorkflowOptions{
		WorkflowID: workflow.GetInfo(ctx).WorkflowExecution.ID + "-child",
	})
	f := workflow.ExecuteChildWorkflow(childCtx, "child_sleeper")
	if err := f.GetChildWorkflowExecution().Get(ctx, nil); err != nil {
		return "", err
	}
	cancel()
	if err := f.Get(ctx, nil); !temporal.IsCanceledError(err) {
		return "", fmt.Errorf("the child was not cancelled: %v", err)
	}
	return "child cancelled", nil
}

func ContinueAsNew3(ctx workflow.Context, n int) (int, error) {
	if n < 3 {
		return 0, workflow.NewContinueAsNewError(ctx, "continue_as_new_3", n+1)
	}
	return n, nil
}

// IDReuseAllowDuplicateFailedOnly fails on its first run and succeeds on the
// second, which the generator starts with the same ID under
// ALLOW_DUPLICATE_FAILED_ONLY.
func IDReuseAllowDuplicateFailedOnly(ctx workflow.Context, fail bool) (string, error) {
	if fail {
		return "", temporal.NewNonRetryableApplicationError("first run fails", "CorpusFirstRun", nil)
	}
	return "second run", nil
}

// IDConflictFail runs until the generator signals it; a second start with
// the same ID under conflict policy FAIL is refused in between.
func IDConflictFail(ctx workflow.Context) error {
	workflow.GetSignalChannel(ctx, SignalFinish).Receive(ctx, nil)
	return nil
}

func LocalActivity(ctx workflow.Context) (string, error) {
	ctx = workflow.WithLocalActivityOptions(ctx, workflow.LocalActivityOptions{StartToCloseTimeout: 10 * time.Second})
	var out string
	err := workflow.ExecuteLocalActivity(ctx, LocalGreet, "local").Get(ctx, &out)
	return out, err
}

func SideEffectAndVersionMarker(ctx workflow.Context) (string, error) {
	v := workflow.GetVersion(ctx, "corpus-change", workflow.DefaultVersion, 1)
	var n int
	if err := workflow.SideEffect(ctx, func(workflow.Context) any { return 42 }).Get(&n); err != nil {
		return "", err
	}
	return fmt.Sprintf("version %d side effect %d", v, n), nil
}

// --- DX3 ----------------------------------------------------------------------

var corpusKeyword = temporal.NewSearchAttributeKeyKeyword(SearchAttribute)

func SearchAttributesUpsert(ctx workflow.Context) error {
	if err := workflow.UpsertTypedSearchAttributes(ctx, corpusKeyword.ValueSet("first")); err != nil {
		return err
	}
	if err := workflow.Sleep(ctx, 100*time.Millisecond); err != nil {
		return err
	}
	return workflow.UpsertTypedSearchAttributes(ctx, corpusKeyword.ValueSet("second"))
}

func Memo(ctx workflow.Context) error {
	return workflow.UpsertMemo(ctx, map[string]any{"step": "upserted", "n": 2})
}

func LargePayload1MB(ctx workflow.Context) (int, error) {
	var s string
	if err := workflow.ExecuteActivity(ao(ctx), "large", 1<<20).Get(ctx, &s); err != nil {
		return 0, err
	}
	return len(s), nil
}

// --- DX4 ----------------------------------------------------------------------

func updates(ctx workflow.Context) (string, error) {
	total := 0
	err := workflow.SetUpdateHandlerWithOptions(ctx, UpdateIncrement,
		func(ctx workflow.Context, by int) (int, error) {
			total += by
			return total, nil
		},
		workflow.UpdateHandlerOptions{Validator: func(ctx workflow.Context, by int) error {
			if by < 0 {
				return errors.New("increment must not be negative")
			}
			return nil
		}})
	if err != nil {
		return "", err
	}
	workflow.GetSignalChannel(ctx, SignalFinish).Receive(ctx, nil)
	return fmt.Sprintf("total %d", total), nil
}

func UpdateAcceptedCompleted(ctx workflow.Context) (string, error) { return updates(ctx) }

func UpdateRejected(ctx workflow.Context) (string, error) { return updates(ctx) }

func ScheduleTriggered(ctx workflow.Context) (string, error) { return Hello(ctx, "schedule") }

func ResetToFirstWorkflowTask(ctx workflow.Context) (string, error) {
	var a, b string
	if err := workflow.ExecuteActivity(ao(ctx), "greet", "before reset").Get(ctx, &a); err != nil {
		return "", err
	}
	if err := workflow.ExecuteActivity(ao(ctx), "greet", "after").Get(ctx, &b); err != nil {
		return "", err
	}
	return a + ", " + b, nil
}
