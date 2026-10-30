# CA1 — Cost Attribution

> **For agentic workers:** Work task by task, test first. Where this plan gives exact names, paths, branches or tests, use them verbatim. Record any deviation in "Rulings made during execution" at the end of this file (decision IDs D2380–D2399, questions Q1503–Q1509).
>
> **Status: Active** (2026-10-29). Track CA. The design is [§84](../design/84-cost-attribution.md) (D2350–D2379, Q1490–Q1502), **approved by the owner with all defaults on 2026-10-29**: "yes start it, approve all defaults". One lead builds it alone, one PR per task to `dev`. Status label: **preview**.

**Goal:** an operator sees what each unit of work cost on the infrastructure they own: $ per 1M ops, $ per GB-month and $ per job, by engine, tenant, project and query class. Every number carries its basis and a trace to its inputs.

**Architecture** (§84 §5):
- `crates/loams-cost`: sans-I/O. It holds the model, inputs, import and export formats, allocation, unit costs and traces. Its `cli` feature is `loams cost`.
- `crates/loams-meter-house/src/cost_sql.rs`: std-only. It builds the driver query and compiles the query-class rules.
- `crates/loams-meter/src/costs.rs`: `CostService`, the cost job, the `meter-costs` document and the House cost tables.
- `proto/loams/meter/v1/cost.proto`.
- `web/plugins/billing/src/operator/costs/*`.
- Node-agent `io.stat` in `crates/loams-meter-agent`.

## Global constraints

- **Worktrees:** `~/Documents/Ostriumlabs/loams-wt/ca1` (lead) or `loams-wt/ca1-<task>`. One branch and one PR per task, based on `dev`. `git commit -s`. Never `git stash` or force-push a shared branch.
- **Builds:** through `~/Documents/Ostriumlabs/loams-wt/.he0-shared/buildslot.sh`, with `-j 4`, touched crates only (`cargo test -p loams-cost`). Never build in `/tmp`, and never set `CARGO_TARGET_DIR`.
- **Labels:** `review:greptile` on PRs that add or change an API (Tasks 6, 7, and any proto change).
- **Money:** exact decimals: `i128` nano-units of the currency. Never floats in the arithmetic. JSON and FOCUS output is rendered from decimals.
- **Honesty (§84 §10):** every output row has `basis` and `trace_id`. No test fixture or doc quotes a dollar figure without saying where its inputs came from. The unsourced "40.4%" figure is never used.
- **No new runtime dependencies** beyond the workspace and lockfile: `csv` 1.4 and `flate2` are already in the lockfile, and `parquet` 58 in the workspace.

## Review focus

1. **Cost is created or lost.** Tests: `allocation_conserves_every_pool` (proptest), `report_totals_reconcile_to_inputs`.
2. **The same inputs give different results.** Tests: `allocation_order_independent`, `report_digest_stable`.
3. **An estimate is shown as a measurement.** Tests: `basis_takes_weakest_input`, `default_split_marks_estimated`, `bench_report_is_example_estimated`.
4. **A bill line disappears.** Tests: `unmapped_lines_reported`, `import_totals_match_file`.
5. **Usage counts twice.** Tests: `driver_overlap_refused`, `node_agent_preferred_for_cpu`.
6. **A tenant sees what it should not.** Tests: `org_viewer_sees_only_own_rows`, `agent_cannot_apply_cost_inputs`.

## Tasks

