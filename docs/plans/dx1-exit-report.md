# DX1 exit report: the Temporal gateway MVP

Task 11 of [plan DX1](2026-10-21-dx1-durable-execution-compat.md), track DX, design
[§74](../design/74-durable-execution-compat.md). Written 2026-10-27 at `dev` `0e9c0e8fb`. Its rulings
are X11.R1 onward in the plan's table. Every number below was measured on 2026-10-27 on this
machine unless the line cites another run.

## Summary

- **§74 §13.3's DX1 bar is met on loopback.** The official Go SDK v1.49.0 and TypeScript SDK
  1.24.0 ran hello (one activity), a signal and the seven other DX1 corpus scenarios against
  `loams-temporal`. Each SDK's replayer accepted all nine of its histories, and all eighteen match
  the corpus after normalisation. The SDK legs passed **26 of 26** cases, with an empty
  expected-failure list. `capabilities_match_impl` is green.
- **Conditional go for DX2.** Two defects should be fixed first, at the start of DX2 and before
  any user is pointed at the gateway (§4): the history long poll for a result can end without a
  page token, so the Go SDK fails on runs that outlast the poll; and three of X1.R7's limits are
  defined but not enforced. Neither changes the design.
- **What DX1 did not deliver as planned:** the compose profile `temporal-gateway` (it needs a
  gateway image), any CI run of the SDK legs (their nightly lands with DX0 Task 7), and the switch
  from the interim Resonate adapter to `resonate-client`. FW3, FW7 and FW8 have merged in the
  fork, but Loams does not pin them yet.

## 1. What DX1 delivered

| Task | Delivered | PR |
|---|---|---|
| 0 Rulings | X1.R1–X1.R9 | #1203 |
| 1 `loams-temporal-proto` | `temporalio/api` v1.63.5 vendored (123 `WorkflowService` RPCs), licence and NOTICE | #1204 |
| 2 `loams-temporal-testkit` | Corpus loader, a Rust normaliser that agrees with `normalize.py`, the differ | #1210 |
| 3, 4 `loams-temporal-history`, `-statemachine` | The history builder, `fold`, `Limits`; the pure `apply`, with golden tests over the DX1 corpus | #1212, #1218 |
| 5 `loams-temporal-store` | `MemStore` and `PgStore`: compare-and-set commits, the outbox, 64 shards | #1208 |
| 6 `loams-temporal-resonate` | The `Durability` port, `FakeDurability`, the interim `HttpDurability`, IDs (X1.R13), the relay | #1209 |
| 7 `loams-temporal-matching` | Long polls with a claim from Resonate before every hand-out | #1219 |
| 8 `loams-temporal-frontend` | The 21 DX1 methods, task tokens, `StaticKeys`, `UNIMPLEMENTED` with the matrix link | #1220 |
| 9 `loams-temporal` binary and the gateway's `temporal` module | `--dev` and `--config`; `loams-gateway --module temporal` | #1227 |
| 10 SDK legs | `conformance/compute/temporal/gateway/{run.sh,compare.py,xfail.toml}` on loopback | #1228 |
| 11 Matrix, guide, this report | `scripts/compute/matrix.py`, [`docs/guides/durable/temporal-gateway.md`](../guides/durable/temporal-gateway.md), an API-key test | this PR |

## 2. Results

### 2.1 The SDK legs (Task 10's `run.sh`)

Run on 2026-10-27 at `dev` `0e9c0e8fb`: `loams-temporal` (debug build) in development mode
(memory store, in-process Resonate) on loopback, with the corpus's generators in the pinned
`golang:1.26.8` and `node:22.23.2` images. The run took 2 min 17 s.

| Cases | Result |
|---|---|
| `gateway_dx1_scenarios_{go,ts}` | 2 of 2 passed |
| `gateway_hello_{go,ts}`, `gateway_signal_{go,ts}` | 4 of 4 passed |
| `gateway_histories_replay_{go,ts}` | 2 of 2 passed (9 histories per SDK) |
| `gateway_dx1_differential_<sdk>_<scenario>` | 18 of 18 passed |
| **Total** | **26 of 26**; `xfail.toml` has no entries |

The JUnit is committed as `conformance/compute/temporal/gateway/last-run.junit.xml`, and the
guide's matrix is generated from it (`matrix_guide_current` checks that they agree).

**Real Resonate.** This run did not use a Resonate server: building the fork's server binary on
this shared machine was left out. The real-server evidence is X1.R21 (2026-10-25): the same 26
cases passed with `loams-temporal` on the interim adapter against the fork's server (`loams/main`
`f419b8be`, SQLite).

**Which methods the SDKs call.** For this run a temporary request log (not committed) recorded
every `WorkflowService` call: 301 calls to 18 methods. 16 are DX1 methods, and two are
unimplemented methods the SDKs tolerate: `ListWorkflowExecutions` (18, the Go generator's lookup,
which falls back to the current run) and `ShutdownWorker` (2, one per SDK at worker shutdown).
Five DX1 methods were never called: `ListNamespaces`, `RespondWorkflowTaskFailed`,
`RespondActivityTaskFailed`, `RespondActivityTaskCanceled` and `ResetStickyTaskQueue`. The
TypeScript SDK called neither `RegisterNamespace` nor `ListNamespaces`. `methods.toml` records
this, and the matrix shows these cells as "not exercised".

