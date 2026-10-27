# 06 — The Meter Record and the Usage Reporter (authoritative; moved from Loams §27 and RN1)

> **Imported on 2026-10-19.** Written in the former private `ostrium-labs/loams-platform` repository; open source (Apache-2.0) in the monorepo since the owner's order of 2026-10-19. "This repository", "proprietary" and "private" below describe that repository and the open-core boundary of the time, which no longer holds; see [the platform docs index](../README.md).

Status: **Proposed** · 2026-10-02. **This document is the authoritative specification of billing-grade metering input** (the meter record, its delivery protocol, and the usage reporter). It is proprietary. It moved here from the open-source repository `ostrium-labs/loams` under the owner's open-core ruling of 2026-10-02 (Loams decisions D540 to D559; [Loams `docs/open-core.md`](https://github.com/ostrium-labs/loams/blob/main/docs/open-core.md)). Together with doc 01 (the node agent and the aggregator), doc 07 (commercial APIs) and doc 05 (the marketplace), it is the metering and commercial-API reference for Loams Cloud.

Numbering: decisions **PD61–PD68**; questions **Q-OC-1..4**. A bare D-number or a "Loams §N" reference points into the open-source repository's design docs.

## 1. The ruling, and why it is a security principle

The owner (2026-10-02): "in open-core, multi-tenant Knative and GitOps using Argo CD is fully open source, so name it as Multitenant BYOC Control Plane with GitOps, and move the commercial API and metering to private, because they may be used to abuse by agents. Integrity is the security principle of Loams."

**Integrity.** The figures that set what a tenant pays, and the APIs that spend money, must not have their producers or validators open to manipulation by agents or tenants. A producer whose wire format, trust rules and acknowledgement protocol are published (and whose code can be run, modified and fed forged input in a tenant's own cluster) hands an attacker a map: forged usage, replayed or under-reported meters, duplicate or reordered host reports, a spoofed `x-loams-usage` header, a scripted loop of paid calls. The open repository therefore carries generic observability (OpenTelemetry metrics, logs and traces) and quota **enforcement** counters, and never billing-grade metering.

| | Open (`ostrium-labs/loams`) | Private (this repository) |
|---|---|---|
| Observability | OTel metrics, logs, traces; cgroup layout and pod labels; Envoy access logs; the gateway-set `x-loams-tenant` | Reads them as untrusted hints; cross-checks against its own trusted sources |
| Metering | Nothing billing-grade | The meter record, host reports, the reporter, the delivery and acknowledgement protocol, the final-read guarantee, reconciliation, the ledger |
| Quotas | The limits record and **enforcement** (token buckets, semaphores, `ResourceQuota`) | **Setting** limits per plan (doc 01 §9), through the open admin API |

## 2. Decisions

| # | Decision | Status |
|---|---|---|
| PD61 | **The meter record is specified here, not in the open repository.** `loams.meter.v1` (`HostReport`, `Invocation` fields 1–16, `HostReportAck`, `SandboxFinished`, `SandboxReadAck`, the framing) is defined in §3 below, moved verbatim from Loams §27 §3.3 and §3.6 and RN1 Rulings 1–5. The open repository no longer publishes the proto, the socket path or the delivery rules | Proposed |
| PD62 | **The producer side is private code.** The host-side reporter (`loams-meter`, formerly RN1 Tasks 1–2), the socket, the sequence and buffer rules and the Lambda usage header are built in `crates/loams-meter` here (the existing `loam-meter` agent crate is renamed with the product rename). It plugs into the open supervisor and runner host through the open **`InvocationObserver`** extension trait (Loams §24 §16, D549): a Rust trait with no wire format and no persistence | Proposed |
| PD63 | **The consumer never trusts a tenant-reachable value.** Cgroup `cpu.stat` (`usage_usec`), read by the node agent from outside the sandbox, is authoritative for CPU totals (doc 01 §5). Memory counters follow doc 01 PD3 (`memory.current` for byte-seconds, `memory.peak` for the peak) and are **not billed by default**; they are read at the same points for the same integrity reasons. Host reports are the per-invocation split and are reconciled against the cgroup totals per window; a window whose host-reported sum exceeds the cgroup total, or falls short of it by more than the tolerance, is flagged and the cgroup total bills. Reports are authenticated by `SO_PEERCRED` on the node-local socket (doc 01 §5.3) | Proposed |
| PD64 | **Anti-replay and anti-forgery rules for the record.** `(host_id, seq)` dedupe is extended with a per-boot `epoch` and a `mac` (fields 4 and 5 of `HostReport`, §3.3): the node agent hands each host process a random 128-bit epoch and a per-boot key over the socket handshake (the peer is identified by `SO_PEERCRED`), and each report carries `HMAC-SHA-256(key, host_id ‖ epoch ‖ seq ‖ canonical invocations)`. A report replayed from another node or an earlier boot, or with a bad `mac` or a backwards `seq`, is rejected. Invocation ids are generated by the host, never by the tenant; the Lambda usage header is stripped from tenant responses and replaced by the platform wrapper's own measurement (§3.2, PD66); cumulative acks never move backwards and never cover a gap (§3.3) | Proposed |
| PD65 | **Provider reconciliation and the usage-event form** stay as doc 01 §13 (PD26–PD28). The CloudEvents form of an `Invocation` (`io.loams.dev.meter.usage.v1`, formerly Loams §27 §3.6) is subsumed by doc 01's PD2 meter event; there is one type | Proposed |
| PD66 | **Q366 (Lambda CPU attribution) is this repository's question.** The default recorded here: meter Lambda by the **billed duration** (`cpu_usec = billed_ms × 1 000 × memory_mb / 1 769`, `cpu_estimated = true`), and keep the bootstrap's `getrusage` delta only as a cross-check gauge. The provider's figure is not tenant-forgeable; the in-sandbox delta is | Proposed (default chosen under "do suggested for all") |
| PD67 | **No billing-grade value is derived from an open-source component's self-report alone.** Every meter has a second source the tenant cannot reach (cgroup, Envoy at the edge, the provider's invoice, Knative's activator at the platform's edge) | Proposed |
| PD68 | **Commercial APIs are private** (doc 07): the hosted Loams Cloud's paid APIs, the marketplace install and billing APIs, partner and commercial integrations. They call the open control plane's admin API (Loams §41) after an entitlement check | Proposed |

