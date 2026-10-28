// Command corpus-go generates the Go half of the history corpus (DX0 Task 4)
// against a real Temporal server, and exports any workflow's histories.
//
//	corpus-go generate -address temporal-frontend:7233 -namespace corpus-1 -out histories/go
//	corpus-go export   -address ... -namespace corpus-1 -out histories/ts -scenarios hello,memo -prefix corpus-ts-
//
// TEMPORAL_API_KEY, when set, is sent as `authorization: Bearer <key>` on
// every call, in plain text (the Loams gateway's API keys, X1.R5; its
// compose profile has no TLS on the compose network). Unset, nothing changes.
//
// generate runs a worker for the scenarios package and drives each scenario
// (signals, queries, cancels, updates, resets) with the fixed workflow ID
// corpus-go-<scenario>, then exports every run of that ID as
// <scenario>.json (protojson, the format `temporal workflow show -o json`
// writes and the SDK replayers read) and <scenario>.binpb (the
// GetWorkflowExecutionHistory History message). A scenario with several runs
// (continue-as-new, ID reuse, reset) gets <scenario>.run<k>.{json,binpb}, in
// start order. It also writes <out>/../generate-<lang>.json: what each
// driver observed (expected errors included), for the manifest.
package main

import (
	"context"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"log"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"

	commonpb "go.temporal.io/api/common/v1"
	enumspb "go.temporal.io/api/enums/v1"
	historypb "go.temporal.io/api/history/v1"
	"go.temporal.io/api/operatorservice/v1"
	"go.temporal.io/api/serviceerror"
	"go.temporal.io/api/workflowservice/v1"
	"go.temporal.io/sdk/client"
	"go.temporal.io/sdk/activity"
	"go.temporal.io/sdk/temporal"
	"go.temporal.io/sdk/worker"
	"go.temporal.io/sdk/workflow"
	"google.golang.org/protobuf/encoding/protojson"
	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/types/known/durationpb"

	"github.com/ostrium-labs/loams/conformance/compute/temporal/corpus/workers/go/scenarios"
)

const queue = "corpus-go"

func main() {
	if len(os.Args) < 2 {
		log.Fatal("usage: corpus-go generate|export [flags]")
	}
	cmd := os.Args[1]
	fs := flag.NewFlagSet(cmd, flag.ExitOnError)
	address := fs.String("address", "temporal-frontend:7233", "")
	namespace := fs.String("namespace", "corpus", "created when missing")
	out := fs.String("out", "histories/go", "")
	only := fs.String("scenarios", "", "comma-separated; default all (generate) or required (export)")
	prefix := fs.String("prefix", "corpus-go-", "workflow ID prefix (export)")
	// The Loams gateway serves no OperatorService before DX4 (design 74
	// §5.8); its DX1 scenarios need no custom search attribute.
	noSearchAttribute := fs.Bool("no-search-attribute", false, "skip registering the corpus's search attribute")
	_ = fs.Parse(os.Args[2:])

	ctx := context.Background()
	if err := ensureNamespace(ctx, *address, *namespace, !*noSearchAttribute); err != nil {
		log.Fatalf("corpus: namespace %s: %v", *namespace, err)
	}
	c, err := client.Dial(withAPIKey(client.Options{HostPort: *address, Namespace: *namespace}))
	if err != nil {
		log.Fatalf("corpus: dial: %v", err)
	}
	defer c.Close()
	if err := os.MkdirAll(*out, 0o755); err != nil {
		log.Fatal(err)
	}

	switch cmd {
	case "generate":
		if err := generate(ctx, c, *namespace, *out, pick(*only)); err != nil {
			log.Fatalf("corpus: %v", err)
		}
	case "export":
		if *only == "" {
			log.Fatal("corpus: export needs -scenarios")
		}
		for _, name := range strings.Split(*only, ",") {
			if err := export(ctx, c, *prefix+name, filepath.Join(*out, name)); err != nil {
				log.Fatalf("corpus: export %s: %v", name, err)
			}
		}
	default:
		log.Fatalf("corpus: unknown command %q", cmd)
	}
}

