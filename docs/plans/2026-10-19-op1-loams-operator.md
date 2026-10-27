# OP1 — Loams Operator: the Minimum Viable Slice and Loams Operator Conformance 0.1 (plan stub)

> **For agentic workers:** REQUIRED SUB-SKILL: use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Work task by task, test first. Where this plan gives exact names, paths, branches or tests, use them verbatim. Record any deviation in "Rulings made during execution" at the end of this file.
>
> **Status: Stub** (2026-10-19). Track OP, design [§65](../design/65-loams-operator.md) (D1172–D1201, Q984–Q999, approved by the owner on 2026-10-19 with every default). Owner request, 2026-10-19: **"a single operator for the entire Loams platform, with HA. Only design it; we can build it slowly. It should have a conformance suite that inspires all other operators."** This plan covers OP0 (the spike) and OP1 (the minimum viable slice) of §65 §14. OP2 (native PD and TiKV), OP3 (the other components, Day-2) and OP4 (LOC 1.0) get their own plans when their gates are met (D1201). **Nothing here starts before v0.1.0 ships (Q996).** Each task lists its files, the tests that must exist and pass, and its gate. Interfaces are fixed in §65; Task 0 of each milestone checks the code as it stands and records rulings.

**Goal:**
- one `LoamsCluster` object brings up the engine, an object store, PD and TiKV (through tidb-operator v2 or external endpoints), the gateway and tenant namespaces, in order, on kind and on a real three-zone cluster;
- the operator runs two or three replicas, and killing, pausing or partitioning its leader at any step never takes two members of a group down or repeats a harmful step;
- nothing it renders is public; it holds no `pods/exec` and no cluster-wide Secret listing;
- Loams Operator Conformance 0.1 exists as a spec, a harness and a Loams adapter, and Loams passes LOC Core and the `LOC-HA-*` requirements in CI.

**Architecture** (§65 §6): three crates, none in the `loams` binary's default build. `loams-operator-api` holds the CRD types; `loams-kube` holds leader election, the fences, the operation journal, server-side apply, conditions, events, metrics and drift detection; `loams-operator` holds the controllers, the PD/TiKV drivers `external` and `bridge`, the webhooks and `crdgen`. LOC lives under `conformance/operator/` as `loc-spec`, `loc-harness` and `loc-adapter`.

