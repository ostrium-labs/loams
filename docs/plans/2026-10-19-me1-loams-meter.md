# ME1 — Loams Meter (plan stub)

> **For agentic workers:** REQUIRED SUB-SKILL: use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Work task by task, test first. Where this plan gives exact names, paths, branches or tests, use them verbatim. Record any deviation in "Rulings made during execution" at the end of this file.
>
> **Status: Stub** (2026-10-19). Track ME, design [§63](../design/63-loams-meter.md) (D1120–D1138, Q960–Q969), **approved by the owner with all defaults on 2026-10-19**. Owner direction, 2026-10-19: metering and enforcement are different; enforcement is first-class; build a small Rust core, keep the event format compatible with OpenMeter and CloudEvents, and make OpenMeter, Lago and Stripe optional exporters. The 2026-10-19 open-source decision (D1119) moves metering into this repository. The plan code is ME because MT2 is the Knative plan. Each task gets its full steps when its milestone starts.

**Goal:**
- every billable resource has an admission point that can refuse work when an entitlement is exhausted, without a network call on a lease hit;
- usage is counted exactly once and billed only from House windows;
- a tenant with no payment method is hard-stopped at its limit; overage needs a card, an opt-in and a spend cap;
- Stripe, OpenMeter and Lago receive usage when configured, and nothing depends on them.

**Architecture** (§63 §5): `loams-meter-agent` (the node agent imported from `loams-platform`, §63 §21), `loams-meter-core` (types, balance engine, lease allocator; sans-I/O), `loams-meter-gate` (the gate, `MeterSink`, observer adapters, `NoGate`), the `meter` role of `loams` (feature `meter`), House tables in the system namespace's `_meter` database, exporters behind features, and the console plugin `web/plugins/billing`.

## Global Constraints

- **Worktrees and branches:** `~/Documents/Ostriumlabs/loams-wt/me1-<task>`, one branch and one PR per task into `dev` (CodeRabbit reviews; merge on a green `CI required`). `git commit -s`. Never `git stash`, force-push or rebase shared branches.
- **Builds:** the shared target directory, one cargo build at a time, touched crates only (`cargo check -p <crate>`, `cargo nextest run -p <crate>`). Never build in `/tmp`. `fabric/` is its own workspace (D343): `loams-meter-core` and `loams-meter-gate` must build there by path with no Arrow dependency.
- **The dependency runs one way** (D202, D551 as a build rule): products depend only on `loams-meter-gate`'s traits; with `NoGate` a product behaves exactly as before. A product never calls the House or the meter's APIs on its request path.
- **Billing reads only the House** (`_meter.windows`, `_meter.period_totals`). Counters are never exported or shown as billed amounts.
- **Money** is integer micros plus an ISO 4217 code (D374); amounts are fixed-point decimals, never floats.
- **Agent principals can never hold `meter_admin` or `billing_admin`**, and only the billing exporter's service identity may call `SetPaymentState`.
- **No card data** passes through Loams; payment methods are the provider's hosted pages.
- **Nothing is cloned** without the owner's approval. `loams-platform` was cloned with approval on 2026-10-19 and is imported under `platform/` on `import-platform`; ME1 starts after that import and the no-open-core change (D1119) are on `dev`.

## Review Focus

1. **An event is counted twice or lost silently.** Tests: `duplicate_event_counted_once_in_windows`, `reconcile_splits_at_pipe_offset`, `pipe_restart_no_double_count`, `dropped_events_are_counted_metric`.
2. **A hard-limit subject overshoots more than §63 §11.4 allows.** Tests: `hard_limit_overshoot_bounded_10_nodes`, `owner_failover_learning_mode`, `exact_mode_near_zero`.
3. **A free tenant is served when the meter is down.** Test: `free_fails_closed_when_owner_unreachable`. A paid tenant fails open only within its cap: `paid_fail_open_bounded_by_cap`.
4. **A tenant or agent forges usage or grants.** Tests: `reserved_types_refused_from_tenant_credentials`, `subject_overwritten_from_credential`, `agent_cannot_write_grants`.
5. **The gate adds latency.** Bench: `gate_check_p99_under_5us` (64 threads).
6. **An exporter double-bills on retry.** Tests: `stripe_identifier_stable_across_retries`, `reclosed_window_sends_adjustment`.

## Execution order

