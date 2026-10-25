// Command soak is DX0 Task 3's load for compat mode: the stock Temporal
// server on Loams Postgres (design 74 D1509). It starts workflows at a fixed
// rate, each running one activity, with a worker in the same process, and
// then checks the effects rather than an exit code: every workflow it
// started completed, none failed or was lost, and each activity ran at least
// once (a second run is allowed, since activities are at-least-once, and is
// counted). History-service restarts and a Loams Postgres compute suspend
// happen around it (soak.sh); start calls are retried through them with a
// fixed workflow ID, so a retry never starts a second workflow.
//
//	soak -address temporal-frontend:7233 -rate 20 -duration 30m -out soak.json
package main

import (
	"context"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"log"
	"os"
	"sync"
	"sync/atomic"
	"time"

	enumspb "go.temporal.io/api/enums/v1"
	"go.temporal.io/api/serviceerror"
	"go.temporal.io/sdk/activity"
	"go.temporal.io/sdk/client"
	"go.temporal.io/sdk/worker"
	"go.temporal.io/sdk/workflow"
)

// Summary is what the run writes to -out; soak.sh turns it into JUnit cases.
type Summary struct {
	RunID              string  `json:"run_id"`
	Rate               float64 `json:"rate"`
	DurationSeconds    float64 `json:"duration_seconds"`
	Started            int64   `json:"started"`
	StartRetries       int64   `json:"start_retries"`
	StartGaveUp        int64   `json:"start_gave_up"`
	Completed          int64   `json:"completed"`
	Failed             int64   `json:"failed"`
	NotClosed          int64   `json:"not_closed"`
	WrongResult        int64   `json:"wrong_result"`
	ActivityRuns       int64   `json:"activity_runs"`
	ActivityRerun      int64   `json:"activity_rerun"`
	ActivityNeverRan   int64   `json:"activity_never_ran"`
	AchievedStartRate  float64 `json:"achieved_start_rate"`
	MaxStartLatencyMs  int64   `json:"max_start_latency_ms"`
	MaxCompleteSeconds float64 `json:"max_complete_seconds"`
	FirstFailure       string  `json:"first_failure,omitempty"`
	OK                 bool    `json:"ok"`
}

type counter struct {
	mu   sync.Mutex
	runs map[string]int
}

func (c *counter) add(id string) {
	c.mu.Lock()
	c.runs[id]++
	c.mu.Unlock()
}

var runs = &counter{runs: map[string]int{}}

// Noop is the one activity: it records that it ran for this workflow.
func Noop(ctx context.Context, id string) (string, error) {
	runs.add(id)
	return "done:" + id, nil
}

// SoakWorkflow runs Noop once and returns its result.
func SoakWorkflow(ctx workflow.Context, id string) (string, error) {
	ctx = workflow.WithActivityOptions(ctx, workflow.ActivityOptions{StartToCloseTimeout: time.Minute})
	var out string
	err := workflow.ExecuteActivity(ctx, Noop, id).Get(ctx, &out)
	return out, err
}

