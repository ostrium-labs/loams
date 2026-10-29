// bq1-durable loadgen: raw-protocol load for the Loams Temporal frontend
// (gRPC WorkflowService) and for Resonate (HTTP protocol envelopes).
//
// Modes:
//
//	start     StartWorkflowExecution, unique workflow IDs, closed loop
//	signal    SignalWorkflowExecution against -workflows pre-started runs
//	activity  closed loop of -workflows runs, each scheduling -k no-op
//	          activities; this process is also the worker (raw polls)
//	timer     closed loop of -workflows runs, each one durable timer of
//	          -timer-ms, then complete; counts TimerFired events seen
//	rcreate   Resonate promise.create then promise.settle per iteration
//
// One JSON object goes to stdout at the end; progress goes to stderr.
package main

import (
	"bytes"
	"context"
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"net/http"
	"os"
	"sort"
	"sync"
	"sync/atomic"
	"time"

	commandpb "go.temporal.io/api/command/v1"
	commonpb "go.temporal.io/api/common/v1"
	enumspb "go.temporal.io/api/enums/v1"
	historypb "go.temporal.io/api/history/v1"
	taskqueuepb "go.temporal.io/api/taskqueue/v1"
	"go.temporal.io/api/workflowservice/v1"
	"google.golang.org/grpc"
	"google.golang.org/grpc/credentials/insecure"
	"google.golang.org/protobuf/types/known/durationpb"
)

type cfg struct {
	mode        string
	addr        string
	resonateURL string
	ns          string
	tq          string
	conc        int
	conns       int
	dur         time.Duration
	warmup      time.Duration
	workflows   int
	k           int
	timerMs     int
	wfPollers   int
	actPollers  int
	label       string
	runID       string
	h2c         bool
	httpConns   int
}

// lat collects latencies (ns) per goroutine and merges them at the end.
type lat struct {
	mu  sync.Mutex
	all []int64
}

func (l *lat) add(xs []int64) {
	l.mu.Lock()
	l.all = append(l.all, xs...)
	l.mu.Unlock()
}

func (l *lat) pct(p float64) float64 {
	if len(l.all) == 0 {
		return 0
	}
	i := int(p * float64(len(l.all)-1))
	return float64(l.all[i]) / 1e6
}

type result struct {
	Label        string             `json:"label"`
	Mode         string             `json:"mode"`
	Target       string             `json:"target"`
	DurationS    float64            `json:"duration_s"`
	Concurrency  int                `json:"concurrency"`
	Ops          int64              `json:"ops"`
	OpsPerS      float64            `json:"ops_per_s"`
	Errors       int64              `json:"errors"`
	P50Ms        float64            `json:"p50_ms"`
	P99Ms        float64            `json:"p99_ms"`
	P999Ms       float64            `json:"p999_ms"`
	Counters     map[string]int64   `json:"counters,omitempty"`
	Rates        map[string]float64 `json:"rates_per_s,omitempty"`
	Params       map[string]any     `json:"params"`
	FirstError   string             `json:"first_error,omitempty"`
	StartedAtUTC string             `json:"started_at_utc"`
}

var firstErr atomic.Value

func noteErr(err error) {
	if err != nil {
		firstErr.CompareAndSwap(nil, err.Error())
	}
}