| # | Task | Files | Tests | PR label |
|---|---|---|---|---|
| 0 | Design §84 + this plan; decision-log rows | `docs/design/84-cost-attribution.md`, this file, `docs/design/13-decision-log.md`, `docs/design/README.md` | `scripts/docs/check-decision-ids.sh` | — |
| 1 | **Core model and allocation** | `crates/loams-cost/{Cargo.toml,BUCK,src/lib.rs,src/money.rs,src/model.rs,src/basis.rs,src/allocate.rs,src/trace.rs}` | `allocation_conserves_every_pool`, `allocation_order_independent`, `allocation_proportional`, `no_negative_line_from_positive_pool`, `over_capacity_flagged_not_negative_idle`, `zero_usage_goes_unattributed`, `idle_line_from_capacity`, `basis_takes_weakest_input`, `two_level_split_conserves` | — |
| 2 | **Cost inputs** (`costs.yaml`) | `crates/loams-cost/src/{inputs.rs,amortise.rs,prices.rs}` | `amortisation_sums_to_cost_over_life`, `default_life_marked_in_trace`, `monthly_spread_per_day`, `price_effective_dates`, `currency_mismatch_refused`, `fx_converts_declared`, `inputs_version_immutable`, `default_split_marks_estimated` | — |
| 3 | **Bill import** | `crates/loams-cost/src/import/{mod.rs,focus.rs,cur.rs,csv.rs,mapping.rs}`, `crates/loams-cost/tests/data/*.csv` (synthetic) | `focus_detected_and_parsed`, `cur2_detected_and_parsed`, `legacy_cur_names_normalised`, `generic_csv_column_map`, `line_split_across_days`, `credit_is_negative_source`, `tax_without_resource_is_shared`, `unmapped_lines_reported`, `import_totals_match_file`, `gzip_streamed`, `mapping_first_match_wins`, `tag_template_with_default` | — |
| 4 | **Reports and unit costs** | `crates/loams-cost/src/{report.rs,unit.rs,trend.rs}` | `unit_cost_per_million_ops`, `gb_month_uses_mean_month`, `insufficient_sample_withheld`, `fully_loaded_spreads_idle_labelled`, `report_totals_reconcile_to_inputs`, `report_digest_stable`, `trend_marks_input_changes`, `revenue_absent_not_zero` | — |
| 5 | **Drivers from the House** | `crates/loams-meter-house/src/cost_sql.rs`, `crates/loams-cost/src/drivers.rs` (catalogue) | `driver_sql_groups_by_day_engine_tenant_class`, `class_rules_compile_to_multiif`, `class_rule_rejects_injection`, `driver_overlap_refused`, `node_agent_preferred_for_cpu`, real-House test in `fabric/crates/loams-house-worker/tests/meter.rs` (`cost_drivers_from_events`) | — |
| 6 | **`CostService` and the cost job** | `proto/loams/meter/v1/cost.proto`, `crates/loams-proto/build.rs`, `crates/loams-meter/src/{costs.rs,connect/costs.rs,authz.rs}`, `crates/loams-meter-house/src/sql.rs` (cost tables), `crates/loams/src/api/meter.rs`, `core.fga` (`cost_viewer`) | `apply_inputs_requires_meter_admin`, `agent_cannot_apply_cost_inputs`, `import_bill_idempotent_by_digest`, `supersede_stops_old_lines`, `org_viewer_sees_only_own_rows`, `cost_unavailable_without_house`, `cost_job_idempotent`, `recompute_bounded_400_days` | `review:greptile` |
| 7 | **Exports** | `crates/loams-cost/src/export/{focus.rs,openmeter.rs}`, `docs/api/meter/io.loams.dev.cost.allocation.v1.json`, `docs/api/meter/openmeter-cost-meters.yaml` | `focus_columns_match_v1_3`, `allocated_ratios_sum_to_one`, `focus_validator_passes` (pinned validator, or the column-rule checker), `openmeter_event_passes_usage_profile`, `cost_micros_conserve_per_pool_day`, `recompute_sends_adjustment` | `review:greptile` |
| 8 | **CLI** | `crates/loams-cost/src/cli.rs` (feature `cli`), `crates/loams/src/main.rs` | `offline_report_from_files`, `bench_report_is_example_estimated`, `cli_json_matches_service` | — |
| 9 | **Console** | `web/plugins/billing/src/operator/costs/*`, `web/plugins/billing/src/types.ts` | vitest: `costs_overview_totals`, `basis_badge_per_cell`, `explain_drawer_shows_sources`, `org_view_hidden_unless_shared` | — |
| 10 | **Node agent I/O and engine label** | `crates/loams-meter-agent/src/{cgroup.rs,usage.rs,resolver.rs,event.rs}`, `docs/api/meter/io.loams.dev.usage.node.io.v1.json` | `io_stat_parsed`, `io_event_passes_usage_profile`, `engine_label_from_pod`, `engine_label_absent_is_empty` | — |
| 11 | **Parquet bill import** | `crates/loams-cost/src/import/parquet.rs` (feature `parquet`) | `focus_parquet_matches_csv`, `cur2_parquet_matches_csv` | — |
| 12 | **BYOC tags** | `deploy/byoc/*/` OpenTofu modules | `tofu validate`; `tags_include_loams_pool` | — |
| 13 | **Docs and the claims check** | `docs/guides/cost-attribution.md`, `scripts/docs/check-cost-claims.sh` (+ `--self-test`) | the self-test; the check on `docs/` and `web/` | — |