// bearer sends TEMPORAL_API_KEY as a bearer token (not
// client.NewAPIKeyStaticCredentials, which also turns TLS on).
type bearer string

func (b bearer) GetHeaders(context.Context) (map[string]string, error) {
	return map[string]string{"authorization": "Bearer " + string(b)}, nil
}

// withAPIKey sets the bearer header provider when TEMPORAL_API_KEY is set.
// The SDK does not export the provider's interface type at v1.49.0, so the
// field is assigned here rather than returned.
func withAPIKey(o client.Options) client.Options {
	if k := os.Getenv("TEMPORAL_API_KEY"); k != "" {
		o.HeadersProvider = bearer(k)
	}
	return o
}

func pick(only string) []scenarios.Scenario {
	if only == "" {
		return scenarios.All
	}
	want := map[string]bool{}
	for _, n := range strings.Split(only, ",") {
		want[n] = true
	}
	var out []scenarios.Scenario
	for _, s := range scenarios.All {
		if want[s.Name] {
			out = append(out, s)
		}
	}
	return out
}

// ensureNamespace registers the namespace (72 h retention, DX1 R0.9) and the
// corpus's custom search attribute, both idempotently.
func ensureNamespace(ctx context.Context, address, ns string, searchAttribute bool) error {
	c, err := client.Dial(withAPIKey(client.Options{HostPort: address}))
	if err != nil {
		return err
	}
	defer c.Close()
	_, err = c.WorkflowService().RegisterNamespace(ctx, &workflowservice.RegisterNamespaceRequest{
		Namespace:                        ns,
		WorkflowExecutionRetentionPeriod: durationpb.New(72 * time.Hour),
	})
	var exists *serviceerror.NamespaceAlreadyExists
	if err != nil && !errors.As(err, &exists) {
		return err
	}
	if !searchAttribute {
		return nil
	}
	// A new namespace takes a moment to reach every service's cache.
	deadline := time.Now().Add(time.Minute)
	for {
		_, err = c.OperatorService().AddSearchAttributes(ctx, &operatorservice.AddSearchAttributesRequest{
			Namespace:        ns,
			SearchAttributes: map[string]enumspb.IndexedValueType{scenarios.SearchAttribute: enumspb.INDEXED_VALUE_TYPE_KEYWORD},
		})
		var already *serviceerror.AlreadyExists
		if err == nil || errors.As(err, &already) {
			break
		}
		if time.Now().After(deadline) {
			return fmt.Errorf("search attribute: %w", err)
		}
		time.Sleep(2 * time.Second)
	}
	time.Sleep(3 * time.Second)
	return nil
}

func newWorker(c client.Client) worker.Worker {
	// One activity at a time, so the parallel fan-out's activities complete
	// in the order they were scheduled and regenerating is stable.
	w := worker.New(c, queue, worker.Options{MaxConcurrentActivityExecutionSize: 1})
	for _, s := range scenarios.All {
		w.RegisterWorkflowWithOptions(s.Workflow, workflowOptions(s.Name))
	}
	for name, fn := range scenarios.Children {
		w.RegisterWorkflowWithOptions(fn, workflowOptions(name))
	}
	for name, fn := range scenarios.Activities {
		w.RegisterActivityWithOptions(fn, activityOptions(name))
	}
	return w
}

// Observation is what a driver saw, written to generate-go.json.
type Observation struct {
	Scenario   string   `json:"scenario"`
	WorkflowID string   `json:"workflow_id"`
	Runs       int      `json:"runs"`
	Result     string   `json:"result,omitempty"`
	Expected   []string `json:"expected,omitempty"`
}

