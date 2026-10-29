# 84 — Cost Attribution: Prove What It Costs

Status: **Approved** · 2026-10-29. The owner approved the start and every default on 2026-10-29: "yes start it, approve all defaults". That came after a market note on the capital efficiency of AI infrastructure. The direction, as the coordinator relayed it: Loams should show customers measurable cost per unit of work on infrastructure they own, by engine, tenant and query class, without fabricated claims.
- Decisions **D2350–D2379** are in the [decision log](13-decision-log.md) with status **Approved**.
- Questions **Q1490–Q1502** are answered with their defaults (§17).
- D2380–D2399 and Q1503–Q1509 are kept free for the plan's rulings.

Plan: [CA1](../plans/2026-10-29-ca1-cost-attribution.md). Status label: **preview** everywhere this ships (console, CLI, API docs) until CA1's gates pass.

Builds on [§63](63-loams-meter.md), Loams Meter (D1120–D1138). §63 counts what each tenant used and enforces allowances. This design prices that usage against what the infrastructure cost its owner, and shows the result.

Markers, as in §63 and §83:
- **(verify)**: not yet checked against a primary source or running code. The task that depends on it checks it first.
- **(estimate)**: judged, not measured.
- **(target)**: a number this document sets as a gate.

---

## 1. Summary

**Loams Cost** is the part of Loams Meter that answers one question: what did this unit of work cost, on this infrastructure, and how do we know?

It takes three kinds of **cost input** from the operator:
- the hardware they own, amortised over its life;
- their cloud bill, imported as FOCUS, AWS CUR or plain CSV;
- a price table for anything with no bill or asset behind it.

It then **attributes** those costs to engines, tenants, projects and query classes. The attribution is in proportion to resource telemetry that Loams already measures outside tenant code: CPU-seconds, memory byte-seconds, bytes stored, bytes moved and I/O operations. The telemetry comes from §63's usage events in the House, plus one new node-agent event for I/O.

The outputs are:
- **$ per 1M operations**, per engine and query class;
- **$ per GB-month** stored;
- **$ per job**;
- unit economics per tenant (cost against revenue, where the operator gives the revenue);
- daily, weekly and monthly trends.

Every number carries a **basis**: `measured`, `billed`, `declared` or `estimated`. Every number also has a **trace** back to the bill lines, assets, price-table entries and meter windows it came from. Cost that cannot be attributed (idle capacity, unmapped bill lines, shared charges) is shown as its own line. It is never spread silently. The results are served by a new `CostService` in `loams.meter.v1`, and can be exported as OpenMeter-compatible CloudEvents and as a FOCUS 1.3 dataset with split cost allocation columns. Console pages and `loams cost` in the CLI show the same numbers.

### 1.1 Decisions