## 3. The record (moved verbatim from Loams §27)

The text below is the former Loams §27 §3.2 to §3.3 and §3.6, unchanged in substance. Paths and names use the `loams` prefix. `/run/loams/meter.sock` is now a private-deployment detail, created by `loam-meter` (the node agent). Inside the moved text, "§3.2" means §3.0 here (cgroup layout), "§3.3" means §3.1, "§3.6" means §3.2, and "§5" and "Q-RT-6" point into the Loams repository; the open copy of the cgroup layout (Loams §27 §3.2) keeps the layout and labels and drops the `SandboxFinished` acknowledgement, which is part of the record and lives here.

### 3.0 Cgroup layout and sandbox labels

The runtime places every tenant workload in a cgroup whose path and labels are documented and stable:

| Tier | Cgroup | Tenant from |
|---|---|---|
| T0 workerd (one process per tenant, D171) | `loams.slice/tenant-<org>.slice/workerd.scope` under the supervisor's delegated subtree | the path; the split by namespace and function from the metrics and host reports |
| T1 wasmtime host | `loams.slice/wasm-host.scope` (shared) | per-invocation reports (§3.3), not the cgroup |
| T2 gVisor sandbox (a pod, `RuntimeClass: gvisor`) | the pod's cgroup (`kubepods-…-pod<uid>.slice` or `pod<uid>`) | pod labels `loams.dev/org`, `loams.dev/namespace`, `loams.dev/function`, `loams.dev/tier`, set by the operator |

This refines §24 §7's `loams.slice/tenant-<org>.slice/fn-<id>.scope`: T0 has one process per tenant, not per function, and T2 sandboxes are pods whose cgroups the kubelet creates. With these, a node agent can read `cpu.stat`, `memory.current`, `memory.peak` and `cgroup.events` for every sandbox and attribute them without any engine API. `populated 0` in `cgroup.events` means the sandbox has finished; it does not by itself allow cleanup. For T0 and T1, whose cgroups the supervisor owns, the supervisor then sends a `SandboxFinished` notice with the cgroup path on the host-report socket (§3.3) and **removes the cgroup only after the consumer acknowledges that it has read `cpu.stat` and `memory.peak`**. With no consumer connected, the cgroup is kept for a retention window (default 10 minutes, configurable) and then removed, since nobody is reading. T2 pod cgroups belong to the kubelet, which removes them when it cleans up the terminated pod; how the final reading is taken before that is open (Q-UH-3).


### 3.1 Per-invocation reports from runtime hosts

