# HE0 Task 10: the ClickHouse dialect on DataFusion (spike report)

Task 10 of [the HE1 plan](2026-10-21-he1-loams-house-engine.md) (phase HE0) asks three things of design [§75](../design/75-loams-house-engine.md) §6.3's "ClickHouse-dialect SQL frontend that produces DataFusion logical plans": how much of the dialect sqlparser's `ClickHouseDialect` parses, what DataFusion can plan, and what each missing piece costs. It also picks the DataFusion and iceberg-rust versions that share an Arrow major (Q1175). The work was done on 2026-10-21. The spike's code is on branch [`he0-t10-dialect-spike`](https://github.com/ostrium-labs/loams/tree/he0-t10-dialect-spike) (`spikes/he0-dialect/`, a Cargo workspace of its own). That branch is **never merged**, and no DataFusion dependency entered a merged crate. This file is the only output that merges. Its rulings are R10.1–R10.6 in the plan.

## Summary

- **Parse rate.** sqlparser 0.63.0's `ClickHouseDialect` parses **77.9 %** of the spike's 195-statement corpus. Only **75.4 %** parses faithfully, meaning the AST keeps what the text says. By statement kind:
  - `SELECT`: 87.0 % faithful.
  - `CREATE`: 46.4 %.
  - `ALTER`, mutations and `OPTIMIZE`: 29.4 %.
  - `INSERT`: 100 %.
  - The House's own HS1 differential corpus: 100 %.

  0.62.0, which DataFusion links, scores one statement lower. The two **lossy** parses matter most:
  - `FROM t FINAL` is read as the table alias `FINAL`, and `FROM t AS a FINAL` does not parse at all.
  - `ARRAY JOIN a AS x, b AS y` is read as an `ARRAY JOIN` followed by a cross join.
- **DataFusion silently drops ClickHouse constructs.** DataFusion 54.1's SQL planner accepts five things and ignores them without an error: `PREWHERE`, the `GROUP BY` modifiers (`WITH TOTALS`, `WITH ROLLUP`, `WITH CUBE`), a function's parameters (`quantile(0.95)(x)` plans as `quantile(x)`), `SAMPLE`, and `FINAL` (because sqlparser read it as an alias). DataFusion also lowercases every unquoted function name, whatever `enable_ident_normalization` says, while ClickHouse's names are case-sensitive (`toStartOfDay`, `argMax`).
- **Planning, as shipped.** Of the 20 representative Tier 0 queries, DataFusion 54.1 with stub tables and functions plans **6 correctly**. It plans **5 more wrongly but without an error**, and rejects the other 9.
- **Planning, with the spike's frontend.** The spike's AST pass (about 500 lines), DataFusion's own extension hooks (`TypePlanner`, `RelationPlanner`, a `UserDefinedLogicalNode` for `FINAL`) and stub functions together plan **all 20**. The same pass plans 90 of the corpus's 112 `SELECT`s, against 58 for DataFusion as shipped.
- **Cost.** What remains is semantics rather than syntax. The two largest items are ClickHouse's type and coercion rules (`/` is always `Float64`, booleans are `UInt8`, non-`Nullable` columns give default values in outer joins) and a real `FINAL` operator. Together with the rest, Tier 0's dialect work is about **100–140 engineer-days** (estimate; §4).
- **Versions (Q1175).**
  - **The pick: DataFusion 54.1.0 with iceberg 0.10.1, both on Arrow 58** (`cargo tree` shows one `arrow-array` 58.4.0). It is the only released, at-least-14-days-old pair on one Arrow major, and it matches the engine workspace (DataFusion 54.1, arrow 58.4) and `loams-house-lake` (iceberg 0.10.1, arrow 58).
  - **The planned move:** DataFusion 55.x with iceberg 0.11 on Arrow 59, once iceberg 0.11 is released and 14 days old. iceberg-rust's `main` has been on DataFusion 55 and Arrow 59.2 since 2026-09-03.
  - DataFusion 55.2 beside iceberg 0.10.1 brings two Arrow majors.
- **Recommendation for HE1.** **Write our own frontend and extension nodes on top of sqlparser and DataFusion, used as libraries. Send only small, dialect-neutral fixes upstream:**
  - sqlparser: the ClickHouse syntax it lacks.
  - DataFusion: errors instead of silent drops, and function names that follow `enable_ident_normalization`.

  ClickHouse semantics (alias scoping, type rules, `FINAL`, `WITH TOTALS`, the combinators) belong in Loams' crates, through the hooks DataFusion already exposes (§6). Nothing was posted upstream; §7 lists the candidates for the owner's consent.

