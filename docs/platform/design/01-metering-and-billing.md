# 01 — Metering and Billing: a Node Agent and a Usage Aggregator

> **Imported on 2026-10-19.** Written in the former private `ostrium-labs/loams-platform` repository; open source (Apache-2.0) in the monorepo since the owner's order of 2026-10-19. "This repository", "proprietary" and "private" below describe that repository and the open-core boundary of the time, which no longer holds; see [the platform docs index](../README.md).

Status: **Proposed** · 2026-09-29. This is Loam Cloud's metering and billing design. It is proprietary and lives here, outside the open-source engine, by the open-core rule in this repository's README. The engine's side is in operon's `docs/design/27-usage-hooks.md` (operon PD1–PD3): the Rust Dapr server does not meter, the engine exposes usage through open hooks, and metering and billing are a Loam Cloud component. This document is how Loam Cloud consumes those hooks. The owner's direction (2026-09-29): "rust dapr server is not fully implemented, create a separate billing sidecar will solve the issue, do whats best". This document takes the intent (billing must not wait for `loam-dapr`) and picks the shape: **a node-level agent, not a sidecar per pod** (§2).

Markers as in operon §24: **(verify)** is a claim not checked against a primary source; **(estimate)** is computed, not measured. Licenses, releases and activity were read on 2026-09-29 from the sources in §14.

Numbering: this repository's decisions are **PD1–PD10**; questions are **Q-MB-n**. A bare D-number (D65, D72, …) or § reference prefixed "operon" points into operon's `docs/design`.

---

## 1. Summary

| # | Decision | Status |
|---|---|---|
| PD1 | **Metering leaves `loam-dapr`.** A node-level metering agent, **`loam-meter`** (a DaemonSet), reads usage from outside the sandboxes, and a central **usage aggregator**, **`loam-usage`**, rolls it up and exports it to billing. `loam-dapr` records no meter events. Amends D175 (where meters are read), D177 (F1 scope), D180 (who writes `_meter`) and D182 (what `loam-dapr` does) | Proposed |
| PD2 | **Meter events are CloudEvents 1.0** in JSON structured mode, `type = dev.loam.meter.v1` (`io.loams.dev.meter.v1` after the rename, PD27), with `subject = <org>/<namespace>`, and `id` = the idempotency key `<node>/<source>/<seq>`. One event is one `(tenant, meter, window, resource)` delta (§4) | Proposed |
| PD3 | **Sources per tier** (§5): cgroup v2 `cpu.stat` and `memory.current` for T0 (the per-tenant workerd cgroup) and T2 (the gVisor sandbox cgroup); `/proc/<pid>/stat` as the T0 fallback when a workerd process has no cgroup of its own; wasmtime reports fuel, epochs and CPU per invocation over a node-local Unix socket (`loam.meter.v1.HostReport`); Envoy access logs over gRPC ALS for requests and bytes; Resonate promise and timer counts from the durable server; eBPF last, as a cross-check only | Proposed |
| PD4 | **The transport is a system stream, written ahead to a local spool.** The agent writes every event to an on-disk spool first (fsync), then forwards it to the stream `_meter` in the system namespace `_system`, partitioned by tenant. It uses idempotent producers when D72 ships; until then the aggregator dedupes. A cumulative counter's baseline advances only after the spool accepts the event, so a full spool delays usage and does not lose it (§6) | Proposed |
| PD5 | **`loam-usage` rolls up per `(org, namespace, meter, hour)`**, dedupes by idempotency key, and stores rollups in the `ControlStore`'s usage records (D65), which on the TiKV deployment is a TiKV keyspace (D179). It serves `GET /v1/orgs/{org}/usage` and feeds quotas (D65, D98) | Proposed |
| PD6 | **OpenMeter is not adopted now; Lago and Flexprice are rejected.** OpenMeter (Apache-2.0, active, owned by Kong since September 2025) needs Kafka, ClickHouse and Postgres next to Loam, and its releases are still `v1.0.0-beta.*`. Loam's events stay OpenMeter-compatible (CloudEvents with `subject` = the customer), so it can be adopted later as an unmodified service. Lago and Flexprice are AGPL-3.0 (§8) | Proposed |
| PD7 | **Usage above the plan is billed with Stripe meter events** (owner decision on Q-MB-1, 2026-09-29). The `StripeMeterExporter` (MB5) sends hourly per-tenant overage rollups as Stripe meter events to hour-pre-aggregated meters, with a deterministic rollup id as Stripe's `identifier` and an export ledger for exactly-once delivery (§9.4). Its customer and subscription side is set by PD10 | Proposed · Q-MB-1 decided by the owner 2026-09-29; amended by PD10 |
| PD8 | **Accuracy rules** (§7): meters read cumulative counters, so a missed sample loses nothing; a decrease is a reset; cgroups are keyed by path and inode; a final read on `cgroup.events` `populated 0`; a restarted agent resumes from its spool, and one that lost its state undercounts rather than double-bills; windows carry the agent's wall clock and the aggregator allows two hours of lateness; platform overhead is metered under `_platform` and folded into the request price | Proposed |
| PD9 | **Track MB** (§11): MB0 the `loam-meter` crate (cgroup reader, delta tracker, static resolver, spool, stream sink); MB1 `loam-usage`; MB2 the kube resolver and the DaemonSet chart; MB3 Envoy ALS; MB4 the wasmtime socket; MB5 the billing export; MB6 Resonate counts and per-sandbox egress; later, eBPF | Proposed |
| PD10 | **Billing is phased** (§9.1). **Beta:** Clerk Billing (`<PricingTable for="organization">`) with hard quotas and no overage, as the loam-cloud pricing page promises; no Stripe meter events. **GA:** billing moves to **Stripe Billing directly**, and Clerk handles auth and orgs only. Each Loam org has one Stripe subscription holding the plan's flat price plus metered prices (Billing Meters, hour windows), so it gets one invoice. Stripe Checkout and the Customer Portal replace the PricingTable. The Clerk org id maps to the Stripe customer (`billing/<org>` in the `ControlStore`, mirrored in Clerk `privateMetadata`). Stripe webhooks drive entitlements and quotas. Beta subscribers move at renewal, and the Clerk Billing plans are retired. Amends PD7 | Proposed · coordinator decision under the owner's "do best", 2026-09-29 |

