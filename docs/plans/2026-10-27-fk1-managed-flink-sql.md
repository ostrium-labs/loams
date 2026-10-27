# FK1 — Managed Flink SQL

> **For agentic workers:** REQUIRED SUB-SKILL: use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Work task by task, test first. Where this plan gives exact names, paths, branches or tests, use them verbatim. Record any deviation in "Rulings made during execution" at the end of this file.
>
> **Status: specified 2026-10-27; code in progress.** Track FK, design [§81](../design/81-managed-stream-batch-engines.md) (D1900–D1931 and the lanes' D1950–D2019 **approved by the owner on 2026-10-27** with all defaults; Q1400–Q1411 answered). The track id is **FK**, not FL: FL1 and FL2 are the Fabric plans. The lanes are still called FL-a (core, controller, gateway, deploy) and FL-b (catalogs, connectors, API, image, CLI). They started on 2026-10-27, at the same time as the design. Their assumptions (D1970–D1989) and ENG81's rulings (R13, R14, R21–R26) are in `loams-wt/.he0-shared/engines-81.md`. No local builds that wave: CI is the check. Nothing here has run end to end.

**Goal:**
- Each namespace can run Apache Flink SQL instances: one session cluster and one Flink SQL Gateway per instance.
- Users reach an instance with Flink's own SQL Client (the raw SQL Gateway REST protocol, through `loams-gateway`) or Loams' API, CLI and console.
- Pipelines (`INSERT`, `STATEMENT SET`) survive suspend, resume, restarts and patch upgrades through savepoints on S3.
- Catalogs and connectors come wired:
  - Iceberg `loams` on Lakekeeper;
  - Kafka through the Iggy gateway, at-least-once;
  - JDBC `loams_pg`;
  - Postgres CDC (experimental until checked).
- Everything is tested against Flink's own SQL tests and Nexmark.

**Architecture:**
- `crates/loams-flink` is a stateless adapter behind `loams-engines` (RW1 Tasks 3–6). It holds a SQL Gateway REST client (v3, negotiating up to v4), a JobManager REST client (stop with savepoint, overview), config rendering and the lifecycle, plus FL-b's catalog, connector, API and CLI modules.
- Compose runs `jobmanager`, `taskmanager` and `sql-gateway`. Kubernetes runs a `FlinkDeployment` in session mode through the Flink Kubernetes Operator 1.16.1, plus a SQL Gateway Deployment.
- Pipelines are stored as SQL with secret refs plus their last savepoint, and replayed through a session bootstrap. There are no compiled plans and no catalog store in FK1 (D1909).

**Tech stack:**
- Rust: tokio, reqwest, axum, serde, `time`.
- Flink 2.2.1 (`flink:2.2.1-scala_2.12-java21` by digest) and the Flink Kubernetes Operator 1.16.1.
- The jar bundle: Iceberg 1.12.0, Kafka 5.0.0-2.2, JDBC 4.1.0-2.2, Flink CDC 3.7.0, Hadoop client 3.4.2, PostgreSQL JDBC 42.7.14.
- Flink's SQL Client for conformance, and Nexmark.

**Effort (estimate):** 3–4 engineer-weeks after RW1 Tasks 1 and 3–5. Tasks 1–5 (FL-a) and 6–10 (FL-b) run in parallel. Task 11 needs Tasks 3 and 9; Task 12 needs Task 5; Task 13 is last.

## Global Constraints

- **Worktrees and branches.** As RW1. FL-a owns `Cargo.toml`, `src/lib.rs`, `src/{error,http,sql_gateway,rest,config,storage,k8s,controller,runtime_kube,engine}.rs`, `src/gateway/*`, `deploy/release/compose.flink.yaml` and `deploy/flink/k8s/*`. FL-b owns `src/catalog/*`, `src/connectors/*`, `src/api.rs`, `src/cli.rs`, `tests/goldens/*` and `deploy/flink/{Containerfile,connector-jars.toml,fetch-jars.sh}`. Union of `Cargo.toml` entries; nothing deleted.
- **Decision IDs (R0).** Cite D1970–D1989; add no decision-log rows.
- **Licences (D1901).** Apache Flink and ASF connectors, Flink CDC (ASF) and the Apache Iceberg runtime only. The PostgreSQL driver is BSD-2-Clause. No Ververica (VVR, VVP) or Confluent pieces. Every jar is checked by SHA-256 against `connector-jars.toml`. Test `jar_manifest_licences_allowed`.
- **No user code in FK1 (D1930).** No UDF jars, no `ADD JAR`, no DataStream. The gateway refuses `ADD JAR` and `CREATE FUNCTION … USING JAR`. Test `gateway_refuses_add_jar`.
- **Protected keys (D1977).** `SET` and `RESET` of the checkpoint and savepoint directories, the state backend, HA and `execution.state-recovery.path` are refused at the gateway.
- **Never public; credentials (D1917, D1918).** As RW1. Kafka uses the instance's Iggy user, never the user's key (R26).
- **Delivery (D1915).** The Kafka sink is always `at-least-once`, and the builder refuses `exactly-once`.

## Review Focus

1. **State outside the instance's prefix.** Tests: `protected_set_keys_refused`, `scripts_outside_prefix_403`, `render_dirs_under_state_prefix`.
2. **A pipeline lost on suspend or upgrade.** Tests: `suspend_stops_with_savepoint`, `upgrade_aborts_on_failed_savepoint`, `resume_restores_from_last_savepoint`.
3. **A credential persisted by Flink.** Tests: `bootstrap_catalogs_are_session_scoped`, `saved_pipeline_stores_refs_only`, `redacted_render_has_no_secret`.
4. **Cross-tenant session use.** Tests: `foreign_session_handle_404`, `gateway_routes_by_hostname`.
5. **Exactly-once promised where it is not.** Test: `kafka_sink_refuses_exactly_once`.

## Task 0 — Rulings

- **Pins:** the Flink 2.2.1 image digest (D1970); the jar bundle checksums (D1986); the operator 1.16.1 Helm chart digest; whether the operator 1.16.1 supports Flink 2.2 in session mode (read its compatibility table).
- **SQL Gateway at 2.2.1:** the REST API versions offered (`/api_versions`), whether `scripts` (FLIP-480, Q1409) is present, and the `STOP JOB … WITH SAVEPOINT` statement's result shape.
- **Iceberg v3:** whether Iceberg 1.12.0's Flink runtime reads v3 tables the House writes, including deletion vectors. Record per feature.
- **Flink CDC 3.7.0** and Flink 2.2.1 compatibility, and the `postgres-cdc` option names.
- **Q1400 is answered** (the owner's request of 2026-10-27): managed Flink SQL goes ahead, and FK1 is not gated on it.

Files: this plan. Tests: none. Gate: rulings recorded.

## Task 1 — Crate shell and REST clients (lane FL-a)

- **The SQL Gateway client** (D1971): `open_session`, `execute`, `fetch_result` (paging by `nextResultUri`, retry on `NOT_READY`, end on `EOS`), `cancel`, `close_operation`, `close_session`, `heartbeat`, and `negotiate()` (v4, v3, v2). `scripts` is used only when v4 is offered and a caller asks.
- **The JobManager REST client:** `overview`, `jobs`, `stop` (`POST /jobs/:id/stop` with a CANONICAL savepoint), `savepoint status` and `checkpoints`.
- **FL-b's `FlinkSqlBackend` trait** is implemented for the SQL Gateway client in `api.rs`, so there is one client (engines-81.md, FL-b's seam).

Files: `crates/loams-flink/{Cargo.toml,src/lib.rs,src/error.rs,src/http.rs,src/sql_gateway.rs,src/rest.rs}`, `tests/{sql_gateway_client,rest_client}.rs` (recorded responses). Tests: `negotiate_picks_highest`, `fetch_follows_next_uri`, `not_ready_is_retried`, `stop_with_savepoint_request_shape`. Gate: CI green.

## Task 2 — Config, storage and HA rendering (lane FL-a)

- **`config.yaml`** for the JobManager, TaskManagers and SQL Gateway (D1973–D1975):
  - RocksDB, incremental, 60 s, `EXACTLY_ONCE`, retained, 3 kept, exponential-delay restarts;
  - CANONICAL savepoints by default;
  - `execution.checkpointing.dir = s3://loams-engines/engines/<ns>/<id>/checkpoints`, and the savepoint directory alongside it;
  - both the 2.x and legacy keys;
  - `flink-s3-fs-presto` with `s3.endpoint` and path style.
- **Credentials** go in env only, never in `config.yaml`.
- **HA:** in k8s, `high-availability.type: kubernetes`, `storageDir = …/ha`, and `kubernetes.cluster-id = eng-<26>`. In compose, HA is off.

Files: `crates/loams-flink/src/{config.rs,storage.rs}`, `tests/config-goldens/*`. Tests: `render_dirs_under_state_prefix`, `config_has_no_secret`, `k8s_ha_rendered`, `compose_ha_off`. Gate: CI green.

## Task 3 — The adapter and the compose runtime (lane FL-a)

- **`ManagedEngine` for Flink** (R1). Sizes come from the tier table (TaskManagers × slots, R19).
- **Lifecycle (D1976):**
  - create;
  - scale (TaskManager replicas and slots; running jobs rescale only on stop and resume);
  - upgrade (Task 11);
  - suspend (stop every managed pipeline with a CANONICAL savepoint, then replicas to 0);
  - resume;
  - delete (cancel, remove; prefix only with `purge`).
- **The compose recovery record** `recovery/jobs.json`, holding the statement refs and the last checkpoint or savepoint per job. It is written after each completed checkpoint the controller observes, and replayed after a JobManager restart (D1974).
- **`controller::JobCatalog`** is implemented by FL-b's `JobStore` (Task 9).
- **Compose:** `deploy/release/compose.flink.yaml`, profile `flink`, with `jobmanager`, `taskmanager`, `sql-gateway` and `flink-init` on an internal `flink` network. No host ports: `engine-gateway` fronts them.

Files: `crates/loams-flink/src/{engine.rs,controller.rs}`, `deploy/release/compose.flink.yaml`. Tests: `suspend_stops_with_savepoint`, `resume_restores_from_last_savepoint`, `recovery_record_replayed_after_jm_restart`, `no_public_ports_in_engine_compose`, `delete_without_purge_keeps_prefix`. Gate: CI green.

## Task 4 — Kubernetes through the operator (lane FL-a)

- A `FlinkDeployment` in session mode per instance in `eng-<26>`, plus a SQL Gateway Deployment pointed at the JobManager's REST endpoint.
- The NetworkPolicy of §81 §7.2: the SQL Gateway and JobManager REST admit only `loams-gateway` and the controller.
- A Role for HA ConfigMaps and leases.
- The sandbox profile of D1905: non-root, drop ALL, RuntimeDefault seccomp, and a service-account token only because Kubernetes HA needs it.
- The web UI with `web.submit.enable=false` and `web.cancel.enable=false`.
- The base is a kustomize directory, and per-instance objects are rendered by `loams_flink::k8s`.

Files: `deploy/flink/k8s/**`, `crates/loams-flink/src/{k8s.rs,runtime_kube.rs}`. Tests: `flinkdeployment_session_golden`, `networkpolicy_denies_metadata_ip`, `web_ui_submit_and_cancel_disabled`, `no_public_service`. Gate: kustomize build and schema check in CI.

## Task 5 — The gateway route (lane FL-a)

- **The raw SQL Gateway REST protocol, routed by hostname** (R25): `<instance>.<ns>.<region>.flink.<base-domain>` with paths `/v{2,3,4}/…` unchanged, so `sql-client.sh gateway --endpoint https://…` works. In compose, the first Flink instance is on `127.0.0.1:8083` of `engine-gateway`.
- **Guards (D1977):**
  - session handles are bound to the principal and instance, and a foreign handle is a 404;
  - protected `SET` and `RESET` keys get 403;
  - `ADD JAR` and `CREATE FUNCTION … USING JAR` get 403;
  - `scripts` with a `scriptUri` outside the prefix gets 403.
- **The web UI**, read-only, on the same host under `/ui/`.
- **Authentication:** Loams API keys and tokens through the shared verifier, checking `can_connect`. The keys file is a test seam only (D1978).

Files: `crates/loams-flink/src/gateway/{mod,auth,guard}.rs`, the module wiring in `crates/loams-gateway`, the `engine-gateway` section in `compose.lake.yaml`. Tests: `gateway_routes_by_hostname`, `foreign_session_handle_404`, `protected_set_keys_refused`, `gateway_refuses_add_jar`, `scripts_outside_prefix_403`. Gate: CI green.

## Task 6 — Catalogs, builders and the session bootstrap (lane FL-b)

- **Typed Flink SQL builders** with backtick quoting and deterministic output (D1980).
- **The bootstrap** (D1981), run at every session open: the `SET` options, then `CREATE CATALOG IF NOT EXISTS loams` (Iceberg on Lakekeeper, `base-namespace = ns_<id>`, vended credentials; R26 renames `loams_lake` to `loams`), then `CREATE CATALOG IF NOT EXISTS loams_pg` (JDBC), then the temporary tables the caller asked for.
- **Catalogs are session-scoped.** There is no catalog store, and Flink persists no credential.

Files: `crates/loams-flink/src/catalog/{mod,iceberg,jdbc,bootstrap,quote}.rs`, `tests/goldens/*`, `tests/sql_goldens.rs`. Tests: `bootstrap_catalogs_are_session_scoped`, `iceberg_catalog_named_loams`, `quote_ident_backticks`, `bootstrap_golden`. Gate: CI green.

## Task 7 — Connectors and CDC (lane FL-b)

- **Kafka and `upsert-kafka`** through `kafka-gateway:9093`, as the instance's Iggy user, with SASL/PLAIN and the shaded login module (D1983 as amended). The sink is `at-least-once`.
- **JDBC** to Loams Postgres with `sslmode=require`.
- **`postgres-cdc`** with incremental snapshot, pgoutput, slot `loams_fk_<26>_<n>`, and a publication created by the controller (D1984, D1916).
- **(verify) CDC on the fork:** the same nightly check as RW1 Task 8, with a Flink CDC leg. Until it passes, the guide labels CDC **experimental**.
- **Secrets** are refs (D1985), resolved at session open, with a redacted render.

Files: `crates/loams-flink/src/connectors/{mod,kafka,jdbc,postgres_cdc,secret}.rs`, `.github/workflows/engines-nightly.yml` (job `fk-cdc-on-loams-postgres`). Tests: `kafka_sink_refuses_exactly_once`, `kafka_uses_instance_credentials`, `cdc_slot_name_format`, `redacted_render_has_no_secret`. Gate: CI green; the nightly result recorded.

## Task 8 — The image (lane FL-b)

- `deploy/flink/Containerfile`: a fetch stage downloads each jar in `connector-jars.toml` and checks its SHA-256, then copies the jars into `/opt/flink/lib` of the pinned official image (D1987). The S3 plugin is enabled with `ENABLE_BUILT_IN_PLUGINS`.
- The image is `ghcr.io/ostrium-labs/loams-flink:2.2.1-<bundle>`. It is built by the release workflow, not in this wave.
- The manifest is mirrored as a Rust const and checked equal by a unit test.

Files: `deploy/flink/{Containerfile,connector-jars.toml,fetch-jars.sh}`, `crates/loams-flink/src/connectors/bundle.rs`. Tests: `jar_manifest_matches_const`, `jar_manifest_licences_allowed`, `fetch_script_checks_sha256` (shellcheck plus a dry run). Gate: CI green.

## Task 9 — The API, pipelines and the console tabs (lane FL-b)

- **Loams' Flink API** under `/v1/namespaces/{ns}/engines/{id}/flink/…` (D1988 with R24's paths):
  - sessions;
  - statements, with NDJSON results that carry changelog kinds;
  - saved pipelines with start, stop (`{savepoint: true, drain: false}`) and resume.