func generate(ctx context.Context, c client.Client, ns, out string, list []scenarios.Scenario) error {
	w := newWorker(c)
	if err := w.Start(); err != nil {
		return err
	}
	defer w.Stop()
	var obs []Observation
	for _, s := range list {
		id := "corpus-go-" + s.Name
		dctx, cancel := context.WithTimeout(ctx, 3*time.Minute)
		o, err := drive(dctx, c, ns, s.Name, id)
		cancel()
		if err != nil {
			return fmt.Errorf("%s: %w", s.Name, err)
		}
		if o.WorkflowID == "" {
			o.WorkflowID = id
		}
		o.Scenario = s.Name
		if o.Runs, err = exportCount(ctx, c, o.WorkflowID, filepath.Join(out, s.Name)); err != nil {
			return fmt.Errorf("%s: export: %w", s.Name, err)
		}
		log.Printf("corpus: %s: %d run(s), %s", s.Name, o.Runs, o.Result)
		obs = append(obs, o)
	}
	b, _ := json.MarshalIndent(map[string]any{"namespace": ns, "language": "go", "scenarios": obs}, "", "  ")
	return os.WriteFile(filepath.Join(filepath.Dir(out), "generate-go.json"), append(b, '\n'), 0o644)
}

func opts(id string) client.StartWorkflowOptions {
	return client.StartWorkflowOptions{ID: id, TaskQueue: queue}
}

func result(ctx context.Context, run client.WorkflowRun) (string, error) {
	var v any
	err := run.Get(ctx, &v)
	b, _ := json.Marshal(v)
	return string(b), err
}

// waitStarted waits until the run's first workflow task completed, so a
// signal, query, cancel or update lands on a running workflow.
func waitStarted(ctx context.Context, c client.Client, id string) error {
	for {
		d, err := c.DescribeWorkflowExecution(ctx, id, "")
		if err == nil && d.PendingWorkflowTask == nil && d.WorkflowExecutionInfo.GetHistoryLength() >= 4 {
			return nil
		}
		select {
		case <-ctx.Done():
			return ctx.Err()
		case <-time.After(200 * time.Millisecond):
		}
	}
}

