// Command loadgen drives the BQ1-stream benchmark (docs/benchmarks/2026-10-28-1b-qps/stream.md).
//
//	loadgen produce  -brokers 127.0.0.1:9093 -topic events -partitions 4 -events 2000000
//	loadgen pg       -dsn "host=127.0.0.1 port=4566 ..." -conns 8 -duration 30s [-reconnect]
//	loadgen topic    -brokers 127.0.0.1:9093 -topic events -partitions 4 [-delete]
//
// `produce` writes JSON events {"user_id","amount","ts"} (ts RFC 3339, UTC) through the Kafka
// protocol (the Iggy fork's Kafka gateway) as fast as acks allow and reports
// events/s and per-batch ack latency percentiles. `pg` runs a query in a loop
// on N connections (optionally a new connection per query, to measure the
// gateway's TLS + auth + upstream login path) and reports QPS and latency
// percentiles. Results are one JSON object on stdout.
package main

import (
	"context"
	"encoding/json"
	"flag"
	"fmt"
	"math/rand/v2"
	"os"
	"sort"
	"strconv"
	"sync"
	"sync/atomic"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/twmb/franz-go/pkg/kadm"
	"github.com/twmb/franz-go/pkg/kgo"
)

func main() {
	if len(os.Args) < 2 {
		fmt.Fprintln(os.Stderr, "usage: loadgen produce|pg|topic [flags]")
		os.Exit(2)
	}
	var err error
	switch os.Args[1] {
	case "produce":
		err = produce(os.Args[2:])
	case "pg":
		err = pgLoad(os.Args[2:])
	case "topic":
		err = topic(os.Args[2:])
	default:
		err = fmt.Errorf("unknown command %q", os.Args[1])
	}
	if err != nil {
		fmt.Fprintln(os.Stderr, "loadgen:", err)
		os.Exit(1)
	}
}

// percentiles of a sorted slice, in milliseconds.
func percentiles(d []time.Duration) map[string]float64 {
	out := map[string]float64{}
	if len(d) == 0 {
		return out
	}
	sort.Slice(d, func(i, j int) bool { return d[i] < d[j] })
	at := func(q float64) float64 {
		i := int(q * float64(len(d)-1))
		return float64(d[i].Microseconds()) / 1000.0
	}
	out["p50_ms"] = at(0.50)
	out["p99_ms"] = at(0.99)
	out["p999_ms"] = at(0.999)
	out["max_ms"] = float64(d[len(d)-1].Microseconds()) / 1000.0
	return out
}

func emit(v any) error {
	enc := json.NewEncoder(os.Stdout)
	return enc.Encode(v)
}

func topic(args []string) error {
	fs := flag.NewFlagSet("topic", flag.ExitOnError)
	brokers := fs.String("brokers", "127.0.0.1:9093", "Kafka bootstrap")
	name := fs.String("topic", "events", "topic")
	partitions := fs.Int("partitions", 1, "partitions")
	del := fs.Bool("delete", false, "delete instead of create")
	_ = fs.Parse(args)
	cl, err := kgo.NewClient(kgo.SeedBrokers(*brokers))
	if err != nil {
		return err
	}
	defer cl.Close()
	adm := kadm.NewClient(cl)
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	if *del {
		_, err = adm.DeleteTopics(ctx, *name)
		return err
	}
	resp, err := adm.CreateTopics(ctx, int32(*partitions), 1, nil, *name)
	if err != nil {
		return err
	}
	for _, r := range resp {
		if r.Err != nil {
			return fmt.Errorf("create %s: %w", r.Topic, r.Err)
		}
	}
	return emit(map[string]any{"topic": *name, "partitions": *partitions})
}