| # | Decision (short) | § | Status |
|---|---|---|---|
| D2350 | **Loams Cost is part of Loams Meter.** It has a sans-I/O crate `loams-cost` (model, allocation, unit costs, traces, import and export formats) and a `CostService` in the `meter` role. It adds no new service, store or external dependency. It is a preview | 5 | Approved |
| D2351 | **The model**: cost sources → cost pools → resource kinds → drivers → consumers. Attribution is per UTC day | 5.1 | Approved |
| D2352 | **The operator supplies the cost inputs**: assets (amortised), bill imports and a price table, plus mapping rules. Assets, prices, mappings and classes are one YAML document (`costs.yaml`), applied through `CostService` like plans (§63 §22) and kept in Git | 6 | Approved |
| D2353 | **Hardware amortisation**: straight line per day, `(cost − residual) / life_days`, over the asset's life from its in-service date. Optional fixed monthly costs (hosting, power, support) are spread per day. Each asset declares its capacity (cores, memory, disk, network) | 6.1 | Approved |
| D2354 | **Bill imports**: FOCUS 1.0–1.4 (the first format), AWS CUR 2.0 and legacy CUR, and generic CSV with a column map. Files may be plain or gzip. `EffectiveCost` (commitments amortised) is the basis by default, and `BilledCost` is kept to reconcile with the invoice | 6.2 | Approved |
| D2355 | **Mapping rules** turn assets and bill lines into pools and resource kinds. A rule matches resource id globs, tags, service and SKU. The first match wins. **Unmapped lines are reported, never dropped.** The default rules read the tags `loams-cluster`, `loams-pool` and `loams-kind` | 6.3 | Approved |
| D2356 | **Price table**: rates per resource kind and scope, with effective dates, a currency, and a `source` (`contract`, `list` or `estimate`). It prices pools that have no asset or bill behind them. A price never overrides a billed amount | 6.4 | Approved |
| D2357 | **One reporting currency per deployment.** A line in another currency is refused unless a declared FX rate covers its day | 6.5 | Approved |
| D2358 | **Resource kinds**: `cpu` (core-seconds), `memory` (byte-seconds), `storage` (byte-seconds), `network_egress`, `network_ingress` and `network_internal` (bytes), `io` (operations), `gpu` (device-seconds, reserved) and `fixed` (no driver) | 5.2 | Approved |
| D2359 | **Drivers come from Loams Meter's usage events** in `_meter.events FINAL`, through a driver catalogue: event type and field → resource kind and engine. **There is one source per (engine, resource kind)**, so no usage counts twice. The node agent is the preferred source for CPU, memory and network | 7 | Approved |
| D2360 | **New telemetry**: the node agent reads cgroup `io.stat` into `io.loams.dev.usage.node.io.v1` (read and write bytes and operations), and puts an `engine` label on its node events | 7.3 | Approved |
| D2361 | **Consumers** are (engine, tenant, project, query class). Here tenant = org and project = namespace. Jobs are keyed by job id for $ per job. Pseudo-consumers are `idle`, `unattributed` and `shared`, and Loams' own overhead is the engine `_platform` | 5.3 | Approved |
| D2362 | **Query classes**: each engine has default classes, and operators can add rules (predicates on event fields), which compile into the House query. A producer-set `query_class` field wins. An unclassified event is `other` | 7.4 | Approved |
| D2363 | **Allocation is proportional to measured driver usage, with capacity-aware idle.** Rounding is largest-remainder in nano-units of the currency, so each pool's parts sum exactly to the pool. The same inputs give byte-identical results | 8 | Approved |
| D2364 | **Idle, unattributed and shared costs are shown, not spread**, by default. An operator may choose to spread them; the spread lines are labelled `estimated` and name the method | 8.3 | Approved |
| D2365 | **Unit costs**: $ per 1M ops (with op counters defined per engine), $ per GB-month, $ per job and $ per core-hour realised. Each one says what it includes (§9.1) | 9 | Approved |
| D2366 | **Unit economics and trends**: per tenant, cost against revenue when revenue is known (plan prices on the meter's period totals, or a declared revenue import). Trends are daily, weekly and monthly, with every input change marked on them. **No forecasts** | 9.3 | Approved |
| D2367 | **Basis labels**: every input and output is `measured`, `billed`, `declared` or `estimated`. An output takes the weakest basis of its inputs | 10.1 | Approved |
| D2368 | **Traceability**: every output carries a trace (input ids, versions and the formula), `ExplainCost` returns the tree, and each report has a digest over its inputs | 10.2 | Approved |
| D2369 | **Minimum sample**: a unit cost is withheld ("not enough data") below 1,000 ops, 1 GiB-day or 10 jobs in its period | 10.3 | Approved |
| D2370 | **The public-claims rule**: a Loams doc, site page, blog post or deck may quote a cost figure only with a link to a reproducible report and its inputs. Benchmark-derived figures are labelled with their hardware and say that the price is an example. Unsourced percentages are never used | 10.4 | Approved |
| D2371 | **Storage**: cost inputs live in a versioned control-store document `meter-costs`. Bill lines go to the House table `_meter.cost_lines`, and allocations to `_meter.cost_allocations` (`ReplacingMergeTree(cost_run)`). Without the House, only the CLI's offline mode works | 11 | Approved |
| D2372 | **The cost job** closes each day 3 hours after it ends, after the meter's day windows have closed. It recomputes the affected days when an input changes (up to 400 days back). Runs are versioned | 11.2 | Approved |
| D2373 | **API**: `loams.meter.v1.CostService`. `meter_admin` writes the inputs. A new `cost_viewer` relation on `platform` reads everything. An org's `billing_viewer` reads that org's costs only when `share_with_orgs` is on | 12 | Approved |
| D2374 | **OpenMeter-compatible export**: allocations as CloudEvents of type `io.loams.dev.cost.allocation.v1` (`cost_micros` plus dimensions). They go through the OpenMeter exporter or into a file, with suggested OpenMeter meter definitions | 13.1 | Approved |
| D2375 | **FOCUS export** (optional): a FOCUS 1.3 Cost and Usage dataset with the split cost allocation columns (`AllocatedMethodId` `loams-proportional-v1`, `AllocatedMethodDetails` with `AllocatedRatio`, `UsageUnit`, `UsageQuantity` and `x_` properties), as CSV, checked by a FOCUS validator | 13.2 | Approved |
| D2376 | **Console**: operator pages under the billing plugin (Costs overview, Unit costs, Breakdown, Trends, Inputs, Explain). An org page appears only when costs are shared | 14 | Approved |
| D2377 | **CLI**: `loams cost` with `inputs`, `import`, `report`, `unit-costs`, `explain` and `export`. It works online against `CostService`, and offline from files (inputs, a bill, usage NDJSON). `--from-bench` builds an example report from the BQ1 results, always labelled `estimated` and `example` | 15 | Approved |
| D2378 | **Security**: only `meter_admin` writes inputs, and every write is audited. Agent principals cannot write inputs. Bill files stay in `_system`. Org viewers never see account ids, other orgs' costs or raw bill lines | 16 | Approved |
| D2379 | **Plan CA1**, status file `loams-wt/.he0-shared/ca1-status.md`; the status label stays **preview** | 18 | Approved |

---

## 2. Why

**The owner, 2026-10-29**: "yes start it, approve all defaults". The coordinator relayed the direction: measurable cost per unit of work on infrastructure the customer owns, by engine, tenant and query class, without fabricated claims.

Loams' pitch is that a team can run its data platform on infrastructure it owns ("Don't rent a cloud. Loam it."). That pitch is only as strong as the evidence behind it. Today Loams can say how many operations an engine served and how many CPU-seconds it used (the BQ1 benchmarks, §4). It cannot say what those operations cost the owner, on the owner's hardware or bill. Every vendor claim of "N% cheaper" asks the reader to trust inputs they cannot see. Loams should do the opposite. It should compute cost from the owner's own bill and the owner's own telemetry, and show its working.

What already exists makes this small:
- §63 measures usage outside tenant code (cgroups, the House front, the gateway), deduplicates it, and stores it in the House.
- The House can aggregate it by tenant, engine and day.
- What is missing is the price side (assets, bills, rates), the allocation arithmetic, and the rules that keep the result honest.

**What this design does not claim.** It does not claim that Loams is cheaper than any cloud service, and it does not publish savings percentages. A comparison appears only when both sides' inputs are in the report (§10.4). Figures from a laptop benchmark are resource quantities (CPU-seconds per 1M ops). They become dollars only when an operator supplies a price (§9.4).

---

## 3. Goals and non-goals

### 3.1 Goals

1. **Cost per unit of work** ($ per 1M ops, $ per GB-month, $ per job) per engine, tenant, project and query class, on the operator's own infrastructure.
2. **Every number traces to its inputs.** Anyone with access can see which bill lines, assets, prices and usage windows a number came from, and recompute it.
3. **Measured and estimated are never mixed silently.** Each number says how it was obtained.
4. **Conservation**: for every day, attributed + idle + unattributed + shared = total cost input, exactly.
5. **Standard formats in and out**: FOCUS for bills and exports, and OpenMeter-shaped CloudEvents.
6. **No new infrastructure.** It uses the meter role, the House and the control store that §63 already runs.

### 3.2 Non-goals

- **Invoicing tenants.** §63 meters and bills tenants. Cost attribution is about what the operator paid. It can show margin, but it never changes a bill.
- **Forecasting, budgeting and rightsizing advice.** Trends show history only. A forecast would be an estimate dressed as a measurement.
- **A full FinOps platform.** Loams does no commitment planning, anomaly detection across cloud accounts, or multi-cloud governance. Operators who need these export FOCUS to their FinOps tool.
- **Pricing cloud services that Loams does not run.** Bill lines that do not map to Loams' pools stay `unmapped`.
- **Per-request cost on the hot path.** Attribution runs on closed days in the House. Nothing is added to any request.

---

## 4. Where Loams stands today (checked 2026-10-29 on `dev` at `3bada72f`)

| Piece | State | Path |
|---|---|---|
| Usage events (CloudEvents, §63 §6), `_system/_meter` stream, House `_meter.events` (`ReplacingMergeTree`, read with `FINAL`) | Built (ME1a, ME1b) | `crates/loams-meter-house/src/sql.rs` |
| Closed windows `_meter.windows`, close job, period totals, `QueryMeter`, OpenMeter query path | Built | `crates/loams-meter-house/src/{close,period,query}.rs` |
| The `meter` role: customers, entitlements, plans, notifier, counter owners, versioned docs in the control store (`DocStore` over `VersionedDoc`) | Built, preview | `crates/loams-meter/src/*`, `crates/loams/src/api/meter.rs` |
| Exporters (Stripe, OpenMeter, Lago, webhook) and `DocExports` ledger | Built, partly scheduled (§63 §16) | `crates/loams-meter-export`, `crates/loams-meter/src/export.rs` |
| Node agent: cgroup v2 CPU (`cpu_usec`), memory (`byte_seconds`, `peak_bytes`), network (`ingress_bytes`, `egress_bytes`) per (tenant, resource, window) | Built. No I/O, and no engine label (the resource is a cgroup path) | `crates/loams-meter-agent/src/{cgroup,usage}.rs` |
| House usage events with `cpu_ms`, `bytes_scanned`, storage gauges; gateway connection bytes; Postgres compute `cu_ms` | Built (observer adapters, ME1 T3) | `loams-meter-gate` adapters |
| Console billing plugin (usage, entitlements, spend, payment, alerts, operator pages) | Built, preview | `web/plugins/billing` |
| Per-engine throughput and busy CPU-seconds on one laptop | Measured 2026-10-29 (BQ1-ALL) | [`docs/benchmarks/2026-10-28-1b-qps/combined.md`](../benchmarks/2026-10-28-1b-qps/combined.md) |
| Any notion of price, asset, bill or cost | None | — |

---

## 5. The model (D2350, D2351, D2358, D2361)

### 5.1 Vocabulary

| Term | Meaning | Example |
|---|---|---|
| **Cost source** | One priced input for one day: an asset's amortised day, a bill line's share of a day, or a price-table rate times a declared quantity | `asset:rack1-node3` on 2026-10-28 = 7.123287671 USD |
| **Pool** | A set of infrastructure whose cost is shared by the same consumers. The operator names it in the mapping rules | `prod-eu1/compute`, `prod-eu1/bucket` |
| **Resource kind** | What a pool's cost buys, and so which driver splits it (§5.2) | `cpu`, `storage` |
| **Driver** | Measured usage of a resource kind by a consumer, from meter events | 5,184 core-seconds of `cpu` |
| **Capacity** | What the pool could have delivered in the day. It is declared by assets, or absent | 3 nodes × 16 cores × 86,400 s |
| **Consumer** | Who used it: (engine, tenant, project, query class), or a pseudo-consumer | (`house`, `org_42`, `7`, `scan`) |
| **Allocation** | One cost line: the part of a pool-day's cost assigned to one consumer | |
| **Unit cost** | Allocated cost divided by work done | $ per 1M ops |

```
 cost inputs (operator)                 telemetry (Loams Meter, House)
 ┌──────────────┐ ┌──────────────┐      ┌─────────────────────────────────┐
 │ assets       │ │ bill imports │      │ _meter.events FINAL             │
 │ (amortised)  │ │ FOCUS/CUR/CSV│      │  node.cpu/memory/network/io     │
 └──────┬───────┘ └──────┬───────┘      │  house.query, postgres.compute, │
        │  ┌─────────────┘ price table  │  gateway.*, jobs.run, storage…  │
        ▼  ▼          ┌────────┘        └───────────────┬─────────────────┘
   mapping rules ─────┤                                 │ driver catalogue
        │             │                                 │ + query-class rules
        ▼             ▼                                 ▼
   pool × kind × day: cost  ───────── allocate ◄─── consumer × kind × day: usage
                                         │
              ┌──────────────────────────┼─────────────────────────┐
              ▼                          ▼                         ▼
   allocations (+ idle, unattributed,   unit costs           trends, unit economics
   shared), basis + trace per line      ($/1M ops, $/GB-mo,  (input changes marked)
                                         $/job)
              │
              ├── CostService (Connect, REST), console, `loams cost`
              ├── OpenMeter CloudEvents export
              └── FOCUS 1.3 export (split cost allocation)
```

### 5.2 Resource kinds (D2358)

| Kind | Driver unit | Driver sources (§7) | Capacity from |
|---|---|---|---|
| `cpu` | core-seconds | `node.cpu` `cpu_usec`; `house.query` and `house.compaction` `cpu_ms`; `function.invocation` and `jobs.run` `cpu_ms`; `sandbox.session` `cpu_ms` | asset `cores` × seconds |
| `memory` | byte-seconds | `node.memory` `byte_seconds` | asset `memory_bytes` × seconds |
| `storage` | byte-seconds | storage gauges (`house.storage`, `postgres.storage`, `storage.logical`, `git.storage`) as hourly `LATEST` × 3,600 | asset `disk_bytes` × seconds, or none for a bucket |
| `network_egress`, `network_ingress`, `network_internal` | bytes | `node.network`; `gateway.connection` `bytes_in` and `bytes_out` | asset `network_bytes_per_s` × seconds, or none |
| `io` | operations | `node.io` `read_ops + write_ops` (new, §7.3) | asset `iops` × seconds, or none |
| `gpu` | device-seconds | reserved for §67's GPU work; no source yet | asset `gpus` × seconds |
| `fixed` | none | none | none |

A pool has one resource kind. A node that costs one amount (one asset or one bill line) and serves several kinds is **split into kind pools** by the asset's declared `split` (for example `{cpu: 0.6, memory: 0.3, io: 0.1}`). With no declared split, the whole node goes to `cpu`. Its lines are then labelled `estimated`, with the reason `default_split_cpu_only` (D2367).

### 5.3 Consumers (D2361)

A consumer key has four parts:
- **engine**: one of `house`, `postgres`, `sql`, `kv`, `stream`, `iggy`, `gateway`, `durable`, `graph`, `git`, `functions`, `jobs`, `sandbox`, `flow`, `_platform`. The list is open, and new engines add a name.
- **tenant**: the org (`org_<id>` subject, D65).
- **project**: the namespace from `tenantid`.
- **query class** (§7.4).

Missing parts are `_` (for example, storage has no query class).

The pseudo-consumers are:

| Consumer | Holds |
|---|---|
| `idle` | Capacity that no consumer used: `cost × (capacity − Σ usage) / capacity` |
| `unattributed` | A pool-day with cost but no driver usage at all (the telemetry is missing, or the pool was unused with no declared capacity) |
| `shared` | `fixed` pools and charges with no pool (support, tax with no resource, account-level credits) |
| `_platform` | Loams' own processes (meter, control plane, migrations). This is a real engine, shown like any other |

**$ per job** uses a fifth key, the job id. It is present on `jobs.run`, `durable` and `function.invocation` events, and is aggregated only for the per-job view (§9.1). Per-job lines are not kept per day, to bound cardinality: the report keeps the top 1,000 jobs by cost and their sum, and the rest as one `other_jobs` line.

---

## 6. Cost inputs (D2352–D2357)

All inputs except bill files are one YAML document, applied through `CostService.ApplyCostInputs` (or `loams cost inputs apply`). It lives in the operator's GitOps repository next to the plans (§63 §22). Each apply creates a new version, and the document is kept whole. A report records the version it used.

```yaml
# costs.yaml
version: 7                      # monotonic; same version + other content is refused
currency: USD                   # the reporting currency (D2357)
assets:
  - id: rack1-node3
    pool: prod-eu1/compute
    cost: "9600.00"             # purchase price, reporting currency
    residual: "0"
    in_service: 2026-03-01
    life_months: 60             # Q1490 default
    monthly: { hosting: "45.00", power: "18.00" }    # declared, spread per day
    capacity: { cores: 16, memory_bytes: 68719476736, disk_bytes: 3840000000000, iops: 400000 }
    split: { cpu: 0.6, memory: 0.3, io: 0.1 }        # optional (§5.2)
    nodes: [node-3]             # node ids in the meter events
prices:
  - kind: storage
    scope: prod-eu1/bucket
    rate: "0.000000000008"      # USD per byte-second (about 0.0207 USD per GiB-month)
    source: contract            # contract | list | estimate
    effective: 2026-10-01
fx: []                          # declared rates, if a bill is in another currency
mappings:                       # bill lines → pools (first match wins)
  - match: { tags: { loams-pool: "*" } }
    pool: "{tags.loams-pool}"
    kind: "{tags.loams-kind|cpu}"
  - match: { service: "Amazon Simple Storage Service", region: eu-central-1 }
    pool: prod-eu1/bucket
    kind: storage
engines:                        # node-agent resources → engines (§7.2)
  - match: { resource: "*/loams-house-worker*" }
    engine: house
classes:                        # operator query classes (§7.4)
  - engine: house
    class: heavy_scan
    when: "bytes_scanned >= 10737418240"
options:
  idle: show                    # show | spread  (D2364)
  shared: show                  # show | spread
  share_with_orgs: false        # D2373
```

### 6.1 Assets and amortisation (D2353)

For each day `d` in `[in_service, in_service + life_months)`:

```
asset_day(d) = (cost − residual) / life_days
             + Σ monthly_k / days_in_month(d)
life_days    = the number of days from in_service to in_service + life_months
```

The amounts are exact decimals: 9 fractional digits, rounded half-even only at the end of each day's sum. Outside the asset's life the asset costs nothing, and its capacity is still counted. A fully depreciated server is not free capacity: it shows as `idle` at zero cost, which is the truth. All asset lines are basis `declared`. `life_months` left out takes the default (Q1490), and the trace records `default:life_months=60`.

### 6.2 Bill imports (D2354)

| Format | Detected by | Columns read |
|---|---|---|
| **FOCUS 1.0–1.4** | header has `BilledCost` and `ChargePeriodStart` | `ChargePeriodStart`, `ChargePeriodEnd`, `EffectiveCost`, `BilledCost`, `ListCost`, `BillingCurrency`, `ChargeCategory`, `ChargeClass`, `ResourceId`, `ResourceName`, `ServiceName`, `ServiceCategory`, `SkuId`, `RegionId`, `ConsumedQuantity`, `ConsumedUnit`, `Tags`, `InvoiceId` (when present), `ServiceProviderName` or `ProviderName` |
| **AWS CUR 2.0** | header has `line_item_unblended_cost` | `line_item_usage_start_date`, `line_item_usage_end_date`, `line_item_unblended_cost`, `line_item_net_unblended_cost` (when present), `line_item_currency_code`, `line_item_line_item_type`, `line_item_resource_id`, `line_item_product_code`, `line_item_usage_type`, `line_item_usage_amount`, `product_region_code`, `resource_tags` (verify the CSV encoding of the map) |
| **Legacy CUR** | header has `lineItem/UnblendedCost` | the same fields, with the names converted (`lineItem/UsageStartDate` → `line_item_usage_start_date`), and tags from `resourceTags/user:<key>` |
| **Generic CSV** | `--format csv` with a column map in the request | `start`, `end`, `cost`, `currency`, plus optional `resource`, `service`, `sku`, `region`, `quantity`, `unit`, `tags` (JSON or `k=v;k=v`), `category` |

Rules:
- **Days.** A line's cost is split across the UTC days its charge period covers, in proportion to the seconds in each day. Monthly lines (`ChargeFrequency` `Recurring`, or a period longer than one day) are spread evenly over their period.
- **Charge categories.** FOCUS `Usage` and `Purchase` lines enter pools. `Tax` goes to the pool of the charge it taxes when the line names a resource, and otherwise to `shared`. `Credit` and `Adjustment` lines are negative sources in the pool they map to, or in `shared` otherwise. A correction (`ChargeClass` `Correction`) replaces nothing: it adds, as FOCUS intends.
- **EffectiveCost by default.** It includes commitment amortisation, so a reserved instance's cost lands in the days it serves. `BilledCost` is summed per import and shown next to the invoice total (§10.2) so the operator can reconcile. CUR has no `EffectiveCost`, so its basis is `line_item_net_unblended_cost` when present and `line_item_unblended_cost` otherwise. The import records which one it used.
- **Idempotent.** An import is keyed by the SHA-256 of its bytes. Importing the same file twice is a no-op. A file that replaces an earlier one (a restated month) is imported with `supersedes: <import id>`, and the earlier import's lines stop counting from then on. Both stay listed.
- **Size.** Files are streamed and never held in memory. The first build reads CSV and CSV gzip. Parquet follows in CA1 Task 11 (Q1496).
- All billed lines are basis `billed`.

### 6.3 Mapping rules (D2355)

A rule has a `match` (any of `resource` glob, `service`, `sku`, `region`, `category`, `tags` with glob values) and a target `pool` and `kind`. Targets may use `{tags.<key>}` with a fallback `|default`. Rules are tried in order, and the first match wins.

A line that matches no rule is **unmapped**. It is stored with its import, counted in the report's `unmapped` total (per service), and never allocated. The console shows unmapped cost as a share of the import, so an operator sees at once that 30% of the bill did not land anywhere.

The built-in rules come last and read the tags `loams-pool` and `loams-kind`. The OpenTofu modules for BYOC (§70, §80) set these tags on every resource they create (CA1 Task 12).

### 6.4 Price table (D2356)

A price is `{kind, scope, rate, currency, source, effective, until?}`. It prices a pool only when no asset and no billed line exists for that pool-day. Typical uses:
- a contract rate for a colocation bucket;
- an estimate for a laptop;
- the list price of a managed service, for comparison.

A `list` or `estimate` price makes the line `estimated`, and a `contract` price makes it `declared`.

Prices also give **reference costs**. A report can show what the same driver usage would cost at a reference price, next to the actual cost. This is how a comparison is made honestly: both numbers are in one report, and both have traces. Nothing in Loams ships a reference price. The operator enters it, with its source URL in `note`.

### 6.5 Currency (D2357)

Each deployment has one reporting currency. A bill line in another currency is refused at import with `currency_mismatch`, unless `fx` has a declared rate for that currency and day (`{from: EUR, to: USD, rate: "1.0812", day: 2026-10-28, source: "ECB reference rate"}`). A converted line is `declared`, not `billed`.

---

## 7. Drivers: usage from Loams Meter (D2359–D2362)

### 7.1 The driver catalogue

Drivers are read from `_meter.events FINAL` (§63 §7.5) for whole closed UTC days. A catalogue in `loams-cost` lists, per event type, the fields that give each resource kind and how they convert to the driver unit:

| Event type | Engine | Kind ← field (conversion) | Ops counter |
|---|---|---|---|
| `…usage.node.cpu.v1` | from `engine` label (§7.3) | `cpu` ← `cpu_usec` / 10⁶ | — |
| `…usage.node.memory.v1` | from `engine` label | `memory` ← `byte_seconds` | — |
| `…usage.node.network.v1` | from `engine` label | `network_egress` ← `egress_bytes`; `network_ingress` ← `ingress_bytes` | — |
| `…usage.node.io.v1` (new) | from `engine` label | `io` ← `read_ops + write_ops` | — |
| `…usage.house.query.v1` | `house` | `cpu` ← `cpu_ms` / 10³ | `statements` |
| `…usage.house.insert.v1` | `house` | — | 1 per event |
| `…usage.house.storage.v1` | `house` | `storage` ← `bytes` (gauge) | — |
| `…usage.postgres.compute.v1` | `postgres` | `cpu` ← `cu_ms` / 10³ (1 CU = 1 core, §46; verify) | — |
| `…usage.postgres.storage.v1` | `postgres` | `storage` ← `logical_bytes` (gauge) | — |
| `…usage.gateway.connection.v1` | `gateway` | `network_egress` ← `bytes_out`; `network_ingress` ← `bytes_in` | 1 per event |
| `…usage.gateway.request.v1` | `gateway` | as above | 1 per request |
| `…usage.jobs.run.v1` | `jobs` | `cpu` ← `cpu_ms` / 10³ | 1 per job |
| `…usage.function.invocation.v1` | `functions` | `cpu` ← `cpu_ms` / 10³ | 1 per invocation |
| `…usage.stream.ingest.v1` | `stream` | — | `records` |
| `…usage.storage.logical.v1` | from `product` field, or `stream` | `storage` ← `logical_bytes` (gauge) | — |
| `…usage.git.storage.v1`, `…git.transfer.v1` | `git` | `storage`; `network_egress` | 1 per transfer |
| `…usage.sandbox.session.v1` | `sandbox` | `cpu` ← `cpu_ms` / 10³; `memory` ← `memory_mb_ms` × 2²⁰ / 10³ | 1 per session |

Engines without a product event yet (KV, SQL, Iggy, Durable, Graph) get their CPU, memory, network and I/O from the node agent's `engine` label. Their **ops counters** come from new product events that each engine adds when its own meter call site lands (§63 §10.2). Until then they have cost but no $ per 1M ops, and the report says `ops_unavailable`.

**One source per (engine, kind)** (D2359). If the node agent and a product event both give `cpu` for `house`, the catalogue's precedence picks the node agent. The cgroup is the whole worker, so it includes CPU that the per-query `cpu_ms` misses (compaction, background merges).

The per-query field is still used for the **split inside the engine**. The engine's node-agent CPU is divided among (tenant, project, class) in proportion to their product-event CPU. This is a two-level allocation, and the trace shows both levels. If an engine has no product event, the node-agent tenant attribution (the resolver's `tenantid`) is used directly.