func drive(ctx context.Context, c client.Client, ns, name, id string) (Observation, error) {
	var o Observation
	switch name {
	case "hello":
		run, err := c.ExecuteWorkflow(ctx, opts(id), name, "corpus")
		if err != nil {
			return o, err
		}
		o.Result, err = result(ctx, run)
		return o, err
	case "signal_then_complete", "signal_with_start":
		var run client.WorkflowRun
		var err error
		if name == "signal_with_start" {
			run, err = c.SignalWithStartWorkflow(ctx, id, scenarios.SignalGo, "s", opts(id), name)
		} else {
			if run, err = c.ExecuteWorkflow(ctx, opts(id), name); err == nil {
				if err = waitStarted(ctx, c, id); err == nil {
					err = c.SignalWorkflow(ctx, id, "", scenarios.SignalGo, "s")
				}
			}
		}
		if err != nil {
			return o, err
		}
		o.Result, err = result(ctx, run)
		return o, err
	case "query_while_running":
		run, err := c.ExecuteWorkflow(ctx, opts(id), name)
		if err != nil {
			return o, err
		}
		if err := waitStarted(ctx, c, id); err != nil {
			return o, err
		}
		v, err := c.QueryWorkflow(ctx, id, "", scenarios.QueryState)
		if err != nil {
			return o, err
		}
		var state string
		if err := v.Get(&state); err != nil || state != "waiting" {
			return o, fmt.Errorf("query answered %q, %v", state, err)
		}
		o.Expected = append(o.Expected, "query state=waiting")
		if err := c.SignalWorkflow(ctx, id, "", scenarios.SignalFinish, nil); err != nil {
			return o, err
		}
		o.Result, err = result(ctx, run)
		return o, err
	case "cancel_requested", "terminated":
		run, err := c.ExecuteWorkflow(ctx, opts(id), name)
		if err != nil {
			return o, err
		}
		if err := waitStarted(ctx, c, id); err != nil {
			return o, err
		}
		if name == "cancel_requested" {
			err = c.CancelWorkflow(ctx, id, "")
		} else {
			err = c.TerminateWorkflow(ctx, id, "", "corpus terminates it")
		}
		if err != nil {
			return o, err
		}
		_, err = result(ctx, run)
		if err == nil {
			return o, errors.New("the workflow completed instead of closing as " + name)
		}
		o.Expected = append(o.Expected, err.Error())
		return o, nil
	case "workflow_failed_application_error":
		run, err := c.ExecuteWorkflow(ctx, opts(id), name)
		if err != nil {
			return o, err
		}
		_, err = result(ctx, run)
		var app *temporal.ApplicationError
		if !errors.As(err, &app) || app.Type() != "CorpusBoom" {
			return o, fmt.Errorf("want an application error CorpusBoom, got %v", err)
		}
		o.Expected = append(o.Expected, err.Error())
		return o, nil
	case "workflow_task_failure_then_fix":
		scenarios.FailWorkflowTasks.Store(1)
		defer scenarios.FailWorkflowTasks.Store(0)
		run, err := c.ExecuteWorkflow(ctx, opts(id), name)
		if err != nil {
			return o, err
		}
		o.Expected = append(o.Expected, "one failed workflow task before the fix")
		o.Result, err = result(ctx, run)
		return o, err
	case "continue_as_new_3":
		run, err := c.ExecuteWorkflow(ctx, opts(id), name, 0)
		if err != nil {
			return o, err
		}
		o.Result, err = result(ctx, run)
		return o, err
	case "id_reuse_allow_duplicate_failed_only":
		so := opts(id)
		so.WorkflowIDReusePolicy = enumspb.WORKFLOW_ID_REUSE_POLICY_ALLOW_DUPLICATE_FAILED_ONLY
		run, err := c.ExecuteWorkflow(ctx, so, name, true)
		if err != nil {
			return o, err
		}
		if _, err := result(ctx, run); err == nil {
			return o, errors.New("the first run should fail")
		}
		run, err = c.ExecuteWorkflow(ctx, so, name, false)
		if err != nil {
			return o, fmt.Errorf("the second start was refused: %w", err)
		}
		o.Expected = append(o.Expected, "the first run failed; the second started with the same ID")
		o.Result, err = result(ctx, run)
		return o, err
	case "id_conflict_fail":
		so := opts(id)
		so.WorkflowIDConflictPolicy = enumspb.WORKFLOW_ID_CONFLICT_POLICY_FAIL
		// Without this the SDK hands back the running workflow instead of
		// the server's refusal.
		so.WorkflowExecutionErrorWhenAlreadyStarted = true
		run, err := c.ExecuteWorkflow(ctx, so, name)
		if err != nil {
			return o, err
		}
		_, err = c.ExecuteWorkflow(ctx, so, name)
		var started *serviceerror.WorkflowExecutionAlreadyStarted
		if !errors.As(err, &started) {
			return o, fmt.Errorf("the second start should be refused, got %v", err)
		}
		o.Expected = append(o.Expected, "the second start was refused: WorkflowExecutionAlreadyStarted")
		if err := c.SignalWorkflow(ctx, id, "", scenarios.SignalFinish, nil); err != nil {
			return o, err
		}
		o.Result, err = result(ctx, run)
		return o, err
	case "memo":
		so := opts(id)
		so.Memo = map[string]any{"initial": "set at start"}
		run, err := c.ExecuteWorkflow(ctx, so, name)
		if err != nil {
			return o, err
		}
		o.Result, err = result(ctx, run)
		return o, err
	case "update_accepted_completed", "update_rejected":
		run, err := c.ExecuteWorkflow(ctx, opts(id), name)
		if err != nil {
			return o, err
		}
		if err := waitStarted(ctx, c, id); err != nil {
			return o, err
		}
		by := 2
		if name == "update_rejected" {
			by = -1
		}
		h, err := c.UpdateWorkflow(ctx, client.UpdateWorkflowOptions{
			WorkflowID: id, UpdateName: scenarios.UpdateIncrement, Args: []any{by},
			UpdateID: id + "-update", WaitForStage: client.WorkflowUpdateStageCompleted,
		})
		if err == nil {
			var total int
			err = h.Get(ctx, &total)
			o.Expected = append(o.Expected, fmt.Sprintf("update returned %d", total))
		}
		if name == "update_rejected" {
			if err == nil {
				return o, errors.New("the validator should reject a negative increment")
			}
			o.Expected = append(o.Expected, "update rejected: "+err.Error())
		} else if err != nil {
			return o, err
		}
		if err := c.SignalWorkflow(ctx, id, "", scenarios.SignalFinish, nil); err != nil {
			return o, err
		}
		o.Result, err = result(ctx, run)
		return o, err
	case "schedule_triggered":
		h, err := c.ScheduleClient().Create(ctx, client.ScheduleOptions{
			ID:     id,
			Spec:   client.ScheduleSpec{},
			Paused: true,
			Action: &client.ScheduleWorkflowAction{ID: id + "-wf", Workflow: name, TaskQueue: queue},
		})
		if err != nil {
			return o, err
		}
		defer func() { _ = h.Delete(context.Background()) }()
		if err := h.Trigger(ctx, client.ScheduleTriggerOptions{}); err != nil {
			return o, err
		}
		for {
			d, err := h.Describe(ctx)
			if err != nil {
				return o, err
			}
			if len(d.Info.RecentActions) > 0 {
				o.WorkflowID = d.Info.RecentActions[0].StartWorkflowResult.WorkflowID
				break
			}
			time.Sleep(300 * time.Millisecond)
		}
		o.Expected = append(o.Expected, "started by schedule "+id+" (the server appends the action time to the ID)")
		o.Result, err = result(ctx, c.GetWorkflow(ctx, o.WorkflowID, ""))
		return o, err
	case "reset_to_first_workflow_task":
		run, err := c.ExecuteWorkflow(ctx, opts(id), name)
		if err != nil {
			return o, err
		}
		if _, err := result(ctx, run); err != nil {
			return o, err
		}
		first, err := firstEvent(ctx, c, id, run.GetRunID(), enumspb.EVENT_TYPE_WORKFLOW_TASK_COMPLETED)
		if err != nil {
			return o, err
		}
		r, err := c.ResetWorkflowExecution(ctx, &workflowservice.ResetWorkflowExecutionRequest{
			Namespace:                 ns,
			WorkflowExecution:         &commonpb.WorkflowExecution{WorkflowId: id, RunId: run.GetRunID()},
			Reason:                    "corpus: reset to the first workflow task",
			WorkflowTaskFinishEventId: first,
			RequestId:                 id + "-reset",
		})
		if err != nil {
			return o, err
		}
		o.Expected = append(o.Expected, fmt.Sprintf("reset to event %d", first))
		o.Result, err = result(ctx, c.GetWorkflow(ctx, id, r.GetRunId()))
		return o, err
	default:
		run, err := c.ExecuteWorkflow(ctx, opts(id), name)
		if err != nil {
			return o, err
		}
		o.Result, err = result(ctx, run)
		return o, err
	}
}