func produce(args []string) error {
	fs := flag.NewFlagSet("produce", flag.ExitOnError)
	brokers := fs.String("brokers", "127.0.0.1:9093", "Kafka bootstrap")
	name := fs.String("topic", "events", "topic")
	events := fs.Int("events", 1_000_000, "events to write")
	users := fs.Int("users", 10_000, "distinct user ids")
	lingerMs := fs.Int("linger-ms", 5, "producer linger")
	batchBytes := fs.Int("batch-bytes", 1<<20, "max batch bytes")
	inflight := fs.Int("inflight", 5, "max in-flight requests per broker")
	startMs := fs.Int64("start-ms", 0, "event-time of the first event (0 = now)")
	spanMs := fs.Int64("span-ms", 60_000, "event-time span the events cover")
	_ = fs.Parse(args)

	cl, err := kgo.NewClient(
		kgo.SeedBrokers(*brokers),
		kgo.DefaultProduceTopic(*name),
		kgo.ProducerLinger(time.Duration(*lingerMs)*time.Millisecond),
		kgo.ProducerBatchMaxBytes(int32(*batchBytes)),
		kgo.MaxProduceRequestsInflightPerBroker(*inflight),
		kgo.RequiredAcks(kgo.LeaderAck()),
		kgo.DisableIdempotentWrite(),
		kgo.ProducerBatchCompression(kgo.NoCompression()),
		kgo.RecordPartitioner(kgo.RoundRobinPartitioner()),
	)
	if err != nil {
		return err
	}
	defer cl.Close()

	start := *startMs
	if start == 0 {
		start = time.Now().UnixMilli()
	}
	var (
		mu        sync.Mutex
		latencies = make([]time.Duration, 0, *events/100+1)
		failed    atomic.Int64
		wg        sync.WaitGroup
	)
	rng := rand.New(rand.NewPCG(1, 2))
	buf := make([]byte, 0, 96)
	t0 := time.Now()
	for i := 0; i < *events; i++ {
		ts := start + int64(i)*(*spanMs)/int64(*events)
		buf = buf[:0]
		buf = append(buf, `{"user_id":`...)
		buf = strconv.AppendInt(buf, int64(rng.IntN(*users)), 10)
		buf = append(buf, `,"amount":`...)
		buf = strconv.AppendInt(buf, int64(rng.IntN(10_000)), 10)
		buf = append(buf, `,"ts":"`...)
		buf = time.UnixMilli(ts).UTC().AppendFormat(buf, "2006-01-02T15:04:05.000Z")
		buf = append(buf, `"}`...)
		rec := &kgo.Record{Value: append([]byte(nil), buf...)}
		sent := time.Now()
		sample := i%100 == 0
		wg.Add(1)
		cl.Produce(context.Background(), rec, func(_ *kgo.Record, err error) {
			defer wg.Done()
			if err != nil {
				failed.Add(1)
				return
			}
			if sample {
				d := time.Since(sent)
				mu.Lock()
				latencies = append(latencies, d)
				mu.Unlock()
			}
		})
	}
	wg.Wait()
	elapsed := time.Since(t0)
	out := map[string]any{
		"mode":         "produce",
		"topic":        *name,
		"events":       *events,
		"failed":       failed.Load(),
		"seconds":      elapsed.Seconds(),
		"events_per_s": float64(*events) / elapsed.Seconds(),
		"bytes_approx": *events * len(buf),
		"ack_latency":  percentiles(latencies),
		"linger_ms":    *lingerMs,
		"inflight":     *inflight,
	}
	return emit(out)
}

func pgLoad(args []string) error {
	fs := flag.NewFlagSet("pg", flag.ExitOnError)
	dsn := fs.String("dsn", "host=127.0.0.1 port=4566 user=root dbname=dev sslmode=disable", "libpq DSN")
	conns := fs.Int("conns", 4, "concurrent connections")
	duration := fs.Duration("duration", 20*time.Second, "how long")
	query := fs.String("query", "SELECT 1", "the query")
	reconnect := fs.Bool("reconnect", false, "a new connection per query (connection setup QPS)")
	label := fs.String("label", "", "label in the result")
	_ = fs.Parse(args)

	cfg, err := pgx.ParseConfig(*dsn)
	if err != nil {
		return err
	}
	// The simple protocol: RisingWave and the gateway path see one round trip per query.
	cfg.DefaultQueryExecMode = pgx.QueryExecModeSimpleProtocol
	ctx, cancel := context.WithTimeout(context.Background(), *duration+30*time.Second)
	defer cancel()
	deadline := time.Now().Add(*duration)

	var (
		mu    sync.Mutex
		lat   []time.Duration
		ok    atomic.Int64
		fails atomic.Int64
		first atomic.Value
		wg    sync.WaitGroup
	)
	worker := func() {
		defer wg.Done()
		local := make([]time.Duration, 0, 1<<16)
		var conn *pgx.Conn
		for time.Now().Before(deadline) {
			t := time.Now()
			if conn == nil {
				c, err := pgx.ConnectConfig(ctx, cfg)
				if err != nil {
					fails.Add(1)
					first.CompareAndSwap(nil, err.Error())
					time.Sleep(10 * time.Millisecond)
					continue
				}
				conn = c
			}
			var one int
			err := conn.QueryRow(ctx, *query).Scan(&one)
			if *reconnect {
				_ = conn.Close(ctx)
				conn = nil
			}
			if err != nil {
				fails.Add(1)
				first.CompareAndSwap(nil, err.Error())
				if conn != nil {
					_ = conn.Close(ctx)
					conn = nil
				}
				continue
			}
			ok.Add(1)
			local = append(local, time.Since(t))
		}
		if conn != nil {
			_ = conn.Close(ctx)
		}
		mu.Lock()
		lat = append(lat, local...)
		mu.Unlock()
	}
	t0 := time.Now()
	for i := 0; i < *conns; i++ {
		wg.Add(1)
		go worker()
	}
	wg.Wait()
	elapsed := time.Since(t0)
	out := map[string]any{
		"mode":      "pg",
		"label":     *label,
		"conns":     *conns,
		"reconnect": *reconnect,
		"query":     *query,
		"ok":        ok.Load(),
		"failed":    fails.Load(),
		"seconds":   elapsed.Seconds(),
		"qps":       float64(ok.Load()) / elapsed.Seconds(),
		"latency":   percentiles(lat),
	}
	if e := first.Load(); e != nil {
		out["first_error"] = e
	}
	return emit(out)
}