func main() {
	var c cfg
	flag.StringVar(&c.mode, "mode", "start", "start|signal|activity|timer|rcreate")
	flag.StringVar(&c.addr, "addr", "127.0.0.1:7233", "Temporal frontend address")
	flag.StringVar(&c.resonateURL, "resonate", "http://127.0.0.1:18001/", "Resonate protocol URL")
	flag.StringVar(&c.ns, "namespace", "default", "Temporal namespace")
	flag.StringVar(&c.tq, "task-queue", "bq1", "task queue")
	flag.IntVar(&c.conc, "c", 64, "concurrent callers (start, signal, rcreate)")
	flag.IntVar(&c.conns, "conns", 8, "gRPC connections")
	flag.DurationVar(&c.dur, "d", 30*time.Second, "measured duration")
	flag.DurationVar(&c.warmup, "warmup", 5*time.Second, "warm-up before measuring")
	flag.IntVar(&c.workflows, "workflows", 500, "workflows in flight (signal targets; activity/timer closed loop)")
	flag.IntVar(&c.k, "k", 4, "activities per workflow (activity)")
	flag.IntVar(&c.timerMs, "timer-ms", 100, "timer duration (timer)")
	flag.IntVar(&c.wfPollers, "wf-pollers", 32, "workflow task pollers")
	flag.IntVar(&c.actPollers, "act-pollers", 32, "activity task pollers")
	flag.StringVar(&c.label, "label", "", "free-form label stored in the result")
	flag.BoolVar(&c.h2c, "h2c", false, "rcreate: HTTP/2 without TLS (many streams per connection) instead of HTTP/1.1")
	flag.IntVar(&c.httpConns, "http-conns", 0, "rcreate: most connections to the server (0: one per caller)")
	flag.StringVar(&c.runID, "run", fmt.Sprintf("r%d", time.Now().Unix()), "run ID (workflow ID prefix)")
	flag.Parse()

	var r result
	switch c.mode {
	case "start":
		r = runTemporal(c, startLoop)
	case "signal":
		r = runTemporal(c, signalLoop)
	case "activity", "timer":
		r = runWorkerLoop(c)
	case "rcreate":
		r = runResonate(c)
	default:
		fmt.Fprintf(os.Stderr, "unknown mode %q\n", c.mode)
		os.Exit(2)
	}
	if v := firstErr.Load(); v != nil {
		r.FirstError = v.(string)
	}
	enc := json.NewEncoder(os.Stdout)
	enc.SetIndent("", "  ")
	_ = enc.Encode(r)
}

func dial(c cfg) []workflowservice.WorkflowServiceClient {
	out := make([]workflowservice.WorkflowServiceClient, c.conns)
	for i := range out {
		conn, err := grpc.NewClient(c.addr, grpc.WithTransportCredentials(insecure.NewCredentials()))
		if err != nil {
			fmt.Fprintln(os.Stderr, "dial:", err)
			os.Exit(2)
		}
		out[i] = workflowservice.NewWorkflowServiceClient(conn)
	}
	return out
}

func startReq(c cfg, id string) *workflowservice.StartWorkflowExecutionRequest {
	return &workflowservice.StartWorkflowExecutionRequest{
		Namespace:           c.ns,
		WorkflowId:          id,
		WorkflowType:        &commonpb.WorkflowType{Name: "bq1"},
		TaskQueue:           &taskqueuepb.TaskQueue{Name: c.tq, Kind: enumspb.TASK_QUEUE_KIND_NORMAL},
		RequestId:           id,
		WorkflowTaskTimeout: durationpb.New(10 * time.Second),
		Identity:            "bq1-loadgen",
	}
}

type opFn func(ctx context.Context, cl workflowservice.WorkflowServiceClient, worker, iter int) error

// window tracks whether we are in the measured window.
type window struct {
	start, end time.Time
}

func (w window) in(t time.Time) bool { return !t.Before(w.start) && t.Before(w.end) }

func runTemporal(c cfg, mk func(c cfg, clients []workflowservice.WorkflowServiceClient) opFn) result {
	clients := dial(c)
	op := mk(c, clients)
	t0 := time.Now()
	w := window{start: t0.Add(c.warmup), end: t0.Add(c.warmup + c.dur)}
	var ops, errs atomic.Int64
	var L lat
	var wg sync.WaitGroup
	ctx := context.Background()
	for g := 0; g < c.conc; g++ {
		wg.Add(1)
		go func(g int) {
			defer wg.Done()
			cl := clients[g%len(clients)]
			var mine []int64
			for i := 0; ; i++ {
				s := time.Now()
				if !s.Before(w.end) {
					break
				}
				err := op(ctx, cl, g, i)
				e := time.Now()
				if w.in(s) {
					if err != nil {
						errs.Add(1)
						noteErr(err)
					} else {
						ops.Add(1)
						mine = append(mine, e.Sub(s).Nanoseconds())
					}
				}
			}
			L.add(mine)
		}(g)
	}
	wg.Wait()
	return finish(c, "temporal:"+c.addr, ops.Load(), errs.Load(), &L, nil, nil)
}