A catalogue check runs at apply time. It refuses a `costs.yaml` whose rules would count the same (engine, kind) from two sources at the same level (`driver_overlap`).

### 7.2 Engines from node-agent resources

The node agent reports `resource` as a cgroup path. The `engines` rules in `costs.yaml` map path globs to engines. The agent's resolver also learns the engine from the pod or unit label `loams.dev/engine` (§7.3), and that label wins over rules. A resource that matches neither is engine `_unknown`. It is shown, and it counts toward the "unattributed telemetry" health number (§14).

### 7.3 New telemetry (D2360)

- **`io.loams.dev.usage.node.io.v1`**: the node agent reads cgroup v2 `io.stat` (`rbytes`, `wbytes`, `rios`, `wios`, summed over devices) per metered resource and window, like the CPU reader. The data is `read_bytes`, `write_bytes`, `read_ops`, `write_ops`, plus the usual dimensions. It is a counter delta and is billed by no plan. It exists for cost.
- **`engine`** on every node event: the value of the `loams.dev/engine` label on the pod (Kubernetes resolver) or the systemd unit's `X-Loams-Engine=` (compose and VM resolver) (verify that the compose resolver can read it). It is empty when the label is unknown.
- **No new product events here.** Ops counters for engines that lack them come with those engines' meter call sites. CA1 does not block on them.