## 2. Why a node agent, and not a sidecar or `loam-dapr`

operon §24 put metering in `loam-dapr` (D182: "meter events are recorded there"). That couples billing to a server that does not exist yet (operon §24 §3.1), and it meters from the wrong place: `loam-dapr` sees Dapr API calls, not CPU. The owner asked for a separate billing component. The choice is where it runs.

| Option | Tenant attribution | Process count | Tamper resistance | Sees CPU | Verdict |
|---|---|---|---|---|---|
| Inside `loam-dapr` (operon §24) | from the credential | 0 extra | good | **no**, only API calls | Superseded |
| **Sidecar per pod** | one pod's usage only; **cannot split** a workerd or wasmtime process that serves many isolates or components | **doubles** pods' container count; one more process per tenant in T0 | the sidecar shares the pod's network and lifecycle | only its own pod's cgroup | Rejected |
| **Node agent (DaemonSet)** + central aggregator | from the cgroup path, pod labels and the supervisor's reports | one per node | **outside every sandbox**; tenants cannot reach it | every cgroup on the node | **Chosen (PD1)** |

Four reasons decide it:

1. **Multi-tenant processes.** T1 runs many tenants' components in one wasmtime host, and a T0 workerd process runs many isolates. Only the host knows the per-tenant split, and only something outside the pod can combine the host's reports with the cgroup's totals.
2. **The data is node-level.** cgroup v2 `cpu.stat`, `memory.current` and `/proc` are read from the host's cgroupfs and procfs. A sidecar would need the same host mounts, times the number of pods.
3. **Tampering.** An agent in the tenant's pod, or reached through the tenant's credentials, can be starved, killed or fed lies. The node agent runs in the host's privileged DaemonSet, and tenants have no route to it.
4. **Independence.** Billing ships before `loam-dapr` and keeps working if the Dapr server changes shape (Q-RT-1).

`loam-dapr` keeps tenant identity and authorization (D182). It may later **report** API call counts to the agent like any other source (§5.6), but it is not on the billing path.

## 3. Architecture

```
 node ───────────────────────────────────────────────────────────────────────┐
 │  T0 workerd/tenant   T1 wasmtime host      T2 runsc sandbox               │
 │   (cgroup per tenant)  (one process)        (cgroup per sandbox)          │
 │        │                    │ HostReport over                │            │
 │        │ cpu.stat           │ /run/loam/meter.sock           │ cpu.stat   │
 │        │ memory.current     │ (fuel, epochs, cpu per call)   │ memory.*   │
 │  ┌─────▼────────────────────▼────────────────────────────────▼─────┐      │
 │  │ loam-meter (DaemonSet, privileged, hostPath /sys/fs/cgroup ro) │      │
 │  │  sample 1 s → deltas → tenant resolver → meter events          │      │
 │  │  spool (hostPath /var/lib/loam-meter, fsync, bounded)          │      │
 │  └──────────────────────────────┬──────────────────────────────────┘      │
 └─────────────────────────────────┼─────────────────────────────────────────┘
   Envoy (edge) ── gRPC ALS ───────┤ (requests, bytes in/out per tenant)
   Resonate ── promise/timer counts┤
                                   ▼
              Loam stream `_system/_meter` (the WAL), partitioned by org
                                   │
                          ┌────────▼────────┐
                          │   loam-usage    │  dedupe by id; hourly rollups;
                          │  (Deployment)   │  quotas; GET /v1/orgs/{org}/usage
                          └───┬─────────┬───┘
               ControlStore usage     BillingExporter
               records (TiKV, D65)    (beta: Clerk plans, hard quotas; GA: Stripe Billing, PD10)
```

| Component | Kind | Role | Phase |
|---|---|---|---|
| `loam-meter` | Rust, DaemonSet (crate and binary `loam-meter`, this repository) | Reads meters on the node, resolves tenants, writes events to its spool, forwards them to `_meter`; also receives Envoy ALS and wasmtime reports | MB0, MB2–MB4 |
| `_system/_meter` | Loam stream | The durable log of meter events. Retention 35 days (a month plus replay room) **(estimate)** | MB0 |
| `loam-usage` | Rust, Deployment (2 replicas, one owner per partition through the node registry, D108) | Consumes `_meter`, dedupes, rolls up, serves the usage API, drives quotas and the billing export | MB1, MB5 |
| `BillingExporter` | trait in `loam-usage` | Beta: Clerk plan sync for quotas. GA: `StripeMeterExporter`, hourly overage rollups as Stripe meter events (PD7, PD10) | MB5 |

The events live in a **system** namespace, not in each tenant's namespace as operon §24 §7 had it. Tenants must not be able to write, drop or truncate their own meter stream, and one stream partitioned by org is cheaper to consume than millions of per-namespace streams (D63). Tenants read their usage through the usage API, not the raw stream.

## 4. The meter event (PD2)

CloudEvents 1.0, JSON structured mode (`application/cloudevents+json`). The same shape is the stream record's value; the record key is the org, which picks the partition.

```json
{
  "specversion": "1.0",
  "id": "node-7/cgroup/184467",
  "source": "loam-meter://node-7/cgroup",
  "type": "dev.loam.meter.v1",
  "subject": "org_2f9a/ns_prod",
  "time": "2026-09-29T10:15:01.000Z",
  "datacontenttype": "application/json",
  "data": {
    "org": "org_2f9a",
    "namespace": "ns_prod",
    "meter": "cpu_usec",
    "value": 1834,
    "window_start": "2026-09-29T10:15:00.000Z",
    "window_end": "2026-09-29T10:15:01.000Z",
    "tier": "t0",
    "function": "chat-agent",
    "resource": "loam.slice/tenant-org_2f9a.slice#ino=51234",
    "counter": 99183422,
    "node": "node-7",
    "seq": 184467
  }
}
```