func finish(c cfg, target string, ops, errs int64, L *lat, counters map[string]int64, rates map[string]float64) result {
	sort.Slice(L.all, func(i, j int) bool { return L.all[i] < L.all[j] })
	return result{
		Label:       c.label,
		Mode:        c.mode,
		Target:      target,
		DurationS:   c.dur.Seconds(),
		Concurrency: c.conc,
		Ops:         ops,
		OpsPerS:     float64(ops) / c.dur.Seconds(),
		Errors:      errs,
		P50Ms:       L.pct(0.50),
		P99Ms:       L.pct(0.99),
		P999Ms:      L.pct(0.999),
		Counters:    counters,
		Rates:       rates,
		Params: map[string]any{
			"conns": c.conns, "warmup_s": c.warmup.Seconds(), "workflows": c.workflows,
			"k": c.k, "timer_ms": c.timerMs, "wf_pollers": c.wfPollers, "act_pollers": c.actPollers,
			"namespace": c.ns, "task_queue": c.tq, "h2c": c.h2c, "http_conns": c.httpConns,
			"streams_per_conn": float64(c.conc) / float64(max(1, c.conns)),
		},
		StartedAtUTC: time.Now().UTC().Add(-c.dur - c.warmup).Format(time.RFC3339),
	}
}

func startLoop(c cfg, _ []workflowservice.WorkflowServiceClient) opFn {
	return func(ctx context.Context, cl workflowservice.WorkflowServiceClient, g, i int) error {
		_, err := cl.StartWorkflowExecution(ctx, startReq(c, fmt.Sprintf("%s-s-%d-%d", c.runID, g, i)))
		return err
	}
}

func signalLoop(c cfg, clients []workflowservice.WorkflowServiceClient) opFn {
	ids := make([]string, c.workflows)
	ctx := context.Background()
	for i := range ids {
		ids[i] = fmt.Sprintf("%s-sig-%d", c.runID, i)
		if _, err := clients[i%len(clients)].StartWorkflowExecution(ctx, startReq(c, ids[i])); err != nil {
			fmt.Fprintln(os.Stderr, "pre-start:", err)
			os.Exit(2)
		}
	}
	fmt.Fprintf(os.Stderr, "signal: %d targets started\n", len(ids))
	return func(ctx context.Context, cl workflowservice.WorkflowServiceClient, g, i int) error {
		_, err := cl.SignalWorkflowExecution(ctx, &workflowservice.SignalWorkflowExecutionRequest{
			Namespace:         c.ns,
			WorkflowExecution: &commonpb.WorkflowExecution{WorkflowId: ids[(g*7919+i)%len(ids)]},
			SignalName:        "bq1",
			Identity:          "bq1-loadgen",
			RequestId:         fmt.Sprintf("%s-sg-%d-%d", c.runID, g, i),
		})
		return err
	}
}

// ---- activity / timer: this process is the worker ----

type counters struct {
	wfTasks, actDone, timersFired, wfDone, starts, errs atomic.Int64
	// measured-window counts
	mActDone, mTimers, mWfDone, mWfTasks atomic.Int64
}

