// Command app is the Loams cases' app for DBOS Transact Go (DX0 Task 5),
// with the same commands as ../py/app.py: setup, start <wf> (CRASH_AT kills
// it when that step starts), resume <wf>, count <wf>, enqueue <prefix> <n>,
// serve <s>, queued <prefix>, set-event <wf> <v> [s], get-event <wf> <s>.
// Environment: DBOS_URL, DBOS_EXECUTOR. Go has no polling switch (R0.11).
package main

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"strconv"
	"syscall"
	"time"

	"github.com/dbos-inc/dbos-transact-golang/dbos"
	"github.com/jackc/pgx/v5"
)

var (
	url     = os.Getenv("DBOS_URL")
	crashAt = atoi(os.Getenv("CRASH_AT"))
)

func atoi(s string) int { n, _ := strconv.Atoi(s); return n }

func exec(q string, args ...any) error {
	c, err := pgx.Connect(context.Background(), url)
	if err != nil {
		return err
	}
	defer c.Close(context.Background())
	_, err = c.Exec(context.Background(), q, args...)
	return err
}

func step(ctx dbos.Context, wf string, n int) error {
	_, err := dbos.RunAsStep(ctx, func(context.Context) (int, error) {
		if n == crashAt {
			_ = syscall.Kill(os.Getpid(), syscall.SIGKILL)
		}
		return n, exec("INSERT INTO loams_steps (wf, step) VALUES ($1, $2)", wf, n)
	}, dbos.WithStepName(fmt.Sprintf("step%d", n)))
	return err
}

func five(ctx dbos.Context, wf string) (string, error) {
	for n := 1; n <= 5; n++ {
		if err := step(ctx, wf, n); err != nil {
			return "", err
		}
	}
	return "done", nil
}

func one(ctx dbos.Context, wf string) (string, error) {
	return wf, step(ctx, wf, 1)
}

func setter(ctx dbos.Context, v string) (string, error) {
	if err := dbos.SetEvent(ctx, "k", v); err != nil {
		return "", err
	}
	_, err := dbos.Recv[string](ctx, "done", time.Minute)
	return v, err
}

func out(v any) { b, _ := json.Marshal(v); fmt.Println(string(b)) }

func must(err error) {
	if err != nil {
		fmt.Fprintln(os.Stderr, "app:", err)
		os.Exit(1)
	}
}

func main() {
	args := os.Args[1:]
	bg := context.Background()
	switch args[0] {
	case "setup":
		must(exec("CREATE TABLE IF NOT EXISTS loams_steps (wf text NOT NULL, step int NOT NULL)"))
		return
	case "count", "queued":
		c, err := pgx.Connect(bg, url)
		must(err)
		defer c.Close(bg)
		if args[0] == "count" {
			rows, err := c.Query(bg, "SELECT step, count(*)::int FROM loams_steps WHERE wf = $1 GROUP BY step", args[1])
			must(err)
			m := map[string]int{}
			for rows.Next() {
				var s, n int
				must(rows.Scan(&s, &n))
				m[strconv.Itoa(s)] = n
			}
			out(m)
		} else {
			var wfs, maxRuns int
			must(c.QueryRow(bg, "SELECT count(*)::int, coalesce(max(n), 0)::int FROM (SELECT count(*) AS n FROM loams_steps WHERE wf LIKE $1 GROUP BY wf) t", args[1]+"%").Scan(&wfs, &maxRuns))
			out(map[string]int{"workflows": wfs, "max_runs": maxRuns})
		}
		return
	}
	executor := os.Getenv("DBOS_EXECUTOR")
	if executor == "" {
		executor = "local"
	}
	ctx, err := dbos.NewContext(bg, dbos.Config{DatabaseURL: url, AppName: "loams-crash-go", ExecutorID: executor})
	must(err)
	dbos.RegisterWorkflow(ctx, five, dbos.WithWorkflowName("five"))
	dbos.RegisterWorkflow(ctx, one, dbos.WithWorkflowName("one"))
	dbos.RegisterWorkflow(ctx, setter, dbos.WithWorkflowName("setter"))
	queue, err := dbos.RegisterQueue(ctx, "loams", dbos.WithGlobalConcurrency(10))
	must(err)
	must(dbos.Launch(ctx))
	defer dbos.Shutdown(ctx, 5*time.Second)
	switch args[0] {
	case "start":
		h, err := dbos.RunWorkflow(ctx, five, args[1], dbos.WithWorkflowID(args[1]))
		must(err)
		r, err := h.GetResult()
		must(err)
		fmt.Println(r)
	case "resume":
		h, err := dbos.RetrieveWorkflow[string](ctx, args[1])
		must(err)
		r, err := h.GetResult()
		must(err)
		fmt.Println(r)
	case "enqueue":
		for i := 0; i < atoi(args[2]); i++ {
			wf := fmt.Sprintf("%s-%05d", args[1], i)
			_, err := dbos.RunWorkflow(ctx, one, wf, dbos.WithWorkflowID(wf), dbos.WithQueue(queue))
			must(err)
		}
		fmt.Println("enqueued")
	case "serve":
		time.Sleep(time.Duration(atoi(args[1])) * time.Second)
	case "set-event":
		h, err := dbos.RunWorkflow(ctx, setter, args[2], dbos.WithWorkflowID(args[1]))
		must(err)
		out(map[string]float64{"set_at": float64(time.Now().UnixNano()) / 1e9})
		wait := 5
		if len(args) > 3 {
			wait = atoi(args[3])
		}
		time.Sleep(time.Duration(wait) * time.Second)
		must(dbos.Send(ctx, args[1], "ok", "done"))
		_, err = h.GetResult()
		must(err)
	case "get-event":
		v, err := dbos.GetEvent[string](ctx, args[1], "k", time.Duration(atoi(args[2]))*time.Second)
		must(err)
		out(map[string]any{"value": v, "got_at": float64(time.Now().UnixNano()) / 1e9})
	default:
		must(fmt.Errorf("unknown command %s", args[0]))
	}
}