| Field | Rule |
|---|---|
| `id` | **The idempotency key**: `<node>/<source>/<seq>`. `seq` is monotonic per node and assigned before the event enters the spool; retries resend the spooled event, so a replay carries the same id. An agent that lost its state starts `seq` at the current Unix time in microseconds, above any id it used before. CloudEvents requires `(source, id)` to be unique; ours is unique on its own |
| `source` | `loam-meter://<node>/<source kind>`; the kinds are `cgroup`, `proc`, `wasm`, `envoy`, `durable` |
| `subject` | `<org>/<namespace>`: the billed customer, as OpenMeter reads it (§8) |
| `time` | The window's end, from the agent's wall clock |
| `data.meter` | One of the meters below. The unit is part of the name |
| `data.value` | An unsigned integer delta for the window; never negative, never a running total |
| `data.counter` | The reading at `window_end`: the cumulative value for a counter meter, the instantaneous value for a gauge (`memory.current` for `memory_byte_seconds`). The agent rebuilds its baselines from it after a crash (§6.2); the aggregator ignores it |
| `data.resource` | What was measured (a cgroup with its inode, a pid, a wasm instance pool, an Envoy route). Used for audits and for Q-RT-6's per-invocation split |
| `data.function`, `data.tier` | Present when known (supervisor labels or reports) |

**Meters.**

| Meter | Unit | Kind | Source | Billed (D180) |
|---|---|---|---|---|
| `cpu_usec` | CPU microseconds | counter | cgroup `cpu.stat usage_usec`, `/proc` utime+stime, wasmtime host | **yes** |
| `memory_byte_seconds` | bytes × seconds | gauge integral (trapezoid over the window) | cgroup `memory.current` | not by default (Q-RT-2) |
| `memory_peak_bytes` | bytes | gauge (maximum in the window) | cgroup `memory.peak` | no; limits and audits |
| `wasm_fuel` | fuel units | counter | wasmtime host | no; cross-check for `cpu_usec` in T1 |
| `requests` | count | counter | Envoy ALS | **yes** |
| `ingress_bytes`, `egress_bytes` | bytes | counter | Envoy ALS; later per-sandbox counters | egress **yes** once priced (Q-MB-3) |
| `durable_promises`, `durable_timers` | count | counter | Resonate | not yet (Q-MB-4) |

**Protobuf.** The wasmtime host reports over a local socket in protobuf, not JSON (the report is on the host's hot path). The message is part of operon's open usage-hooks contract (operon §27 §3.3, D201), which owns the `.proto`; it is repeated here for reference:

```protobuf
syntax = "proto3";
package loam.meter.v1;

// One report per invocation (or per batch of invocations), sent by a
// runtime host to the node agent over /run/loam/meter.sock as a
// length-delimited stream. The agent authenticates the peer by SO_PEERCRED
// against the supervisor's uid; tenants never reach the socket.
message HostReport {
  string host_id = 1;          // stable per host process
  uint64 seq = 2;              // per host_id, monotonic; the agent dedupes on (host_id, seq)
  repeated Invocation invocations = 3;
}

message Invocation {
  string org = 1;
  string namespace = 2;
  string function = 3;
  string version = 4;
  string invocation_id = 5;
  uint64 cpu_usec = 6;         // thread CPU time measured by the host
  uint64 fuel = 7;             // 0 when fuel is off
  uint64 epochs = 8;           // epoch ticks consumed
  int64 start_unix_ms = 9;
  int64 end_unix_ms = 10;
}

message HostReportAck { uint64 seq = 1; }
```

This sketch predates the published contract. **Operon §27 §3.3 is authoritative**: `Invocation` also has `bool cpu_estimated = 11` (true when CPU was apportioned or capped, not measured) and §3.6's fields 12–16 (`runner`, `region`, `provider_billed_ms`, `compile_usec`, `overhead_usec`). `loam-meter` and `loam-usage` follow operon's `proto/loam/meter/v1/meter.proto` (RN1), not this sketch.

## 5. Sources per tier (PD3)

### 5.1 T2 gVisor and containers

Each sandbox is a pod with `RuntimeClass: gvisor`. Its cgroup v2 directory, under `/sys/fs/cgroup/kubepods.slice/…/kubepods-<qos>-pod<uid>.slice/` (systemd driver) or `/sys/fs/cgroup/kubepods/<qos>/pod<uid>/` (cgroupfs driver), holds `cpu.stat` (`usage_usec`), `memory.current`, `memory.peak` and `cgroup.events`. gVisor's sentry and gofer run inside that cgroup, so the sandbox's overhead is billed to the sandbox, which matches what the tenant chose **(verify with runsc's systemd-cgroup mode in F2)**.

**Tenant resolution.** The pod UID is in the path. The kube resolver (MB2) watches pods on its own node (`spec.nodeName` field selector, one watch per agent) and maps the UID to the labels `loam.dev/org`, `loam.dev/namespace`, `loam.dev/function` set by the operator. It can fall back to the kubelet's `/pods` endpoint when the API server is unreachable. A pod without the labels is metered under `_platform`.

### 5.2 T0 workerd

One workerd process per tenant (D171). The supervisor starts each one in its own cgroup (`loam.slice/tenant-<org>.slice/workerd.scope`, operon §27 §3.2), and the agent reads that cgroup exactly as in §5.1. Where the supervisor cannot create cgroups (a node without delegation), the agent reads `/proc/<pid>/stat` fields 14 and 15 (`utime`, `stime`, in clock ticks) for the pids the supervisor reports, and marks the events `source = proc`. `/proc` misses a process's exited children and loses the final ticks when the process exits between samples, so cgroups are the default.

### 5.3 T1 wasmtime

Many tenants share one host process, so the cgroup gives only the host's total. The host measures each invocation (thread CPU time from `clock_gettime(CLOCK_THREAD_CPUTIME_ID)` around the call, plus fuel or epochs) and sends `HostReport`s to the agent over `/run/loam/meter.sock` (§4). The agent turns them into `cpu_usec` and `wasm_fuel` events. It also meters the host's cgroup and attributes the **difference** between the cgroup total and the sum of the reports to `_platform` (host overhead, allocator, compilation). A persistent gap above a threshold (5% **(estimate)**) raises an alert: either the host under-reports or the overhead is worth pricing.

### 5.4 Requests and bytes: Envoy

Envoy at the edge sends access logs over **gRPC ALS** (`envoy.service.accesslog.v3.AccessLogService/StreamAccessLogs`) to the agents, or to a small ALS receiver inside `loam-usage` when the edge runs off the runtime nodes (Q-MB-2). The tenant comes from the `x-loam-tenant` header that `loam-gateway` sets after routing, with any client-supplied value stripped by Envoy (operon §27 §3.4). Each log entry gives one request, `request_headers_bytes + request_body_bytes` in and `response_headers_bytes + response_body_bytes` out. The receiver aggregates per second before writing events, so the stream carries one event per `(tenant, meter, second)`, not one per request. OTLP logs (D73) are the alternative transport if Envoy Gateway's telemetry makes ALS awkward **(verify)**.

**Per-sandbox egress** (a function calling out) is metered later (MB6) from the sandbox's network namespace counters (`/proc/<pid>/net/dev` of a process in the pod, or the veth's host-side counters) and the supervisor's egress proxy (D141's allowlist). eBPF (aya) comes last, as a cross-check only (D175).

