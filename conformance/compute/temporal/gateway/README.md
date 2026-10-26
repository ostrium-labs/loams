# The Temporal gateway's SDK legs

Plan DX1 Task 10 (design 74 §13): the official Temporal SDKs, at the
corpus's pins (Go v1.49.0, TypeScript 1.24.0), against `loams-temporal`.

`run.sh` starts the gateway (and, with `LOAMS_RESONATE_BIN`, a real
Resonate server on SQLite) on loopback ports, then:

| Case | What it checks |
|---|---|
| `gateway_dx1_scenarios_{go,ts}` | the corpus's own generators run every DX1 scenario through the gateway |
| `gateway_hello_{go,ts}`, `gateway_signal_{go,ts}` | §74 §13.3's DX1 exit: hello (one activity) and a signal complete |
| `gateway_burst_{hello,timer_short}_go` | `BURST_N` (100) hello workflows, and half as many one-second timers, started at once; every result awaited with `run.Get`, whose long poll for the close event outlives the gateway's `history_wait_ms` (1 s here) |
| `gateway_histories_replay_{go,ts}` | each SDK's replayer accepts every history the gateway wrote |
| `gateway_dx1_differential_<lang>_<scenario>` | `compare.py`: the gateway's history equals the corpus's after `normalize.py`, a sticky queue read as its normal queue, and the SDK's name and version dropped from later tasks' `sdkMetadata` (plan ruling X1.R20) |

```sh
cargo build -p loams-temporal-frontend --bin loams-temporal
conformance/compute/temporal/gateway/run.sh                       # in-process Resonate
LOAMS_RESONATE_BIN=…/resonate conformance/compute/temporal/gateway/run.sh   # a real one
```

The containers use host networking and reach only `127.0.0.1`. Expected
failures are in `xfail.toml`.