## Gates (target)

- **Conservation:** for 10,000 random proptest cases, every pool-day's lines sum to its cost exactly (Task 1). For the synthetic month in `tests/data`, the report's input total equals the sum of the fixture's sources (Task 4).
- **Determinism:** shuffled inputs give an identical report digest (Tasks 1, 4).
- **Honesty:** no output row lacks `basis` and `trace_id` (a schema test in Task 6). `--from-bench` output always says `example` (Task 8).
- **FOCUS:** the export passes the validator, or the column-rule checker (Task 7).
- **Cost of the job:** one day's cost job over 10⁶ events and 10³ consumers finishes in under 60 s on the laptop's House (Task 6, measured, not estimated).
- **Exit:** a local compose stack with the House shows the Costs overview for a synthetic day, end to end: inputs → import → usage events → cost job → console. This is preview: it gates the label "preview", not "GA".

## Rulings made during execution

- **R2.1 (Task 2, 2026-10-29): prices name a unit.** A rate per byte-second needs more than nine decimals, so a price is quoted per `core_hour`, `gb_month`, `gb`, `million_operations` (or the driver unit) and the arithmetic divides by the unit's size exactly (design §84 §6.4 updated, #1596).
- **R4.1 (Task 4): `pipeline::compute`** is the one path from inputs to allocation lines; the `meter` role and `loams cost` offline both call it.
- **R5.1 (Task 5): one source per (engine, kind) by construction.** The catalogue fixes the sources, and the node agent wins over product events for level one, so no apply-time `driver_overlap` check is needed. `costs.yaml` gains `usage_pools` rules (node, type, kind). Storage gauges are each object's mean sampled size × 86,400 per day. The real-House test (`cost_drivers_from_events`) is still to do.
- **R6.1 (Task 6a): bills are stored aggregated** per (import, day, pool, kind) with line counts in the `meter-costs` document (`SourceRef::BillImport`), not as raw lines in `_meter.cost_lines`; the file digest names the file and `loams cost import` re-reads it line by line. Task 6 is split: 6a (the domain, #1616), 6b (`CostService` proto, `cost_viewer`, wiring, SDK regeneration), 6c (the cost job and `_meter.cost_allocations`). Until 6b, reading costs needs `meter_admin`.
- **R7.1 (Task 7): FOCUS validator not yet run** (licence and pin to check, D1274); the export's tests check the column set and the ratio rule.
- **R8.1 (Task 8): `loams cost` ships offline first** (`report`, `import`, `export`, `report --from-bench`), from files, through the same code as the role; online subcommands follow 6b. Query-class rules are parsed and evaluated in Rust (`classes`) with the same grammar the House compiles, and `costs.yaml` refuses a bad rule at apply. `--from-bench` lines are `estimated` and flagged `example`.
