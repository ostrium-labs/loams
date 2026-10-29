# Loams Meter toward 1B QPS (BQ1-meter)

Status: **measured on one laptop, projected for the fleet.** Nothing here measured 1B QPS. The 1B figures are **projected, not measured**, and every assumption is written next to them.

Lane BQ1-meter, 2026-10-28. Harness: [`bench/meter`](../../../bench/meter). Raw results: [`bench/results/bq1/meter`](../../../bench/results/bq1/meter).

## What is on the hot path

Loams Meter (design [§63](../../design/63-loams-meter.md)) touches a request in up to three places:

| Piece | Where | Per what |
|---|---|---|
| **Gate** (`LeaseGate::check`, `loams-meter-gate`) | in-process, inside every engine's admission point (House, gateway API/LLM/MCP, Postgres connect, jobs, functions, streams) | **every request** |
| **MeterSink** (best-effort, D365) | in-process, a bounded queue flushed by one thread | every metered unit of work: per query, per LLM call, gateway requests batched per namespace per second (§63 §7.4) |
| **Node agent** (`loams-meter-agent`) | a DaemonSet per node: cgroup deltas → spool (fsync) → `StreamSink` → `_system/_meter` | per (tenant, resource, window), every 10 s, **independent of QPS** |
| **Counter owner** (`Allocator`, `loams-meter-core`) | the `meter` role, one per `_system/_meter` partition | per lease refresh: one per (subject, feature, node) per ≤ 10 s, **independent of QPS** |

So only the gate scales with QPS. The sink scales with metered work, which at 1B QPS has to be pre-aggregated (see the projection). The agent and the owners scale with tenants × nodes.

## Setup

- Intel Core Ultra 5 225H, no SMT. CPUs 0–3 are P-cores (4.9 GHz) and 4–11 are E-cores (4.4 GHz); 12–13 are low-power cores and are not used. 15 GB RAM, governor `powersave`, a laptop shared with other agents. Every number was taken inside the exclusive bench lock (`.he0-shared/benchslot.sh`), with the process under `taskset` and every worker thread pinned.
- Rust 1.97.1, the shipped release profile (opt-level 3, 16 codegen units, no LTO) plus line tables.
- Micro-benches: criterion 0.5.1, pinned to P-core 0.
- Load generator: `meter-loadgen` (`bench/meter/src/main.rs`), 10 s per run. The gate's scaling runs use 1, 2, 4 and 8 E-cores. For the agent runs, agents run on E-cores 4–7 and the mock produce server on 8–11.
- No `perf` or `cargo flamegraph` on this machine. The hot path was profiled by isolating it: the gate against `NoGate`, the sink against a counting producer, and the agent with and without the spool. The code was read alongside.

<!-- RESULTS -->
