# RW1 — Managed RisingWave, with the Shared Engines Foundation

> **For agentic workers:** REQUIRED SUB-SKILL: use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Work task by task, test first. Where this plan gives exact names, paths, branches or tests, use them verbatim. Record any deviation in "Rulings made during execution" at the end of this file.
>
> **Status: specified 2026-10-27; code in progress.** Track RW, design [§81](../design/81-managed-stream-batch-engines.md) (D1900–D1931 and the lanes' D1950–D2019 **approved by the owner on 2026-10-27** with all defaults; Q1400–Q1411 answered). The lanes ENG-UI (foundation), RW-a (controller, deploy) and RW-b (connectors, SQL API, CLI) started on 2026-10-27, at the same time as the design. Their assumptions and ENG81's rulings R0–R30 are in `loams-wt/.he0-shared/engines-81.md`, and this plan follows those rulings. The owner rule for that wave is **no local builds**: CI is the compile and test check. Nothing here has run end to end.

**Goal:**
- **The shared foundation**, used by RW1, [FK1](2026-10-27-fk1-managed-flink-sql.md) and [SP1](2026-10-27-sp1-managed-spark.md):
  - `crates/loams-engines` with the facade, store, phase machine, authorisation and metering;
  - the OpenFGA `engine` type;
  - `deploy/release/compose.lake.yaml` (Lakekeeper, `engines-init`, `engine-gateway`);
  - the console's Engines pages;
  - the `conformance/engines/` harness.
- **Managed RisingWave:**
  - an instance per namespace, with its meta store on Loams Postgres and Hummock in the bucket;
  - reached only through `loams-gateway` with a Loams key;
  - wired to Lakekeeper, the Iggy Kafka gateway and Loams Postgres CDC;
  - metered, with a CLI and SQL API, and tested with RisingWave's own sqllogictest files.

**Architecture:**
- `loams-engines` owns state. `loams-risingwave` is a stateless adapter: it renders compose services or Helm values from an `Instance`, applies them, and observes the result.
- RisingWave runs distributed (meta, frontend, compute, compactor) at every size.
- The gateway's `pg` module gains a `risingwave` route kind. It terminates TLS, checks a Loams key, and logs in upstream as the instance's service user.

**Tech stack:**
- Rust: tokio, axum, `tokio-postgres`, serde, `async-trait`, `time`.
- RisingWave v3.1.0, pinned by digest.
- Lakekeeper (v0.13.6, Q1411).
- RustFS, the Loams Postgres direct endpoint and the Iggy fork's `kafka` profile.
- Helm 3 and OpenFGA DSL.
- `sqllogictest-rs` for conformance, and the console's React and `@loams/ui`.

**Effort (estimate):** 4–5 engineer-weeks: foundation 2 (Tasks 1, 3–6, 10, 12), RisingWave 2–3 (Tasks 2, 7–9, 11, 13–14). Tasks 1 and 3–5 unblock FK1 and SP1.

## Global Constraints

- **Worktrees and branches.** `~/Documents/Ostriumlabs/loams-wt/<branch>`, branched from `origin/dev`; one PR per task or per lane PR as the lane planned it (RW-a: crate, gateway, deploy; RW-b: connectors and SQL API; ENG-UI: crate, UI). `git commit -s`. Never `git stash`. Never reuse another lane's branch.
- **Crate ownership (wave rule).**
  - `crates/loams-engines` and the console belong to ENG-UI.
  - `crates/loams-risingwave` belongs to RW-a. RW-b adds `src/connectors/*`, `src/sql_api.rs` and `src/cli.rs`, plus its `pub mod` lines at RW-a's marked spot.
  - Whoever lands second takes the union of `Cargo.toml` entries and deletes nothing.
- **Decision IDs (R0).** The lanes cite D1950–D1962 and D2010–D2019 but add **no** decision-log rows. ENG81 declared them. Run `scripts/docs/check-decision-ids.sh` before every merge.
- **Licences (D1901).**
  - No RisingWave licence key (`RW_LICENSE_KEY` never set) and no Premium SQL in any generator: no `CREATE SECRET`, time travel, resource groups, database isolation or CDC auto schema change.
  - Test `rw_paid_tier_is_off` asserts that `SELECT rw_test_paid_tier()` errors.
  - Pins must be at least 14 days old (D1274).
- **Never public (D1918).** No host port except `engine-gateway` on `127.0.0.1`. No k8s `LoadBalancer`, `NodePort` or Ingress. Test `no_public_ports_in_engine_compose` greps the engine compose files.
- **Credentials (D1917).** A user's key is never in rendered SQL, config, env files or argv. Test `no_secret_in_argv` covers every renderer, and `render_has_no_user_key` covers the generators.
- **Namespaces (D1903).** Paths are `/v1/namespaces/{ns}/engines/…`. Runtime names come from the instance id, and the prefix is `engines/<ns>/<id>/`.
- **Honest labels (D1928).** Docs say "Preview. Not yet run end to end" until Task 14 passes.

## Review Focus

1. **A user's key leaking into the engine.** Tests: `render_has_no_user_key`, `no_secret_in_argv`, `sql_api_connects_as_service_user`.
2. **Premium creep.** Tests: `rw_paid_tier_is_off`, `generators_emit_no_premium_sql` (a deny list: `CREATE SECRET`, `FOR SYSTEM_TIME AS OF`, `RESOURCE_GROUP`, `auto.schema.change`).
3. **A tenant reaching another's instance.** Tests: `gateway_rw_refuses_foreign_namespace`, `facade_checks_engine_relations`, `prefix_credentials_are_scoped`.
4. **Data loss on lifecycle.** Tests: `delete_without_purge_keeps_prefix`, `suspend_keeps_meta_and_hummock`, `upgrade_takes_meta_backup_first`.
5. **A metering event that cannot be deduplicated.** Tests: `usage_event_id_is_window_derived`, `usage_values_are_integers`.

## Task 0 — Rulings

Check and record, in "Rulings made during execution":
- **Pins:** the RisingWave v3.1.0 digest (RW-a has it, D1958); the Lakekeeper digest (v0.13.6, or v0.14.0 per Q1411); RustFS as in `deploy/loams-postgres-dev/compose.yaml`.
- **RisingWave's CLI and env names at v3.1.0:** `--backend postgres` and the `RW_SQL_*` variables, `RW_DATA_DIRECTORY`, `RW_BACKUP_STORAGE_*`, `RW_DO_NOT_CONFIG_BUCKET_LIFECYCLE`, `ENABLE_TELEMETRY`, and how `risectl meta backup` is invoked.
- **Connector option names at v3.1.0:**
  - the Iceberg REST options: `catalog.type`, `catalog.uri`, `catalog.credential`, `catalog.oauth2_server_uri`, `catalog.header`, `commit_checkpoint_interval`;
  - the `postgres-cdc` options, including `publication.create.enable`;
  - the native `postgres` sink, against `jdbc`.
- **Redaction:** whether RisingWave redacts connector passwords in `rw_catalog` definitions and `SHOW CREATE`. Record the answer either way. It does not change D1917.
- **The facade's host binary:** the `loams` engine binary behind a cargo feature `engines`, or a separate `loams-engines` binary. Default: the engine binary, the way jobs share the main API port (D851).

Files: this plan. Tests: none. Gate: rulings recorded.

## Task 1 — `compose.lake.yaml`: Lakekeeper and `engines-init` (lane RW-a; FL-a and SP-a append)

- **Services:** `lakekeeper-migrate` (one-shot), `lakekeeper` (port 8181, no host port, on the internal network `lake`), and `engines-init` (one-shot).
- **`engines-init`:**
  - creates buckets `loams-engines` and `loams-lake` on RustFS with a SigV4 PUT, as `deploy/loams-postgres-dev` does;
  - creates the databases and roles `lakekeeper` (and per-instance `rw_<26>` later, through the provisioner) on `compute1:55433`;
  - bootstraps Lakekeeper and creates warehouse `loams` on `s3://loams-lake` with path style and the RustFS endpoint.
- **Profiles:** shared services list `[risingwave, flink, spark, spark-jvm]`. `engines-init` refuses to run without RustFS reachable and prints "engine profiles need COMPOSE_PROFILES to include rustfs".
- **Lakekeeper:** authentication off, documented in the file header as single-tenant (D1913). Its database is on Loams Postgres in compose only (D413 unchanged for k8s).
- **Q1411:** move to v0.14.x here if its digest is recorded and the round trip of Task 13 passes. Otherwise keep v0.13.6.

Files: `deploy/release/compose.lake.yaml`, `deploy/release/engines/init.sh`, `deploy/release/.env.example` (engine variables, commented). Tests: `compose_lake_has_no_host_ports`, `engines_init_refuses_without_rustfs` (shell test), `lakekeeper_pinned_by_digest`. Gate: `docker compose config` validates in CI (no run).

## Task 2 — RisingWave's meta store on Loams Postgres (lane RW-a)

- **`MetaDbProvisioner` trait** (create and drop the database and role `rw_<26>`, return a URL), with two implementations:
  - `ComposeProvisioner`: the admin role on `compute1`;
  - `PgControlProvisioner`: pg-control's role and database RPCs.
- **Passwords** are `[A-Za-z0-9]{32}` (RisingWave splices them into a URL unencoded, D1951).
- **The endpoint used by a running instance's meta database** has scale to zero off (§81 §13.3). The provisioner sets it, or records a ruling if pg-control cannot yet.
- **(verify) Does the meta node run on the fork?** A nightly job starts RisingWave's meta node against `compute1`, creates a materialised view, restarts meta, and checks the view survives. If it fails, record the failure and fall back to a plain `postgres:16` container for compose meta only, labelled as such (§81 §19).

Files: `crates/loams-risingwave/src/meta_db.rs`, `.github/workflows/engines-nightly.yml` (job `rw-meta-on-loams-postgres`). Tests: `meta_password_is_url_safe`, `meta_db_name_from_instance_id`, `compose_provisioner_sql_is_quoted`. Gate: unit tests green in CI; the nightly result recorded.

## Task 3 — `crates/loams-engines`: types, store, phase machine, registry (lane ENG-UI)

- ENG-UI's shapes (engines-81.md, "Canonical `ManagedEngine` trait"), with R2 applied:
  - `Instance.namespace` replaces `project`, and `org` is kept;
  - `state_prefix = engines/<ns>/<id>/`.
- `InstanceStore` with `MemoryStore`, and a `PgStore` stub behind a feature only if it is complete; otherwise the Postgres store goes to RW2.
- The phase machine (D2012) as a pure function `phase::next(phase, op, outcome)`, with one operation at a time (`Conflict`).
- `EngineRegistry` (D2014), with 501 `engine_not_enabled`.
- **Purge sweeper (Q1404):** an instance deleted without `purge` keeps its prefix for 7 days. A sweeper then calls the adapter's purge.

Files: `crates/loams-engines/{Cargo.toml,src/lib.rs,src/types.rs,src/phase.rs,src/store.rs,src/registry.rs,src/sweeper.rs}`, the workspace dependency entry, Buck regeneration (reindeer). Tests: `phase_transitions_table`, `second_op_conflicts`, `registry_rejects_duplicate_kind`, `delete_without_purge_keeps_prefix`, `sweeper_purges_after_seven_days`, `instance_id_format`. Gate: CI green.

## Task 4 — The facade and authorisation (lane ENG-UI)

- `loams_engines::http::router` serves the routes of §81 §12.1, with errors `{error, message}`.
- `api/engines/openapi.json` is OpenAPI 3.1, with a test that the router and the file agree.
- **OpenFGA (D1919):** add `type engine` to `deploy/openfga/core.fga` and regenerate `config/openfga-model.json`. The built-in authorizer gets the same mapping. Until tuples exist, checks run on `environment:<ns>` (`can_read`, `can_write`, `can_admin`).
- **Create** writes the tuple `engine:<id>#environment@environment:<ns>`.

Files: `crates/loams-engines/src/{http.rs,authz.rs}`, `api/engines/openapi.json`, `deploy/openfga/core.fga`, the generated model, `crates/loams-authz` built-in mapping. Tests: `facade_checks_engine_relations` (view, operate, admin per route), `openapi_matches_router`, `unknown_kind_is_501`, `core_fga_has_engine_type`. Gate: CI green; the OpenFGA model test passes.

## Task 5 — `engine-gateway` and the routes file (lane RW-a, with FL-a and SP-a for their sections)

- **Compose:** an `engine-gateway` service in `compose.lake.yaml`, running `loams-gateway` with:
  - the `pg` module with a `[risingwave]` routes section;
  - the `engines` HTTP route for Flink (FK1 Task 5);
  - the `spark` module (SP1 Task 6).
- It publishes `127.0.0.1:4566`, `127.0.0.1:8083` and `127.0.0.1:15002`.
- **RisingWave route (D1957):**
  - terminate TLS (or plaintext on loopback in compose, behind a flag the guard refuses on non-loopback binds);
  - send `AuthenticationCleartextPassword` and check the password as a Loams API key or token with `can_connect` on the instance;
  - log in upstream as the service user with MD5, then relay.
- **Hostnames:** `<instance>.<ns>.<region>.rw.<base-domain>`. The routes file is rendered by the facade (`loams_risingwave::gateway::render`) with a generation digest, like §57 §8.1.

Files: `crates/loams-gateway/src/risingwave.rs` (route config and the auth step), `crates/loams-gateway/src/routes.rs` (section), `crates/loams-risingwave/src/gateway.rs`, `compose.lake.yaml`. Tests: `gateway_rw_refuses_foreign_namespace`, `gateway_rw_refuses_plaintext_on_public_bind`, `rw_routes_render_is_deterministic`, `rw_route_never_logs_password`. Gate: CI green.

## Task 6 — Metering (lane ENG-UI)

- **The ticker:** every 60 s per instance not suspended or deleted, `usage()` becomes `io.loams.dev.usage.engine.runtime.v1`. Hourly, `…engine.storage.v1` is emitted for every instance not deleted (§81 §11).
- **Event fields:** `id = <instance>/<window_end_unix>`, integer `cpu_ms` and `memory_mib_ms`, and `estimated` when the size stood in for a measurement.
- **JSON Schemas** go in `docs/api/meter/` and the types are added to §63 §6.3's table by reference.
- **The gate:** the Meter gate is called on create, resume and scale up with the size's reservation. A hard-limit signal suspends the instance.

Files: `crates/loams-engines/src/metering.rs`, `docs/api/meter/io.loams.dev.usage.engine.runtime.v1.json`, `…engine.storage.v1.json`. Tests: `usage_event_id_is_window_derived`, `usage_values_are_integers`, `suspended_emits_storage_only`, `hard_limit_suspends_not_deletes`. Gate: CI green.

## Task 7 — The RisingWave adapter (lane RW-a)

- **`ManagedEngine` for RisingWave** (R1): `validate`, `create`, `scale`, `upgrade`, `suspend`, `resume`, `delete`, `observe` and `usage`. Until `loams-engines` merges, depend on it by path.
- **Sizes:** the tier to role layout table (R19), mapping RW-a's dev/small/medium/large onto `xs`…`xl`, with per-role overrides from `config`.
- **Compose runtime:** render and apply services through a `ComposeApi` seam (no container socket inside the engine: the facade's host renders, and an operator command applies; record the ruling).
- **Probes (D1956).** Suspend and resume as D1954. Delete as §81 §13.1.
- **State (D1952):** `hummock+s3://loams-engines`, `RW_DATA_DIRECTORY=engines/<ns>/<id>/hummock`, backups under `backup/`, and `RW_DO_NOT_CONFIG_BUCKET_LIFECYCLE=true`.

Files: `crates/loams-risingwave/src/{lib.rs,model.rs,render.rs,lifecycle.rs,engine.rs,probes.rs,sizes.rs}`, `deploy/release/compose.risingwave.yaml`. Tests: `render_compose_golden_xs`, `render_compose_golden_l`, `suspend_keeps_meta_and_hummock`, `resume_starts_meta_first`, `no_secret_in_argv`, `no_public_ports_in_engine_compose`, `telemetry_off_and_no_licence_key`. Gate: CI green.

## Task 8 — Connectors and CDC (lane RW-b)

- **Typed DDL builders** (D1960) for:
  - Kafka sources and sinks through `kafka-gateway:9093`, as the instance's Iggy user (D1915);
  - Postgres CDC (one source per database, then tables, A5);
  - the Iceberg sink and source on catalog `loams` (A6);
  - the Postgres sink (A8).
- **Credentials:** `Credential::Inline` with the instance's scoped credentials is the default. `Credential::Secret` is not used by any default path and is documented as Premium (R5).
- **Publications** are created by the controller under role `cdc_<26>_<n>` (`publication.create.enable = 'false'`). Slots are `loams_rw_<26>_<n>`, dropped on source drop and instance delete.
- **(verify) CDC on the fork:** a nightly job runs `wal_level = logical` on `compute1`, a publication, a RisingWave CDC table, a compute restart, more rows, and checks a row count. It also checks whether an active slot keeps the compute from suspending. Until it passes, the guide labels CDC **experimental** (§81 §9.4).

Files: `crates/loams-risingwave/src/connectors/{mod,quote,value,format,kafka,postgres_cdc,iceberg,postgres_sink,secret,pipeline}.rs`, `.github/workflows/engines-nightly.yml` (job `rw-cdc-on-loams-postgres`). Tests: `quote_ident_doubles_quotes`, `quote_literal_refuses_nul`, `generators_emit_no_premium_sql`, `render_has_no_user_key`, `cdc_publication_not_created_by_engine`, `iceberg_sink_uses_catalog_loams`, `kafka_sink_is_at_least_once_documented`. Gate: CI green; the nightly result recorded.

## Task 9 — The SQL API and the CLI (lane RW-b)

- **`SqlEngine` for RisingWave** (D2015's `/sql`) over pg wire to the frontend as the service user, never `root` (A10 as amended). Results are capped, with a timeout.
- **Lists** come from `rw_catalog` (materialised views, sources, sinks, tables), progress from `rw_ddl_progress`, and cancel is `CANCEL JOBS`.
- **The CLI:** `loams risingwave sql|list|progress|cancel|render|apply`, as a client of the facade, behind feature `risingwave`, plus the generic `loams engines …` (ENG-UI, D1922).
- `render` prints redacted DDL.

Files: `crates/loams-risingwave/src/{sql_api.rs,cli.rs}`, `crates/loams/src/main.rs` (the `Risingwave` and `Engines` arms), `crates/loams-engines/src/cli.rs`. Tests: `sql_api_connects_as_service_user`, `sql_api_caps_rows`, `cli_render_is_redacted`, `cli_parses_pipeline_spec`. Gate: CI green.

## Task 10 — Engine service credentials and rotation (lane ENG-UI, with RW-a)

- **On create:**
  - create `service_account:eng_<26>` and its key;
  - create the Iggy user `eng_<26>` with permissions on the namespace's streams (through the Iggy admin API);
  - create a Lakekeeper client (k8s; none in compose);
  - create S3 keys scoped to the prefix (the `ObjectStoreProvisioner` seam; compose uses the RustFS admin key and records that as a ruling);
  - create the CDC roles on demand (Task 8).
- **On delete:** revoke all of them.
- **`loams engines rotate-credentials <id>`** mints new credentials, updates the engine (RisingWave: `ALTER SOURCE … WITH` where supported, otherwise recreate the source from its stored spec; record per connector), and revokes the old ones.
- Credentials reach processes through env or mounted files only.

Files: `crates/loams-engines/src/credentials.rs`, `crates/loams-risingwave/src/credentials.rs`. Tests: `prefix_credentials_are_scoped`, `delete_revokes_credentials`, `rotation_revokes_old_key`, `no_secret_in_argv`. Gate: CI green.

## Task 11 — Kubernetes, upgrades and the rollback runbook (lane RW-a)

- **Helm chart** `deploy/helm/loams-risingwave`, one release per instance in namespace `eng-<26>`:
  - a NetworkPolicy (ingress from `loams-gateway` and the controller only; egress to Loams Postgres, the bucket, Lakekeeper, the Kafka gateway, OTLP and DNS; never `169.254.169.254`);
  - a ResourceQuota from the size;
  - the sandbox profile of D1905 (non-root, drop ALL, RuntimeDefault seccomp, read-only root where RisingWave allows).
- **Values** are rendered by the adapter.
- **Upgrade (D1925):** take a meta backup (`risectl meta backup`), then roll meta → compactor → compute → frontend, each ready before the next. Forward only.
- **Q1406:** a written runbook for a manual rollback (restore the meta backup into a fresh database, start the old version on the same Hummock prefix) in `docs/operations/risingwave-rollback.md`, marked "not yet exercised".

Files: `deploy/helm/loams-risingwave/**`, `crates/loams-risingwave/src/{k8s.rs,upgrade.rs}`, `docs/operations/risingwave-rollback.md`. Tests: `helm_values_golden`, `networkpolicy_denies_metadata_ip`, `upgrade_takes_meta_backup_first`, `upgrade_refuses_major_skip`, `chart_has_no_public_service`. Gate: `helm template` and the schema check in CI.

## Task 12 — Console: the Engines pages (lane ENG-UI)

- An Engines plugin in the console shell (§59):
  - the list per namespace and the create dialog (kind, name, size, version);
  - the detail view with phase, health checks, endpoints (gateway hostnames only) and usage;
  - the SQL workspace on `@loams/ui`'s shared editor (Postgres dialect for RisingWave);
  - for RisingWave, tabs for materialised views, sources, sinks and DDL progress.
- Flink and Spark tabs are added by FK1 Task 9 and SP1 Task 9 against the same plugin.
- Mock routes in `loams-console-mock`. Every page carries the **Preview** badge (D2019).

Files: the console plugin under the console app (ENG-UI's layout), `loams-console-mock` routes. Tests: component tests for the list, create and SQL views; `preview_badge_on_every_engine_page`. Gate: CI green.

## Task 13 — Conformance (lane RW-b, harness with ENG-UI)

- **`conformance/risingwave/`:** a script that fetches RisingWave's `e2e_test/` sqllogictest files at the pinned tag (a sparse fetch of named paths into a temporary directory under the runner's workspace, never vendored and never cloned into the repository), then runs the chosen subset with `sqllogictest-rs` through `engine-gateway`. The subset is streaming, batch, sinks and Iceberg, plus Postgres CDC once Task 8's check passes.
- **`xfail.toml`** is strict: a reason and an issue per entry, and an unexpected pass fails the run.
- **`conformance/engines/`:**
  - the cross-engine harness: RisingWave writes Iceberg, Spark (SP1) and the House (cluster mode) read, and the reverse, on v2 and v3 tables;
  - a Kafka round trip through the Iggy gateway;
  - `rw_paid_tier_is_off`.
- **Nexmark** q0–q8 on RisingWave as FK1's cross-check (FK1 Task 12 owns the generator).
- Nightly and `workflow_dispatch` only, on GitHub-hosted runners.

Files: `conformance/risingwave/{run.sh,subset.txt,xfail.toml,README.md}`, `conformance/engines/{lib.sh,iceberg-roundtrip.sh,kafka-roundtrip.sh,README.md}`, `.github/workflows/engines-nightly.yml`. Tests: `xfail_entries_have_reason_and_issue`, `xfail_strict_unexpected_pass`, `subset_files_exist_at_pin`. Gate: the workflow validates. The first nightly run is Task 14.

## Task 14 — First run, docs, exit (lanes RW-a and RW-b)

- **The first end-to-end run on compose** (the owner's local-test rule applies to releases; this run is the lane's own):
  1. `rustfs`, `kafka` and `risingwave` up.
  2. Create an instance through the facade.
  3. `psql` through `engine-gateway` with a Loams key.
  4. A Kafka source, a materialised view and an Iceberg sink; read the table from Spark (SP1) or DuckDB.
  5. Suspend, then resume, and check the view is intact.
  6. Delete without `purge`, and check the prefix is kept.
- **Guides:** `docs/guides/risingwave.md` (quick start, the porting note from §26 §10.1, the at-least-once note, the Premium boundary, CDC labelled experimental until Task 8 passes).
- **An exit report** with the results, the expected-failure count and every ruling.

Files: `docs/guides/risingwave.md`, `docs/guides/engines.md` (shared: what an instance is, sizes, the gateway hostnames, metering), the exit report in this plan's rulings. Tests: `docs_engines_wording` (no "RisingWave-compatible", no Premium feature named as Loams'). Gate: the run recorded; labels updated per D1928.

## Owner actions

- None open. The owner approved design 81 with all defaults on 2026-10-27, so Q1400–Q1411 in [§81](../design/81-managed-stream-batch-engines.md) §20 are answered: among them no prices in preview (Q1403), no Loams Cloud hosting in this phase (Q1408), 7-day retention after a non-purge delete (Q1404), a manual rollback runbook in RW1 (Q1406) and Lakekeeper v0.14.x in Task 1 once its digest and round trip are recorded (Q1411).

## Rulings made during execution

- **R-RW1-0 (2026-10-27, ENG81):** the lanes' assumptions as amended by rulings R1–R30 in `loams-wt/.he0-shared/engines-81.md` are this plan's starting rulings. Lanes record further rulings here.