### 2.2 The crates' own tests

`cargo test` (2026-10-27, `-j 2`), all green:

| Crates | Tests | Notes |
|---|---:|---|
| `loams-temporal-frontend` | 14 | `capabilities_match_impl`, `gateway_dx1_scenarios_match_corpus`, the method tests, the standalone tests and the new `api_keys_authenticate_and_bind_the_loams_namespace` |
| `loams-temporal-{proto,testkit,history,statemachine,store,resonate,matching}` | 94 | `golden_dx1`, `fold_corpus_dx1`, `normalize_agrees_with_python`, `corpus_binpb_roundtrip`, `statemachine_has_no_io_deps`, the matching and relay tests |

Two caveats. The `profile_*` tests return early without `LOAMS_TEMPORAL_TEST_RESONATE_URL`, so
locally they checked nothing; their real-server evidence is Task 6's PR and X1.R21. `PgStore` ran
only in CI's `temporal-store` job (stock Postgres 17), not here.

**CI.** The SDK legs do not run in CI. Their nightly lands in `compute-compat.yml` with DX0 Task 7.
The crates' tests run in `Rust: tests`, but the `CI required` checks of #1220 and #1228 were red
when they merged, so CI is not independent evidence for this report. All the evidence above is
from local runs.

## 3. The DX1 exit criteria

| Criterion (§74 §13.3; plan DX1 Task 11) | Verdict | Evidence |
|---|---|---|
| Hello world, an activity and a signal pass on Go and TypeScript | **Met** (loopback) | `gateway_hello_{go,ts}`, `gateway_signal_{go,ts}`; activities in hello and in `two_activities_sequential`, `activity_parallel_fanout_10` and `activity_heartbeat_details` |
| The replayers accept the gateway's histories | **Met** | `gateway_histories_replay_{go,ts}`: 18 histories |
| The DX1 differential is green | **Met** | 18 of 18 `gateway_dx1_differential_*`; in process, `gateway_dx1_scenarios_match_corpus` and `golden_dx1` |
| `capabilities_match_impl` is green | **Met** | Local run; it calls all 123 methods and checks `IMPLEMENTED` against §5.8's DX1 list |
| "With only a host change and an API key" (the plan's DX1 goal) | **Met in process; not run by the SDK legs** | The legs use `insecure_dev` (no key). `api_keys_authenticate_and_bind_the_loams_namespace` (added here) checks the `[[keys]]` path over gRPC: no key or an unknown key gets `UNAUTHENTICATED`, a key sees only its own Loams namespace, and only `admin` registers namespaces. Before this PR nothing tested `StaticKeys` |
| The interim adapter is replaced by `resonate-client`, or listed with FW3's state | **Met by listing** | §5 |
| `PgStore` with `HttpDurability` against the engine's Resonate (Task 9's exit) | **Partly met** | `PgStore` passes its contract tests on Postgres 17 in CI. The SDK legs used the memory store, and X1.R21 used the fork's server, not the engine's listener. The fork's bench ran the gateway on Postgres (resonate#26) and found the bug in §4.1 |
| The compose profile `temporal-gateway` (Task 10) | **Not met** | Not in `deploy/release/compose.compute.yaml`; it needs a gateway image |

## 4. Open issues

These issues are not filed yet: this session's tools could not create issues, so the owner or the
orchestrator should file them from the text here.