## 1. Method

| What | Value |
|---|---|
| Crates | `datafusion-sql`, `-expr`, `-common`, `-functions`, `-functions-aggregate`, `-functions-window`, `-functions-nested`, `-optimizer` at **54.1.0** (logical planning only; no physical plans, no execution); `sqlparser` **0.62.0** (what DataFusion 54.1 and 55.x link) and **0.63.0** (the House's pin, HS1 R0.7), side by side |
| Corpus | `corpus/tier0.sql`: 175 statements written for this spike in the shapes of §75 §7.3's Tier 0 list. It covers DDL with every Tier 0 engine, type, TTL, codec, index and topology form, plus mutations, inserts with formats and settings, `SELECT` with each Tier 0 clause and function family, and the utility statements. `corpus/plan20.sql` holds 20 more `SELECT`s for planning. **No application source and no ClickBench text** was used. Each statement carries feature tags. The House's existing corpus (`conformance/clickhouse/corpus/`) was run too |
| Mined statements | **Not yet available.** Tasks 3–5 have not landed (only the `loams-house-mine` skeleton, #992, is merged). §8 says how the measurement is re-run on them |
| "Faithful" | The statement parses, `to_string()` re-parses to an equal AST, every ClickHouse keyword in the text (`FINAL`, `PREWHERE`, `TOTALS`, `SAMPLE`, `SETTINGS`, `FORMAT`, `ARRAY JOIN`, `GLOBAL`, `ANY`, `ASOF`, `WITH FILL`, `TTL`, `CODEC`, `ON CLUSTER`, `ENGINE`, …) survives the round trip, no table alias is `FINAL`, and no `ARRAY JOIN` list became a cross join |
| Planning | `SqlToRel` → the analyzer (type coercion) → the optimizer, over stub tables shaped like Langfuse and OpenPanel tables (written for the spike from §75 §4.4–§4.5, not from their source) and about 80 stub ClickHouse functions with typed returns, which plan but never execute. "As shipped" means DataFusion's own planners plus the stubs, with case-folded function lookup so that a lowercased name is not counted as a gap. "Proto-frontend" adds the spike's AST pass, a ClickHouse `TypePlanner` and a `RelationPlanner` for `FINAL`. A plan that succeeds while the AST holds a construct `SqlToRel` ignores is counted as **silent**: the detector reads the AST, and the ignored fields were read in `datafusion-sql` 54.1's source (§3.1) |

## 2. Parse rate

### 2.1 The spike's corpus (195 statements)

| Kind | Statements | Parse 0.62 | Parse 0.63 | Faithful 0.62 | Faithful 0.63 |
|---|---|---|---|---|---|
| `SELECT` (incl. the 20 planning queries) | 131 | 118 | 119 | 113 | **114 (87.0 %)** |
| `CREATE`, `DROP`, `RENAME`, `TRUNCATE` | 28 | 13 | 13 | 13 | **13 (46.4 %)** |
| `ALTER`, `DELETE FROM`, `OPTIMIZE` | 17 | 5 | 5 | 5 | **5 (29.4 %)** |
| `INSERT` | 8 | 8 | 8 | 8 | **8 (100 %)** |
| Utility (`SET`, `USE`, `SHOW`, `DESCRIBE`, `EXISTS`, `EXPLAIN`, `SYSTEM`, `KILL`) | 11 | 7 | 7 | 7 | **7 (63.6 %)** |
| **All** | **195** | **151 (77.4 %)** | **152 (77.9 %)** | **146 (74.9 %)** | **147 (75.4 %)** |

0.63 differs from 0.62 on one statement (`IN {ids: Array(String)}` parses in 0.63). For this corpus the House pin buys nothing over DataFusion's.

### 2.2 The House's corpus

| Corpus | Statements | Faithful 0.63 | Note |
|---|---|---|---|
| HS1 differential (`select/`, `mv/`) | 41 | **41 (100 %)** | Written against chDB in HS1 Tasks 11 and 17 |
| HS1 classifier and deny list (`classify/`, `deny/`) | 135 | 93 (68.9 %) | Adversarial by design (`INTO OUTFILE`, `SYSTEM`, `BACKUP`, identifier parameters, …): the front reads these by keyword today, not with sqlparser (`classify.rs`) |

### 2.3 What does not parse, or parses wrongly

| Area | Failing forms (0.63) | Where it matters |
|---|---|---|
| **`FINAL`** | `FROM t FINAL` → **alias `FINAL`** (lossy); `FROM t AS a FINAL`, `… FINAL JOIN … FINAL` fail | Query path (Tier 0, Langfuse) |
| **`ARRAY JOIN` lists** | `ARRAY JOIN a AS x, b AS y` → **an `ARRAY JOIN` plus a cross join** (lossy) | Query path (Tier 0) |
| `LIMIT BY` variants | `LIMIT n BY k LIMIT m`; `LIMIT m, n BY k` (plain `LIMIT n [OFFSET m] BY k` parses) | Query path (Tier 0) |
| Joins | `ANY LEFT JOIN`, `ASOF LEFT JOIN`, `GLOBAL IN` (`GLOBAL LEFT JOIN` parses) | Query path |
| Expressions | `WITH 10 AS n` and `WITH (subquery) AS x` (ClickHouse's scalar `WITH`); the ternary `c ? a : b`; tuple access `t.1` | Query path |
| Parameters | `{t: Identifier}` in `FROM` (typed value parameters parse) | Front (substitutes parameters before parsing) |
| `CREATE TABLE` | **Clause order**: `PARTITION BY` before `ORDER BY` or `PRIMARY KEY` fails, so does anything after it; `TTL` (table and column); `CODEC(...)`; `INDEX … TYPE … GRANULARITY`; table `SETTINGS`; `DateTime('tz')`; `ON CLUSTER` on `CREATE DATABASE` and `CREATE MATERIALIZED VIEW`; `DROP … ON CLUSTER … SYNC`; `AS t ENGINE = Distributed(...)` | Front (it already reads lake DDL with its own reader, `classify.rs` `lake_ddl`, HS1 R4.1) |
| `ALTER` | **`ALTER … DELETE WHERE`**, **`ALTER … UPDATE … WHERE`**, `ADD COLUMN IF NOT EXISTS`, `DROP PARTITION <expr>`, `MODIFY TTL`, `MODIFY SETTING`, `ADD INDEX IF NOT EXISTS`, `MATERIALIZE INDEX` | Front; mutations are HE2 (D1552), and R0.2 shows they answer 62 today for this reason |
| Utility | `EXISTS TABLE`, `EXPLAIN AST`, `SYSTEM …`, `KILL QUERY WHERE …` | Front (read by keyword today) |

The DDL and `ALTER` gaps are the front's problem, not DataFusion's, because the front owns those statements (§75 §8). HE2's mutations need either upstream parser support or the front's own reader extended to `ALTER … DELETE/UPDATE`.

## 3. Planning 20 Tier 0 queries

### 3.1 What DataFusion 54.1's planner ignores

Read in `datafusion-sql` 54.1.0's source and confirmed by the runs:

| Construct | Where `SqlToRel` drops it | Effect |
|---|---|---|
| `PREWHERE` | `select_to_plan` never reads `select.prewhere` | Rows the `PREWHERE` would remove are returned |
| `GROUP BY … WITH TOTALS / ROLLUP / CUBE` | `GroupByExpr::Expressions(exprs, _)` discards the modifiers | No totals row, no rollup rows |
| Function parameters (`quantile(0.95)(x)`, `windowFunnel(3600)(…)`, `topK(10)(x)`, `groupArray(10)(x)`) | `FunctionArgs::try_new` destructures `Function { .., }` without `parameters` | `quantile(x)` (the median), `windowFunnel` with no window |
| `SAMPLE` | `TableFactor::Table { name, alias, args, .. }` | The whole table is read |
| `FINAL` | sqlparser's alias (§2.3), planned as an alias | Duplicates and deleted rows are returned |
| `SETTINGS`, `FORMAT` | `Query { settings: _, format_clause: _, .. }` | Harmless: the front owns both. The frontend must still refuse or map settings that change results (`join_use_nulls`, `prefer_column_name_to_alias`, `final`) |
| Function-name case | `normalize_ident` lowercases every unquoted function name, even with `enable_ident_normalization = false` | `toStartOfDay` is looked up as `tostartofday` |

`LIMIT BY`, `ARRAY JOIN`, `WITH FILL`, `COLLATE` and multi-column `IN (subquery)` fail loudly, which is the right behaviour. `LIMIT BY`'s error is misleading, though ("No field named trace_id"), because its expressions are planned against an empty schema before the "not supported" check.

### 3.2 Results

"Silent" means the query planned and gave wrong results without an error.

| # | Query (Tier 0 feature) | DataFusion 54.1 as shipped | With the proto-frontend | What the frontend did |
|---|---|---|---|---|
| q01 | Daily counts, `uniq`, alias in `GROUP BY` | plans | plans | Quoted camelCase function names |
| q02 | `countIf`, `sumIf`, `uniqIf` | fails (`countif` unknown) | plans | `-If` → `agg(args) FILTER (WHERE cond)` |
| q03 | `quantile(0.95)(d)`, `quantiles(…)(d)` | **silent** (parameters dropped) | plans | Parameters → leading arguments |
| q04 | `FROM traces FINAL` | **silent** (`FINAL` as alias) | plans | `RelationPlanner` → `ReplacingFinal` extension node |
| q05 | `LIMIT 3 BY trace_id` | fails | plans | `row_number() OVER (PARTITION BY k ORDER BY …)` + filter |
| q06 | `GROUP BY country WITH TOTALS` | **silent** (no totals) | plans | `GROUPING SETS ((keys), ())`, see §4 |
| q07 | `ARRAY JOIN tags AS tag` | fails (`tags` read as a table) | plans | Derived table with `unnest()` |
| q08 | `LEFT ARRAY JOIN mapKeys(metadata) AS k` | fails | plans, **empty arrays drop** | As q07; `LEFT` needs an option DataFusion lacks |
| q09 | `arrayJoin(tags)` in `SELECT` | fails | plans | `arrayJoin()` → `unnest()` |
| q10 | `metadata['k']` | plans | plans | None needed |
| q11 | `windowFunnel(86400)(ts, c1, c2, c3)` | **silent** (window dropped) | plans | Parameters → leading arguments (stub UDAF) |
| q12 | `argMax(x, ver)` with `HAVING` on an alias | plans | plans | Quoted names |
| q13 | CTE and `LEFT JOIN` | plans | plans | None needed |
| q14 | `row_number()`, `lagInFrame()` over windows | plans | plans | Quoted names |
| q15 | `PREWHERE` | **silent** (filter dropped) | plans | Folded into `WHERE` |
| q16 | `uniqMerge(state)` | plans (stub) | plans (stub) | A real `-Merge` needs a state format (§4) |
| q17 | Aliases used in `WHERE` and in another select item | fails (`No field named d`) | plans | Aliases substituted (simplified, §4) |
| q18 | `arrayFilter(x -> …)`, `arrayMap(x -> …)` | fails | plans | → `array_filter(a, λ)`, `array_transform(a, λ)` |
| q19 | `{p: String}` parameters, `SETTINGS`, `FORMAT` | fails (`datetime64` read as a function) | plans | Front-style parameter substitution; `SETTINGS`/`FORMAT` left to the front |
| q20 | `CAST(x AS LowCardinality(String))`, `toStartOfInterval`, `multiIf`, `dateDiff` | fails (`LowCardinality` type) | plans | `TypePlanner` maps ClickHouse types to Arrow |

**Tallies.** As shipped: **6/20** plan correctly, **5/20** are silent, and 9 fail. With the proto-frontend: **20/20** plan. The same two configurations over the corpus's 112 `SELECT`s give 58 correct, 16 silent and 38 failing as shipped, and **90/112** with the frontend. On the HS1 `select/` corpus they give 17 correct and 1 silent as shipped, and 19/20 with the frontend (the twentieth needs `toTypeName`). The branch's `spikes/he0-dialect/plans/` has each query's rewritten SQL and optimized logical plan.

The 22 corpus `SELECT`s the frontend still cannot plan are:
- 11 parse failures (§2.3).
- `SAMPLE`, which is still dropped silently because the spike has no rewrite for it.
- `WITH FILL` (×2) and `COLLATE`.
- `numbers()` (×2): a table function DataFusion's `ContextProvider` supports but the spike did not register.
- `(a, b) IN (subquery)`.
- The `-Array` and `-ForEach` combinators, and `COLUMNS('re')`.
- A two-array lambda.
- `WHERE … AND active`, where `active` is `UInt8`: DataFusion needs a `Boolean`, while ClickHouse accepts any number.

### 3.3 Three findings from the plans

- **`FINAL` as an extension node works and composes.** The optimizer kept the `WHERE` filter above `ReplacingFinal`, so a non-key column's filter is not pushed below deduplication (correct). It also read every column under the node, because the spike did not implement `necessary_children_exprs`. HE1's node must allow pushdown only on sorting-key columns (`prevent_predicate_push_down_columns`), and must project the key, version and `is_deleted` columns plus whatever the query reads.
- **Type rules diverge silently.** In q17, `duration / 1000` (`UInt64 / Int64`) planned as `Decimal128(20, 0)` division. ClickHouse's `/` always returns `Float64`. Such a query plans and runs, and the oracle (Task 6) would report it as a mismatch.
- **`LowCardinality` plans cleanly as `Dictionary(Int32, Utf8)`.** The analyzer cast the literal to the dictionary type (`country = 'DE'`) rather than unpacking the column.

## 4. What DataFusion lacks, and what each gap costs

Costs are engineer-days including tests against the oracle (estimate). "Where" says who should fix it: **ours** means Loams' frontend or executor crates, **sqlparser** and **DataFusion** mean an upstream patch (§7).

| Gap | What DataFusion has / lacks | Approach | Cost (estimate) | Where |
|---|---|---|---|---|
| **Silent drops** (`PREWHERE`, `GROUP BY` modifiers, parameters, `SAMPLE`, `FINAL`-as-alias) | Ignores them (§3.1) | The frontend walks the AST and rewrites every field `SqlToRel` ignores, or answers `48` naming it, **before** planning. This is a whitelist check and the first HE1 task | 1–2 | ours; DataFusion (error instead of ignore) |
| Function-name case | Lowercases unquoted names | Quote camelCase names in the frontend (done in the spike) | 0.5 | ours; DataFusion (honour `enable_ident_normalization`) |
| **`LIMIT BY`** | Fails loudly | Rewrite to `row_number()` + filter (done, q05); `LIMIT n BY k LIMIT m` and `LIMIT m, n BY k` need parser support; a per-group top-N operator later for speed (HE3) | 4 (+5 in HE3) | ours; sqlparser |
| **`WITH TOTALS`** | Dropped silently; `GROUPING SETS` exists | `GROUPING SETS ((keys), ())` gives the numbers (q06). ClickHouse, however, computes totals before `ORDER BY`/`LIMIT`, applies `totals_mode` with `HAVING`, and puts the totals row in the format's totals slot (`"totals"` in `JSON*`, a blank-line block in `TSV`). So it needs an extension node with a second output, or two plans over one input, plus the front's format writers | 6–8 | ours |
| **`ARRAY JOIN`** | `unnest()` in projections; `UnnestOptions` only has `preserve_nulls` | Derived table with `unnest()` (q07). **`LEFT ARRAY JOIN`** must keep rows with **empty** arrays and give the element's default value, which `unnest` cannot (q08); rewrite to `if(empty(a), [default], a)` or add an upstream option. Arrays in one `ARRAY JOIN` zip and must have equal sizes (DataFusion pads with nulls). `arrayJoin()` outside the projection needs hoisting. The comma list needs the sqlparser fix | 8–10 | ours; sqlparser; DataFusion (optional `preserve_empty`) |
| **`FINAL`** | No such concept | sqlparser support (a `final` flag on the table factor). Planning through `RelationPlanner` → `ReplacingFinal` (done, q04). A physical operator: a merge of sorted runs by sorting key, keeping the greatest version per key, dropping `is_deleted = 1`, **within a partition only** (ClickHouse never merges across partitions); predicate pushdown only on key columns; `PREWHERE` applied before deduplication. HS1's `read.rs` already defines the House's rule (greatest `(ver, file seq, row)`, R0.2), and the operator ports it. Deletion vectors (HE2) make it cheaper later | 12–15 | ours; sqlparser |
| **`-If`** | `FILTER (WHERE …)` exists | Rewrite (done, q02) | 1 | ours |
| **`-State` / `-Merge`** (and `-MergeState`) | `Accumulator::state()` and `merge_batch()` exist | A generic wrapper: `xState` returns the accumulator's state as a versioned binary column, `xMerge` merges it and evaluates. It is **not** byte-compatible with ClickHouse's `AggregateFunction` serialization, which matters only if a client inserts states produced by ClickHouse (refuse those with `48`) | 8–10 | ours |
| Other combinators (`-Array`, `-ForEach`, `-OrNull`, `-OrDefault`, `-Distinct`, `-SimpleState`, `-Resample`) | None | The same wrapper approach, with a name parser for compositions (`sumIfArray`, `uniqMergeState`) | 5 | ours |
| **`LowCardinality`** | `Dictionary` arrays; the analyzer casts literals to the dictionary | `TypePlanner` (done, q20). Keeping dictionaries end to end (Parquet dictionary pages read as Arrow dictionaries, kernels that do not unpack) is HE3 performance work | 2 (+5 in HE3) | ours |
| **`PREWHERE`** | Dropped silently; Parquet row filters (`pushdown_filters`, `reorder_filters`) give late materialization | Fold into `WHERE` (done, q15) and keep its conjuncts first as a hint to the Parquet row filter; under `FINAL`, apply before deduplication (in the `FINAL` row) | 2 | ours |
| **`windowFunnel`** | None | A UDAF: state is the list of `(timestamp, event bitmask)`, two-phase merge, modes `strict_order`, `strict_deduplication`, `strict_increase`, `strict_once`, `allow_reentry`; the parameters are literal leading arguments | 5–7 | ours |
| **Parametric aggregates** | Parameters dropped silently | Parameters → leading literal arguments (done, q03, q11), validated per function. The `quantile*` family: `quantile` is reservoir sampling, so non-deterministic (the oracle needs a tolerance); `quantileExact`, `quantileTDigest` and `quantiles` map in part onto `percentile_cont` and `approx_percentile_cont`. `topK`, `groupArray(n)` and `histogram` are small UDAFs | 7–9 | ours |
| **Alias scoping** | Aliases only in `GROUP BY`, `HAVING`, `ORDER BY`, `QUALIFY` | ClickHouse aliases are visible everywhere, including `WHERE` and other select items, and an alias **shadows a column** of the same name (`prefer_column_name_to_alias = 0`). The spike's substitution (q17) handles simple cases. HE1 needs the source schema, self-references (`f(x) AS x`), subqueries and lambda parameters. This is a likely source of silent mismatches | 4–6 | ours |
| **Types and coercion** | DataFusion's coercion; a replaceable `TypeCoercion` analyzer rule; per-UDF `coerce_types` and return types | ClickHouse's rules: `/` → `Float64`; `+` and `*` widen (`UInt8 + UInt8 → UInt16`); integer overflow wraps; booleans are `UInt8` and `WHERE` takes any number; non-`Nullable` columns give **default values** in outer joins (`join_use_nulls = 0`) and in missing `Map` keys; sqlparser reads ClickHouse's `Int8` as Postgres's 64-bit `int8` (fixed in the spike's `TypePlanner`). A ClickHouse coercion rule plus UDF return types; open-ended, driven by the oracle | 20–25 | ours; sqlparser (`Int8`) |
| Higher-order functions | `array_transform`, `array_filter`, `array_any_match`, one array each (`HigherOrderUDF`) | ClickHouse's lambda functions (`arrayMap`, `arrayFilter`, `arrayExists`, `arrayAll`, `arrayCount`, `arraySum`, `arrayFirst`, `arraySort`, `arrayFold`, …) over several arrays, as our own `HigherOrderUDF`s | 5–8 | ours |
| `ANY` / `SEMI` / `GLOBAL` joins; scalar `WITH`; `?:`; `t.1` | Not parsed | Parser support, then: `ANY LEFT JOIN` as a deduplicated right side; `GLOBAL` ignored (one bucket); scalar `WITH` substituted | 6–8 | sqlparser; ours |
| `(a, b) IN (subquery)` | Single-column `IN` only | Rewrite to a semi join | 2 | ours |
| Table functions (`numbers`, …) | `ContextProvider::get_table_function_source` | Register the House's safe table functions; everything that reaches outside the namespace stays `344` (D1558) | 2 | ours |
| `CAST(x, 'T')`, `COLUMNS('re')`, `COLLATE`, `toTypeName` | None | Rewrite (`CAST`, done) and small additions; `toTypeName` needs the reverse of the type map | 3 | ours |
| `SAMPLE` | Dropped silently | Refuse with `48` until a table declares a sampling key; then a `RelationPlanner` filter on the sample key's hash | 1 (refuse), 3 (if mined) | ours |
| `WITH FILL` / `INTERPOLATE` | Fails loudly | A gap-filling sort node. Not in §75 §7.3's list; include it if mining finds dashboards using it | 5 (if mined) | ours |
| `ASOF JOIN` | None | An operator; Tier 1 unless mining finds it | 10 (if mined) | ours |
| DDL and `ALTER` parsing | Not DataFusion's | The front's reader, or upstream sqlparser support for §2.3's forms; `ALTER … DELETE/UPDATE` first, for HE2 | 8–12 (HE2) | sqlparser or ours (front) |

**Total for Tier 0's dialect work in HE1**, excluding the `if mined` rows, HE3's performance follow-ups and HE2's DDL: about **100–140 engineer-days** (estimate). The type-rule work and `FINAL` are about a third of it. The syntax fixes are a small part, and they can go upstream.

## 5. Versions (Q1175)

Checked on crates.io and GitHub on 2026-10-21. The "one Arrow" column comes from `cargo tree -e normal -d` on the spike branch.

| Set | DataFusion | iceberg-rust | Arrow | sqlparser | One Arrow? | Status |
|---|---|---|---|---|---|---|
| **A (picked)** | **54.1.0** (2026-07-21) | **0.10.1** (2026-08-01) | **58** | 0.62.0 | **yes**: one `arrow-array` 58.4.0 | Both released and 14 days old or more; matches the engine workspace (`datafusion` 54.1, `arrow` 58.4) and `loams-house-lake` (iceberg 0.10.1, arrow 58) |
| B (next) | 55.1.0 (2026-09-11) or 55.2.0 (2026-10-07) | `main` (unreleased): DataFusion 55.0 and Arrow 59.2 since apache/iceberg-rust#2958 (2026-09-03) | 59 | 0.62.0 | yes, once iceberg 0.11 ships | Waits for an iceberg-rust release; 59 is the `fabric/` workspace's Arrow (the `adbc_core` cap, CN1 R4) |
| C (later) | `main` (to be 56) | – | 60 | 0.63.0 | – | Unreleased. Arrow 60 is outside `fabric/`'s `< 60` cap, so the engine crates would carry their own Arrow beside the front's, as `loams-house-lake` does today |
| ✗ | 55.2.0 | 0.10.1 | 58 + 59 | – | **no**: `arrow-array` 58.4.0 and 59.3.1 | Rejected |

Three things come with the pick:
- **`iceberg-datafusion` is not used.** 0.10.1 requires DataFusion `^53.1`, so it cannot sit beside 54.1. iceberg-rust also moved its DataFusion integration to `apache/datafusion-iceberg` (apache/iceberg-rust#3228, 2026-09-17). The House needs its own `TableProvider` anyway, for `FINAL`, deletion vectors, foyer and pruning, so `loams-house-iceberg` builds on `iceberg` alone.
- **Two sqlparsers in `fabric/`.** DataFusion 54.1 and 55.x link sqlparser **0.62**; the House front pins **0.63**. HE1's `loams-house-sql` must parse with DataFusion's sqlparser (`datafusion_sql::sqlparser`), because `SqlToRel` takes that AST. The front can keep 0.63 for its classifier (they agree on this corpus except one statement), or move to 0.62 so that one parser serves both. Set C would bring them back to one version.
- **Agent E (Task 11, iceberg-rust v3)** may need `main` for deletion-vector reads, which are not in 0.10.1 (§75 §4.1). Reading v3 from `main` in a spike does not change this pick, but if HE1 needs those reads before iceberg 0.11 ships, Set B through a `[patch]` git dependency is the fallback. The controller reconciles the two spikes.

## 6. Recommendation for HE1

**Write our own frontend and extension nodes, using sqlparser and DataFusion as libraries, and send upstream only the small, dialect-neutral fixes.** Neither "patch DataFusion to speak ClickHouse" nor "write our own planner" is the right base:

- **Patching DataFusion upstream with ClickHouse semantics is the wrong place.**
  - DataFusion's SQL planner is dialect-neutral, and ClickHouse's alias scoping, coercion rules, `FINAL` and `WITH TOTALS` would be special cases its maintainers have no reason to own.
  - DataFusion 54 already exposes the hooks this needs: `TypePlanner`, `RelationPlanner`, `ExprPlanner`, `HigherOrderUDF`, `UserDefinedLogicalNode`, custom analyzer rules, UDFs and UDAFs. The spike used the first two and the extension node, and planned all 20 queries without patching DataFusion.
- **Writing our own planner from the AST to `LogicalPlanBuilder`** would give full control of scoping and types, but it re-implements joins, subqueries, CTEs, windows, `unnest` and set operations, which `SqlToRel` handles today: about 40–60 engineer-days (estimate) before any ClickHouse work. **Not now.** Revisit at HE1's midpoint only if the oracle shows alias or type rules that a rewrite plus a coercion rule cannot express.
- **The shape for `loams-house-sql`:**
  1. Parse with DataFusion's sqlparser (`ClickHouseDialect`).
  2. Run a normalisation pass with ClickHouse semantics. It refuses (`48`, naming the construct) any AST field `SqlToRel` would ignore. It rewrites aliases, combinators, parameters, `LIMIT BY`, `ARRAY JOIN`, `PREWHERE`, casts and function names.
  3. Plan with `SqlToRel` over a House `ContextProvider` (the ClickHouse `TypePlanner`; a `RelationPlanner` for `FINAL` and table functions; the profile's functions as UDFs and UDAFs).
  4. Add extension nodes (`ReplacingFinal`, `Totals`, later `LimitBy` and `LeftArrayJoin`) with their physical planners in `loams-house-exec`.

  The ClickHouse coercion rule replaces DataFusion's `TypeCoercion` analyzer rule.
- **Upstream** (with the owner's consent, §7):
  - sqlparser: the syntax gaps of §2.3. They are pure syntax and useful to every ClickHouse user of sqlparser.
  - DataFusion: errors instead of silent drops, and case handling for function names. Both are small and dialect-neutral.

  Patches would be carried through a `[patch.crates-io]` git dependency only while a PR is open, as Q1171 rules for iceberg-rust.
- **First HE1 tasks, in order:**
  1. The silent-drop guard and its test, which fails on any ignored field.
  2. The ClickHouse `TypePlanner` and coercion rule, under the oracle.
  3. `FINAL`.
  4. The combinators and the parametric aggregates, including `windowFunnel`.
  5. `LIMIT BY`, `ARRAY JOIN` and `WITH TOTALS`.
  6. The rest by mined frequency.

## 7. Upstream candidates (none posted; the owner's consent is needed)

| Project | Candidate | Size (estimate) |
|---|---|---|
| sqlparser-rs | `FROM t [AS a] FINAL` as a flag on `TableFactor::Table`, not an alias | 2–3 days (AST change) |
| sqlparser-rs | `ARRAY JOIN a AS x, b AS y` as one join with a list | 1–2 days |
| sqlparser-rs | `LIMIT n BY k LIMIT m`; `LIMIT m, n BY k` | 1 day |
| sqlparser-rs | `ANY`/`ALL` join strictness, `ASOF LEFT JOIN`, `GLOBAL IN`; scalar `WITH expr AS name`; `?:`; `t.1` | 3–5 days |
| sqlparser-rs | ClickHouse `CREATE TABLE` clauses in any order with `TTL`, `CODEC`, skip indexes, `SETTINGS`; `DateTime('tz')`; `ON CLUSTER` on `CREATE DATABASE` and `CREATE MATERIALIZED VIEW`; `DROP … SYNC` | 4–6 days |
| sqlparser-rs | `ALTER … DELETE WHERE`, `ALTER … UPDATE … WHERE`, `DROP PARTITION <expr>`, `MODIFY TTL`, `MODIFY SETTING`, `ADD/MATERIALIZE INDEX`, `ADD COLUMN IF NOT EXISTS` | 3–5 days |
| sqlparser-rs | ClickHouse `Int8` as 8 bits, distinct from Postgres's `int8` | 1 day |
| DataFusion | `SqlToRel` returns "not supported" for `PREWHERE`, `GROUP BY … WITH` modifiers, function `parameters` and table `SAMPLE`, instead of ignoring them | 1–2 days |
| DataFusion | Function-name normalisation follows `enable_ident_normalization` | 1 day |
| DataFusion (optional) | `UnnestOptions` keeps empty lists (for `LEFT ARRAY JOIN`) | 2–3 days |

## 8. Follow-ups

- **Mined statements.** When Tasks 3–5 land their scrubbed inventories, re-run the parse rate on them from the spike branch: `he0-dialect-spike parse "<app>" <dir-of-.sql>` loads plain `.sql` files split on `;`. Then append each application's rate to this report. This report's corpus is hand-written, so its rates show the dialect's coverage, not how often applications use each feature.
- **The oracle.** Task 6's runner should seed its corpus with the spike's 20 queries over Task 6's generated data. q06 (totals), q08 (`LEFT ARRAY JOIN`), q11 (`windowFunnel`) and q17 (alias scoping and `/`) are the expected first mismatches for the Rust engine.
- **The branch.** `he0-t10-dialect-spike` stays unmerged. Its build output (the shared target's `he0-dialect-spike` artifacts) was deleted after the runs.