### 5.5 Durable waits: Resonate

The Resonate server is embedded in `crates/operon-durable` (operon §21). Counting promises created and timers scheduled per namespace is cheap if it is a counter on the create path. The durable crate exposes those counts, and a reporter sends them over the same local socket (MB6). If the counts need a table scan, they are deferred (Q-MB-4). Waiting itself is never billed (D173); the counts exist for abuse limits and a possible storage price.

### 5.6 Other reporters

Any host component can send `HostReport`-like records over the socket: the supervisor (invocation counts and wall time for Q-RT-6's split), `loam-dapr` (API call counts) and Loam's engine (D103's logical bytes, until the engine writes those itself). The agent authenticates reporters by `SO_PEERCRED` against an allowlist of uids.

## 6. The pipeline (PD4, PD5)

### 6.1 The agent loop

Every second (configurable):

1. **Scan** the configured cgroup subtrees; for each cgroup read `cpu.stat`, `memory.current`, `memory.peak` and `cgroup.events`, and stat the directory for its inode.
2. **Resolve** the tenant: the static map (MB0) or the pod watch (MB2). Unknown → `_platform`.
3. **Delta.** For each counter, `value = reading − baseline`; for memory, the trapezoid integral since the last sample. Zero deltas are not emitted.
4. **Spool.** Append the batch to the spool and fsync. Only then **commit** the new baselines in memory.
5. **Forward** (a separate task). Read unacknowledged spool entries in order, produce them to `_system/_meter`, and acknowledge the spool on success.

### 6.2 The spool

- Append-only segment files of JSON lines under `/var/lib/loam-meter/spool` (a hostPath), each line `{offset, event}`.
- An `ack` file (written by rename) holds the highest acknowledged offset. Fully acknowledged segments are deleted.
- **Bounded**: 256 MiB by default, and no age limit (expiring unacknowledged events would lose usage). When the spool is full, the append fails, the baselines do **not** advance, and the next successful append carries the whole backlog in one larger delta. Cumulative counters make this lossless for everything that still exists on the node. The only loss is usage of cgroups that disappear while the spool stays full, and the agent counts that as `meter_spool_lost_usec` so it shows on dashboards.
- The agent's state file (baselines and the next `seq`) is written by rename after each append. After a crash the agent loads it and replays the spooled events whose `seq` is at or past the saved one, taking `data.counter` as the new baseline for each resource, so an event that reached the spool is never emitted twice under a different id.

### 6.3 Exactly once, end to end

| Failure | What happens |
|---|---|
| Stream unreachable | Events wait in the spool; the forwarder retries with backoff; order per partition is kept |
| Forwarder crashes after the stream acknowledged, before the spool ack | The same events are sent again with the same ids; the aggregator drops them (§6.4). With D72's idempotent producers (producer id = `(node, source)`, sequence = offset) the stream drops them itself |
| Agent crashes between spool append and state write | Replay of the spool past the state's offset restores the baselines (§6.2) |
| Agent's disk is wiped (node replaced, hostPath lost) | No state: every cgroup is baselined at its current reading on the first scan. Usage since the last forwarded event is **not billed** (undercount, never double-billing) and counted as `meter_baseline_skipped_usec` |
| Aggregator crashes | It resumes from its committed consumer offset; the rollup and the dedupe marks are committed in the same transaction as the offset |

### 6.4 The aggregator: `loam-usage`

- A named consumer (D72) on `_system/_meter`. Each partition has one owner at a time (the node registry's rendezvous ownership, D108).
- **Dedupe**: a key `dedupe/<event id>` in the TiKV keyspace, written in the same transaction as the rollup increment and the consumer offset, with a TTL of 35 days (the stream's retention: an id older than that cannot be replayed). On the openraft and Postgres deployments the same transaction runs on the `ControlStore`.
- **Rollups**: `usage/<org>/<namespace>/<meter>/<hour>` → sum, assigned by `data.window_end`. Hours close two hours after they end (the lateness allowance, §7.4); a later event goes into a `late` bucket for the current hour and is reported, not dropped.
- **API**: `GET /v1/orgs/{org}/usage?from=&to=&meter=&group_by=namespace|function|hour|day`. The console (operon §19) reads it. Quotas (D65, D98) read the current hour's rollups.
- **Export**: at hour close, the `BillingExporter` receives the closed rollups (§9).

## 7. Accuracy (PD8)

### 7.1 Counters, not gauges

CPU, requests, bytes and fuel are read as **cumulative** counters, so a missed sample (agent paused, node overloaded) folds into the next delta. Nothing is lost; only granularity is. Memory is a gauge; a missed sample widens the trapezoid, which is an approximation and is documented as one.

### 7.2 Resets and churn

- A cgroup is identified by **path and inode**. A pod restarted with the same path gets a new directory and a new inode, so it is a new resource starting from zero.
- A counter that **decreases** under the same key is a reset: the delta is the new reading (usage since the reset), never negative.
- **Final read.** When a cgroup's last process exits, `cgroup.events` changes to `populated 0` before the runtime removes the directory. The agent watches `cgroup.events` with inotify (MB2; MB0 checks it on each scan), takes a final reading, emits it and drops the resource. For T0 and T1 the supervisor can also hold the cgroup until the agent acknowledges the final read (`drain` over the socket).
- A cgroup that vanished between two scans without a final read loses at most one interval of usage (≤ 1 s by default), counted as `meter_unflushed_removals`.

### 7.3 First sight

When a new cgroup appears while the agent has continuous state, its whole counter is usage (it started at zero). When the agent starts **without** state, every existing cgroup is baselined at its current reading (§6.3).

### 7.4 Clocks

Windows use the agent's wall clock (nodes run chrony; the agent refuses to emit, and spools a gap marker, if the kernel reports the clock unsynchronized through `adjtimex`, **(verify)** the cheapest check). Deltas are computed on counters, not on clock differences, so skew moves usage between adjacent hours but never changes totals. The aggregator's two-hour lateness allowance covers a spooled backlog after a short outage; longer outages land in `late` buckets.

### 7.5 Platform overhead

Usage that no tenant owns (the supervisor, the agent, Envoy, workerd's shared overhead that the host reports as unattributed, wasmtime compilation) is metered under the tenant `_platform`. It is not billed; it is priced into the request fee (operon §24 §9, Q-RT-7), and the `_platform` share per node is a capacity-planning signal.

## 8. Buy vs build: OpenMeter, Lago, Flexprice (PD6)

Rule: linked dependencies must not be AGPL, BSL, SSPL or ELv2; an unmodified separate service under those licenses is allowed but flagged.

| Project | License | Activity (2026-09-29) | Stack | Verdict |
|---|---|---|---|---|
| **OpenMeter** (`openmeterio/openmeter`) | **Apache-2.0** | last push 2026-09-28; release `v1.0.0-beta.234` (2026-09-25), roughly weekly; 2,350 stars; about 40 contributors; Go. **Acquired by Kong in September 2025**; the metering engine became "Konnect Metering & Billing", and Kong said the project stays open source | Kafka (ingest), ClickHouse (aggregation), Postgres (catalog, billing), optional Redis (dedupe) and Svix (webhooks) | **Not now.** Compatible by design |
| Lago (`getlago/lago`) | **AGPL-3.0** | active | Ruby, Postgres, Redis, ClickHouse optional | **Rejected** (license) |
| Flexprice (`flexprice/flexprice`) | **AGPL-3.0** | active; v2.1.33 (2026-09-25); Go | Postgres, ClickHouse, Kafka | **Rejected** (license) |

**OpenMeter, compared.**

| | Adopt OpenMeter unmodified | Small aggregator on Loam streams (chosen) |
|---|---|---|
| New infrastructure | Kafka, ClickHouse, Postgres (+ Redis) in every cloud and BYOC-managed deployment | none: `_meter` is a Loam stream, rollups live in the `ControlStore` |
| Fit with "the log is the spine" (D4) | a second log (Kafka) beside Loam's | the WAL is the log |
| Features | meters, entitlements, plans, invoicing, credits, Stripe app | rollups, quotas and export only; plans stay in Clerk |
| Maturity | beta releases with breaking changes; production-proven engine | new code, small (MB1 ≈ 1–2k lines **(estimate)**) |
| Ownership risk | Kong's roadmap now centers on Konnect | ours |
| Dogfooding | none | the first internal consumer of named consumers (D72) |

The agent is the same either way. OpenMeter ingests CloudEvents with `subject` as the customer and dedupes on `(id, source)`, which is exactly PD2's shape, so switching later means pointing the forwarder at OpenMeter's `POST /api/v1/events` (or a Kafka topic) instead of `_meter`. Revisit if Loam needs prepaid credits, invoicing or entitlements beyond what Clerk and Stripe provide.

## 9. Billing: Clerk in the beta, Stripe Billing at GA (PD7, PD10)

**Decisions.** The owner decided Q-MB-1 on 2026-09-29: usage above the plan is billed with **Stripe meter events** (PD7). The customer and subscription side is phased (PD10), so there is never a second invoice and nothing depends on Clerk's Stripe customer ids.

### 9.1 Phases

| | **Beta** | **GA** |
|---|---|---|
| Plans, checkout | Clerk Billing, `<PricingTable for="organization">` on loam-cloud | **Stripe Billing directly**: Stripe Checkout and the Customer Portal |
| Clerk's role | auth, orgs and plans | auth and orgs only |
| Overage | **none**: hard quotas, as the pricing page promises ("hard quotas with no overage charges") | metered prices on the org's subscription, fed by the `StripeMeterExporter` (§9.4) |
| Subscription and invoice | Clerk's | **one Stripe subscription per Loam org**: the plan's flat price plus one metered price per billed meter (Billing Meters with `event_time_window = hour`), so **one invoice** |
| Included usage | Clerk plan and feature metadata → `loam-usage` → quotas (D65) | Stripe price and product metadata → entitlements and quotas |
| Plan changes and state | Clerk `subscription.*` webhooks | Stripe webhooks (`customer.subscription.*`, `invoice.*`) → entitlements and quotas |
| Stripe meter events | not sent | sent hourly |

**The migration.** The date is Q-MB-6. At each beta subscriber's next renewal, `loam-usage` creates a Stripe customer and subscription on the matching GA price and does not renew the Clerk plan, so no period is charged twice. After the last renewal, the Clerk Billing plans are retired and the PricingTable is removed from loam-cloud.

### 9.2 The Stripe Billing Meters API (read 2026-09-29)

| Item | What the docs say | How we use it |
|---|---|---|
| Endpoint | `POST /v1/billing/meter_events` with `event_name`, a `payload` and an optional `identifier` and `timestamp`. The payload carries the keys named by the meter's `customer_mapping.event_payload_key` (default `stripe_customer_id`) and `value_settings.event_payload_key` (default `value`) | The default path |
| High throughput | API v2 **meter event streams**: create a meter event session (`/v2/billing/meter_event_session`; the token is valid for 15 minutes), then `POST /v2/billing/meter_event_stream` in batches. Up to 10,000 events/s, live mode only (200,000/s through sales) | Only if v1's limit is ever reached, which it will not be at hourly granularity (see Volume below) |
| `identifier` | "A unique identifier for the event … Stripe enforces uniqueness within a rolling period of at least 24 hours". At most 100 characters | The rollup id (§9.4) |
| `timestamp` | "Must be within the past 35 calendar days or up to 5 minutes in the future" | The start of the rollup's hour |
| Values | Decimals are accepted. A negative total for the cycle shows as 0 | CPU in CPU-ms and egress in GB, as decimals |
| Rate limits | Live mode: **1,000 calls/s per account**, separate from the global limit, and **one concurrent call per customer per meter**. Sandbox calls count against the global limit. Calls made by a Connect platform with `Stripe-Account` get 100 operations/s | The exporter serializes calls per (customer, meter) and backs off on 429. Direct calls on our own account (PD10) are not platform calls, so the 100/s limit does not apply |
| Aggregation | `default_aggregation.formula` is `sum`, `count` or `last`. `event_time_window` set to `hour` or `day` makes a meter **pre-aggregated**: "Stripe only uses the most recently received meter event in that time interval. A newer event … overwrites the previous one" | `sum` with `event_time_window = hour` (§9.4) |
| Customer mapping | `customer_mapping` `{type: by_id, event_payload_key}` | `stripe_customer_id` (§9.3) |
| Dimensions | At most 10,000 unique dimension combinations per meter per hour, and 100 per customer | We send none |
| Errors | Events are processed asynchronously. Failures arrive as thin events `v1.billing.meter.error_report_triggered` and `v1.billing.meter.no_meter_found`, with codes such as `meter_event_customer_not_found` and `timestamp_too_far_in_past` | An event destination in `loam-usage` marks the ledger row `failed` and alerts |
| Corrections | A meter event adjustment cancels an event by `identifier`, "only … within the last 24 hours" | Not needed: pre-aggregated meters overwrite instead (§9.4) |

**Volume.** One event per (tenant with overage, meter, hour). At 10,000 such tenants and 3 meters, that is 30,000 events an hour, about 8 per second, far below v1's 1,000/s **(estimate)**.

### 9.3 Mapping a Clerk org to a Stripe customer (GA)

- **The billed principal** is the Clerk **organization**, which maps 1:1 to the Loam org (D65). A personal account is billed through the single-member org that loam-cloud creates for it.
- **Loam creates the customer.** It happens on first checkout: a Stripe Checkout session in subscription mode for the chosen plan creates the customer and the subscription together. The customer carries `metadata.clerk_org_id` and `metadata.loam_org`, and the Checkout session carries `client_reference_id = <clerk org id>`. There is no dependency on customers created by Clerk Billing.
- **Where the mapping lives.** In the `ControlStore`, as `billing/<org>` → `{clerk_org_id, stripe_customer_id, stripe_subscription_id, plan, period_start, period_end, status}`, written from the `checkout.session.completed` and `customer.subscription.*` webhooks. It is mirrored in the Clerk org's `privateMetadata.stripe_customer_id` for support tooling. The exporter and quotas read only the `ControlStore`.
- **Self-service.** The console links to the Stripe Customer Portal for payment methods, invoices, plan changes and cancellation. Portal changes arrive as webhooks, like any other change.
- **Calls use our own secret key on our own account**, not a platform connection with `Stripe-Account`, so the meter-event limit is 1,000/s.

### 9.4 The exporter: exactly once

**What is sent (GA).** Take each org whose plan allows overage, each meter `m`, and each closed hour `h` in the current Stripe billing period:

```
cum(h)     = Σ usage(org, m, hours from period_start through h)      # from the rollups (§6.4)
overage(h) = max(0, cum(h) − included(plan, m)) − max(0, cum(h−1) − included(plan, m))
```

**Billing periods start on whole UTC hours**, so no rollup hour straddles `period_start`. Splitting a straddling hour would not work: its two parts would be two events in one Stripe hour window, and the later would overwrite the earlier. So:

- Checkout sets `subscription_data.billing_cycle_anchor` to the next whole UTC hour. The stub before the anchor (under an hour) is prorated by Stripe and has no overage: its usage counts against no allowance and is never exported.
- Plan changes keep the anchor (`billing_cycle_anchor = unchanged`); Loam never resets it to `now`.
- If a webhook reports a `period_start` that is not on a UTC hour, the exporter stops for that org and alerts, not guessing which side of the boundary the hour belongs to.

`overage(h)` is sent as one meter event with `timestamp = h`, `payload = {stripe_customer_id, value}`, and as `event_name` the Stripe meter for `m` (`loam_cpu_ms`, `loam_requests` or `loam_egress_gb`). An hour with no overage is sent only if an earlier revision of that hour was sent with a non-zero value, to overwrite it with 0.

**The identifier follows our event ids.** There are too many raw meter events to send individually (one per resource per second), so the exported unit is the hourly rollup. Its id is built the way a meter event id is: deterministic and stable across retries. It is `<org>/<meter>/<hour>/r<revision>`, which stays under 100 characters because org ids are short. `revision` starts at 0 and increases only when a late event changes a closed hour.

**Two layers make the export exactly once:**

1. **Stripe's `identifier`** drops a retry of the same rollup within 24 hours. This covers a crash after Stripe accepted the call but before our ledger recorded it.
2. **Hour-pre-aggregated meters** (`event_time_window = hour`) keep only the latest event per hour. A retry after 24 hours, or a revision carrying a corrected value, **overwrites** the earlier event instead of adding to it. A duplicate can never double-bill.

**The export ledger** is in the `ControlStore`, with one row per `(org, meter, hour)`: `{revision, value, identifier, state: pending | sent | failed, sent_at}`.

- The exporter writes the row as `pending`, with its identifier, in the same transaction that reads the rollup. It then calls Stripe and marks the row `sent`.
- On restart, `pending` rows are resent with their stored identifier.
- Rows for the same (customer, meter) are sent one at a time, to respect Stripe's one-concurrent-call rule.

**Late data.** A late event that lands in a closed hour (§6.4) changes that hour's rollup. The exporter recomputes `overage` for that hour and for every later hour in the period whose value changes, and sends them with `revision + 1`. Stripe accepts timestamps up to 35 days old, so corrections within the billing period always land.

**Period close.** Stripe drafts the overage invoice at the end of the period. The exporter flushes the last hours, after the two-hour lateness allowance, before the invoice finalizes. The subscription's invoices must therefore stay in draft long enough **(verify the auto-finalization delay, and set it to at least 3 hours)**. Usage that arrives after finalization is carried into the next period as an adjustment line, never silently lost.

**Failures.**

- 429 and 5xx: exponential backoff; the row stays `pending`.
- Other 4xx: the row goes `failed` and pages someone.
- Asynchronous errors (§9.2): the row matching the identifier is marked `failed`.
- A daily reconciliation job compares Stripe's meter event summaries per customer and period with the ledger.

## 10. What operon provides, and what it no longer does

> **Amended 2026-10-02 (Loams D540 to D559, PD61 to PD68).** The meter record, the host-report socket and the usage reporter are specified in [doc 06](06-meter-record-and-usage-reporter.md), which is authoritative; the open repository exposes only generic observability and quota enforcement, for the integrity reasons stated there. The "documented local interface" below is now private. The control plane, BYOC management and multi-cluster GitOps are open source in Loams §41; this document's references to them as private are superseded.

operon's PR for doc 27 (operon D200–D202) makes the engine side generic:

- `loam-dapr` checks identity and authorizes; it records no meter events (operon D200).
- The engine exposes usage through **open hooks** any metering system can consume (operon D201): Prometheus/OpenTelemetry metrics per namespace, a documented cgroup layout with `loam.dev/org`, `loam.dev/namespace` and `loam.dev/function` labels per sandbox, the runtime hosts' per-invocation usage on a documented local interface, and Envoy access logs with the tenant in route metadata.
- Metering and billing are a Loam Cloud component outside the open-source repository (operon D202): this repository's `loam-meter` and `loam-usage`, deployed by the cloud overlay of the umbrella chart, not by the open-source chart.

## 11. Track MB (PD9)

| Phase | Scope | Exit gate |
|---|---|---|
| **MB0** (this repository's first PR) | Crate `loam-meter`: event types; cgroup v2 reader (`cpu.stat`, `memory.current`, `memory.peak`, `cgroup.events`); delta tracker (resets, inode keys, final reads, first sight); `TenantResolver` with a static implementation; `Sink` with the spool and a stream sink over the HTTP produce route; a binary that runs the loop | unit tests over a fake cgroupfs; no double count across simulated crashes |
| **MB1** | `loam-usage`: named consumer, dedupe, hourly rollups in the `ControlStore`, usage API | replaying a stream with duplicates gives identical rollups |
| **MB2** | Kube resolver (pod watch by node, kubelet fallback); inotify final reads; the DaemonSet and its chart in this repository (a cloud overlay on operon's `loam-stack`) | churn test on k3d: 1,000 short pods, totals within 1% of the cgroup sums |
| **MB3** | Envoy gRPC ALS receiver; per-second request and byte events | requests metered equal Envoy's own stats |
| **MB4** | The wasmtime socket (`HostReport`), `SO_PEERCRED`, host-overhead attribution | operon §24 §11's gate: meter totals within 2% of cgroup totals |
| **MB5** | GA billing (PD10): Stripe products and prices (flat plus metered), Checkout and the Customer Portal, the Clerk org ↔ Stripe customer mapping, Stripe webhooks → entitlements and quotas, the `StripeMeterExporter` with its export ledger, meter error events, and migration at renewal. **Beta (before MB5):** Clerk plan sync → hard quotas only | a test org's single Stripe invoice (flat plus overage) matches its rollups, including a late correction and an exporter crash mid-send; a beta subscriber migrates at renewal without a double charge |
| **MB6** | Resonate counts; per-sandbox egress | — |
| **Later** | eBPF (aya) cross-check (D175) | — |

MB0–MB2 do not depend on `loam-dapr`, D72 or the unified auth plan. MB1 needs the stream API's consumer side (D72) or reads partitions with the existing fetch route until then.

## 12. Open questions

| # | Question | Owner | Needed by |
|---|---|---|---|
| Q-MB-1 | ~~Overage invoicing before Clerk ships usage-based billing~~ **Decided by the owner, 2026-09-29: Stripe meter events** (PD7, §9) | Founder | Resolved |
| Q-MB-2 | Where the Envoy ALS receiver runs: in each `loam-meter` (Envoy on runtime nodes) or in `loam-usage` (a separate edge pool) | Eng | MB3 |
| Q-MB-3 | Egress pricing and whether ingress bytes are ever billed | Founder | Before cloud beta |
| Q-MB-4 | Whether durable promises and timers are priced, and whether the durable crate can count them without a scan | Founder / Eng | MB6 |
| Q-MB-5 | Whether tenants may export their own raw meter events (a read-only view of `_meter` filtered to their org) for audits | Founder | MB1 |
| Q-MB-6 | The GA migration date: when Stripe Billing replaces Clerk Billing, and so when beta subscribers move at renewal (PD10) | Founder | Before GA |

## 13. Runner usage, reconciliation and the usage-event form (PD26–PD28; moved from operon, 2026-10-02)

The owner ruled on 2026-10-02 that there is **no metering in operon** (operon §38 D440, D444). Three pieces that operon §34 §12, RN1 Task 6 and operon §38 had placed in or near operon belong here.

| # | Decision | Status |
|---|---|---|
| PD26 | **Provider reconciliation.** `loam-usage` reconciles host reports from external runners against the providers' own figures once a day: Lambda's billed duration (from the `REPORT` lines operon's runner host already reads, and the AWS cost and usage export) and Cloudflare's CPU-ms for Workers and WfP scripts (from Cloudflare's analytics API). **Attribution:** provider records join to tenants only through tenant-scoped resources. WfP user scripts are named `t-<org>-<ns>-<fn>` and carry `org` and `namespace` script tags (doc 03 PD23); Lambda functions Loam deploys carry the tags `loam:org` and `loam:namespace` and one function per tenant function. A provider record without those tags (shared front Workers, the Tail Worker, platform functions) is attributed to `_platform` and never compared with or billed to a tenant. A mismatch above 2% per tenant-day on tenant-scoped records opens a billing hold for that tenant and an alert, not a silent correction. Was operon §34 §12 item 5 and Q-RT-7's pricing of compile and overhead CPU (`compile_usec`, `overhead_usec`), which stays an owner pricing decision here | Approved (owner defaults, 2026-10-02) |
| PD27 | **The usage-event form is the meter event.** Operon D376 item 4 (a host report also as the CloudEvent `dev.loam.meter.usage.v1`) and RN1's former Task 6 are **subsumed by PD2 and PD4**. `loam-meter` already turns host reports into CloudEvents on `_system/_meter`. There is one type, the PD2 `dev.loam.meter.v1`, and no separate `meter.usage.v1` type. A host report is a meter event with the same type and payload (§4). The type moves to the owner's prefix with the rename (`io.loams.dev.meter.v1`; operon §34 §2), and the old `dev.loam.meter.usage.v1` name retires. The Arrow event table for `_meter` (operon D372) is a keyed Iceberg table (operon §08 §1, `PRIMARY KEY (source, id)`) that `loam-usage` writes **as upserts**, so a replayed event replaces its row instead of appending a duplicate. Counts for audit and analytics read that table, and MB7's tests include `replayed_events_do_not_double_count`. It is plan MB7, not yet written | Approved (owner defaults, 2026-10-02) |
| PD28 | **Knative and Cloudflare sources.** `loam-meter` reads Knative pods (operon §27 §3.5a: labels `loam.dev/runner=knative`, tier `t2`) like any T2 sandbox, and counts requests from the queue-proxy metrics. On Cloudflare, the Tail Worker of doc 03 (PD23) is the source for WfP and front Workers, delivered to `loam-usage` over HTTPS with the same ack rule (PD23 refines operon D201) | Approved (owner defaults, 2026-10-02) |

## 14. Sources

Read on 2026-09-29.

- Repository: operon §24, operon §25, D65, D72, D98, D103, D108, D128, D141, D171–D188; `crates/operon/src/api/mod.rs` (the HTTP produce route `POST /v1/namespaces/{ns}/streams/{stream}/partitions/{p}/records`); `crates/operon-log/src/batch.rs` (producer fields reserved in the batch header, no idempotent producer yet); `crates/operon-durable`.
- OpenMeter: `openmeterio/openmeter` (GitHub API: license Apache-2.0, pushed 2026-09-28, 2,350 stars; releases `v1.0.0-beta.234` 2026-09-25, `beta.233` 2026-09-18), `README.md` ("Releases are beta and can include breaking changes"), `docker-compose.yaml` (kafka, clickhouse, redis, postgres, svix), `config.example.yaml`. Kong's announcement: konghq.com/blog/news/kong-acquires-openmeter (September 2025); siliconangle.com/2025/09/03/kong-acquires-openmeter-power-api-ai-monetization.
- Lago `getlago/lago` and Flexprice `flexprice/flexprice` (GitHub API: AGPL-3.0; Flexprice v2.1.33 2026-09-25).
- Stripe Billing Meters (read 2026-09-29): docs.stripe.com/billing/subscriptions/usage-based/recording-usage-api (endpoint, idempotency, 35-day/5-minute timestamps, 1,000 calls/s and one concurrent call per customer per meter, v2 meter event streams at 10,000 events/s with 15-minute sessions, error events, dimension limits); docs.stripe.com/api/billing/meter-event/create (`identifier` unique "within a rolling period of at least 24 hours", max 100 characters); docs.stripe.com/api/billing/meter/create (`default_aggregation.formula`, `customer_mapping`, `event_time_window` = `hour`/`day`); docs.stripe.com/billing/subscriptions/usage-based/meters/configure (sum/count/last; pre-aggregated ingestion overwrites within the window; adjustments within 24 hours).
- Clerk Billing: clerk.com/changelog/2025-11-14-clerk-billing-existing-stripe-accounts (production uses your own Stripe account; Clerk syncs users, payment methods and transactions); clerk.com/docs/guides/billing/overview ("Clerk Billing only uses Stripe for payment processing"; not synced to Stripe Billing); clerk.com/billing.
- Linux: `Documentation/admin-guide/cgroup-v2.rst` (`cpu.stat usage_usec`, `memory.current`, `memory.peak`, `cgroup.events populated`); `proc(5)` (`/proc/<pid>/stat` fields 14–15).
- Envoy: `envoy.service.accesslog.v3.AccessLogService`; CloudEvents 1.0 spec (`id` + `source` uniqueness).