### 7.4 Query classes (D2362)

Each engine has default classes, defined in the catalogue from fields its events already carry:

| Engine | Default classes (first match) |
|---|---|
| `house` | `insert` (type `house.insert`), `compaction` (type `house.compaction`), `scan` (`bytes_scanned ≥ 1 GiB`), `point` (`rows_read ≤ 1,000`), `query` (the rest) |
| `postgres` | `compute` (one class until pg-control reports statement classes) |
| `gateway` | `pg`, `http`, `llm`, `mcp` (by event type) |
| `functions` | by `tier` (`t0`, `t1`, …) |
| `jobs` | by `queue` |
| others | `default` |

Operators add rules in `classes` (§6). A rule is a predicate over event fields: `field op value`, joined by `and`, with `op` one of `= != < <= > >=` and `in`. Rules compile into a ClickHouse `multiIf(...)` over `JSONExtract*` of `data`. They run in the House, so classification adds no data movement. An event whose data carries `query_class` (a future producer field) takes that value first.

---

## 8. Allocation (D2363, D2364)

### 8.1 The algorithm

For each UTC day `d` and each pool `p` (with kind `k`):

1. **Cost** `C` = Σ sources mapped to `(p, k, d)`: amortised asset days, bill-line day shares, and prices × quantities. Credits are negative.
2. **Usage** `u_i` = the driver usage of each consumer `i` on the nodes or scope of `p` for kind `k` on `d` (§7). `U = Σ u_i`.
3. **Capacity** `Cap` = declared capacity of `p`'s assets for kind `k` on `d`, or none.
4. Allocate:
   - `C = 0`: nothing to allocate. The usage is still recorded, so unit costs can say "free on this day".
   - `U = 0`: one line, `unattributed`, gets `C`.
   - `Cap` known and `U ≤ Cap`: consumer `i` gets `C × u_i / Cap`, and `idle` gets `C × (Cap − U) / Cap`.
   - `Cap` known and `U > Cap`: the measurement exceeds the declared capacity, so the capacity is wrong or the clock windows straddle. Allocate `C × u_i / U`, and flag the pool-day `over_capacity` (an estimate).
   - `Cap` unknown (buckets, bill-only pools): consumer `i` gets `C × u_i / U`. The pool-day is flagged `idle_unknown`: fully loaded, idle not separable.
