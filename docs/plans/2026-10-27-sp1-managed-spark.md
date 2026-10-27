# SP1 — Managed Spark: Sail and Apache Spark over Spark Connect

> **For agentic workers:** REQUIRED SUB-SKILL: use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Work task by task, test first. Where this plan gives exact names, paths, branches or tests, use them verbatim. Record any deviation in "Rulings made during execution" at the end of this file.
>
> **Status: specified 2026-10-27; code in progress.** Track SP, design [§81](../design/81-managed-stream-batch-engines.md) (D1900–D1931 and the lanes' D1950–D2019 **approved by the owner on 2026-10-27** with all defaults; Q1400–Q1411 answered). The lanes SP-a (the Connect proxy, backends, lifecycle, deploy) and SP-b (jobs, catalog, Python helpers, CLI) started on 2026-10-27, at the same time as the design. Their assumptions (D1990–D2009) and ENG81's rulings (R15, R16, R27–R30) are in `loams-wt/.he0-shared/engines-81.md`. No local builds that wave: CI is the check. Nothing here has run end to end.

**Goal:**
- Each namespace can run Spark instances, reached with `pyspark` (`SparkSession.builder.remote("sc://…")`) through `loams-gateway`'s `spark` module (D1263) with a Loams key.
- The backend is Sail (default, no JVM) or Apache Spark 4.1.3 (opt-in, for coverage). Every table is in the Iceberg catalog `loams` on Lakekeeper.
- Batch jobs (PySpark, SQL, jars) run with retries and schedules.
- The PySpark Spark Connect parity tests run against both backends with strict expected-failure lists.

**Architecture:**
- `crates/loams-spark` is a stateless adapter behind `loams-engines` (RW1 Tasks 3–6). It contains:
  - a transparent Spark Connect proxy library (it reads only `session_id` and `user_context`) that runs as `loams-gateway --module spark` or as the standalone `loams-spark-proxy`;
  - the backend rendering for Sail and Apache Spark;
  - the lifecycle;
  - SP-b's jobs, catalog and CLI.
- Python helpers live in `sdks/python` (`loams.spark`).

**Tech stack:**
- Rust: tokio, tonic and prost with the Spark Connect protos vendored from Spark v4.1.3, axum and serde.
- Sail 0.7.2 (`pysail`, in a Loams-built image on `python:3.13-slim`).
- Apache Spark 4.1.3 (official image by digest), with `iceberg-spark-runtime-4.1_2.13` 1.12.0, `iceberg-aws-bundle` 1.12.0 and `hadoop-aws` 3.4.2.
- `pyspark-client` 4.1.3 and pytest for conformance.

**Effort (estimate):** 3–4 engineer-weeks after RW1 Tasks 1 and 3–5. Tasks 1–6 (SP-a) and 7–10 (SP-b) run in parallel. Task 11 needs Tasks 4 and 6; Task 12 is last.

## Global Constraints

- **Worktrees and branches.** As RW1. SP-a owns `Cargo.toml`, `build.rs`, `proto/`, `src/lib.rs` and every module except SP-b's `src/jobs/*` and `src/catalog.rs`. SP-b owns those two and the Python helpers under `sdks/python`. lib.rs keeps SP-b's three lines (`pub mod catalog; pub mod jobs; pub use jobs::cli;`). Union of `Cargo.toml` entries.
- **Decision IDs (R0).** Cite D1990–D2009; add no decision-log rows.
- **Licences (D1901).** Apache Spark, Sail, the Apache Iceberg runtime and `hadoop-aws` only. The vendored protos keep Spark's licence file and a `VENDORED.md` with hashes. No Databricks pieces.
- **Loams parses no Spark plans (D1263).** The proxy forwards bytes with a raw codec and reads only fields 1 (`session_id`) and 2 (`user_context`). Test `proxy_forwards_unknown_fields`.
- **Never public; credentials (D1917, D1918).** Sail has no authentication, so it is reachable only from the proxy. The proxy strips the client's credential and sends the backend's own token to Apache Spark (`SPARK_CONNECT_AUTHENTICATE_TOKEN`). The token is never in the `sc://` URI.
- **Tenant Python (D1905).** Sail runs Python UDFs in-process: one backend per instance, and `gvisor` on shared nodes (Task 5).

## Review Focus

1. **One tenant's session on another's backend.** Tests: `authority_must_match_credential`, `session_ids_must_be_uuids`, `proxy_routes_by_instance`.
2. **A credential in the wrong place.** Tests: `token_never_in_uri`, `backend_token_replaces_client_credential`, `conf_file_has_no_secret`.
3. **A hidden coverage gap.** Tests: `parity_xfail_strict_sail`, `parity_xfail_strict_spark`.
4. **A job that runs twice or silently fails.** Tests: `run_state_machine_table`, `retry_respects_max_attempts`, `schedule_promise_id_is_deterministic`.
5. **Scale to zero that drops work.** Tests: `session_with_running_op_is_not_idle`, `first_call_resumes_and_waits`.

## Task 0 — Rulings

- **Pins:** the Spark 4.1.3 image digest; Sail 0.7.2 and `pyspark-client` 4.1.3; the Iceberg 1.12.0 runtime for 4.1, `iceberg-aws-bundle` and `hadoop-aws` 3.4.2, with checksums (SP-a's pins, R27).
- **Sail at 0.7.2:**
  - the `SAIL_CATALOG__LIST` syntax for iceberg-rest with vended credentials;
  - the object-store env names (`AWS_ENDPOINT`, `AWS_ALLOW_HTTP`);
  - `kubernetes-cluster` mode's objects and RBAC;
  - where Sail writes shuffle and checkpoints.
- **PySpark 4.1.3:** how the `sc://…/;token=` parameter and a `ChannelBuilder` subclass send `authorization: Bearer` (§26 §9.2 asked to verify per version).
- **Iceberg v3:** whether Sail 0.7.2 and Iceberg 1.12.0 on Spark 4.1.3 read the House's v3 tables, deletion vectors included.
- **Q1401 is answered** (the owner's approval of 2026-10-27, the default): the Apache Spark backend is in SP1, opt-in (`spark-jvm`).

Files: this plan. Tests: none. Gate: rulings recorded.

## Task 1 — Crate shell, vendored protos and the proxy library (lane SP-a)

- **Vendored protos:** the Spark Connect protos from Spark v4.1.3 under `crates/loams-spark/proto/spark/connect`, generated with tonic-prost-build (the `loams-temporal-proto` pattern), with `VENDORED.md` and the licence file.
- **`loams_spark::proxy`:** HTTP/2 gRPC, forwarding bytes unchanged with a raw codec. It reads only `session_id` and `user_context`, and sets `user_context.user_id` to the Loams subject by appending a field-2 sub-message (protobuf merge, last value wins) (D1994). Sessions are sticky by `session_id`, and session ids must be UUIDs.

Files: `crates/loams-spark/{Cargo.toml,build.rs,proto/**,VENDORED.md,LICENSE-apache-spark,src/lib.rs,src/proxy/{mod,codec,route}.rs}`. Tests: `proxy_forwards_unknown_fields`, `user_id_is_overridden`, `session_ids_must_be_uuids`, `vendored_hashes_match`. Gate: CI green.

## Task 2 — Authentication, limits and idle suspend (lane SP-a)

- **The credential** comes from `authorization: Bearer` or `x-loams-api-key`. Both present and different gives `UNAUTHENTICATED`.
- **Verification** goes through the shared Loams verifier with `can_connect` on the instance. The keys file is a test seam (D1993, R28).
- **`:authority`** must name an instance the credential can reach, otherwise `PERMISSION_DENIED`.
- **Limits per instance (D1995):** 8 sessions and 16 concurrent operations by default, giving `RESOURCE_EXHAUSTED` beyond them.
- **Idle handling:** a session is released after 30 minutes idle, never while an operation runs. An instance with no session for 15 minutes is suspended. The first call to a suspended backend resumes it and waits up to 120 s.

Files: `crates/loams-spark/src/proxy/{auth,limits,sessions}.rs`. Tests: `authority_must_match_credential`, `conflicting_credentials_rejected`, `session_with_running_op_is_not_idle`, `first_call_resumes_and_waits`, `limits_return_resource_exhausted`. Gate: CI green.

## Task 3 — Backends and configuration (lane SP-a)

- **`SparkBackend`** with two implementations, `Sail` (default) and `ApacheSpark` (D1990).
- **JVM Spark configuration (D1996):**
  - `spark.sql.catalog.loams=org.apache.iceberg.spark.SparkCatalog`, `type=rest`, the Lakekeeper `uri`, warehouse `loams`, vended credentials, and `spark.sql.defaultCatalog=loams`;
  - S3A on the bucket with path style and credentials from env;
  - `spark.eventLog.dir=s3a://loams-engines/engines/<ns>/<id>/spark-events/`, rolling;
  - `spark.sql.warehouse.dir` set to `scratch/` (D1912).
- **Sail configuration:** `SAIL_CATALOG__LIST` with the same catalog, the object-store env, shuffle and checkpoints under `scratch/`, and OTLP telemetry.
- **The backend token** for Apache Spark (`SPARK_CONNECT_AUTHENTICATE_TOKEN`), minted per instance.

Files: `crates/loams-spark/src/{backend.rs,conf.rs}`, `deploy/spark/Containerfile.loams-sail`. Tests: `conf_file_has_no_secret`, `default_catalog_is_loams`, `event_log_under_state_prefix`, `sail_env_golden`, `backend_token_replaces_client_credential`. Gate: CI green.

## Task 4 — The adapter and compose (lane SP-a)

- **`ManagedEngine` for Spark** (R1, D1999). Sizes come from the tier table (Sail worker cap or Spark executors × per-replica resources, R19). `spark()` returns the sessions and jobs surface.
- **Compose:** `deploy/release/compose.spark.yaml`:
  - profile `spark` for the Sail server;
  - profile `spark-jvm` for the Apache Spark Connect server (`local[*]`), with `LOAMS_SPARK_BACKEND=sail|spark` choosing the backend.
- The proxy runs in `engine-gateway` (Task 6) on `127.0.0.1:15002`.
- Lakekeeper is in `compose.lake.yaml` (R30), on bucket `loams-lake`.
- No host ports.

Files: `crates/loams-spark/src/{managed.rs,sizes.rs}`, `deploy/release/compose.spark.yaml`. Tests: `render_compose_golden_sail`, `render_compose_golden_spark`, `no_public_ports_in_engine_compose`, `suspend_scales_backend_to_zero`. Gate: CI green.

## Task 5 — Kubernetes and the sandbox (lane SP-a)

- **Sail:** a server Deployment in `kubernetes-cluster` mode in `eng-<26>`, with RBAC limited to creating worker pods in that namespace.
- **Apache Spark:** a Connect server with `spark.master=k8s://…` in client mode and executors in the same namespace.
- **NetworkPolicy:** the Sail and Spark ports admit only the proxy.
- **The D1905 profile.** A pod that runs Python UDFs on a shared node requires `runtimeClassName: gvisor`. If the cluster reports no gVisor, the facade refuses placement unless the cluster is marked single-tenant.
- The base is a kustomize directory, and per-instance objects are rendered by `loams_spark::k8s`.

Files: `deploy/spark/k8s/**`, `crates/loams-spark/src/k8s.rs`. Tests: `sail_rbac_is_namespace_scoped`, `gvisor_required_on_shared_nodes`, `networkpolicy_admits_proxy_only`, `no_public_service`. Gate: kustomize build and schema check in CI.

## Task 6 — The `spark` gateway module (lane SP-a)

- Wire `loams_spark::proxy` into `loams-gateway` as `--module spark`, behind cargo feature `spark` (the `temporal` module's pattern), and as the standalone `loams-spark-proxy` binary.
- **Cloud endpoint:** `sc://<instance>.<ns>.<region>.spark.<base-domain>:443/;use_ssl=true`, with TLS at the gateway and `traceparent` at the first hop (D1263).
- **Compose:** the `engine-gateway` section in `compose.lake.yaml`, on `127.0.0.1:15002`.

Files: `crates/loams-gateway/src/spark/mod.rs`, `crates/loams-gateway/Cargo.toml` (feature), `crates/loams-spark/src/bin/loams-spark-proxy.rs`, `compose.lake.yaml`. Tests: `module_config_requires_tls_off_loopback`, `proxy_routes_by_instance`, `token_never_in_uri`. Gate: CI green.

## Task 7 — Jobs (lane SP-b)

- **The job spec (D2000):** `pyspark {file, py_files}`, `jar {file, main_class}` or `sql {file}`, plus `args`, `conf`, `resources`, `retry`, `image` and `backend`.
- **Execution (D2001):**
  - PySpark and SQL jobs run in a short-lived driver container against the instance's Spark Connect endpoint.
  - Jar jobs run in a driver pod with `spark-submit --deploy-mode client`, with no Kubeflow Spark Operator (D1911).
  - SQL files run through the bundled `loams-spark-sql-runner.py`.
  - Files are uploaded under `jobs/`.
- **Local runner (D2002)** for compose.
- **The run state machine (D2003):** `Pending → Submitted → Running → Succeeded | Failed | Canceled`, with `Retrying` and backoff. Logs are reachable by attempt.
- **The facade's `/jobs` routes** (R15).

Files: `crates/loams-spark/src/jobs/{mod,spec,render,runner_local,runner_k8s,status}.rs`, `sdks/python/loams/spark/loams-spark-sql-runner.py`. Tests: `run_state_machine_table`, `retry_respects_max_attempts`, `jar_job_renders_client_mode_driver`, `job_files_under_state_prefix`. Gate: CI green.

## Task 8 — Schedules (lane SP-b)

Schedules go through the embedded Resonate server's `schedule.create` (D2004): a cron, the promise id template `spark-run-{{.id}}-{{.timestamp}}`, the target `inproc://any@loams` and the job spec as the parameter. The work goes through a `DurableTransport` trait, so `loams-spark` does not depend on `loams-durable`.

Files: `crates/loams-spark/src/jobs/schedule.rs`. Tests: `schedule_promise_id_is_deterministic`, `schedule_request_golden`. Gate: CI green.

## Task 9 — Catalog conf, the Python helpers and the console tabs (lane SP-b)

- **The client-side catalog conf** (D2005, with R27's pins). It is equal to the server's (SP-a's `conf.rs`) by test.
- **Python (D2007):**
  - `loams.spark.session()` returns `SparkSession.builder.remote("sc://…;use_ssl=true")`, with the token in gRPC metadata through a `ChannelBuilder` subclass and the `loams` catalog conf applied;
  - `submit_job`, `status` and `logs` over the facade;
  - `pyspark` is imported lazily (extra `loams[spark]`).
- **Console:** Spark tabs in the Engines plugin (sessions, jobs with logs, and the SQL workspace in Spark SQL mode).

Files: `crates/loams-spark/src/catalog.rs`, `sdks/python/loams/spark/**`, the console plugin's Spark tabs. Tests: `client_and_server_catalog_conf_equal`, Python `test_session_token_not_in_uri`, `test_lazy_pyspark_import`. Gate: CI green.

## Task 10 — CLI (lane SP-b)

`loams spark submit|status|logs|session|schedule` is a client of the facade behind feature `spark`. `loams spark check <script>` runs a script on Sail and lists the unsupported calls it hit, from Sail's errors (§26 §9.4), so a user learns which jobs need `backend = spark`.

Files: `crates/loams-spark/src/jobs/cli.rs`, `crates/loams/src/main.rs` (the `Spark` arm). Tests: `cli_submit_parses_spec`, `cli_check_reports_unsupported`. Gate: CI green.

## Task 11 — Conformance (lane SP-b, with SP-a)

- **`conformance/compute/spark/`:**
  - the PySpark Spark Connect parity test modules (`pyspark.sql.tests.connect`) at 4.1.3, run with pytest through `engine-gateway` against Sail and against Apache Spark, with one strict `xfail.toml` per backend (D2009);
  - §68 §7.4's corpus: TPC-H-derived SQL, DataFrame jobs, a `pandas_udf`, an Iceberg `MERGE` on a table Loams does not maintain, and `format("loams")` once it exists.
- **Cross-engine (`conformance/engines/`):** Spark reads what RisingWave and Flink wrote and what the House wrote (cluster mode), and writes tables they read.
- **Lifecycle:** idle suspend, then a first call resumes it, and a session survives the proxy's restart only as a new session. The client retries, and the docs say so.

Files: `conformance/compute/spark/{run.sh,modules.txt,xfail-sail.toml,xfail-spark.toml,corpus/,README.md}`, `.github/workflows/engines-nightly.yml` (Spark jobs). Tests: `parity_xfail_strict_sail`, `parity_xfail_strict_spark`, `xfail_entries_have_reason_and_issue`. Gate: the workflow validates; the first run is Task 12.

## Task 12 — First run, docs, exit (lanes SP-a and SP-b)

- **The first end-to-end run on compose:**
  1. `rustfs` and `spark` up.
  2. Create an instance.
  3. Run `loams.spark.session()` from a laptop on the tailnet.
  4. Create an Iceberg table in `loams` and read it from DuckDB through Lakekeeper.
  5. Submit a PySpark job, then a scheduled run.
  6. Let the instance idle-suspend, then resume it.
  7. Repeat with `spark-jvm`.
- **Guide:** `docs/guides/spark.md`: quick start, Sail against Apache Spark, `loams spark check`, the `loams` catalog, jobs and schedules, and what Spark Connect cannot do (RDD, `SparkContext`).
- **An exit report.**

Files: `docs/guides/spark.md`, the exit report in rulings. Tests: `docs_engines_wording` (no "Loams Spark" or "Spark replacement"). Gate: the run recorded; labels updated per D1928.

## Owner actions

- None open. The owner approved design 81 with all defaults on 2026-10-27: Q1401 puts the Apache Spark backend in SP1, opt-in; Q1410 puts the History Server in SP2.

## Rulings made during execution

- **R-SP1-0 (2026-10-27, ENG81):** the lanes' D1990–D2009 as amended by rulings R15, R16 and R27–R30 in `loams-wt/.he0-shared/engines-81.md` are this plan's starting rulings.