- **Pipeline states:** `Draft → Starting → Running → Stopping → Stopped → Resuming → Running`, and any state → `Failed`.
- **`SqlEngine`** for the facade's `/sql`.
- **`JobStore`**: a memory store now, and the facade's store once RW1 Task 3's Postgres store exists. It implements FL-a's `JobCatalog`.
- **Console:** Flink tabs in the Engines plugin (pipelines, savepoints, and the SQL workspace in Flink SQL mode).

Files: `crates/loams-flink/src/api.rs`, `crates/loams-flink/src/api/{store,pipelines}.rs` if split, the console plugin's Flink tabs. Tests: `saved_pipeline_stores_refs_only`, `pipeline_state_machine_table`, `results_stream_ndjson`, `one_op_at_a_time_conflicts`. Gate: CI green.

## Task 10 — CLI (lane FL-b)

`loams flink sql` (a line REPL over the API, `;`-terminated, `\q`, `\jobs`), `loams flink jobs list|create|start|stop|resume|delete`, `bootstrap` (prints the redacted session SQL) and `jars`. It is a client of the facade behind feature `flink`.

Files: `crates/loams-flink/src/cli.rs`, `crates/loams/src/main.rs` (the `Flink` arm). Tests: `cli_bootstrap_is_redacted`, `cli_parses_statements_across_lines`. Gate: CI green.