5. **Rounding**: amounts are integers in nano-units of the currency (`i128`). Each share is floored. The remainder, `C − Σ floor`, is handed out one nano-unit at a time to the largest fractional parts. Ties are broken by consumer key order. So `Σ lines = C` exactly, and the result does not depend on input order.

The second level (§7.1, engine → tenant, project and class) runs the same procedure on each engine's line, with the product-event CPU as the usage and no capacity.

### 8.2 Properties (test-enforced)

| Property | Test |
|---|---|
| Conservation: per pool-day, Σ lines = cost exactly; per day, Σ all lines = Σ inputs | `allocation_conserves_every_pool` (proptest) |
| Determinism: permuting inputs gives byte-identical output and digest | `allocation_order_independent` |
| Proportionality: doubling one consumer's usage (with capacity unchanged) doubles its line ± 1 nano-unit | `allocation_proportional` |
| No negative lines unless the pool's cost is negative (credits) | `no_negative_line_from_positive_pool` |
| Idle never negative; `over_capacity` flagged instead | `over_capacity_flagged_not_negative_idle` |

### 8.3 Idle, unattributed and shared (D2364)

By default these stay as their own lines. Unit costs are then **used-capacity** unit costs. The report also gives **fully loaded** unit costs, in which idle and shared are spread over consumers in proportion to their attributed cost. Those are labelled `estimated` with the method `spread:attributed_cost`. With `options.idle: spread`, the fully loaded figures become the headline, and they keep the label. The console always shows both, and says which is the headline.

---

## 9. Outputs (D2365, D2366)

### 9.1 Unit costs

| Unit cost | Numerator | Denominator | Shown per |
|---|---|---|---|
| **$ per 1M ops** | attributed cost of (engine, class), all kinds | ops counter (§7.1) × 10⁻⁶ | engine, class, tenant, project |
| **$ per GB-month** | attributed `storage` cost | byte-seconds ÷ (10⁹ × 2,629,746 s), the mean Gregorian month | engine, tenant, project |
| **$ per job** | attributed cost for the job id, all kinds | 1 per job | engine (`jobs`, `durable`, `functions`), queue, tenant |
| **$ per core-hour realised** | attributed `cpu` cost (used only) | core-seconds ÷ 3,600 | pool, engine |

"GB" is 10⁹ bytes. The console also offers GiB, and the unit is always written out.

Each unit cost states what it includes. Example: "house / scan: compute (cpu, memory, io) on prod-eu1/compute and egress; storage excluded (shown as $ per GB-month)". Storage is never folded into $ per op: query volume does not drive storage cost, and folding it in would make a busy day look cheaper.

### 9.2 Reports

`GetCostReport(from, to, group_by ⊆ {engine, tenant, project, query_class, pool, kind}, filters, granularity ∈ {day, week, month})` returns rows. Each row has:
- `cost` (decimal string, 9 fractional digits) and `currency`;
- the driver quantities per kind;
- `ops` and the unit costs that apply;
- `basis` (§10.1) and flags (`idle_unknown`, `over_capacity`, `default_split_cpu_only`, `ops_unavailable`, `insufficient_sample`);
- a `trace_id`.

Pseudo-consumers appear as rows with `engine` set to `idle`, `unattributed` or `shared`. Each report header carries:
- `as_of`: the last closed day;
- the cost-input version;
- the import ids;
- the cost job's run;
- the report `digest`;
- the totals: input cost, attributed, idle, unattributed, shared, unmapped (not allocated) and billed (for reconciliation).

### 9.3 Unit economics and trends (D2366)

**Unit economics** per tenant (and project): cost, revenue, margin and margin %. Revenue comes from one of two places:
- the meter's billed usage priced by the plan's overage prices plus the plan's base price, where plans carry prices (§63 §9.5);
- a declared revenue import (CSV `tenant, period, amount, currency`).

Without revenue, the columns are absent, not zero.

**Trends**: the same report over consecutive periods, with:
- the period-over-period change;
- markers on the dates when an input changed (a new asset, a price change, a new bill import, or a mapping change), so a step in a trend is explained by its cause;
- unit costs per period, with the sample rule (§10.3) applied per period.

There are no forecasts, moving-average projections or "at this rate" figures.

### 9.4 Benchmark figures are resource quantities, not prices

The BQ1 combined run (2026-10-29, one 14-core laptop, depth 16; [combined.md](../benchmarks/2026-10-28-1b-qps/combined.md)) measured operations and busy CPU-seconds per engine. Dividing one by the other gives **CPU-seconds per 1M ops**. That is a measured resource quantity on that laptop, and it is what `loams cost report --from-bench` reads:

