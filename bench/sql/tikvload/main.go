// Command tikvload drives TiKV's native APIs for BQ1-sql: the raw KV API
// (Get / BatchGet) and the transactional API (a snapshot read per request,
// at a fresh PD timestamp, with Get / BatchGet), on an API v2 keyspace, with
// client-go v2 at the revision TiDB v8.5.8 pins.
//
//	tikvload -mode load -api raw -keyspace bq1raw -keys 1000000
//	tikvload -mode get  -api raw -keyspace bq1raw -threads 64 -depth 16
//	tikvload -mode load -api txn -keyspace bq1txn -keys 1000000
//	tikvload -mode get  -api txn -keyspace bq1txn -threads 64 -depth 1
//
// -depth N sends N keys per request (BatchGet); requests run concurrently
// on -threads goroutines sharing one client (client-go batches RPCs to a
// store over one gRPC stream, and TSO requests into one PD stream).
package main

import (
	"context"
	"encoding/json"
	"flag"
	"fmt"
	"math/rand/v2"
	"os"
	"sort"
	"sync"
	"sync/atomic"
	"time"

	"github.com/pingcap/kvproto/pkg/kvrpcpb"
	"github.com/tikv/client-go/v2/rawkv"
	"github.com/tikv/client-go/v2/txnkv"
)

type result struct {
	Engine     string             `json:"engine"`
	Op         string             `json:"op"`
	API        string             `json:"api"`
	Threads    int                `json:"threads"`
	Depth      int                `json:"depth"`
	DurationS  float64            `json:"duration_s"`
	Requests   int64              `json:"requests"`
	RequestsPS float64            `json:"requests_per_s"`
	OpsPerS    float64            `json:"ops_per_s"`
	Errors     int64              `json:"errors"`
	FirstError string             `json:"first_error,omitempty"`
	LatencyUs  map[string]float64 `json:"latency_us"`
}

