// Command loadgen drives Loams SQL (TiDB with a keyspace on TiKV) for the
// BQ1-sql benchmark: point selects and a sysbench-like oltp_read_write mix,
// over the MySQL protocol with server-side prepared statements.
//
//	loadgen -mode prepare -addrs 127.0.0.1:44000 -tables 10 -rows 100000
//	loadgen -mode point -addrs 127.0.0.1:44000,127.0.0.1:44001 -threads 64 -duration 60s
//	loadgen -mode oltp  -addrs ... -threads 32 -duration 60s
//
// It scrapes Prometheus counters before and after the measured window
// (-metrics url|name-prefix,...) and reports their rates, so the PD/TSO
// request rate is measured beside the QPS. sysbench is not installed on the
// benchmark host; the oltp mix follows sysbench 1.0's oltp_read_write.lua
// (10 point selects, 4 ranges of 100 rows, 2 updates, a delete and an insert
// per transaction) on sysbench's sbtest schema.
package main

import (
	"bufio"
	"context"
	"database/sql"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"math/rand/v2"
	"net/http"
	"os"
	"sort"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	"github.com/go-sql-driver/mysql"
)

type config struct {
	Mode     string        `json:"mode"`
	Addrs    []string      `json:"addrs"`
	User     string        `json:"-"`
	Pass     string        `json:"-"`
	DB       string        `json:"db"`
	Tables   int           `json:"tables"`
	Rows     int           `json:"rows"`
	Threads  int           `json:"threads"`
	Depth    int           `json:"depth"`
	Duration time.Duration `json:"-"`
	Warmup   time.Duration `json:"-"`
	Metrics  []metricSpec  `json:"-"`
	Label    string        `json:"label"`
	Split    int           `json:"split_regions"`
}

type metricSpec struct{ URL, Prefix string }

type result struct {
	Label       string             `json:"label"`
	Mode        string             `json:"mode"`
	Threads     int                `json:"threads"`
	Addrs       []string           `json:"tidb_addrs"`
	Tables      int                `json:"tables"`
	Rows        int                `json:"rows_per_table"`
	DurationS   float64            `json:"duration_s"`
	Ops         int64              `json:"ops"`
	Depth       int                `json:"depth"`
	RequestsPS  float64            `json:"requests_per_s"`
	OpsPerS     float64            `json:"ops_per_s"`
	QueriesPerS float64            `json:"queries_per_s"`
	Errors      int64              `json:"errors"`
	FirstError  string             `json:"first_error,omitempty"`
	Retries     int64              `json:"retries"`
	LatencyUs   map[string]float64 `json:"latency_us"`
	Rates       map[string]float64 `json:"metric_rates_per_s"`
	Started     string             `json:"started"`
}

func main() {
	var c config
	var addrs, metrics string
	flag.StringVar(&c.Mode, "mode", "point", "prepare | point | oltp")
	flag.StringVar(&addrs, "addrs", "127.0.0.1:44000", "TiDB MySQL addresses, comma-separated (round robin per connection)")
	flag.StringVar(&c.User, "user", "root", "user")
	flag.StringVar(&c.Pass, "pass", "", "password")
	flag.StringVar(&c.DB, "db", "sbtest", "database")
	flag.IntVar(&c.Tables, "tables", 10, "sbtest tables")
	flag.IntVar(&c.Rows, "rows", 100000, "rows per table")
	flag.IntVar(&c.Threads, "threads", 64, "connections (one worker each)")
	flag.IntVar(&c.Depth, "depth", 1, "point: keys per request (1 = one point select; N = one batched point get, WHERE id IN (N keys))")
	flag.DurationVar(&c.Duration, "duration", 60*time.Second, "measured window")
	flag.DurationVar(&c.Warmup, "warmup", 10*time.Second, "warm-up before the window")
	flag.StringVar(&metrics, "metrics", "", "url|metric-prefix,... counters to rate over the window")
	flag.StringVar(&c.Label, "label", "", "free-form run label")
	flag.IntVar(&c.Split, "split", 0, "prepare: split each table into this many scattered regions before loading")
	out := flag.String("out", "", "write the result JSON here (stdout otherwise)")
	flag.Parse()
	c.Addrs = strings.Split(addrs, ",")
	for _, m := range strings.Split(metrics, ",") {
		if m == "" {
			continue
		}
		u, p, ok := strings.Cut(m, "|")
		if !ok {
			fail(fmt.Errorf("-metrics entry %q is not url|prefix", m))
		}
		c.Metrics = append(c.Metrics, metricSpec{u, p})
	}
	switch c.Mode {
	case "prepare":
		fail(prepare(c))
		return
	case "point", "oltp":
	default:
		fail(fmt.Errorf("unknown mode %q", c.Mode))
	}
	r, err := run(c)
	fail(err)
	b, _ := json.MarshalIndent(r, "", "  ")
	if *out != "" {
		fail(os.WriteFile(*out, append(b, '\n'), 0o644))
	}
	fmt.Println(string(b))
}