| Engine (slot) | Ops in 60 s | Busy CPU-s | CPU-s per 1M ops | Core-hours per 1M ops |
|---|---:|---:|---:|---:|
| meter | 23,556,120 | 29.6 | 1.26 | 0.00035 |
| iggy | 12,968,280 | 60.5 | 4.67 | 0.00130 |
| kv (in-process) | 3,179,040 | 29.7 | 9.34 | 0.00260 |
| gateway | 6,024,600 | 88.1 | 14.62 | 0.00406 |
| pg | 5,607,060 | 117.5 | 20.96 | 0.00582 |
| sql | 2,035,200 | 62.8 | 30.86 | 0.00857 |
| kv-tikv | 376,920 | 64.3 | 170.59 | 0.04739 |
| git | 133,680 | 29.6 | 221.42 | 0.06151 |
| graph | 51,840 | 29.7 | 572.92 | 0.15914 |
| durable | 32,040 | 46.2 | 1,441.95 | 0.40054 |

Ops = QPS × 60 s. The House failed at depth 16 and has no row.

What the table does not include:
- memory, storage, network and idle;
- the load generators' cores are counted in the busy CPU-seconds, so network engines are overstated by up to about 2× (combined.md, "Assumptions").

**$ per 1M ops = core-hours per 1M ops × the operator's price per core-hour.** Loams does not supply the price. `--from-bench` asks for one (`--price-core-hour`), and every line it prints is labelled `estimated` and `example: laptop benchmark`.

---

## 10. Honesty rules (D2367–D2370)

### 10.1 Basis labels (D2367)

| Basis | Means | Examples |
|---|---|---|
| `measured` | A driver quantity read from telemetry measured outside tenant code | node-agent CPU, House `cpu_ms`, storage gauges |
| `billed` | An amount from an imported bill line in the reporting currency | FOCUS `EffectiveCost` |
| `declared` | An amount or rule the operator stated | asset amortisation, contract prices, FX rates, declared splits |
| `estimated` | A default, a list price, a split with no measurement behind it, a spread of idle or shared cost, or a benchmark example | `default_split_cpu_only`, `spread:attributed_cost`, `--from-bench` |

A cost line has two bases: one for its **amount** (`billed`, `declared` or `estimated`) and one for its **share** (`measured` or `estimated`). The line's **overall basis** is the weaker of the two, in the order measured = billed > declared > estimated. So "billed + measured" is the strongest line Loams produces. The console shows the overall basis as a badge, and shows both on hover.

### 10.2 Traces and digests (D2368)