func runWorkerLoop(c cfg) result {
	clients := dial(c)
	ctx := context.Background()
	t0 := time.Now()
	w := window{start: t0.Add(c.warmup), end: t0.Add(c.warmup + c.dur)}
	stop := make(chan struct{})
	var cnt counters
	var seq atomic.Int64
	var wfLat lat // workflow start-to-completion latency
	var startedAt sync.Map

	start := func() {
		n := seq.Add(1)
		id := fmt.Sprintf("%s-%s-%d", c.runID, c.mode, n)
		startedAt.Store(id, time.Now())
		if _, err := clients[int(n)%len(clients)].StartWorkflowExecution(ctx, startReq(c, id)); err != nil {
			cnt.errs.Add(1)
			noteErr(err)
			return
		}
		cnt.starts.Add(1)
	}
	for i := 0; i < c.workflows; i++ {
		start()
	}

	var wg sync.WaitGroup
	for p := 0; p < c.wfPollers; p++ {
		wg.Add(1)
		go func(p int) {
			defer wg.Done()
			cl := clients[p%len(clients)]
			var mine []int64
			defer func() { wfLat.add(mine) }()
			for {
				select {
				case <-stop:
					return
				default:
				}
				pctx, cancel := context.WithTimeout(ctx, 5*time.Second)
				t, err := cl.PollWorkflowTaskQueue(pctx, &workflowservice.PollWorkflowTaskQueueRequest{
					Namespace: c.ns,
					TaskQueue: &taskqueuepb.TaskQueue{Name: c.tq, Kind: enumspb.TASK_QUEUE_KIND_NORMAL},
					Identity:  "bq1-worker",
				})
				cancel()
				if err != nil || len(t.GetTaskToken()) == 0 {
					continue
				}
				now := time.Now()
				cmds, done, fired := decide(c, t.GetHistory().GetEvents())
				_, err = cl.RespondWorkflowTaskCompleted(ctx, &workflowservice.RespondWorkflowTaskCompletedRequest{
					TaskToken: t.TaskToken,
					Commands:  cmds,
					Identity:  "bq1-worker",
					Namespace: c.ns,
				})
				if err != nil {
					cnt.errs.Add(1)
					noteErr(err)
					continue
				}
				cnt.wfTasks.Add(1)
				in := w.in(now)
				if in {
					cnt.mWfTasks.Add(1)
				}
				if fired {
					cnt.timersFired.Add(1)
					if in {
						cnt.mTimers.Add(1)
					}
				}
				if done {
					cnt.wfDone.Add(1)
					id := t.GetWorkflowExecution().GetWorkflowId()
					if v, ok := startedAt.LoadAndDelete(id); ok && in {
						mine = append(mine, now.Sub(v.(time.Time)).Nanoseconds())
					}
					if in {
						cnt.mWfDone.Add(1)
					}
					if time.Now().Before(w.end) {
						go start()
					}
				}
			}
		}(p)
	}
	if c.mode == "activity" {
		for p := 0; p < c.actPollers; p++ {
			wg.Add(1)
			go func(p int) {
				defer wg.Done()
				cl := clients[(p+3)%len(clients)]
				for {
					select {
					case <-stop:
						return
					default:
					}
					pctx, cancel := context.WithTimeout(ctx, 5*time.Second)
					t, err := cl.PollActivityTaskQueue(pctx, &workflowservice.PollActivityTaskQueueRequest{
						Namespace: c.ns,
						TaskQueue: &taskqueuepb.TaskQueue{Name: c.tq, Kind: enumspb.TASK_QUEUE_KIND_NORMAL},
						Identity:  "bq1-worker",
					})
					cancel()
					if err != nil || len(t.GetTaskToken()) == 0 {
						continue
					}
					_, err = cl.RespondActivityTaskCompleted(ctx, &workflowservice.RespondActivityTaskCompletedRequest{
						TaskToken: t.TaskToken,
						Identity:  "bq1-worker",
						Namespace: c.ns,
					})
					if err != nil {
						cnt.errs.Add(1)
						noteErr(err)
						continue
					}
					cnt.actDone.Add(1)
					if w.in(time.Now()) {
						cnt.mActDone.Add(1)
					}
				}
			}(p)
		}
	}
	// Progress.
	go func() {
		tk := time.NewTicker(5 * time.Second)
		defer tk.Stop()
		for {
			select {
			case <-stop:
				return
			case <-tk.C:
				fmt.Fprintf(os.Stderr, "progress: starts=%d wf_tasks=%d act_done=%d timers=%d wf_done=%d errs=%d\n",
					cnt.starts.Load(), cnt.wfTasks.Load(), cnt.actDone.Load(), cnt.timersFired.Load(), cnt.wfDone.Load(), cnt.errs.Load())
			}
		}
	}()
	time.Sleep(time.Until(w.end))
	close(stop)
	// Pollers return after their current long poll (at most 5 s).
	wg.Wait()

	secs := c.dur.Seconds()
	var ops int64
	if c.mode == "activity" {
		ops = cnt.mActDone.Load()
	} else {
		ops = cnt.mTimers.Load()
	}
	return finish(c, "temporal:"+c.addr, ops, cnt.errs.Load(), &wfLat,
		map[string]int64{
			"starts": cnt.starts.Load(), "workflow_tasks": cnt.wfTasks.Load(),
			"activities_completed": cnt.actDone.Load(), "timers_fired": cnt.timersFired.Load(),
			"workflows_completed": cnt.wfDone.Load(),
		},
		map[string]float64{
			"activities_completed": float64(cnt.mActDone.Load()) / secs,
			"timers_fired":         float64(cnt.mTimers.Load()) / secs,
			"workflows_completed":  float64(cnt.mWfDone.Load()) / secs,
			"workflow_tasks":       float64(cnt.mWfTasks.Load()) / secs,
		})
}