func firstEvent(ctx context.Context, c client.Client, id, runID string, t enumspb.EventType) (int64, error) {
	it := c.GetWorkflowHistory(ctx, id, runID, false, enumspb.HISTORY_EVENT_FILTER_TYPE_ALL_EVENT)
	for it.HasNext() {
		e, err := it.Next()
		if err != nil {
			return 0, err
		}
		if e.GetEventType() == t {
			return e.GetEventId(), nil
		}
	}
	return 0, fmt.Errorf("no %s event", t)
}

// runsOf lists every run of a workflow ID, oldest first (visibility).
func runsOf(ctx context.Context, c client.Client, id string) ([]string, error) {
	type run struct {
		id    string
		start time.Time
	}
	deadline := time.Now().Add(30 * time.Second)
	if _, err := c.ListWorkflow(ctx, &workflowservice.ListWorkflowExecutionsRequest{
		Query: fmt.Sprintf("WorkflowId = '%s'", id), PageSize: 1,
	}); err != nil {
		var unimplemented *serviceerror.Unimplemented
		if !errors.As(err, &unimplemented) {
			return nil, err
		}
		// A server without visibility (the Loams Temporal gateway before
		// DX3): the current run only, once it has closed.
		for {
			d, err := c.DescribeWorkflowExecution(ctx, id, "")
			if err != nil {
				return nil, err
			}
			if d.WorkflowExecutionInfo.Status != enumspb.WORKFLOW_EXECUTION_STATUS_RUNNING {
				return []string{d.WorkflowExecutionInfo.Execution.RunId}, nil
			}
			if time.Now().After(deadline) {
				return nil, fmt.Errorf("%s: still running", id)
			}
			time.Sleep(200 * time.Millisecond)
		}
	}
	for {
		var runs []run
		var token []byte
		for {
			resp, err := c.ListWorkflow(ctx, &workflowservice.ListWorkflowExecutionsRequest{
				Query: fmt.Sprintf("WorkflowId = '%s'", id), NextPageToken: token,
			})
			if err != nil {
				return nil, err
			}
			for _, e := range resp.Executions {
				runs = append(runs, run{e.Execution.RunId, e.StartTime.AsTime()})
			}
			if token = resp.NextPageToken; len(token) == 0 {
				break
			}
		}
		// Visibility is eventually consistent: wait until every run is closed.
		closed := true
		for _, r := range runs {
			d, err := c.DescribeWorkflowExecution(ctx, id, r.id)
			if err != nil || d.WorkflowExecutionInfo.Status == enumspb.WORKFLOW_EXECUTION_STATUS_RUNNING {
				closed = false
			}
		}
		if len(runs) > 0 && closed {
			sort.Slice(runs, func(i, j int) bool { return runs[i].start.Before(runs[j].start) })
			ids := make([]string, len(runs))
			for i, r := range runs {
				ids[i] = r.id
			}
			return ids, nil
		}
		if time.Now().After(deadline) {
			return nil, fmt.Errorf("%s: %d runs listed, not all closed", id, len(runs))
		}
		time.Sleep(time.Second)
	}
}