Every row has a `trace_id`. `ExplainCost(trace_id)` returns the tree:
- the row's formula with its numbers;
- the pool-days it drew from;
- for each pool-day, its sources: asset id and input version, bill import id with line numbers (or the `_meter.cost_lines` keys), price id;
- the driver query, as the SQL text, `as_of` and the House snapshot id once the House exposes it (D2203's cursor ask);
- the usage per consumer.

A report's `digest` is the SHA-256 of:
- the canonical encoding of its inputs: the cost-input version's bytes, the import digests, and the driver rows;
- the code's `allocation_version`.

Two runs over the same inputs give the same digest (§8.2). **Reconciliation** is in every report header: Σ `BilledCost` per import beside the invoice total, when FOCUS gives `InvoiceId`.

### 10.3 Minimum sample (D2369)

A unit cost is withheld below these thresholds in its period: **1,000 ops** ($ per 1M ops), **1 GiB-day** stored ($ per GB-month), or **10 jobs** ($ per job). The row then says `insufficient_sample` and shows the cost and the count. A $ per 1M ops figure from 12 queries is noise with six significant digits.

### 10.4 The public-claims rule (D2370)

This applies to every artefact the project publishes (docs, loams.dev, blog, decks, READMEs, release notes):
1. A cost figure is quoted only with a link to a reproducible report and its inputs. That means the report digest and the input files, or the benchmark results and the declared price.
2. A figure derived from a benchmark names its hardware and says that the price is an example input.
3. No savings percentage or "N× cheaper" is published unless both sides are in one report with traces (§6.4 reference costs).
4. Unsourced industry figures are not repeated, however widely they circulate.
5. CA1 Task 13 adds `scripts/docs/check-cost-claims.sh` to the docs checks. It flags `$`-figures and percentages near "cost", "cheaper" or "savings" in `docs/` and `web/` that have no `<!-- cost-source: … -->` marker beside them.

---

## 11. Storage and the cost job (D2371, D2372)

### 11.1 Where things live

| Data | Where | Why |
|---|---|---|
| `costs.yaml` versions | control-store document `meter-costs` (`VersionedDoc`, like `meter-records`) | small, versioned, CAS-written |
| Bill import records (id, digest, format, period, totals, `supersedes`) | `meter-costs` | small |
| Bill lines (mapped and unmapped, one row per line-day) | House `_meter.cost_lines` (`ReplacingMergeTree`, keyed `(import_id, line_no, day)`) | bills can have millions of lines |
| Allocations | House `_meter.cost_allocations` (`ReplacingMergeTree(cost_run)`, `ORDER BY (day, pool, kind, engine, tenant, project, query_class)`) | the same versioned-row pattern as `_meter.windows` |
| Driver rows | computed by the House query per run, not stored | they are a view of `_meter.events` |

Without the House (desktop, minimal compose, §63's enforcement-only mode) there is no usage history, so `CostService` answers `cost_unavailable` (`FAILED_PRECONDITION`). The CLI's offline mode still works on files: inputs, a bill, and usage as CloudEvents NDJSON (Q1499).

### 11.2 The cost job (D2372)

The meter role runs it:
- **Daily**, for day `d` at `d + 1 day + 3 h`, after the meter's late-event grace and the day window's close (§63 §8.2).
- **On change**: when an input version, an import or a supersede touches past days, it recomputes those days. Up to 400 days back, oldest first, at most 31 days per run.

Each run:
1. reads the driver rows (one House query per day);
2. reads the pool-days from the inputs and `_meter.cost_lines`;
3. allocates with `loams-cost`;
4. writes `_meter.cost_allocations` with a new `cost_run`.

Runs are idempotent: the same inputs give the same rows, and a later run wins under `FINAL`. Only one instance runs the job. It runs under the meter role's existing period-close lease (verify that the lease is reusable, or add a `meter-cost` lease).

---

## 12. API (D2373)

### 12.1 `loams.meter.v1.CostService`

| Method | Does | Who |
|---|---|---|
| `ApplyCostInputs {yaml, reason}` → `{version, changed, warnings[]}` | Validates and stores a `costs.yaml` version (same version + same content → `changed: false`; other content → `cost_inputs_version_immutable`; overlapping drivers → `driver_overlap`) | `meter_admin` |
| `GetCostInputs {version?}` | The current (or named) version | `meter_admin`, `cost_viewer` |
| `ImportBill` (client-streaming chunks: `{format, column_map?, supersedes?, filename}` then bytes) → `{import}` | Parses, maps, stores lines; returns totals, unmapped share, billed total, warnings | `meter_admin` |
| `ListBillImports`, `GetBillImport {id, unmapped_page?}` | Imports with totals; unmapped lines by service | `meter_admin`, `cost_viewer` |
| `GetCostReport {from, to, group_by[], filters[], granularity, fully_loaded}` | §9.2 | `cost_viewer`; org `billing_viewer` for own org when shared |
| `ListUnitCosts {from, to, by[]}` | §9.1 rows | same |
| `ExplainCost {trace_id}` | §10.2 tree | same (org viewers see their own lines only, without bill line details) |
| `ExportCosts {from, to, format: FOCUS_1_3 \| OPENMETER_CLOUDEVENTS}` (server-streaming bytes) | §13 | `cost_viewer` |
| `RecomputeCosts {from, to, reason}` | Queues a recompute | `meter_admin` |

AP0 conventions apply, as for the rest of `loams.meter.v1`: `NO_SIDE_EFFECTS` on reads, `idempotency_key` on `ImportBill` and `ApplyCostInputs`, AIP-158 pages, and `ErrorInfo.reason`. REST uses the Connect JSON paths (`POST /loams.meter.v1.CostService/<Method>`), as the meter's other services do today. §44 transcoding to `/v1/meter/costs…` follows when it lands for the rest of the package.

### 12.2 Authorization

There is a new OpenFGA relation, **`cost_viewer` on `platform`**. It is implied by `meter_admin`, and it is never assignable to `agent` or `service_account` principals, the same rule as D1603. The option `share_with_orgs` lets an org's `billing_viewer` read that org's rows: cost, unit costs and explain without bill details. They never see other orgs, pseudo-consumers, account ids or import files.

---

## 13. Exports (D2374, D2375)

### 13.1 OpenMeter-compatible CloudEvents (D2374)

Each allocation line (per day) becomes one CloudEvent, in the format OpenMeter's `POST /api/v1/events` accepts (§63 §6):

```json
{
  "specversion": "1.0",
  "id": "cost/2026-10-28/prod-eu1~compute/cpu/house/org_42/7/scan/r12",
  "source": "loams://prod-eu1/meter/cost",
  "type": "io.loams.dev.cost.allocation.v1",
  "subject": "org_42",
  "time": "2026-10-28T23:59:59Z",
  "datacontenttype": "application/json",
  "dataschema": "urn:loams:json:io.loams.dev.cost.allocation.v1",
  "data": {
    "cost_micros": 1234567, "currency": "USD", "day": "2026-10-28",
    "pool": "prod-eu1/compute", "kind": "cpu", "engine": "house", "project": "7",
    "query_class": "scan", "basis": "measured", "amount_basis": "declared",
    "usage": "5184.000000000", "usage_unit": "core_second", "ops": 120000,
    "report_digest": "9f2c…", "cost_run": 12
  }
}
```

- `cost_micros` is an integer, so OpenMeter's `SUM` stays exact. The micros are rounded per line with the same largest-remainder rule, so the micros of a pool-day still sum to its cost rounded to micros.
- The `id` includes `cost_run`, so a recompute is a new event. That means OpenMeter sums the old and new runs. To avoid this, the exporter sends only closed days that have never been sent, and a recompute of an already-exported day is sent as **adjustment events** (`type …cost.adjustment.v1`, `cost_micros` = new − old), as §63 Q963 does for late usage.
- Pseudo-consumers use the subject `_operator`.
- Suggested OpenMeter meters (in `docs/api/meter/openmeter-cost-meters.yaml`): `loams_cost_micros` (SUM of `$.cost_micros`, group by `engine`, `project`, `query_class`, `kind`, `basis`) over both types.

The CloudEvents go through the existing OpenMeter exporter when `export-openmeter` is configured, or into a file from `ExportCosts`.

### 13.2 FOCUS 1.3 (D2375)

FOCUS 1.3 added **split cost allocation** columns for data generators that split shared charges: `AllocatedMethodId`, `AllocatedMethodDetails`, `AllocatedResourceId`, `AllocatedResourceName` and `AllocatedTags` ([FOCUS_Spec v1.3](https://github.com/FinOps-Open-Cost-and-Usage-Spec/FOCUS_Spec/tree/v1.3/specification/datasets/cost_and_usage/columns), read 2026-10-29). That is exactly what Loams does, so Loams exports as a data generator of allocated charges:

- One row per allocation line per day: `ChargePeriodStart` and `ChargePeriodEnd` = the day.
- `BilledCost`, `EffectiveCost`, `ListCost` and `ContractedCost` = the line's amount (`ListCost` = the reference cost where a list price exists, otherwise the same amount). `BillingCurrency` = the reporting currency.
- `ChargeCategory` `Usage` (`Credit` for negative lines). `ChargeClass` is null. `ChargeDescription` = "Loams allocation of <pool> <kind>".
- `ServiceProviderName` and `InvoiceIssuerName` come from the source lines when there is one provider, and are otherwise `Loams (allocated)`. `HostProviderName` is the provider's name. `ServiceName` = `Loams <engine>`. `ServiceCategory` maps by kind (`cpu`, `memory` → Compute; `storage` → Storage; `network_*` → Networking; `io` → Storage).
- `ResourceId` = the pool, `ResourceType` = the kind.
- `AllocatedMethodId` = `loams-proportional-v1`. `AllocatedResourceId` = `<engine>/<tenant>/<project>/<query_class>`. `AllocatedTags` = `{"loams:engine", "loams:tenant", "loams:project", "loams:query_class"}`.
- `AllocatedMethodDetails` = `{"Elements":[{"AllocatedRatio": r, "UsageUnit": "<unit>", "UsageQuantity": q, "x_LoamsBasis": "...", "x_LoamsTrace": "..."}]}`. Here `r` is the line's share of its pool-day, and the ratios of one pool-day sum to 1, as the spec requires.
- `Tags` carries the source tags when there is one source.
- `ConsumedQuantity` and `ConsumedUnit` = the driver quantity and unit.
- Columns Loams cannot know (commitment discount, capacity reservation, SKU price) are null where the spec allows null.

The column list and each column's feature level are checked against the v1.3 column files in Task 7 (verify). The export is checked by the FinOps Foundation's FOCUS validator (verify its licence and pin before use, D1274). If the validator cannot be used, a checker built from the column files' rules stands in. FOCUS export is optional: it is produced on request (`ExportCosts`, `loams cost export --focus`) and never scheduled by default.

---

## 14. Console (D2376)

New pages under the billing plugin's operator section (`web/plugins/billing/src/operator/costs/*`). They are shown when the deployment runs the `meter` role and the caller holds `cost_viewer`, and they carry the **Preview** badge.

| Page | Shows |
|---|---|
| **Costs overview** | Period totals: input, attributed, idle, unattributed, shared, unmapped; billed vs invoice; a stacked daily chart by engine; the basis mix (share of cost by overall basis) |
| **Unit costs** | $ per 1M ops by engine × class, $ per GB-month by engine, $ per job by queue, $ per core-hour by pool. Used-capacity and fully loaded columns, sample flags, a basis badge per cell |
| **Breakdown** | A pivot by engine, tenant, project, class, pool and kind (any two), with filters; CSV download of the view |
| **Trends** | Daily, weekly or monthly series of cost and unit costs, with input-change markers |
| **Inputs** | `costs.yaml` versions with a diff, assets with amortisation schedules, the price table, bill imports (totals, unmapped by service, superseded), FX |
| **Explain** | A drawer on any number: the formula, the sources and the usage; links to the import or asset |
| **Org view** (only with `share_with_orgs`) | Settings → Billing → Costs: the org's own cost and unit costs |

The pages use the shared chart components and money formatting (D374). Cost amounts are shown to the precision their basis supports: whole cents in tables, full precision in Explain.

**Meter health** (§63 §18) gains:
- the cost job's last run per day;
- the unmapped share of the latest import;
- the unattributed share of telemetry (the `_unknown` engine).

---

## 15. CLI (D2377)

`loams cost` (crate `loams-cost`, feature `cli`, wired into `loams` like `loams spark`):

```
loams cost inputs apply costs.yaml [--reason …]     # ApplyCostInputs
loams cost inputs show [--version N]
loams cost import bill.csv.gz [--format focus|cur|csv --map start=…,cost=…] [--supersedes ID]
loams cost imports [--unmapped ID]
loams cost report --from 2026-10-01 --to 2026-11-01 --by engine,query_class [--granularity day] [--fully-loaded] [--json]
loams cost unit-costs --from … --to … [--by engine,class]
loams cost explain <trace-id>
loams cost export --from … --to … --format focus|openmeter > out.csv
# offline: no server; same code path
loams cost report --offline --inputs costs.yaml --bill bill.csv --usage usage.ndjson --from … --to …
loams cost report --from-bench bench/results/bq1/all/<run>-d16.json --price-core-hour 0.05   # labelled example
```

Online mode uses the server URL and API key of the other `loams` client commands. Every table the CLI prints has a basis column. `--json` prints the report as `CostService` returns it.

---

## 16. Security (D2378)

- **Writes** (`ApplyCostInputs`, `ImportBill`, `RecomputeCosts`) need `meter_admin`. They are audited in `_audit` (D100) with the principal, the reason and the input digest. Agent principals cannot hold `meter_admin` (D1603), so agents cannot change what things cost.
- **Bill files** can contain account ids, resource names and tags. Lines are stored in the `_system` namespace's House database. Org viewers never see them.
- **Integrity**: drivers come only from §63's usage events, which only service identities may produce (D1133). A tenant cannot lower its attributed cost by sending events.
- **Exports** carry no account ids by default. FOCUS `BillingAccountId` and `SubAccountId` are copied only with `--include-accounts`, for operator use.

---

## 17. Questions (answered with their defaults)

Each answer below is the recommended default. The owner, 2026-10-29: "yes start it, approve all defaults".

| # | Question | Default (approved) |
|---|---|---|
| Q1490 | Default asset life and residual when `costs.yaml` omits them? | **60 months, residual 0**, straight line. The trace marks the default |
| Q1491 | Show idle capacity separately or spread it over consumers? | **Show** (D2364). Spreading is an option, labelled `estimated` |
| Q1492 | Which bill amount drives allocation? | **`EffectiveCost`** (commitments amortised). `BilledCost` is kept for invoice reconciliation. CUR uses net unblended cost when present |
| Q1493 | Attribution granularity? | **UTC day.** Hourly is a later option if a use case needs it |
| Q1494 | Do orgs see their own costs by default? | **No.** The operator turns on `share_with_orgs` |
| Q1495 | FOCUS versions? | **Import 1.0–1.4** (the core columns are stable across them). **Export 1.3**, the first version with split cost allocation columns |
| Q1496 | Parquet bill files (CUR 2.0 and FOCUS exports are often Parquet)? | **CSV and CSV gzip first; Parquet in CA1 Task 11** with the `parquet` crate already in the workspace |
| Q1497 | Minimum sample thresholds? | **1,000 ops, 1 GiB-day, 10 jobs** per period (D2369) |
| Q1498 | Spread tax, support and account-level credits? | **No.** They go in the `shared` line. They are spread only with `options.shared: spread`, labelled `estimated` |
| Q1499 | Cost attribution without the House? | **The CLI's offline mode only.** `CostService` answers `cost_unavailable` |
| Q1500 | Bills in several currencies? | **Refused unless a declared FX rate covers the day**. Converted lines are `declared` |
| Q1501 | Where does revenue for unit economics come from? | **Plan prices on the meter's period totals, or a declared revenue import.** Nothing else. Without revenue, the margin columns are absent |
| Q1502 | Should Ostrium publish reference $ per 1M ops figures from its benchmarks? | **No dollar figures.** Publish CPU-seconds (and core-hours) per 1M ops with the hardware named, and the formula. Dollar examples appear only in CLI output labelled `example` |

---

## 18. Plan

[CA1](../plans/2026-10-29-ca1-cost-attribution.md) (D2379):

| # | Task |
|---|---|
| 0 | Design and plan (this document) |
| 1 | `loams-cost` core: money, kinds, consumers, allocation, conservation, basis, traces, digest |
| 2 | Cost inputs: `costs.yaml`, validation, amortisation, the price table, FX |
| 3 | Bill import: FOCUS, CUR 2.0 and legacy CUR, generic CSV; mapping rules; unmapped report |
| 4 | Unit costs, reports, trends, sample rules |
| 5 | Driver catalogue and the House driver query (`loams-meter-house::cost_sql`), with query-class compilation |
| 6 | `CostService` proto, the meter role's `costs` module, the cost job, House tables, and role wiring |
| 7 | Exports: FOCUS 1.3 and OpenMeter CloudEvents |
| 8 | CLI `loams cost` (online, offline, `--from-bench`) |
| 9 | Console cost pages |
| 10 | Node agent: `io.stat` and the `engine` label |
| 11 | Parquet bill import |
| 12 | BYOC tags on OpenTofu modules |
| 13 | Docs, the guide, the public-claims check |

The status file is `loams-wt/.he0-shared/ca1-status.md`.

---

## 19. What this amends

- **§63** (Loams Meter):
  - `loams.meter.v1` gains `CostService` (§63 §17.1).
  - The House `_meter` database gains `cost_lines` and `cost_allocations` (§63 §7.5).
  - The event types gain `io.loams.dev.usage.node.io.v1` and `io.loams.dev.cost.{allocation,adjustment}.v1` (§63 §6.3). The cost types are excluded from every meter, like `io.loams.dev.meter.*`.
  - The OpenFGA model gains `cost_viewer` on `platform` (§63 §19, §76).
  - Meter health (§63 §18) gains the cost job.
- **§59 / D1080**: the operator view gains the Costs pages. Settings → Billing gains Costs, shown only when shared.
- **§70 / §80**: the OpenTofu modules tag resources with `loams-pool` and `loams-kind` (Task 12).

## 20. Sources

Loams (on `dev` at `3bada72f`, read 2026-10-29):
- [§63](63-loams-meter.md) and plan [ME1](../plans/2026-10-19-me1-loams-meter.md);
- the meter lanes' notes (`loams-wt/.he0-shared/me1-notes.md`, `me-svc-status.md`);
- `crates/loams-meter*`, `crates/loams/src/api/meter.rs`, `web/plugins/billing`;
- the benchmarks [`docs/benchmarks/2026-10-28-1b-qps/combined.md`](../benchmarks/2026-10-28-1b-qps/combined.md) and [`meter.md`](../benchmarks/2026-10-28-1b-qps/meter.md).

External (read 2026-10-29):
- FOCUS specification: [versions](https://focus.finops.org/focus-specification/) (1.4 is the latest; 1.3 published 2025-12-08 and 1.2 on 2025-05-29, per the [releases](https://github.com/FinOps-Open-Cost-and-Usage-Spec/FOCUS_Spec/releases)); [v1.3 Cost and Usage columns](https://github.com/FinOps-Open-Cost-and-Usage-Spec/FOCUS_Spec/tree/v1.3/specification/datasets/cost_and_usage/columns), including [AllocatedMethodId](https://raw.githubusercontent.com/FinOps-Open-Cost-and-Usage-Spec/FOCUS_Spec/v1.3/specification/datasets/cost_and_usage/columns/allocatedmethodid.md) and [AllocatedMethodDetails](https://raw.githubusercontent.com/FinOps-Open-Cost-and-Usage-Spec/FOCUS_Spec/v1.3/specification/datasets/cost_and_usage/columns/allocatedmethoddetails.md); [BilledCost (v1.2)](https://raw.githubusercontent.com/FinOps-Open-Cost-and-Usage-Spec/FOCUS_Spec/v1.2/specification/columns/billedcost.md).
- AWS CUR 2.0 and legacy CUR column names (verify against the AWS Data Exports data dictionary in Task 3).
- OpenMeter's event ingest and meter definitions, as in §63 §26.