Where many tenants share a process (T1, and T0's per-request split), the host reports each invocation on a node-local Unix socket, `/run/loams/meter.sock`, as length-delimited protobuf. The socket is created by the consumer and the host connects to it; no consumer means no reports and no cost. Tenants cannot reach it.

```protobuf
syntax = "proto3";
package loams.meter.v1;

message HostReport {
  string host_id = 1;          // stable per host process
  uint64 seq = 2;              // per host_id, monotonic; consumers dedupe on (host_id, seq)
  repeated Invocation invocations = 3;
}

message Invocation {
  string org = 1;
  string namespace = 2;
  string function = 3;
  string version = 4;
  string invocation_id = 5;
  uint64 cpu_usec = 6;         // see "CPU accuracy" below; exact only when cpu_estimated is false
  uint64 fuel = 7;             // 0 when fuel is off
  uint64 epochs = 8;           // epoch ticks consumed
  int64 start_unix_ms = 9;
  int64 end_unix_ms = 10;
  bool cpu_estimated = 11;     // true when cpu_usec was apportioned (T0), not measured
}

message HostReportAck { uint64 seq = 1; }   // cumulative: every report with seq <= this one
```

**Delivery.** An ack for `seq` means the consumer has durably recorded every report of that `host_id` up to and including `seq`. The host keeps unacknowledged reports in a bounded in-memory buffer (default 64 MiB) and, after a socket disconnect, resends them from the oldest unacknowledged `seq` when a consumer reconnects; consumers dedupe on `(host_id, seq)`. When the buffer is full the host drops the oldest reports and counts them in `loams_meter_reports_dropped_total`. A host restart loses its buffer and starts a new `host_id`, so sequence numbers never collide. CPU in lost or dropped reports is not lost from the totals: it is still in the cgroup's `cpu.stat` (§3.2), which is authoritative for totals, and a consumer reconciles the sum of host-reported CPU against it per window.

**CPU accuracy.** On T1 `cpu_usec` is measured: the host reads the thread CPU clock (`CLOCK_THREAD_CPUTIME_ID`) around each poll of the invocation, and `cpu_estimated` is false. On T0 one workerd process serves all of a tenant's invocations, so the per-invocation value is **an estimate**: the tenant's cgroup CPU for each interval is apportioned across the invocations that were running in it (the rule is Q-RT-6), and `cpu_estimated` is true. Per-invocation accuracy on T0 is not bounded by this contract. The 2% check in §5 compares aggregate totals only, not individual invocations.


### 3.2 Usage from external runners (D376)

§24 §16 adds runners outside Loams's nodes (D375): `LambdaRunner`, a thin `WorkersRunner`, and later Cloud Run and Container Apps. They have no cgroup the supervisor owns and cannot reach `/run/loams/meter.sock`. The contract still has one consumer interface:

- **One reporter per invocation.** The supervisor reports its own tiers as above. For an external runner, the **runner host** (the gateway process that called `Runner::invoke`) writes the `HostReport`, with its own `host_id`, from the `Usage` the runner returned. A runner never reports and is never reported twice.
- **Where the CPU comes from.** Lambda: Loams's bootstrap reads `getrusage(RUSAGE_SELF)` before and after each invocation and returns the delta to the runner in a response header the bootstrap owns (`x-loams-usage`); the runner host caps it at the billed duration × the function's fractional CPU share (`memory_mb / 1 769`, the memory at which Lambda allocates one vCPU) and sets `cpu_estimated` when the cap applies; whether Lambda is metered this way or by billed duration is the owner's decision (Q366). Workers (the commercial Cloudflare runner in `loam-platform`, §38 D440; recorded here because it uses this contract): the Tail Worker's `CPUTimeMs` (Cloudflare changelog 2025-04-09). It arrives after the response, so the handoff is asynchronous: `Runner::invoke` returns a `Usage` marked *pending* with the invocation's join key, `InvocationCx.invocation_id`: the host generates it, it is unique per invocation, and `WorkersRunner` sends it to the Worker as the request header `x-loams-invocation-id`. The tenant's Tail Worker, deployed by Loams with the function, reads that header from the trace event and posts the event to the runner host's authenticated usage endpoint; the host joins each event to its pending entry by that id and only then writes the one `HostReport` (`cpu_estimated` false). An entry with no event after a timeout (default 5 minutes) is reported once with `cpu_usec` 0, `cpu_estimated` true and `loams_runner_usage_missing_total` incremented; an event that arrives after its entry was reported, or matches no entry, is dropped and counted (`loams_runner_usage_late_total`), never reported a second time. The pending table is in memory and bounded; on a host restart its entries are lost and counted as missing. The `WorkersRunner` plan fixes the endpoint and timeout (Q373, moved to `loam-platform`, private, with that runner; D440).
- **Additive fields** in `loams.meter.v1.Invocation`; consumers that do not know them ignore them:

```protobuf
  string runner = 12;              // "supervisor", "process", "lambda", "workers", …; empty means "supervisor"
  string region = 13;              // the provider's region for external runners; empty on Loams's nodes
  uint64 provider_billed_ms = 14;  // the provider's billed duration (Lambda's platform report), 0 when none
  uint64 compile_usec = 15;        // CPU Loams spent compiling for this tenant (Cranelift, script compile), not in cpu_usec
  uint64 overhead_usec = 16;       // runtime CPU attributable to the invocation but not to tenant code, where measured
```

- **A CloudEvents form.** Each `Invocation` can also be emitted as the CloudEvent `io.loams.dev.meter.usage.v1` (`id` = `<host_id>:<seq>:<index>`, `source` = `/hosts/<host_id>`, `tenantid` = `<org>/<namespace>`, `data` = the `Invocation` in protobuf), through the high-rate stream path of §34 §3. It is off unless a consumer configures the target stream, so the engine keeps no default dependency (D202). The socket stays the authoritative, acknowledged path. **Not built in this repository** since 2026-10-02: the event form moved to `loam-platform` with the former RN1 Task 6 (§38 D440); the record spec here is unchanged.
- **Totals.** For Lambda and Workers there is no cgroup total to reconcile against; the provider's own figures (billed duration, CPU-ms on Cloudflare's invoice) play that role, and the reconciliation is `loam-platform`'s (D190, D202).

