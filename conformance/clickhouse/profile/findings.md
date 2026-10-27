## Findings across applications

Hand-written from the mining runs (plan HE1 Task 5, rulings in `docs/plans/he1-rulings/t5.md`); the tables below are generated.

- **ClickHouse 26.7+ rejects three applications' schemas.** Umami's `website_event_stats_hourly`, SigNoz's metadata tables and OpenPanel's aggregate tables are AggregatingMergeTree tables with dimension columns outside the sorting key, which 26.7 and later refuse unless `allow_dimensions_outside_sorting_key = 1`. Every stack sets it on the oracle (R5.3). The House has to accept these schemas, or document the setting, to run them.
- **The native protocol is real demand.** SigNoz sends every query over it; HyperDX's and OpenLIT's collectors create their tables and insert over it; Langfuse migrates over it (R0.5). Only the HTTP interface is in Tier 0 (D1548).
- **Topology syntax comes in force.** SigNoz writes its whole schema with `ON CLUSTER`, `Replicated*MergeTree` and `Distributed` tables, and reads with `clusterAllReplicas` (a disabled table function, D1558). §75 §7.3 accepts and ignores the first three.
- **Applications define their own SQL functions.** Plausible creates 27 `CREATE FUNCTION` lambdas for its channel classification; their calls are not ClickHouse functions (R2.9) but need `CREATE FUNCTION` support.
- **Query parameters arrive bound.** Applications that send `{name:Type}` parameters reach `query_log` as `_CAST('value', 'Type')` (R2.8); the profile counts them as `syntax:query parameter`.
- **Coverage gaps.** PostHog is not mined (R5.1); HyperDX's browser-built dashboard queries are not driven (R5.5); SigNoz's histogram quantiles need an executable function that is not installed (R5.4).