| Order | Milestone | Tasks | Target (estimate) |
|---|---|---|---|
| 1 | ME1a events and emitters | 0–4 | 2026-11 |
| 2 | ME1b storage and windows | 5–8 | after HS1b and HS1c |
| 3 | ME1c entitlements, counters, reconciliation | 9–15 | 2026-12 |
| 4 | ME1d enforcement and policy | 16–23 | 2027-01 |
| 5 | ME1e exporters, console, migration | 24–31 | 2027-01 to 02 |
| 6 | ME1f user meters and provisioning | 32–34 | 2027-02 |

---

## ME1a — Events and emitters

- **Task 0: Reconcile.** Check §63 against `dev` (D1119 merged; HS1 Task 16's pipe column names; D365's emitter built or not; GW1's observer). Record rulings.
- **Task 0b: Move the node agent.** `platform/crates/loams-meter` → `crates/loams-meter-agent` in the root workspace (reqwest 0.13, base64 0.23), crate and binary renamed; remove the emptied `platform/` workspace and its CI job. Behaviour unchanged; its tests move with it.
- **Task 1: `loams-meter-core` types and event validation.** `Event`, `Meter`, `Feature`, `Entitlement`, `Grant`, `Balance`, `Money`; the Loams usage profile on top of `loams-cloudevents` (`subject` required, JSON data, reserved types, `loamslease`). Tests: `usage_event_requires_subject`, `openmeter_schema_accepts_usage_event`, `reserved_type_prefix_detected`.
- **Task 2: `MeterSink`**: best-effort mode (D365's batched plain produce if not built yet: bounded queue, 50 ms or 1 MiB flush, drop-oldest with `loams_events_dropped_total`) and spooled mode (the agent's `Spool`, baselines advance only after the append). Tests: `sink_never_blocks`, `sink_drops_oldest_when_full`, `retry_keeps_ids`, `spooled_full_keeps_usage_in_counters`.
- **Task 2b: The agent's events and sink** move to §63 §6: types `io.loams.dev.usage.node.{cpu,memory,network}.v1`, `subject` and `tenantid`, D270's binary-mode record layout, key = `subject` with the stream's default partitioner. Tests: `agent_event_passes_usage_profile`, `agent_records_rebuild_as_cloudevents`, `agent_partition_matches_counter_owner`.
- **Task 3: Observer adapters** for `UsageObserver` (House), `ConnectionObserver` (gateway), `ComputeLifecycleObserver` plus a 60 s ticker (pg-control); deterministic ids per §63 §6.1. Tests: `house_query_event_id_is_stable`, `compute_tick_ids_dedupe_after_restart`.
- **Task 4: the `_system/_meter` stream** created by the `migrate` service (§54) in `_system`, 32 partitions, keyed by `subject`; JSON Schemas in `docs/api/meter/`. Test: `meter_stream_created_once`.

## ME1b — Storage and windows

- **Task 5: House tables and pipe** (`_meter.events` `ReplacingMergeTree`, the `LoamsStream` source and MV). Test: `duplicate_event_counted_once_in_windows`.
- **Task 6: Close job** (minute windows, roll-ups, recompute for non-additive aggregations, late re-close, versioned rows). Tests: `close_job_idempotent`, `late_event_recloses_window`, `unique_count_not_rolled_up`.
- **Task 7: Period totals and period close** at end + 2 h, with each customer's anchor. Test: `period_closes_after_grace`.
- **Task 8: `QueryMeter` and the OpenMeter query path** (`/api/v1/meters/{slug}/query`). Test: `query_marks_provisional_open_window`.

## ME1c — Entitlements, counters, reconciliation

- **Task 9: Balance engine** (burn-down order, rollover, resets, prepaid stops at zero), property-tested against brute-force replay and OpenMeter's documented examples.
- **Task 10: `CustomerService`, `EntitlementService`, `PlanService`** with OpenFGA relations `meter_admin`, `billing_admin`, `billing_viewer`; audit events. Test: `agent_cannot_write_grants`.
- **Task 11: Plans as YAML** applied through `PlanService` from the GitOps reconciler (§63 §22). Test: `meter_semantics_immutable_once_used`.
- **Task 12: Counter owners** on `_system/_meter` partition leases; recent-keys dedup; offset buckets. Test: `owner_rebuild_from_house_and_tail`.
- **Task 13: Reconciliation** every 5 minutes and at period close. Tests: `reconcile_splits_at_pipe_offset`, `drift_alert_above_one_percent`.
- **Task 14: Threshold and lifecycle events and the notifier** (webhooks signed, email, console). Test: `threshold_event_reemission_is_duplicate`.
- **Task 15: OpenMeter conformance test** against a pinned `api/openapi.yaml`.

## ME1d — Enforcement and policy

- **Task 16: Lease allocator** (sans-I/O; chunk formula, exact mode, learning mode) with `loams-detsim` runs of node kills and partitions.
- **Task 17: `LeaseService`** (internal Connect, service identities only, fenced owner epochs). Test: `stale_epoch_grant_ignored`.
- **Task 18: `Gate`** (`check`, `reserve`, `settle`; sharded atomics; background refresh). Bench: `gate_check_p99_under_5us`.
- **Task 19: Policy table and payment states** (§63 §13). Tests: `free_fails_closed_when_owner_unreachable`, `paid_fail_open_bounded_by_cap`, `overage_requires_card_and_cap`.
- **Task 20: House admission** calls the gate after the protective quotas; error mapping. Test: `house_entitlement_refusal_names_feature`.
- **Task 21: Gateway `pg` module and pg-control wake** (SQLSTATE `53400`; suspend at next idle or after 15 minutes). Test: `hard_stop_suspends_compute_after_grace`.
- **Task 22: Jobs, functions, streams, Git, sandbox** call sites.
- **Task 23: Gateway `http`, `llm`, `mcp` call sites** as each module lands (402 with reasons; LLM reserve and settle). Test: `llm_reserve_settles_actual_tokens`.

## ME1e — Exporters, console, migration

- **Task 24: Stripe exporter** (closed hour windows, `identifier`, adjustments; payment-state webhooks and daily poll). Tests: `stripe_identifier_stable_across_retries`, `reclosed_window_sends_adjustment`.
- **Task 25: OpenMeter exporter** (raw events, `loamslease` stripped).
- **Task 26: Lago exporter** (HTTP only) and the CloudEvents webhook exporter.
- **Task 27: `web/plugins/billing`**: Usage, Entitlements, Spend controls, Payment, Alerts; the 80%/100% banner. Uses `@loams/ui` and the CF1 shell.
- **Task 28: Operator pages**: Meters, Plans, Customers, Meter health.
- **Task 29: The T1 source**: the wasmtime host-report socket and reporter from platform doc 06 (PD61–PD64: `SO_PEERCRED`, per-boot epoch and HMAC, `Gap` records), reconciled per window against the cgroup total. Test: `host_report_split_never_exceeds_cgroup_total`.
- **Task 30: Kubernetes resolver and final reads** for the agent (platform MB2: pod watch, inotify on `cgroup.events`), the DaemonSet chart; translate `dev.loams.meter.v1` events for one release. Test: `churn_1000_pods_within_1_percent`.
- **Task 31: Retire the no-metering naming rules** in JB1, CP1 Task 27 and §52, and record D1134's amendments in those documents.

## ME1f — User meters and provisioning

- **Task 32: User meters** scoped to a namespace (§63 §9.6).
- **Task 33: OpenTofu provider** (`loams_meter`, `loams_feature`, `loams_plan`, `loams_customer_plan`).
- **Task 34: Docs, runbooks and the exit report.**

## Rulings made during execution

| # | Ruling | Reason |
|---|---|---|
| D2220 | Task 27: the billing plugin runs on a fixture `BillingClient` under Demo data until `loams.meter.v1` is served | The pages are built before ME-svc's services; swapping the client touches no page |
| D2221 | Task 27: Settings › Billing lives at `/settings/billing/:section`; Payment links only to the provider's hosted page | D1080; no card data in Loams |
| D2222 | Task 27: the banner is a `shell.topbar.center` entry, most severe notice first; stops cannot be dismissed | §18, §13 |
| R24.1 (D2210) | Task 24: the exporters are the crate `crates/loams-meter-export` with a feature per provider; the `meter` role's `export-*` features enable them. Inputs are the crate's own `Quantity` and `ExportLedger` until `loams-meter-core` merges | Provider clients stay out of the role unless an exporter is on; the exporters do not wait for the role's crate |
| R24.2 (D2211) | Task 24: Stripe identifier `loams:<meter>:<subject>:<hour>:<rev>`, `<rev>` = highest `close_run` through the hour, `-<n>` for a new value at the same revision, hashed past 100 characters; ledger write before the request | A re-closed earlier hour changes later hours' overage; retries keep the identifier |
| R24.3 (D2212) | Task 24: payment state from the customer's default payment method only, after a verified webhook or the daily poll; only ids read from webhook bodies; `invoice` never sent | No card data in Loams; webhooks are hints |
| R25.1 (D2213) | Task 25: OpenMeter export from `_meter.events` rows, `loamslease` dropped, batches per partition in offset order, own cursor per partition moved after the accept | OpenMeter's dedup window is off by default |