func fail(err error) {
	if err != nil {
		fmt.Fprintln(os.Stderr, "loadgen:", err)
		os.Exit(1)
	}
}

func open(c config, addr, db string) (*sql.DB, error) {
	mc := mysql.NewConfig()
	mc.User, mc.Passwd, mc.Net, mc.Addr, mc.DBName = c.User, c.Pass, "tcp", addr, db
	mc.InterpolateParams = false // server-side prepared statements
	mc.MultiStatements = false
	d, err := sql.Open("mysql", mc.FormatDSN())
	if err != nil {
		return nil, err
	}
	d.SetMaxOpenConns(c.Threads + 4)
	d.SetMaxIdleConns(c.Threads + 4)
	return d, nil
}

// prepare creates sysbench's sbtest schema and loads it.
func prepare(c config) error {
	d, err := open(c, c.Addrs[0], "")
	if err != nil {
		return err
	}
	defer d.Close()
	ctx := context.Background()
	if _, err := d.ExecContext(ctx, "CREATE DATABASE IF NOT EXISTS "+c.DB); err != nil {
		return err
	}
	var wg sync.WaitGroup
	errs := make(chan error, c.Tables)
	sem := make(chan struct{}, 4)
	for t := 1; t <= c.Tables; t++ {
		wg.Add(1)
		go func(t int) {
			defer wg.Done()
			sem <- struct{}{}
			defer func() { <-sem }()
			errs <- loadTable(ctx, d, c, t)
		}(t)
	}
	wg.Wait()
	close(errs)
	for e := range errs {
		if e != nil {
			return e
		}
	}
	return nil
}

func pad(r *rand.Rand, n int) string {
	var b strings.Builder
	for b.Len() < n {
		fmt.Fprintf(&b, "%011d-", r.Int64N(1e11))
	}
	return b.String()[:n]
}

func loadTable(ctx context.Context, d *sql.DB, c config, t int) error {
	name := fmt.Sprintf("%s.sbtest%d", c.DB, t)
	ddl := "CREATE TABLE IF NOT EXISTS " + name + ` (
  id INT NOT NULL,
  k INT NOT NULL DEFAULT 0,
  c CHAR(120) NOT NULL DEFAULT '',
  pad CHAR(60) NOT NULL DEFAULT '',
  PRIMARY KEY (id) CLUSTERED,
  KEY k_1 (k))`
	if _, err := d.ExecContext(ctx, ddl); err != nil {
		return err
	}
	if c.Split > 1 {
		// One connection: the scatter setting is per session.
		conn, err := d.Conn(ctx)
		if err != nil {
			return err
		}
		defer conn.Close()
		if _, err := conn.ExecContext(ctx, "SET SESSION tidb_scatter_region = 'table'"); err != nil {
			return err
		}
		q := fmt.Sprintf("SPLIT TABLE %s BETWEEN (1) AND (%d) REGIONS %d", name, c.Rows+1, c.Split)
		if _, err := conn.ExecContext(ctx, q); err != nil {
			return fmt.Errorf("%s: %w", q, err)
		}
	}
	r := rand.New(rand.NewPCG(uint64(t), 7))
	const batch = 1000
	for start := 1; start <= c.Rows; start += batch {
		var b strings.Builder
		b.WriteString("INSERT INTO " + name + " (id, k, c, pad) VALUES ")
		for id := start; id < start+batch && id <= c.Rows; id++ {
			if id > start {
				b.WriteByte(',')
			}
			fmt.Fprintf(&b, "(%d,%d,'%s','%s')", id, r.IntN(c.Rows)+1, pad(r, 119), pad(r, 59))
		}
		if _, err := d.ExecContext(ctx, b.String()); err != nil {
			return fmt.Errorf("%s: %w", name, err)
		}
	}
	return nil
}

// scrape sums every sample whose metric name starts with prefix.
func scrape(m metricSpec) (float64, error) {
	resp, err := http.Get(m.URL)
	if err != nil {
		return 0, err
	}
	defer resp.Body.Close()
	sum := 0.0
	sc := bufio.NewScanner(resp.Body)
	sc.Buffer(make([]byte, 1<<20), 1<<20)
	for sc.Scan() {
		line := sc.Text()
		if !strings.HasPrefix(line, m.Prefix) {
			continue
		}
		f := strings.Fields(line)
		if len(f) < 2 {
			continue
		}
		// The prefix may include a label set; the value is the second field.
		v, err := strconv.ParseFloat(f[len(f)-1], 64)
		if err == nil {
			sum += v
		}
	}
	return sum, sc.Err()
}