// decide answers a workflow task from the run's full history: schedule the
// activities (or the timer) on the first task, complete the run once they
// are all done. It reports whether the run completes and whether this task
// saw its timer fire.
func decide(c cfg, events []*historypb.HistoryEvent) ([]*commandpb.Command, bool, bool) {
	var scheduled, completed, timerStarted, timerFired int
	for _, e := range events {
		switch e.GetEventType() {
		case enumspb.EVENT_TYPE_ACTIVITY_TASK_SCHEDULED:
			scheduled++
		case enumspb.EVENT_TYPE_ACTIVITY_TASK_COMPLETED:
			completed++
		case enumspb.EVENT_TYPE_TIMER_STARTED:
			timerStarted++
		case enumspb.EVENT_TYPE_TIMER_FIRED:
			timerFired++
		}
	}
	complete := []*commandpb.Command{{
		CommandType: enumspb.COMMAND_TYPE_COMPLETE_WORKFLOW_EXECUTION,
		Attributes: &commandpb.Command_CompleteWorkflowExecutionCommandAttributes{
			CompleteWorkflowExecutionCommandAttributes: &commandpb.CompleteWorkflowExecutionCommandAttributes{},
		},
	}}
	if c.mode == "timer" {
		if timerStarted == 0 {
			return []*commandpb.Command{{
				CommandType: enumspb.COMMAND_TYPE_START_TIMER,
				Attributes: &commandpb.Command_StartTimerCommandAttributes{
					StartTimerCommandAttributes: &commandpb.StartTimerCommandAttributes{
						TimerId:            "t",
						StartToFireTimeout: durationpb.New(time.Duration(c.timerMs) * time.Millisecond),
					},
				},
			}}, false, false
		}
		if timerFired > 0 {
			return complete, true, true
		}
		return nil, false, false
	}
	if scheduled == 0 {
		cmds := make([]*commandpb.Command, c.k)
		for i := range cmds {
			cmds[i] = &commandpb.Command{
				CommandType: enumspb.COMMAND_TYPE_SCHEDULE_ACTIVITY_TASK,
				Attributes: &commandpb.Command_ScheduleActivityTaskCommandAttributes{
					ScheduleActivityTaskCommandAttributes: &commandpb.ScheduleActivityTaskCommandAttributes{
						ActivityId:          fmt.Sprintf("a%d", i),
						ActivityType:        &commonpb.ActivityType{Name: "noop"},
						TaskQueue:           &taskqueuepb.TaskQueue{Name: c.tq, Kind: enumspb.TASK_QUEUE_KIND_NORMAL},
						StartToCloseTimeout: durationpb.New(30 * time.Second),
					},
				},
			}
		}
		return cmds, false, false
	}
	if completed >= scheduled {
		return complete, true, false
	}
	return nil, false, false
}