## Task 11 — Upgrades and failure handling (lane FL-a)

- **Upgrade (D1925, R23):**
  1. Stop every managed pipeline with a CANONICAL savepoint and wait for each to complete. Abort before redeploying if any fails.
  2. Redeploy with the new image.
  3. Wait for the TaskManagers.
  4. Resubmit each pipeline from its savepoint.
  - A pipeline that does not restore becomes `Failed` on the new version with its savepoint kept. The documented rollback is `upgrade` to the previous version from the same savepoint.
- **Versions:** the adapter declares patch versions within 2.2 only (Q1405).
- **Failures:** the rows of §81 §13.3 for Flink become named `observe()` checks: TaskManager lost, JobManager lost (with recovery replay in compose), SQL Gateway lost, and savepoint failed.

Files: `crates/loams-flink/src/{upgrade.rs,controller.rs}`. Tests: `upgrade_aborts_on_failed_savepoint`, `upgrade_refuses_minor_change`, `failed_restore_keeps_savepoint`, `observe_reports_named_checks`. Gate: CI green.

## Task 12 — Conformance (lanes FL-a and FL-b)

- **`conformance/flink/`:** Flink's own SQL Client and SQL Gateway end-to-end cases at the pinned tag (fetched by path at the tag into the runner's workspace, never vendored), run with the stock SQL Client against the instance through `engine-gateway`, with a strict `xfail.toml`.
- **Nexmark** q0–q22 (Apache-2.0; the generator as a pinned jar or container): events into the Iggy gateway, then queries on Flink with checkpoints to the bucket, results compared with a reference run on stock Flink with a local filesystem. q0–q8 also run on RisingWave (RW1 Task 13).
- **Cross-engine:** Flink writes Iceberg, and RisingWave, Spark and the House read it, and the reverse. Flink consumes a Kafka topic RisingWave produced. The CDC leg runs once Task 7's check passes.
- **Lifecycle:** create, a pipeline running, suspend, resume, then counts continue with no loss. At-least-once duplicates are measured and reported, not hidden.