// stmts are one table's prepared statements on one connection.
type stmts struct {
	point, batch, rng, sum, order, distinct, updK, updC, del, ins *sql.Stmt
}

type worker struct {
	conn   *sql.Conn
	tables []stmts
	r      *rand.Rand
	lat    []uint32 // microseconds, one per op in the window
}

func prepareStmts(ctx context.Context, conn *sql.Conn, t int, oltp bool, depth int) (stmts, error) {
	tbl := fmt.Sprintf("sbtest%d", t)
	var s stmts
	qs := []struct {
		dst **sql.Stmt
		q   string
	}{{&s.point, "SELECT c FROM " + tbl + " WHERE id=?"}}
	if depth > 1 {
		// TiDB plans this as a BatchPointGet: one TiKV BatchGet per region.
		marks := strings.TrimSuffix(strings.Repeat("?,", depth), ",")
		qs = append(qs, struct {
			dst **sql.Stmt
			q   string
		}{&s.batch, "SELECT c FROM " + tbl + " WHERE id IN (" + marks + ")"})
	}
	if oltp {
		qs = append(qs, []struct {
			dst **sql.Stmt
			q   string
		}{
			{&s.rng, "SELECT c FROM " + tbl + " WHERE id BETWEEN ? AND ?"},
			{&s.sum, "SELECT SUM(k) FROM " + tbl + " WHERE id BETWEEN ? AND ?"},
			{&s.order, "SELECT c FROM " + tbl + " WHERE id BETWEEN ? AND ? ORDER BY c"},
			{&s.distinct, "SELECT DISTINCT c FROM " + tbl + " WHERE id BETWEEN ? AND ? ORDER BY c"},
			{&s.updK, "UPDATE " + tbl + " SET k=k+1 WHERE id=?"},
			{&s.updC, "UPDATE " + tbl + " SET c=? WHERE id=?"},
			{&s.del, "DELETE FROM " + tbl + " WHERE id=?"},
			{&s.ins, "INSERT INTO " + tbl + " (id, k, c, pad) VALUES (?,?,?,?)"},
		}...)
	}
	for _, x := range qs {
		st, err := conn.PrepareContext(ctx, x.q)
		if err != nil {
			return s, err
		}
		*x.dst = st
	}
	return s, nil
}

func run(c config) (*result, error) {
	ctx := context.Background()
	dbs := make([]*sql.DB, len(c.Addrs))
	for i, a := range c.Addrs {
		d, err := open(c, a, c.DB)
		if err != nil {
			return nil, err
		}
		defer d.Close()
		dbs[i] = d
	}
	workers := make([]*worker, c.Threads)
	for i := range workers {
		conn, err := dbs[i%len(dbs)].Conn(ctx)
		if err != nil {
			return nil, err
		}
		defer conn.Close()
		w := &worker{conn: conn, r: rand.New(rand.NewPCG(uint64(i), 99))}
		for t := 1; t <= c.Tables; t++ {
			s, err := prepareStmts(ctx, conn, t, c.Mode == "oltp", c.Depth)
			if err != nil {
				return nil, err
			}
			w.tables = append(w.tables, s)
		}
		workers[i] = w
	}
	var measuring atomic.Bool
	var stop atomic.Bool
	var ops, errs, retries atomic.Int64
	var firstErr atomic.Value
	var wg sync.WaitGroup
	queriesPerOp, keysPerOp := 1, 1
	if c.Mode == "point" && c.Depth > 1 {
		keysPerOp = c.Depth
	}
	if c.Mode == "oltp" {
		// 14 reads, 4 writes and BEGIN/COMMIT: sysbench's 20 queries.
		queriesPerOp = 20
	}
	for _, w := range workers {
		wg.Add(1)
		go func(w *worker) {
			defer wg.Done()
			for !stop.Load() {
				t0 := time.Now()
				var err error
				if c.Mode == "point" {
					err = pointSelect(ctx, c, w)
				} else {
					var retried int
					err, retried = oltpTxn(ctx, c, w)
					if measuring.Load() {
						retries.Add(int64(retried))
					}
				}
				if !measuring.Load() {
					continue
				}
				if err != nil {
					errs.Add(1)
					firstErr.CompareAndSwap(nil, err.Error())
					continue
				}
				ops.Add(1)
				w.lat = append(w.lat, uint32(min(time.Since(t0).Microseconds(), 1<<32-1)))
			}
		}(w)
	}
	time.Sleep(c.Warmup)
	before := make([]float64, len(c.Metrics))
	for i, m := range c.Metrics {
		v, err := scrape(m)
		if err != nil {
			return nil, err
		}
		before[i] = v
	}
	started := time.Now()
	measuring.Store(true)
	time.Sleep(c.Duration)
	measuring.Store(false)
	elapsed := time.Since(started).Seconds()
	rates := map[string]float64{}
	for i, m := range c.Metrics {
		v, err := scrape(m)
		if err != nil {
			return nil, err
		}
		key := m.Prefix + " @ " + m.URL
		rates[key] = (v - before[i]) / elapsed
	}
	stop.Store(true)
	wg.Wait()
	var all []uint32
	for _, w := range workers {
		all = append(all, w.lat...)
	}
	sort.Slice(all, func(i, j int) bool { return all[i] < all[j] })
	pct := func(q float64) float64 {
		if len(all) == 0 {
			return 0
		}
		return float64(all[min(len(all)-1, int(q*float64(len(all))))])
	}
	n := ops.Load()
	return &result{
		Label: c.Label, Mode: c.Mode, Threads: c.Threads, Addrs: c.Addrs,
		Tables: c.Tables, Rows: c.Rows, DurationS: elapsed, Ops: n,
		Depth: c.Depth, RequestsPS: float64(n) / elapsed,
		OpsPerS: float64(n*int64(keysPerOp)) / elapsed, QueriesPerS: float64(n*int64(queriesPerOp)) / elapsed,
		Errors: errs.Load(), Retries: retries.Load(), FirstError: str(firstErr.Load()),
		LatencyUs: map[string]float64{"p50": pct(0.5), "p99": pct(0.99), "p999": pct(0.999), "max": pct(1)},
		Rates:     rates, Started: started.UTC().Format(time.RFC3339),
	}, nil
}