1. **The history long poll can end without a page token (bug, blocks real use).** The fork's
   bench (ostrium-labs/resonate#26) found that the Go SDK fails with "could not get last history
   event for workflow" on burst, chain and repeated fan-out loads. In `get_workflow_execution_history`
   (`crates/loams-temporal-frontend/src/service.rs`), when a close-event wait (`wait_new_event`,
   `HISTORY_EVENT_FILTER_TYPE_CLOSE_EVENT`) hits its deadline (60 s, or 5 s before the request's)
   before the run closes, the gateway answers an empty history and copies the request's page
   token. On the client's first call that token is empty, so the SDK takes the empty answer as
   the end of a history with no close event. The fix is to always return a non-empty token
   there, with a test of a run that stays open past a short `history_wait`. The DX1 scenarios
   finish in seconds, so the legs do not hit this.
   *Follow-up (2026-10-27):* fixed in #1291 (DX1F.R1; DX1F.R3 records that the store's
   compare-and-set path is not involved), with regression tests. The bench has not been re-run.
2. **X1.R7's limits are only partly enforced.** `loams_temporal_history::Limits` defines all of
   them, but only the 4 MiB request (the transport) and the 2,000 signals per run are checked. The
   2 MiB payload, the 50 MiB and 51,200-event history and the 2,000 pending activities are not,
   so one run's history can grow without bound.
   *Follow-up (2026-10-27):* #1291 enforces them as Temporal v1.32.1 does (DX1F.R2), with one
   unit test per limit; no SDK run has exercised them.
3. **The compose profile `temporal-gateway` and a gateway image** (Task 10's compose half).
4. **No `/metrics` endpoint.** The relay's counters (`loams_temporal_outbox_*_total`) are
   rendered, but neither `loams-temporal` nor the gateway module serves them.
5. **Untested DX1 methods:** `ListNamespaces`, `RespondActivityTaskCanceled` and
   `ResetStickyTaskQueue` have no gRPC test. `RespondWorkflowTaskFailed` is tested only in the
   state machine. Each needs a case in `tests/methods.rs`.
6. **The SDK legs' nightly** (`compute-compat.yml`, with DX0 Task 7). It should include a leg on
   a real Resonate server (`LOAMS_RESONATE_BIN`) and one on `PgStore`.

## 5. The fork: FW3, FW7 and the interim pieces

Checked on 2026-10-27 through the GitHub API on `ostrium-labs/resonate` (`loams/main` at
`f29c04e1`, 2026-10-25) and its `CARRIES.md`. `loams-wt/.he0-shared/resonate-bench.md` is not on
this machine, so the fork's own records stand in for it.

| Fork item | State in the fork | State in Loams |
|---|---|---|
| FW3 `resonate-client` and the public protocol entry | Merged into `loams/main` (#4, 2026-10-24); not offered upstream | **Not pinned.** `loams-temporal-resonate` still talks to Resonate through `wire_interim` (`HttpDurability`); the workspace pins the fork at `37b177cd` for `loams-durable` only |
| FW7 protocol profile and scenario tests | Merged (#3, 2026-10-24); fixes merged (#8, 2026-10-25): Postgres accepts IDs with several `:` (upstream resonatehq/resonate#1195), and the SQL stores honour `resonate:delay` (#1196) | Not pinned. X1.R11 (2) and (5) and X1.R14's `DelaySupport::Defer` are still how the gateway behaves; X1.R13's one-`:` grammar makes the first moot |
| FW8 poll groups with many consumers | Merged (#9, 2026-10-25) | Not needed while DX1 runs one replica (X1.R8) |
| FW11 timer lateness under load | Merged (#23); upstream #1198 open | Not pinned. SQL-store timers fired up to tens of seconds late before it |
| FW5 retention | WIP on `fw5/retention`, no PR | X1.R16 (lazy settlement of a closed run's promises) stays |
| FW6, FW9, FW4 | WIP, not started, not built | DX3 |

**Interim pieces that remain:** `wire_interim` and `HttpDurability` (D1591); the relay's deferral of
delayed creates (X1.R14); lazy settlement of a closed run's promises (X1.R16); the callback gap
after a lost poll connection (X1.R19); the loopback-only path to Resonate (D111). The pin bump to a
`loams/main` revision with FW3, the FW7 fixes and FW11 is one PR: replace `wire_interim` with
`resonate-client`, report `DelaySupport::Native`, and run the `profile_*` tests and the SDK legs on
the real server before and after the bump.

**Update, 2026-10-28 (D2040, D2043; X1.R22, X1.R23):** the FW3 bump has landed in two steps. #1292
moved the requests onto `resonate-client`. The follow-up moved the poll subscription onto the
client's new poll consumer (fork #28), deleted `wire_interim`, and moved every Resonate crate,
`loams-durable` included, to the fork's `53d38ab3` together (question 3 below, decided by the
coordinator). `DelaySupport::Native` stays opt-in (D2042). The `profile_*` tests and the SDK legs
have not yet been rerun on the real server after the bump.

## 6. Go or no-go for DX2

**Go, with two conditions.** DX2's work should start with:

1. the history long-poll fix (§4.1), with its test, before the gateway is offered for anything
   longer than the corpus scenarios; and
2. the remaining X1.R7 limits (§4.2), because DX2's continue-as-new and retries make long
   histories normal.

The pin bump to FW3 (§5) and the SDK legs' nightly (§4.6) should land early in DX2, so that DX2's
differential harness in CI (the plan's DX2 row) runs on the real client from its first day. DX2's own gate
also needs FW5 for retention, which is still WIP in the fork.

Compat mode stays the recommended path for Temporal workloads until an SDK passes DX2's exit (§74
§13.3). DX0's compat mode is itself not go yet (#1185).

## 7. Questions for the owner

1. **Issues:** may the two defects in §4.1 and §4.2 be filed and fixed as the first DX2 tasks, or
   should they be fixed as a DX1 follow-up before DX1 is called done?
2. **The compose profile:** the gateway image is needed for `temporal-gateway`. Should it be built
   as part of the release images (a release-file change, which DX1 stayed out of) or as a
   conformance-only image?
3. **The FW3 pin bump:** the bump would move every Resonate crate Loams pins (`loams-durable` too)
   from `37b177cd` to a `loams/main` revision. Should it move them together, or should
   `loams-temporal-resonate` pin `resonate-client` on its own revision first?
4. **CI for the legs:** the legs need Docker and about 2.5 minutes. Should they run as a
   path-filtered pull-request job, not only nightly, while the gateway changes often in DX2?