func main() {
	pd := flag.String("pd", "127.0.0.1:39379", "PD address")
	ks := flag.String("keyspace", "bq1raw", "API v2 keyspace (created beforehand through PD)")
	api := flag.String("api", "raw", "raw | txn")
	mode := flag.String("mode", "get", "load | get")
	keys := flag.Int("keys", 1000000, "keys in the data set")
	vsize := flag.Int("value-size", 100, "value bytes")
	threads := flag.Int("threads", 64, "concurrent requests")
	depth := flag.Int("depth", 1, "keys per request (BatchGet when > 1)")
	duration := flag.Duration("duration", 30*time.Second, "measured window")
	warmup := flag.Duration("warmup", 5*time.Second, "warm-up")
	out := flag.String("out", "", "result JSON file (stdout otherwise)")
	flag.Parse()
	ctx := context.Background()

	var get func(ctx context.Context, ks [][]byte) error
	var put func(ctx context.Context, ks, vs [][]byte) error
	switch *api {
	case "raw":
		c, err := rawkv.NewClientWithOpts(ctx, []string{*pd},
			rawkv.WithAPIVersion(kvrpcpb.APIVersion_V2), rawkv.WithKeyspace(*ks))
		fail(err)
		defer c.Close()
		get = func(ctx context.Context, k [][]byte) error {
			if len(k) == 1 {
				_, err := c.Get(ctx, k[0])
				return err
			}
			_, err := c.BatchGet(ctx, k)
			return err
		}
		put = func(ctx context.Context, k, v [][]byte) error { return c.BatchPut(ctx, k, v) }
	case "txn":
		c, err := txnkv.NewClient([]string{*pd},
			txnkv.WithAPIVersion(kvrpcpb.APIVersion_V2), txnkv.WithKeyspace(*ks))
		fail(err)
		defer c.Close()
		get = func(ctx context.Context, k [][]byte) error {
			// A read-only transaction: one TSO per request, as a SQL
			// autocommit read without TiDB in front.
			ts, err := c.GetTimestamp(ctx)
			if err != nil {
				return err
			}
			snap := c.GetSnapshot(ts)
			if len(k) == 1 {
				_, err = snap.Get(ctx, k[0])
				return err
			}
			_, err = snap.BatchGet(ctx, k)
			return err
		}
		put = func(ctx context.Context, k, v [][]byte) error {
			tx, err := c.Begin()
			if err != nil {
				return err
			}
			for i := range k {
				if err := tx.Set(k[i], v[i]); err != nil {
					return err
				}
			}
			return tx.Commit(ctx)
		}
	default:
		fail(fmt.Errorf("unknown api %q", *api))
	}
	key := func(i int) []byte { return fmt.Appendf(nil, "k%012d", i) }

	if *mode == "load" {
		const batch = 512
		var wg sync.WaitGroup
		next := atomic.Int64{}
		errs := make(chan error, 8)
		for w := 0; w < 8; w++ {
			wg.Add(1)
			go func() {
				defer wg.Done()
				val := make([]byte, *vsize)
				for {
					start := int(next.Add(batch)) - batch
					if start >= *keys {
						return
					}
					var ks, vs [][]byte
					for i := start; i < min(start+batch, *keys); i++ {
						ks, vs = append(ks, key(i)), append(vs, val)
					}
					if err := put(ctx, ks, vs); err != nil {
						errs <- err
						return
					}
				}
			}()
		}
		wg.Wait()
		close(errs)
		for err := range errs {
			fail(err)
		}
		return
	}

	var measuring, stop atomic.Bool
	var reqs, errs atomic.Int64
	var firstErr atomic.Value
	lats := make([][]uint32, *threads)
	var wg sync.WaitGroup
	for w := 0; w < *threads; w++ {
		wg.Add(1)
		go func(w int) {
			defer wg.Done()
			r := rand.New(rand.NewPCG(uint64(w), 5))
			ks := make([][]byte, *depth)
			for !stop.Load() {
				for i := range ks {
					ks[i] = key(r.IntN(*keys))
				}
				t0 := time.Now()
				err := get(ctx, ks)
				if !measuring.Load() {
					continue
				}
				if err != nil {
					errs.Add(1)
					firstErr.CompareAndSwap(nil, err.Error())
					continue
				}
				reqs.Add(1)
				lats[w] = append(lats[w], uint32(min(time.Since(t0).Microseconds(), 1<<32-1)))
			}
		}(w)
	}
	time.Sleep(*warmup)
	t0 := time.Now()
	measuring.Store(true)
	time.Sleep(*duration)
	measuring.Store(false)
	el := time.Since(t0).Seconds()
	stop.Store(true)
	wg.Wait()
	var all []uint32
	for _, l := range lats {
		all = append(all, l...)
	}
	sort.Slice(all, func(i, j int) bool { return all[i] < all[j] })
	pct := func(q float64) float64 {
		if len(all) == 0 {
			return 0
		}
		return float64(all[min(len(all)-1, int(q*float64(len(all))))])
	}
	n := reqs.Load()
	op := map[string]string{"raw": "raw_get", "txn": "txn_snapshot_get"}[*api]
	if *depth > 1 {
		op = map[string]string{"raw": "raw_batch_get", "txn": "txn_snapshot_batch_get"}[*api]
	}
	fe, _ := firstErr.Load().(string)
	res := result{
		Engine: "tikv", Op: op, API: *api, Threads: *threads, Depth: *depth, DurationS: el,
		Requests: n, RequestsPS: float64(n) / el, OpsPerS: float64(n) * float64(*depth) / el,
		Errors: errs.Load(), FirstError: fe,
		LatencyUs: map[string]float64{"p50": pct(0.5), "p99": pct(0.99), "p999": pct(0.999), "max": pct(1)},
	}
	b, _ := json.MarshalIndent(res, "", "  ")
	if *out != "" {
		fail(os.WriteFile(*out, append(b, '\n'), 0o644))
	}
	fmt.Println(string(b))
}

func fail(err error) {
	if err != nil {
		fmt.Fprintln(os.Stderr, "tikvload:", err)
		os.Exit(1)
	}
}