Files: `conformance/flink/{run.sh,cases.txt,xfail.toml,nexmark/,README.md}`, `.github/workflows/engines-nightly.yml` (Flink jobs). Tests: `xfail_entries_have_reason_and_issue`, `xfail_strict_unexpected_pass`. Gate: the workflow validates; the first run is Task 13.

## Task 13 — First run, docs, exit (lanes FL-a and FL-b)

- **The first end-to-end run on compose:**
  1. `rustfs`, `kafka` and `flink` up.
  2. Create an instance.
  3. Use Flink's SQL Client through `engine-gateway` with a Loams key.
  4. Run a Kafka-to-Iceberg pipeline.
  5. Stop with a savepoint, resume, and check the counts.
  6. Kill the JobManager, let the controller replay, and check the counts.
- **Guide:** `docs/guides/flink-sql.md`: quick start, the SQL Client against Loams, catalogs, at-least-once Kafka, no UDF jars in FK1, CDC experimental, patch upgrades only, and the RisingWave porting option.
- **An exit report.**

Files: `docs/guides/flink-sql.md`, the exit report in rulings. Tests: `docs_engines_wording`. Gate: the run recorded; labels updated per D1928.

## Owner actions

- Q1400 (managed Flink SQL, amending D212) is answered: yes, by the owner's request of 2026-10-27. Q1405 (patch upgrades only in FK1; compiled plans in FK2) and Q1409 (FLIP-480 evaluated in FK2) are answered with their defaults by the owner's approval of 2026-10-27.

## Rulings made during execution

- **R-FK1-0 (2026-10-27, ENG81):** the lanes' D1970–D1989 as amended by rulings R13, R14 and R21–R26 in `loams-wt/.he0-shared/engines-81.md` are this plan's starting rulings. The track is FK; the lane names FL-a and FL-b are unchanged.