**Tech stack:** Rust; `kube` and `kube-runtime` at one workspace version (4.2.0 is current, `dev` pins 3.1.0; Task 0 rules), `k8s-openapi`, `schemars`, `tokio`, `rustls`, `rcgen` (CP1's CA); kind and k3d in CI; tidb-operator v2 at a pinned release (v2.0.1 or newer stable) for the bridge, its CRD types generated with kopium.

## Global Constraints

- **Worktrees and branches.** `~/Documents/Ostriumlabs/loams-wt/op1-<task>`, one branch and one PR per task into `dev` (CodeRabbit reviews; merge on a green `CI required`). `git commit -s`.
- **Builds.** Shared target directory, one cargo build at a time, touched crates only; never build in `/tmp`. kind and k3d clusters run only between builds.
- **The default build does not change.** Test `default_features_exclude_operator`: `cargo tree -p loams -e normal` on default features lists none of `loams-operator`, `loams-kube`, `loams-operator-api`, `loc-harness`.
- **Licences.** PD and TiKV procedures ported from tidb-operator v2 (Apache-2.0) carry the header `Ported from pingcap/tidb-operator@<sha>:<path>, Apache-2.0` and a `NOTICE` line; no code from AGPL, GPL, BSL, SSPL or OSL projects (Stackable's product operators are OSL-3.0). `cargo deny check` passes with no new allow entry.
- **No exec.** No code path calls `pods/exec` or `pods/attach`; test `rbac_has_no_exec` reads the chart's rendered RBAC.
- **Never public** (D1195). Test `no_public_service_rendered` renders every component for every exposure mode and greps for `LoadBalancer` without an internal annotation, `NodePort`, `Ingress` and routes.
- **Secrets.** Specs carry references only; test `no_secret_value_in_status_events_logs` captures status, events, logs and metrics for the whole OP1 suite and greps for every generated secret.
- **Pins.** Images by digest from the release pins; new crates and tools at releases at least 14 days old, except security fixes.
- **GitHub-hosted runners only.** HA runs nightly; heavy cells may use the `loam-bench` runner.

## Review Focus

1. **Two members of one quorum group down at once by the operator's hand.** Expected: never, even with two leaders. Tests: `disruption_lease_is_exclusive`, `stale_leader_cannot_take_disruption_lease`, LOC `LOC-HA-005`.
2. **A paused leader acts after losing its lease.** Expected: its writes fail on resourceVersion or fence, and it exits. Tests: `self_fence_after_missed_renewal`, `journal_write_with_old_resource_version_conflicts`, LOC `LOC-HA-002`.
3. **A crash mid-operation repeats or skips a step harmfully.** Expected: the next leader resumes the same plan from observation. Tests: `journal_resume_each_phase`, `phase_derived_from_observation`, LOC `LOC-HA-003`.
4. **An operator restart or upgrade restarts managed Pods.** Expected: never without a release change. Tests: `restart_causes_no_pod_restart`, LOC `LOC-REC-002`, `LOC-UPG-003`.
5. **Drift is ignored or fights another controller.** Expected: owned fields reverted with an event; foreign fields left alone. Tests: `owned_field_drift_reverted`, `foreign_annotation_kept`.
6. **An object store without conditional writes is accepted.** Expected: refused. Tests: `probe_refuses_no_conditional_put`, `probe_degraded_without_versioning`.
7. **Anything public, any exec, any secret value leaked.** Expected: never. Tests: the three global-constraint tests above.

## Milestones and tasks

### OP0 — Spike (Tasks 0–5)

| Task | Content | Files | Tests | Gate |
|---|---|---|---|---|
| 0 | Rulings: the `kube` version for the workspace (4.2.0 or the pin), the Kubernetes floor (Q997), what CP1 Task 15 has built of `crates/loams-operator`, the tidb-operator v2 sha and its PD/TiKV file paths and defaults (§65 §13), whether CP1's cluster CA exists | this file | — | Rulings recorded |
| 1 | `loams-operator-api`: `LoamsCluster`, `ObjectStore`, `LoamsInstance`, `LoamsEngine`, `LoamsPD`, `LoamsTiKV`, `LoamsGateway` types at `loams.dev/v1alpha1`; `crdgen`; CRDs checked in | `crates/loams-operator-api/`, `deploy/helm/loams-operator/crds/` | `crds_match_types` (drift), `cel_rules_reject_even_pd`, `cel_rules_reject_public_exposure`, `immutable_api_version_enforced` (envtest-style on kind) | — |
| 2 | `loams-kube` leader election: Lease acquire, renew, release, self-fence on a monotonic clock; client-go timings | `crates/loams-kube/src/lease.rs` | `single_leader_under_contention`, `self_fence_after_missed_renewal`, `lease_transitions_increase` | — |
| 3 | The disruption Lease and the fence token; the operation journal type with a fixed plan and derived phases | `crates/loams-kube/src/{fence.rs,journal.rs}` | Review Focus 1–3 unit tests; detsim seeds `journal_resume_each_phase` | — |
| 4 | `loc-spec`: `loc-spec.toml` 0.1 from §65 §12.4 (IDs, levels, classes, text, test names) and its Markdown rendering | `conformance/operator/loc-spec/` | `spec_ids_unique`, `spec_renders_markdown` | — |
| 5 | `loc-harness` skeleton: kind cluster with audit logging and zone labels; pod-kill and stale-leader (`SIGSTOP`) injectors; the `OperatorAdapter` trait; report JSON v0; a toy adapter over a sample controller | `conformance/operator/loc-harness/`, `conformance/operator/loc-adapter/` | `harness_runs_core_on_toy_adapter`, `stale_leader_injector_pauses_holder`, `report_schema_valid` | **Spike gate:** the toy adapter passes `LOC-REC-001` and fails a deliberately broken variant; otherwise rethink the harness before OP1 |

### OP1 — The minimum viable slice (Tasks 6–21)

| Task | Content | Files | Tests | Gate |
|---|---|---|---|---|
| 6 | Rulings for OP1; the operator binary, chart skeleton, RBAC and VAPs (namespace prefix, no public Service types) | `crates/loams-operator/`, `deploy/helm/loams-operator/` | `rbac_has_no_exec`, `rbac_no_cluster_secret_list`, `vap_refuses_foreign_namespace`, `vap_refuses_public_service` | OP0 spike gate passed |
| 7 | Conditions, reasons registry and events in `loams-kube`; `docs/api/operator-reasons.md` | `crates/loams-kube/src/{conditions.rs,events.rs}`, `docs/api/operator-reasons.md` | `reason_in_registry`, `ready_rollup_from_components`, `event_before_and_after_disruptive_step` | — |
| 8 | Server-side apply helpers, drift detection and `loams.dev/paused` | `crates/loams-kube/src/{apply.rs,drift.rs}` | Review Focus 5; `paused_stops_writes` | — |
| 9 | `ObjectStore` with the `s3` and `rustfs` providers and the capability probe Job (D1190) | `crates/loams-operator/src/objectstore/` | Review Focus 6; `rustfs_rendered_with_versioning` | Q999 |
| 10 | `LoamsPD`/`LoamsTiKV` `external` driver: endpoints, TLS material, health, status mapping | `crates/loams-operator/src/tikv/external.rs` | `external_pd_health_maps_to_quorum_condition` | — |
| 11 | `bridge` driver: kopium-generated `core.pingcap.com/v1alpha1` types at the pinned sha; render `Cluster`, `PDGroup`, `TiKVGroup` with `api-version = 2`, `enable-ttl`, location labels; map status | `crates/loams-operator/src/tikv/bridge/` | `bridge_renders_no_tidbgroup`, `bridge_invariants_immutable`, `bridge_status_mapped` | Q985 |
| 12 | `LoamsEngine`: roles as Deployments, `meta` instances with the `raft` metastore (one at a time), metastore wiring for `tikv`, the `migrate` Job before a release change | `crates/loams-operator/src/engine/` | `migrate_runs_before_roll`, `meta_rolls_one_at_a_time`, `engine_roles_spread_across_zones` | — |
| 13 | `LoamsGateway`: Deployment ×2, PDB, drain, internal exposure only | `crates/loams-operator/src/gateway.rs` | `gateway_internal_lb_only`, `gateway_pdb_min_available` | Q992 |
| 14 | `LoamsNamespace`: CP1 Task 15's controller moved in (or built here), unchanged behaviour | `crates/loams-operator/src/namespace/` | CP1's `objects_golden_per_product`, `provision_called_once_per_generation`, `not_ready_until_all_products_ready` | CP1 Task 15 |
| 15 | `LoamsCluster`: owns component objects; release resolution from the pin files; the component order of §65 §8.8 for the OP1 components; `UpgradeBlocked` | `crates/loams-operator/src/cluster/` | `release_resolves_to_digests`, `upgrade_skip_refused`, `order_waits_for_ready`, `failed_step_blocks_without_rollback` | — |
| 16 | Exposure modes `tailnet` and `internal`; default-deny NetworkPolicies from the component graph | `crates/loams-operator/src/exposure.rs` | `no_public_service_rendered`, `networkpolicy_allows_only_graph_edges` | — |
| 17 | Metrics and traces (`loams_operator_*`, no tenant labels); dashboards and alerts | `crates/loams-kube/src/metrics.rs`, `deploy/observability/loams-operator/` | `metrics_have_no_tenant_labels`, `promtool test rules` | — |
| 18 | `loams operator status \| pause \| resume \| plan` in the CLI; `LoamsOperation` types `Pause`, `Resume`, `Restart`, `UpgradePlan` | `crates/loams/src/cli/`, `crates/loams-operator/src/operation/` | `cli_status_reads_conditions`, `upgrade_plan_writes_steps_without_acting` | — |
| 19 | LOC injectors for drain, node stop, partition, leader kill, API-server pause and step-boundary crash; workload probe for the engine; audit-log history checks | `conformance/operator/loc-harness/` | `partition_injector_isolates_zone`, `history_check_detects_double_disruption` (on a broken fixture) | — |
| 20 | The Loams adapter; LOC Core and `LOC-HA-*` in CI (Core per PR, HA nightly) with the `bridge` driver | `conformance/operator/adapters/loams/`, `.github/workflows/` | LOC Core all MUST pass; `LOC-HA-001` to `LOC-HA-005` pass | — |
| 21 | Docs: install, upgrade, pause, the exposure rule, the LOC README and how to write an adapter; amendments recorded in §65 §17 | `docs/`, `conformance/operator/README.md` | Link check | OP1 exit: Review Focus all green, LOC report attached to the PR |

## Owner actions

- §65 and its defaults (Q984–Q999) were approved on 2026-10-19; nothing further for the design.
- Q991: the LOC name and the mirror repository `ostrium-labs/loams-operator-conformance` (create it before OP4).
- A three-zone test cluster (or a `loam-bench` profile) for OP1's exit run.

## Rulings made during execution

(none yet)