func str(v any) string {
	s, _ := v.(string)
	return s
}

func (w *worker) id(c config) int { return w.r.IntN(c.Rows) + 1 }

func pointSelect(ctx context.Context, c config, w *worker) error {
	t := w.tables[w.r.IntN(c.Tables)]
	if c.Depth <= 1 {
		var s string
		return t.point.QueryRowContext(ctx, w.id(c)).Scan(&s)
	}
	args := make([]any, c.Depth)
	for i := range args {
		args[i] = w.id(c)
	}
	return drain(t.batch.QueryContext(ctx, args...))
}

// oltpTxn is sysbench's oltp_read_write: retried (up to 5 times) on write
// conflicts and deadlocks, as sysbench does on the errors it ignores.
func oltpTxn(ctx context.Context, c config, w *worker) (error, int) {
	for attempt := 0; ; attempt++ {
		err := oltpOnce(ctx, c, w)
		if err == nil || attempt == 4 || !retryable(err) {
			return err, attempt
		}
	}
}

func retryable(err error) bool {
	var me *mysql.MySQLError
	if errors.As(err, &me) {
		switch me.Number {
		case 1213, 1205, 9007, 8002, 8022, 8028:
			return true
		}
	}
	return false
}

func drain(rows *sql.Rows, err error) error {
	if err != nil {
		return err
	}
	for rows.Next() {
	}
	return rows.Close()
}

func oltpOnce(ctx context.Context, c config, w *worker) error {
	tx, err := w.conn.BeginTx(ctx, nil)
	if err != nil {
		return err
	}
	defer tx.Rollback() //nolint:errcheck
	st := w.tables[w.r.IntN(c.Tables)]
	in := func(s *sql.Stmt) *sql.Stmt { return tx.StmtContext(ctx, s) }
	var s string
	for i := 0; i < 10; i++ {
		if err := in(st.point).QueryRowContext(ctx, w.id(c)).Scan(&s); err != nil {
			return err
		}
	}
	for _, q := range []*sql.Stmt{st.rng, st.sum, st.order, st.distinct} {
		lo := w.r.IntN(c.Rows-100) + 1
		if err := drain(in(q).QueryContext(ctx, lo, lo+99)); err != nil {
			return err
		}
	}
	if _, err := in(st.updK).ExecContext(ctx, w.id(c)); err != nil {
		return err
	}
	if _, err := in(st.updC).ExecContext(ctx, pad(w.r, 119), w.id(c)); err != nil {
		return err
	}
	id := w.id(c)
	if _, err := in(st.del).ExecContext(ctx, id); err != nil {
		return err
	}
	if _, err := in(st.ins).ExecContext(ctx, id, w.r.IntN(c.Rows)+1, pad(w.r, 119), pad(w.r, 59)); err != nil {
		return err
	}
	return tx.Commit()
}
