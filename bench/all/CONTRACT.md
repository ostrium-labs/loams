# The native-load contract (BQ1, combined run)

Every engine lane provides one script that drives its engine through the
engine's **native API** (see `bench-1b-qps.md`, "Native APIs only") and
prints results in one shared format. `bench/all/run.sh` starts all of these
scripts at the same time, each on its own cores, and adds up what they print.

```
bench/<engine>/native-load.sh --duration S --cores LIST --depth N
                              [--target ADDR] [--server-cores LIST]
                              [--start-at EPOCH] [--ops OP[,OP...]]
```

`<engine>` is one of `kv`, `pg`, `sql`, `house`, `iggy`, `durable`, `meter`,
`graph`, `git`, `gateway` or `stream`.

## Arguments

| Argument | Meaning |
|---|---|
| `--duration S` | The length of the measured window, in seconds. Warm-up is not part of it. |
| `--cores LIST` | The `taskset -c` list for the **load generator**. Every load process the script starts runs inside it. For an in-process engine (the loams-kv embedded store, Grafeo, the meter gate, git-on-S3 on a file store), the engine runs in the load process, so these cores are the engine's cores too. |
| `--depth N` | Pipelining depth. `1` is the unpipelined baseline. A larger N means either N requests in flight on each connection (`depth_kind: "inflight"`) or N operations in one request (`depth_kind: "batch"`). The script says which in its output. |
| `--target ADDR` | The engine's native endpoint, already started by `bench/all/engines.sh`, as `host:port` or a URL. When `--target` is given, the script does not start a server. |
| `--server-cores LIST` | Used only without `--target`. The script then starts its own server, pinned to these cores, and stops it on exit. This is for standalone lane runs. |
| `--start-at EPOCH` | Do all setup (connect, create tables, prefill, warm up) first, then start the measured window at this Unix second. If setup runs past it, start at once and report `late_s`. The combined run needs this so that every window overlaps. |
| `--ops LIST` | Optional. Without it, the script runs the engine's headline operation(s). When it runs several operations, they run **concurrently** in the same window and split the connections between them. They never run one after another, because the wall time must stay `--duration`. |

Unknown arguments are an error (exit 2).

## Output

Only JSON lines go to stdout, one per operation. Logs go to stderr.

```json
{"engine":"kv","op":"get","qps":123456.0,"p50_ms":0.05,"p99_ms":0.4,"p999_ms":1.1,
 "depth":16,"cores":1,"errors":0}
```

Required fields:

| Field | Meaning |
|---|---|
| `engine` | The engine name, as in the path. |
| `op` | The operation, such as `get`, `point-select`, `produce`, `promise.create` or `gate.check`. |
| `qps` | **Operations** per second over the measured window. A batch of N keys counts as N operations. |
| `p50_ms`, `p99_ms`, `p999_ms` | The latency of one request (one round trip, or one batch) in milliseconds, or `null` if unknown. |
| `depth` | The `--depth` that was used. |
| `cores` | The number of load-generator cores (the size of `--cores`). |
| `errors` | The number of failed operations in the window. |

Optional fields, which `bench/all` copies through:

`rps` (requests per second, when it differs from `qps`), `depth_kind`
(`"inflight"` or `"batch"`), `conns`, `window_s`, `t_start` and `t_end`
(Unix seconds of the measured window), `late_s`, `server_cores` (a count, when
the script started its own server), `api` (the native API used, such as
`"tikv txn batch_get"`), and `note`.

## Behaviour

- Exit 0 when the run is measured, even if `errors > 0`. Exit non-zero only
  when nothing was measured.
- Finish within `start-at + duration + 30 s`. `run.sh` kills the script after
  `start-at + duration + 60 s`.
- Keep data under `$BQ1_DATA` (default `/mnt/Projects/bench-data/bq1-<engine>`),
  never under `/tmp`, and delete it on exit. Keep datasets small: the
  combined run shares 15 GB of RAM across every engine.
- Kill only the processes the script started, by PID.
- Measure through the native API only. A compatibility path (RESP, CQL, the
  Kafka gateway, Temporal gRPC, the PG SNI proxy) can be an extra `op`, with
  `"note":"compat"`, but `bench/all` leaves it out of the aggregate.

## Scripts written before this contract

Several lane scripts were written to the first, shorter brief
(`--duration --cores --depth`). They read `--cores` as the engine's **whole**
budget (server and load), and they start their own server. Some read a bare
number as a core *count*. `run.sh` adapts to each of them in `slot_args`:

- It passes a single core as `N-N`, never as `N`.
- It gives the House `--cores` (server) and `--client-cores` (load).
- It launches every script that lacks `--start-at` early, by its measured
  setup time.

New scripts should follow the contract above. The adapter for a lane can be
deleted once its script does.

## Where `bench/all` looks for a script

1. `$NATIVE_<ENGINE>` (for example `NATIVE_HOUSE=~/…/bq1-house/bench/house/native-load.sh`),
   which points at a lane's script before it is merged.
2. `bench/<engine>/native-load.sh`, the lane's script.
3. `bench/all/native/<engine>.sh`, the first version BQ1-ALL wrote for that
   engine. When the lane ships its own script, it takes precedence and the
   fallback can be deleted.