// ---- Resonate ----

func runResonate(c cfg) result {
	tr := &http.Transport{MaxIdleConns: c.conc * 2, MaxIdleConnsPerHost: c.conc * 2, IdleConnTimeout: 90 * time.Second}
	if c.httpConns > 0 {
		tr.MaxConnsPerHost = c.httpConns
	}
	if c.h2c {
		// Prior-knowledge HTTP/2 over plain TCP: callers share connections
		// as concurrent streams (the pipelined leg).
		tr.Protocols = new(http.Protocols)
		tr.Protocols.SetUnencryptedHTTP2(true)
	}
	hc := &http.Client{Transport: tr, Timeout: 30 * time.Second}
	t0 := time.Now()
	w := window{start: t0.Add(c.warmup), end: t0.Add(c.warmup + c.dur)}
	var creates, settles, errs atomic.Int64
	var Lc, Ls lat
	var wg sync.WaitGroup
	post := func(body []byte) error {
		resp, err := hc.Post(c.resonateURL, "application/json", bytes.NewReader(body))
		if err != nil {
			return err
		}
		b, _ := io.ReadAll(resp.Body)
		resp.Body.Close()
		if resp.StatusCode >= 300 {
			return fmt.Errorf("HTTP %d: %.200s", resp.StatusCode, b)
		}
		var env struct {
			Head struct {
				Status int `json:"status"`
			} `json:"head"`
		}
		if json.Unmarshal(b, &env) == nil && env.Head.Status >= 300 {
			return fmt.Errorf("status %d: %.200s", env.Head.Status, b)
		}
		return nil
	}
	for g := 0; g < c.conc; g++ {
		wg.Add(1)
		go func(g int) {
			defer wg.Done()
			var lc, ls []int64
			for i := 0; ; i++ {
				s := time.Now()
				if !s.Before(w.end) {
					break
				}
				id := fmt.Sprintf("%s.p.%d.%d", c.runID, g, i)
				corr := fmt.Sprintf("%x", s.UnixNano())
				create, _ := json.Marshal(map[string]any{
					"kind": "promise.create",
					"head": map[string]any{"corrId": corr, "version": "2026-04-01"},
					"data": map[string]any{
						"id": id, "timeoutAt": s.Add(time.Hour).UnixMilli(),
						"param": map[string]any{"headers": map[string]any{}, "data": ""},
						"tags":  map[string]any{},
					},
				})
				err := post(create)
				m := time.Now()
				in := w.in(s)
				if err != nil {
					if in {
						errs.Add(1)
						noteErr(err)
					}
					continue
				}
				if in {
					creates.Add(1)
					lc = append(lc, m.Sub(s).Nanoseconds())
				}
				settle, _ := json.Marshal(map[string]any{
					"kind": "promise.settle",
					"head": map[string]any{"corrId": corr + "s", "version": "2026-04-01"},
					"data": map[string]any{
						"id": id, "state": "resolved",
						"value": map[string]any{"headers": map[string]any{}, "data": ""},
					},
				})
				err = post(settle)
				e := time.Now()
				if in {
					if err != nil {
						errs.Add(1)
						noteErr(err)
					} else {
						settles.Add(1)
						ls = append(ls, e.Sub(m).Nanoseconds())
					}
				}
			}
			Lc.add(lc)
			Ls.add(ls)
		}(g)
	}
	wg.Wait()
	sort.Slice(Ls.all, func(i, j int) bool { return Ls.all[i] < Ls.all[j] })
	secs := c.dur.Seconds()
	r := finish(c, "resonate:"+c.resonateURL, creates.Load()+settles.Load(), errs.Load(), &Lc,
		map[string]int64{"creates": creates.Load(), "settles": settles.Load()},
		map[string]float64{
			"creates": float64(creates.Load()) / secs, "settles": float64(settles.Load()) / secs,
			"settle_p50_ms": Ls.pct(0.5), "settle_p99_ms": Ls.pct(0.99), "settle_p999_ms": Ls.pct(0.999),
		})
	return r
}