func export(ctx context.Context, c client.Client, id, base string) error {
	_, err := exportCount(ctx, c, id, base)
	return err
}

func exportCount(ctx context.Context, c client.Client, id, base string) (int, error) {
	runs, err := runsOf(ctx, c, id)
	if err != nil {
		return 0, err
	}
	for i, runID := range runs {
		h := &historypb.History{}
		it := c.GetWorkflowHistory(ctx, id, runID, false, enumspb.HISTORY_EVENT_FILTER_TYPE_ALL_EVENT)
		for it.HasNext() {
			e, err := it.Next()
			if err != nil {
				return 0, err
			}
			h.Events = append(h.Events, e)
		}
		name := base
		if len(runs) > 1 {
			name = fmt.Sprintf("%s.run%d", base, i+1)
		}
		js, err := protojson.MarshalOptions{Multiline: true, Indent: "  "}.Marshal(h)
		if err != nil {
			return 0, err
		}
		bin, err := proto.Marshal(h)
		if err != nil {
			return 0, err
		}
		if err := os.WriteFile(name+".json", append(js, '\n'), 0o644); err != nil {
			return 0, err
		}
		if err := os.WriteFile(name+".binpb", bin, 0o644); err != nil {
			return 0, err
		}
	}
	return len(runs), nil
}

func workflowOptions(name string) workflow.RegisterOptions { return workflow.RegisterOptions{Name: name} }

func activityOptions(name string) activity.RegisterOptions { return activity.RegisterOptions{Name: name} }