func main() {
	address := flag.String("address", "temporal-frontend:7233", "the frontend")
	namespace := flag.String("namespace", "default", "")
	rate := flag.Float64("rate", 20, "workflow starts per second")
	duration := flag.Duration("duration", 30*time.Minute, "how long to keep starting")
	iterations := flag.Int64("iterations", 0, "start exactly this many instead of -duration (the smoke)")
	drain := flag.Duration("drain", 10*time.Minute, "how long to wait for the last ones to close")
	startRetryFor := flag.Duration("start-retry-for", 5*time.Minute, "how long one start is retried")
	runID := flag.String("run-id", fmt.Sprintf("r%d", time.Now().Unix()), "names the task queue and workflow IDs")
	out := flag.String("out", "soak.json", "the summary")
	flag.Parse()

	c, err := dialWithRetry(*address, *namespace, 5*time.Minute)
	if err != nil {
		log.Fatalf("soak: dial %s: %v", *address, err)
	}
	defer c.Close()

	queue := "soak-" + *runID
	w := worker.New(c, queue, worker.Options{})
	w.RegisterWorkflow(SoakWorkflow)
	w.RegisterActivityWithOptions(Noop, activity.RegisterOptions{Name: "Noop"})
	if err := w.Start(); err != nil {
		log.Fatalf("soak: worker: %v", err)
	}
	defer w.Stop()

	s := Summary{RunID: *runID, Rate: *rate}
	var started, retries, gaveUp, maxStartMs atomic.Int64
	ids := make(chan string, 1<<16)
	var failOnce sync.Once
	fail := func(msg string) { failOnce.Do(func() { s.FirstFailure = msg }) }

	// Start loop: a fixed rate, each start in its own goroutine so a slow
	// start (a restarting history service) does not slow the offered load.
	var starts sync.WaitGroup
	begin := time.Now()
	tick := time.NewTicker(time.Duration(float64(time.Second) / *rate))
	n := int64(0)
	for range tick.C {
		if *iterations > 0 && n >= *iterations {
			break
		}
		if *iterations == 0 && time.Since(begin) >= *duration {
			break
		}
		n++
		id := fmt.Sprintf("soak-%s-%08d", *runID, n)
		starts.Add(1)
		go func() {
			defer starts.Done()
			t0 := time.Now()
			deadline := t0.Add(*startRetryFor)
			for attempt := 0; ; attempt++ {
				ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
				_, err := c.ExecuteWorkflow(ctx, client.StartWorkflowOptions{
					ID:                       id,
					TaskQueue:                queue,
					WorkflowIDReusePolicy:    enumspb.WORKFLOW_ID_REUSE_POLICY_REJECT_DUPLICATE,
					WorkflowIDConflictPolicy: enumspb.WORKFLOW_ID_CONFLICT_POLICY_USE_EXISTING,
				}, SoakWorkflow, id)
				cancel()
				var already *serviceerror.WorkflowExecutionAlreadyStarted
				if err == nil || errors.As(err, &already) {
					// An earlier attempt that timed out on our side may have
					// started it: the fixed ID makes that one the run.
					ms := time.Since(t0).Milliseconds()
					for {
						cur := maxStartMs.Load()
						if ms <= cur || maxStartMs.CompareAndSwap(cur, ms) {
							break
						}
					}
					started.Add(1)
					ids <- id
					return
				}
				if time.Now().After(deadline) {
					gaveUp.Add(1)
					fail(fmt.Sprintf("start %s: gave up after %d attempts: %v", id, attempt+1, err))
					return
				}
				retries.Add(1)
				time.Sleep(time.Second)
			}
		}()
	}
	tick.Stop()
	starts.Wait()
	close(ids)
	s.DurationSeconds = time.Since(begin).Seconds()
	s.Started = started.Load()
	s.StartRetries = retries.Load()
	s.StartGaveUp = gaveUp.Load()
	s.MaxStartLatencyMs = maxStartMs.Load()
	if s.DurationSeconds > 0 {
		s.AchievedStartRate = float64(n) / s.DurationSeconds
	}

	// Drain: every started workflow must close completed with its result.
	drainCtx, cancel := context.WithTimeout(context.Background(), *drain)
	defer cancel()
	var completed, failed, notClosed, wrong atomic.Int64
	var maxCloseNs atomic.Int64
	sem := make(chan struct{}, 64)
	var waits sync.WaitGroup
	for id := range ids {
		sem <- struct{}{}
		waits.Add(1)
		go func(id string) {
			defer func() { <-sem; waits.Done() }()
			t0 := time.Now()
			var res string
			var err error
			for {
				err = c.GetWorkflow(drainCtx, id, "").Get(drainCtx, &res)
				if err == nil || drainCtx.Err() != nil || !transient(err) {
					break
				}
				time.Sleep(time.Second)
			}
			switch {
			case err == nil && res == "done:"+id:
				completed.Add(1)
			case err == nil:
				wrong.Add(1)
				fail(fmt.Sprintf("%s returned %q", id, res))
			case drainCtx.Err() != nil:
				notClosed.Add(1)
				fail(fmt.Sprintf("%s did not close in the drain window: %v", id, err))
			default:
				failed.Add(1)
				fail(fmt.Sprintf("%s failed: %v", id, err))
			}
			d := time.Since(t0).Nanoseconds()
			for {
				cur := maxCloseNs.Load()
				if d <= cur || maxCloseNs.CompareAndSwap(cur, d) {
					break
				}
			}
		}(id)
	}
	waits.Wait()
	s.Completed, s.Failed, s.NotClosed, s.WrongResult = completed.Load(), failed.Load(), notClosed.Load(), wrong.Load()
	s.MaxCompleteSeconds = float64(maxCloseNs.Load()) / 1e9

	runs.mu.Lock()
	for i := int64(1); i <= n; i++ {
		k := runs.runs[fmt.Sprintf("soak-%s-%08d", *runID, i)]
		s.ActivityRuns += int64(k)
		if k > 1 {
			s.ActivityRerun++
		}
	}
	runs.mu.Unlock()
	s.ActivityNeverRan = s.Completed - countRan(n, *runID)
	if s.ActivityNeverRan < 0 {
		s.ActivityNeverRan = 0
	}
	s.OK = s.Started == n && s.StartGaveUp == 0 && s.Completed == s.Started &&
		s.Failed == 0 && s.NotClosed == 0 && s.WrongResult == 0 && s.ActivityNeverRan == 0

	b, _ := json.MarshalIndent(s, "", "  ")
	if err := os.WriteFile(*out, append(b, '\n'), 0o644); err != nil {
		log.Fatalf("soak: write %s: %v", *out, err)
	}
	fmt.Println(string(b))
	if !s.OK {
		os.Exit(1)
	}
}

func countRan(n int64, runID string) int64 {
	runs.mu.Lock()
	defer runs.mu.Unlock()
	var k int64
	for i := int64(1); i <= n; i++ {
		if runs.runs[fmt.Sprintf("soak-%s-%08d", runID, i)] > 0 {
			k++
		}
	}
	return k
}

// transient: errors a client sees while a service restarts.
func transient(err error) bool {
	var unavailable *serviceerror.Unavailable
	var deadline *serviceerror.DeadlineExceeded
	var notFound *serviceerror.NotFound
	var resource *serviceerror.ResourceExhausted
	return errors.As(err, &unavailable) || errors.As(err, &deadline) ||
		errors.As(err, &notFound) || errors.As(err, &resource) ||
		errors.Is(err, context.DeadlineExceeded)
}

func dialWithRetry(address, namespace string, d time.Duration) (client.Client, error) {
	deadline := time.Now().Add(d)
	for {
		c, err := client.Dial(client.Options{HostPort: address, Namespace: namespace})
		if err == nil {
			return c, nil
		}
		if time.Now().After(deadline) {
			return nil, err
		}
		time.Sleep(2 * time.Second)
	}
}