### 3.3 Integrity additions made here (closing gaps the moved text left open)

These rules are authoritative and override the moved text above where they differ.

**Signed epoch (PD64).** `HostReport` gains `bytes epoch = 4;` and `bytes mac = 5;` (additive). The handshake: the host connects, the node agent reads its credentials with `SO_PEERCRED`, maps its cgroup to a sandbox owner, and replies with `epoch` (16 random bytes, new for every host process and every agent boot) and a 32-byte key. Consumers validate `mac` over `host_id ‖ epoch ‖ seq ‖ canonical(invocations)`, reject an unknown `epoch`, a bad `mac` and a `seq` lower than the last acknowledged one, and count each rejection by reason.

**The final read for T2 pods.** The node agent watches every pod cgroup with inotify on `cgroup.events` and reads `cpu.stat` (and the memory counters) when `populated` becomes `0`, before the kubelet removes the cgroup (the kubelet removes it only after the containers have exited and the pod is terminated, which leaves a window of at least the termination grace period). The agent also records `cpu.stat` for every live sandbox every scrape interval (default 15 s) as a floor. If a cgroup disappears before its final read, the window is marked `final_read_missed`, the last periodic reading is used, and the loss is bounded by one interval and recorded on the ledger entry; the count is an alerting metric. A T0 or T1 final read keeps the supervisor's retention window (Loams §27 §3.2) plus the acknowledged `SandboxFinished` flow above.

**Dropped or lost reports fail closed.** The host keeps unacknowledged reports in a bounded **on-disk spool** (default 256 MiB, `fsync` per batch) in addition to the in-memory buffer, so a host restart does not lose them (the new `host_id` after a restart still resends the old spool under its old `host_id` and `epoch`). When reports are dropped (spool full) or lost (spool destroyed), the host emits a `Gap { host_id, from_seq, to_seq, reason }` message when a consumer next connects, and the cumulative ack **never covers a gap silently**: the consumer acknowledges up to the gap, records it, and acknowledges beyond it only after the `Gap`. A tenant whose window overlaps a gap is marked `incomplete` for that window. Incomplete windows are billed from the cgroup total apportioned by the tenant's shares in the last complete window, and queued for review; they are never billed as zero.

**The T0 CPU split is defined here, not in the open repository (replaces the reference to Q-RT-6).** For a tenant's workerd process, the tenant cgroup CPU is sampled every 100 ms. In each sample, the CPU delta is divided among the invocations in flight during that sample in proportion to their wall-clock overlap with the sample (equal shares when they overlap fully); CPU in a sample with no invocation in flight is the tenant's `overhead_usec`. `cpu_estimated = true` for every T0 invocation. Because only the tenant's total is billed on T0 (the apportionment affects the per-invocation view, not what the tenant pays), two implementations that differ in the split cannot change a bill; the split is specified so that per-invocation reports agree across implementations and so that the 2% aggregate check of §4 holds. The Loams repository's Q-RT-6 is non-authoritative for billing.

## 4. The usage reporter (formerly RN1 Tasks 1, 2 and 5)

The reporter is built here, in `crates/loams-meter`, as RN1 specified it, with the following adjustments for integrity.

| Item | Was (RN1, open) | Now (here) |
|---|---|---|
| `meter.proto` and the framing (RN1 Ruling 1, Task 1) | `proto/loams/meter/v1/` in the open repository, under `buf breaking` | `proto/loams/meter/v1/` in this repository; its own `buf breaking` gate; not published |
| `Reporter` (`start`, `record`, `sandbox_finished`, `shutdown`), `ReporterConfig` (Ruling 3: 1 s or 1 000 invocations, 64 MiB buffer, reconnect 100 ms to 10 s), the test `Consumer` (RN1 Task 2) | open crate `loams-meter` | this crate, same API, same tests (`reports_flow_and_are_acked`, `ack_is_cumulative`, `resends_unacked_after_reconnect`, `dedupe_key_is_host_seq`, `no_consumer_costs_nothing`, `stalled_consumer_drops_oldest_and_counts`, `restart_gets_new_host_id`, `sandbox_finished_waits_for_read_ack`, `shutdown_flushes_within_grace`), plus the epoch signature of PD64 |
| `RunnerHost`'s one-reporter rule (Task 3) | in the open `RunnerHost` | the open `RunnerHost` calls the `InvocationObserver` for every invocation; the one-reporter rule is enforced by this repository's observer implementation, which ignores observations from the supervisor (it reads cgroups) and from `RunnerKind::Knative` (pods have cgroups) |
| The Lambda usage header `x-loams-usage` (Ruling 4), the cap (Ruling 5), the `REPORT` line parser (`report_line`), `provider_billed_ms` (Task 5) | `loams-lambda-bootstrap`, `loams-runner-lambda` | the bootstrap and runner crates stay open **without** usage: they deploy and invoke. The header, cap and parser are built here as a wrapper (`loam-runner-lambda-metered`) around the open runner, applying PD66's default |
| `loams_meter_*` metrics | open | here; the open repository exports only generic `loams_runner_*` call metrics |

**Exit gate (from Loams §24 §11, F1).** Host-reported CPU for a test function, summed, matches its cgroup's `cpu.stat` within 2% (an aggregate check, not per invocation). The gate is now this repository's (track MB, MB4), run against the open supervisor with the observer plugged in.

## 4a. Agent-paid (HTTP 402) calls (added 2026-10-02)

If the Monetization Gateway is adopted (doc 07 §5), a settled 402 payment is **revenue evidence, not a meter record**. The usage reporter still reports the call's resources exactly as for any other call; the settlement is recorded in a **separate envelope** joined to the call on `invocation_id`, so the `Invocation` message of §3.1 (fields 1 to 11) and its CloudEvent form are unchanged:

```proto
message PaymentSettlement {   // proto/loams/meter/v1, written by the commercial API, never by a host
  string settlement_id = 1;   // the x402 receipt id; unique, the idempotency key
  string org = 2;             // the seller org the call was served for
  string invocation_id = 3;   // join key to Invocation.invocation_id
  string payer_id = 4;        // stable payer identity (doc 07 PD82)
  string asset = 5;           // e.g. "USDC"
  string amount = 6;          // decimal string in the asset's base unit
  int64 settled_unix_ms = 7;
}
```

Reconciliation matches what the gateway collected (`PaymentSettlement`) against what `loam-usage` rated (`Invocation`). A call served without a settled payment never produces a billable record, and a settled payment with no matching `invocation_id` raises a reconciliation alert. The integrity rules of §1 are unchanged: the reporter authenticates the host, not the payer, and a `PaymentSettlement` never sets a quantity.

## 5. Open questions

| # | Question | Owner | Needed by |
|---|---|---|---|
| Q-OC-1 | Confirm PD66's default (Lambda metered by billed duration) | Founder | Before the Lambda runner is billed |
| Q-OC-2 | ~~Sign host reports with a per-boot key, or rely on `SO_PEERCRED` alone~~ **Decided (PD64, §3.3): per-boot HMAC plus `SO_PEERCRED`.** Whether to move to asymmetric signatures later is open | Eng | MB4 |
| Q-OC-3 | Whether a self-hosting customer with a commercial support contract may run `loam-meter` (a licence question, not a technical one) | Founder | Before the first support contract |
| Q-OC-4 | A third-party audit of the metering path before GA billing | Founder | Before MB5 |
