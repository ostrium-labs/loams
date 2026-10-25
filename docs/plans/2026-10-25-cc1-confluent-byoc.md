# CC1 — Confluent Cloud with Loams BYOC

> **For agentic workers:** REQUIRED SUB-SKILL: use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Work task by task, test first. Where this plan gives exact names, paths, flags or tests, use them verbatim. Record any deviation in "Rulings made during execution" at the end of this file.
>
> **Status: Proposed** (2026-10-25). Track **CC**, design [§80](../design/80-confluent-byoc.md) (D1820–D1835, Q1333–Q1341). The owner asked on 2026-10-25 for Confluent Cloud with BYOC, a budget limit, use of Ostrium's Confluent credits, and use of the HashiCorp free tier. CC0 and CC1 are specified task by task; CC2 is specified at task level; CC3 is at milestone level. Nothing here is built, and no Confluent or HCP resource is created, until the owner approves the design.

**Goal (CC1):** a user with a Loams BYOC VM on AWS connects it to their own Confluent Cloud cluster (or has the installer create a Basic cluster in the same region) by following one guide; Loams reads CDC and event topics into its streams and writes its streams to topics, exactly once where the cluster allows; no Confluent key sits in an image, user data, a log or plain state; and the round trip runs nightly on Ostrium's credits in `us-east-1` for about a dollar a month. **CC2** adds the budget guard and spend surfaces.

**Architecture** (§80 §4–§8):
- **`fabric/crates/loams-fabric`** gains the `connect` role: native connector instances on the SDK's `Pipeline`, `FabricWriter`/`FabricReader` over the engine's `StreamService` (`ProduceCloudEvents`, `FetchCloudEvents`), `FileSecretStore`, state in the bucket, a loopback admin API.
- **`fabric/crates/loams-connector-kafka`** gains `format: debezium-json | debezium-avro` on the source and a `rate_limit` config wired to the SDK's `RateLimit`.
- **`connectors/profiles/confluent-cloud.yaml`**: the target profile.
- **`crates/loams-confluent`** (new): a small REST client for the Confluent Cloud APIs Loams calls (clusters, API keys, Metrics, Costs, regions) and, in CC2, the guard's logic.
- **`crates/loams`** (the CLI): `loams byoc confluent key|check` (CC1) and `loams confluent cost|budget|guard` (CC2).
- **`deploy/tofu/modules/confluent/`** and **`deploy/tofu/examples/aws-vm-confluent/`**.
- **`deploy/byoc/lib/loams-byoc-secrets`**: the Confluent secret names; **`deploy/byoc/confluent/`**: `regions.toml`, `prices.toml`, the CI role notes.
- **`deploy/release/compose.yaml`**: the `connectors` profile.
- **`.github/workflows/byoc-confluent-nightly.yml`**, **`byoc-confluent-reaper.yml`**; `scripts/byoc/confluent-ci.sh`.
- **`docs/guides/byoc/confluent.md`**, **`docs/guides/connectors/confluent-cloud.md`**.

**Tech stack:**
- Rust 1.97, edition 2024, workspace lints; workspace dependencies only (`reqwest` or `hyper` as the workspace has them, `serde`, `tokio`, `tonic` through `loams-stream-grpc`, `wiremock` if already in the workspace, else a hand-written `hyper` fake as `loams-connector-kafka`'s registry tests do). `rdkafka` stays compiled only in `loams-connector-kafka` (OR-6).
- OpenTofu ≥ 1.11 (state encryption, ephemeral values); providers `confluentinc/confluent` pinned to a release at least 14 days old (2.89.0 on 2026-10-25), `hashicorp/aws ~> 6.0`, from the OpenTofu registry. No Terraform CLI.
- Bash with `set -euo pipefail`, shellcheck-clean, as BY1's scripts.
- GitHub Actions per the repository's runner routing (`CI_RUNNER`, `deploy/ci-hosts/route-workflows.py`); the cloud rows on GitHub-hosted runners with OIDC into AWS (BY1) and the `byoc-confluent` environment.

**Spec:** [§80](../design/80-confluent-byoc.md) (all of it); [§70](../design/70-byoc-clouds.md) §6, §9, §12; [BY1](2026-10-21-by1-byoc-clouds.md) rulings (the fetcher, the generated cloud-init, the VM modules); [§33](../design/33-connectors.md) §4–§6; `docs/guides/connectors/kafka.md`; [§32](../design/32-loams-flow-fabric-house.md) §5.10.

## Global Constraints

- **Worktrees and branches.** `~/Documents/Ostriumlabs/loams-wt/cc1-<task>`, one branch and one PR per task into `dev`. Commit with `git commit -s`. Never `git stash`. Label PRs that touch secrets, the module or the guard `review:greptile` (security, new APIs).
- **Light Rust builds.** `cargo build -p <crate>` or `cargo test -p <crate> --test <name>` with the configured jobs; never `--workspace` locally; the fabric workspace builds from `fabric/`. Build output goes to the shared target; never build in `/tmp`. Use `buildslot.sh` when other agents are building.
- **No credentials anywhere** in the repository, logs, chat, PR bodies or test fixtures. Recorded API responses have ids replaced by `lkc-test01`-style placeholders and no key material. Never print `~/confluent.env` or `~/confluent-oracle.env`.
- **No Confluent or HCP resource is created by an agent** except by the nightly and weekly workflows the owner has enabled (owner checklist), inside `loams-ci`, under the TTL and reaper rules. Spikes in CC0 that need a real cluster use the existing oracle cluster's scoped key (prefix `loams-oracle-`) from the owner's machine, or are run by the owner.
- **Never public.** No inbound rule, Tunnel route or port is added for Confluent. The BY1 exposure scan runs on every cloud row.
- **No key in state in plain text.** The module's `create_api_keys = true` path is allowed only in a root module whose `encryption` block has `enforced = true`.
- **Pinned.** Provider versions and images pinned; `latest` never appears; Dependabot moves pins with the 14-day rule.
- **Honest labels.** Every cost number in code, output or docs says whether it is an estimate (with the price table's date) or reconciled (with the Costs API's last date).

## Review Focus

1. **A Confluent key lands in an image, user data, `.env`, a log, a bundle or plain state.** Expected: never. Tests: `rendered_user_data_has_no_confluent_value`, `env_has_no_confluent_value` (Task 6), `bundle_redacts_confluent_secrets` (Task 6), `example_with_keys_enforces_encryption` (Task 7), `connect_logs_scrub_resolved_secrets` (Task 1).
2. **Loams' data account can create topics, raise retention or change ACLs.** Expected: never. Test: `data_account_acls_are_minimal` (Task 7).
3. **The module creates a premium cluster type or a cluster in another region by default.** Expected: refused. Tests: `refuses_premium_types_without_flag`, `refuses_cross_region_without_flag` (Task 7).
4. **The guard deletes or reconfigures something it should not.** Expected: it only alerts, throttles Loams' instances, optionally lowers `max_ecku`, optionally pauses Loams' instances. Tests: `guard_never_calls_delete`, `guard_refuses_pause_past_retention` (Task 13).
5. **A source loses or duplicates records across a restart.** Expected: neither (dedup by `<topic>/<partition>/<offset>`). Test: `kill_restart_no_loss_no_dup` (Task 1, Task 10).
6. **The nightly leaves a cluster running.** Expected: never past its TTL. Tests: `reaper_deletes_expired_clusters` dry-run fixture (Task 10), the `always()` teardown lint.

## Owner checklist

Owner actions; no step puts a credential in the repository or chat.

### Confluent (Ostrium's organisation, on the startup credits)

1. **Confirm the credit balance and expiry** (Billing → Payment → Promotions) and answer Q1333.
2. **Create the environments** `loams-ci` and `loams-demo` (the oracle stays in `default`).
3. **Create the service account `loams-ci-provisioner`** with the role EnvironmentAdmin on `loams-ci` only, and a Cloud API key for it. Put the key in the GitHub environment `byoc-confluent` (restricted to `dev`) as the secrets `CONFLUENT_CLOUD_API_KEY` and `CONFLUENT_CLOUD_API_SECRET`. Whether EnvironmentAdmin can create the per-run service accounts and keys the module needs is CC0 ruling 6; if not, also create `loams-ci-data` and `loams-ci-metrics` once and let the run create only API keys.
4. **Create the service account `loams-ci-billing`** with BillingAdmin, and its Cloud API key, into `/etc/loams-ci/confluent-billing.env` (mode 0600, user `ci`) on laptop A only (CC2 Task 15). Not in GitHub.
5. Keep the required **"Free usage is 80% consumed"** notification going to the owner's address; add "Cloud Service Quota surpassed 90% usage".
6. **Set the repository variable `CONFLUENT_CI_ENABLED=true`** when the nightly should start (the CI stop sets it to `false`).

### AWS (BY1's checklist first)

The BY1 AWS owner checklist (the `loams-ci-byoc` role, the $30 budget, `byoc-aws`) must be done; the Confluent nightly reuses that VM row. No new AWS permission is needed for CC1 (public endpoints). CC3's PrivateLink row adds `ec2:CreateVpcEndpoint` and `ec2:DeleteVpcEndpoints` on resources tagged `loams:ci-run` (a later policy change).

### HashiCorp (only if Q1336 is answered "yes")

An HCP organisation on the Free plan, one workspace `loams-ci-confluent-base` set to local execution, and a team token in the GitHub environment `byoc-confluent` as `TF_TOKEN_app_terraform_io`. With the default answer ("no"), nothing.

Time: about an hour.

## CC0 — Rulings (Task 0)

### Task 0 — Rulings and spikes

Check these and record the answers under "Rulings made during execution". Items 1–3 need a real cluster: run them from the owner's machine against the oracle cluster with the scoped key (topics and transactional ids under `loams-oracle-`), or hand them to the owner. Nothing is created outside that prefix.

1. **Exactly-once on Basic:** `loams-connector-kafka`'s `exactly_once` sink suite (`cargo test -p loams-connector-kafka --test txn` or its equivalent, pointed at the oracle with `security_protocol: sasl_ssl`) passes on `lkc-120zwvj`, including the compacted `__loams_checkpoints` topic created as `loams-oracle-checkpoints` for the run (the config's `checkpoint_topic`). If it fails, the profile narrows `sink.transactional` (Task 2).
2. **Latency:** p50 and p99 produce latency with `acks=all` and the end-to-end latency of one record from a host in India to `ap-south-1`, as the baseline for §80 §6.3.
3. **TLS roots:** the vendored OpenSSL build of librdkafka verifies the cluster's certificate with `ssl.ca.location=probe` in the `loams-fabric` image (Debian base); if not, the profile ships the CA bundle path.
4. **The region listing endpoint** the `confluent kafka region list` command calls, its auth, and whether it returns cluster types per region (for `regions.toml`).
5. **OAuth for Kafka clients per cluster type** (Basic, Standard), and whether the AWS IAM identity provider accepts an EC2 instance's identity (for CC3).
6. **What EnvironmentAdmin can do:** create service accounts, API keys and ACLs in its environment, or only clusters and topics (the owner checklist item 3).
7. **Metrics API:** whether `received_bytes` and `sent_bytes` break down by `metric.principal_id` on Basic and Standard, the data delay, and the rate limits (for the guard's shared-cluster estimate).
8. **Storage billing:** whether `$0.08/GB-month` is charged on pre-replication bytes (for `confluent_ceiling_monthly_usd`).
9. **Cloud-side data transfer** from a VM to a Confluent public endpoint in the same region: AWS public-IPv4 traffic and NAT gateway per-GB, Google Cloud and Azure equivalents (for the guide's cost table).
10. **IP filtering:** which types `confluent_ip_filter` applies to Kafka traffic on (for §80 §6.2).
11. **OpenTofu and the provider:** `tofu init` and `tofu validate` with `confluentinc/confluent` 2.89.0 and OpenTofu 1.11, and `tofu test` with `mock_provider "confluent"`, offline. If Q1336 is "yes": a state-only `cloud` block with state encryption enforced, against an empty HCP workspace (owner-run).

Commit: `docs: CC0 Task 0 rulings`.

## CC1 — Connect, profile, module, nightly (Tasks 1–11)

### Task 1 — `loams-fabric connect` (D1821)

Files:
- `fabric/crates/loams-fabric/src/cli.rs`: a `Connect(ConnectArgs)` role: `--instances <dir>` (YAML instance files), `--engine <url>` (the engine's stream gRPC address; the engine's `--stream-grpc-listen` is loopback-only, so the profile sets it to `127.0.0.1:<port>` and runs `connect` with `network_mode: service:<engine>`, the pattern the compose file already uses for `house-tailnet`), `--engine-key-file <path>`, `--secrets-root <dir>` (default `/var/lib/loams-connect/secrets`), `--state-bucket-prefix` (default `_meta/connect/`), `--admin-listen` (default `127.0.0.1:7790`).
- `fabric/crates/loams-fabric/src/connect/{mod,fabric_io,state,admin,instances}.rs`:
  - `fabric_io`: `EngineWriter` implements the SDK's `FabricWriter` with `ProduceCloudEvents` (idempotency by the event's `source` + `id`); `EngineReader` implements `FabricReader` with `FetchCloudEvents` and commits a consumer position per instance.
  - `state`: checkpoints and status in the bucket through `loams-store`, create-only versions; one process lock object per install (`_meta/connect/lock`, fenced by epoch, as D1354's create-only epoch object).
  - `admin`: `GET /v1/connect/instances`, `GET /v1/connect/instances/{id}`, `POST /v1/connect/instances/{id}:pause`, `POST …:resume`, `PATCH /v1/connect/instances/{id}` with `{"rate_limit": {"bytes_per_sec": N}}`; loopback only unless `--admin-listen` names the tailnet address; a bearer token from `secrets/connect-admin-token`.
  - `instances`: loads and validates instance files against the registry manifests (`loams-flow::validate`), refuses non-native runtimes with `runtime_not_supported_on_vm`.
- `deploy/release/compose.yaml`: a `connect` service in the `connectors` profile from the `loams-fabric` image, `command: [connect, ...]`, the instance directory and secret root as volumes, `read_only: true`, no published port.
- `deploy/release/.env.example`: `COMPOSE_PROFILES` note for `connectors`.

Tests (`fabric/crates/loams-fabric/tests/connect_*.rs`):
- `connect_roundtrip_memory`: a memory source → `EngineWriter` against an in-process engine test server (the `stream_grpc_events` harness, `crates/loams/tests/`), then `EngineReader` → memory sink; counts and order per key.
- `kill_restart_no_loss_no_dup`: abort mid-batch, restart, no loss and no duplicate in the stream (dedup by id).
- `pause_resume_and_rate`: pause stops `next` calls within one poll; resume continues from the checkpoint; a 64 KiB/s limit holds within 10% over 10 s.
- `refuses_fallback_runtime`, `single_writer_lock_fences_second_process`, `connect_logs_scrub_resolved_secrets`.
- The compose smoke (`deploy/release/ci/`): `connectors` profile starts and `/v1/connect/instances` answers on loopback.

Commit: `fabric: loams-fabric connect, native connectors on a BYOC VM (CC1 Task 1, D1821)`.

### Task 2 — The `confluent-cloud` target profile (D1822)

Files:
- `connectors/profiles/confluent-cloud.yaml`: `profile_kind: target`, `module: kafka`, `system.name: Confluent Cloud`, defaults `security_protocol: sasl_ssl`, `sasl.mechanism: plain`, `tls: {enabled: true, verify_full: true}`, `max_record_bytes: 8000000`, `create_checkpoint_topic: false` (the module or the user creates it; Loams' account has no CREATE); `narrows` only if Task 0 ruling 1 says so.
- `connectors/registry/kafka.yaml`: list the profile; `docs` link.
- `fabric/crates/loams-connector-kafka/src/config.rs`: a `rate_limit: { bytes_per_sec, requests_per_sec }` block mapped to the SDK's `RateLimit`; `schema_registry.basic_auth: { username, password }` where `password` is a secret reference (if not already expressible).
- `connectors/schemas/kafka.config.json`: the same fields.

Tests: `confluent_profile_defaults_apply`, `confluent_profile_requires_sasl_secret`, `rate_limit_config_maps_to_sdk` (unit); `profiles.rs` in `loams-flow` validates the new profile; a SASL_SSL leg of the existing Kafka suite against `apache/kafka:4.3.1` with a test CA and PLAIN users (in `deploy/fabric/compose.yaml`'s `connectors-p2` estate, fixed test credentials only).

Commit: `connectors: the Confluent Cloud target profile and Kafka rate limits (CC1 Task 2, D1822)`.

### Task 3 — Debezium decoding on the Kafka source (D1822)

Files: `fabric/crates/loams-connector-kafka/src/debezium.rs`; `config.rs` gains `format: plain | debezium-json | debezium-avro` (default `plain`) and `debezium: { tombstones: delete | drop, key_fields: [..] }`.

Behaviour (§80 §4.2): JSON with or without the `schema`/`payload` wrapper; Avro through the configured Schema Registry (the existing `registry.rs`); `op` `c`/`u`/`d`/`r` → `loamsop`; `after` (or `before` for deletes) as the row; the record key as the key; `source` as the position extension (`lsn`, `file`/`pos`, `scn`); `ts_ms` as the event time; a schema-change record type from Debezium's schema-change topic passed through as `schema-change`. Malformed envelopes go to the DLQ as poison.

Tests: fixtures from Debezium's documented example events for PostgreSQL and MySQL (Apache-2.0, with a `NOTICE` line in `tests/fixtures/debezium/README.md`): `decodes_pg_create_update_delete`, `decodes_mysql_with_schema_wrapper`, `tombstone_delete_or_drop`, `avro_via_registry` (the fake registry), `malformed_goes_to_dlq`.

Commit: `kafka: Debezium change events on the Kafka source (CC1 Task 3, D1822)`.

### Task 4 — `crates/loams-confluent`: the API client

Files: `crates/loams-confluent/{Cargo.toml,src/lib.rs,src/clusters.rs,src/keys.rs,src/metrics.rs,src/costs.rs,src/regions.rs,src/auth.rs}`; `tests/fixtures/*.json` (recorded shapes, ids replaced).
- `auth`: a Cloud API key (basic auth) or an OAuth bearer token; never logs either; `Debug` redacts.
- `clusters`: `get(env, id)`, `list(env)`, `set_max_ecku(env, id, n)` (`PATCH /cmk/v2/clusters/{id}`).
- `keys`: `create(owner, resource)` returning the secret once in a `Secret` type that zeroises on drop; `delete(id)`; `list(owner)`.
- `metrics`: `query` over `api.telemetry.confluent.cloud/v2/metrics/cloud/query` for `elastic_cku_count`, `received_bytes`, `sent_bytes`, `retained_bytes`, `partition_count`, grouped by hour, per cluster (and per principal if Task 0 ruling 7 allows).
- `costs`: `list(start, end)` over `GET /billing/v1/costs` with paging (`page_size` up to 10,000), the one-month window and the UTC, end-exclusive rules.
- `regions`: per Task 0 ruling 4.

Tests: each call against a local fake server with the fixtures: request shape, paging, error mapping (401, 403, 429 with `Retry-After`, 5xx retry), `secret_never_in_debug_or_error`.

Commit: `confluent: a small Confluent Cloud API client (CC1 Task 4)`.

### Task 5 — Regions and prices tables

Files: `deploy/byoc/confluent/regions.toml` (cloud, region, types), `deploy/byoc/confluent/prices.toml` (per cloud, region where known, and type: eCKU-hour, $/GB in, $/GB out, $/GB-month storage; `source`, `read` date; Basic's first eCKU free), `scripts/byoc/confluent-tables.sh refresh|check` (regions from the Task 4 client; prices are edited by hand with a source URL), `.github/workflows/byoc-confluent-tables.yml` (weekly `check`; opens a PR when the region list changes; fails when `prices.toml`'s `read` date is older than 90 days).

Tests: `regions_cover_by1_defaults` (`us-east-1`, `us-central1`, `eastus`), `prices_have_source_and_date`, shellcheck.

Commit: `deploy/byoc/confluent: region and price tables (CC1 Task 5, D1824)`.

### Task 6 — Secrets: fetch and keys (D1827)

Files:
- `deploy/byoc/lib/loams-byoc-secrets`: optional names `<prefix>/confluent-<integration>-{api-key,api-secret,sr-key,sr-secret,metrics-key,metrics-secret}` (with BY1's `/` → `-` rule on Google Cloud and Azure); `CONFLUENT_INTEGRATIONS` in `config.env` lists the integrations; values written to `<secrets-root>/<namespace>/confluent-<integration>/{username,password,sr-username,sr-password,metrics-key,metrics-secret}` with `umask 077`; never into `.env`. Regenerate the cloud-init files with `deploy/byoc/gen-cloud-init.sh`.
- `crates/loams` CLI: `loams byoc confluent key create --integration <i> --service-account <sa> --cluster <lkc> --cloud aws|gcp|azure --secret-prefix <p>` (uses the user's `CONFLUENT_CLOUD_API_KEY`/`SECRET` from the environment and the cloud's SDK credentials as the user has them; writes the secret store directly; prints only the key id); `… key rotate` (create, write a new version, `POST` the connect role's restart for the integration over the tailnet if reachable, wait, delete the old key after `--grace 1h`); `loams byoc confluent check` (region match, reachability of bootstrap and Schema Registry from the host, ACL sanity with `DescribeAcls` if the key may, the cost warning for cross-region).
- `crates/loams-support` (when Task 8 of BY1 exists): list integration names, never values.

Tests: `tests/byoc/static/mock_cloud.py` gains the Confluent names; `fetch_writes_confluent_files_0600`, `env_has_no_confluent_value`, `rendered_user_data_has_no_confluent_value`, `missing_optional_confluent_secret_is_not_fatal`; CLI tests against the Task 4 fake and a mock secret store: `key_create_never_prints_secret`, `rotate_deletes_old_after_grace`; `bundle_redacts_confluent_secrets` once the bundle exists.

Commit: `byoc: Confluent keys in the cloud secret store, fetched at boot (CC1 Task 6, D1827)`.

### Task 7 — The `confluent` OpenTofu module (D1823–D1826)

Files: `deploy/tofu/modules/confluent/{versions.tf,variables.tf,main.tf,outputs.tf,README.md,tests/plan.tftest.hcl}`; `deploy/tofu/examples/aws-vm-confluent/{main.tf,encryption.tf,README.md}`; `deploy/tofu/README.md` (the module list).
- Inputs and outputs exactly as §80 §5.2. `cluster_type` validation: `basic` or `standard`, or any with `allow_premium_cluster_types`. In CC1 the example uses `basic` and `max_ecku = 1` only (Q1335); `standard` is accepted by the module and exercised by CC2's weekly row.
- `confluent_ceiling_monthly_usd` from `deploy/byoc/confluent/prices.toml` (read with `file()` and `provider::`-free HCL decoding, or a generated `prices.auto.tfvars.json`; verify which OpenTofu supports cleanly) and `confluent_unbounded` as a list of strings.
- ACLs per §80 §7.1; `__loams_checkpoints` topic when `exactly_once_sinks = true`.
- `create_api_keys` (default `false`): when `true`, `confluent_api_key` resources and the cloud secret versions; the example's `encryption.tf` sets `key_provider "aws_kms"` on the install's KMS key, `method "aes_gcm"`, `state { enforced = true }`, `plan { enforced = true }`.
- The example wires `module.vm.region` into `module.confluent.region` and the secret prefix into both; it adds `confluent` to the VM's `CONFLUENT_INTEGRATIONS` and renders the connect instance files (a Postgres CDC source → stream → Kafka sink, and a Kafka source → stream) into the cloud-init's files.

Tests (`tofu test` with `mock_provider "confluent"` and the AWS mock, run by `tests/byoc/tofu/run.sh` and `byoc-static.yml`): `refuses_premium_types_without_flag`, `refuses_cross_region_without_flag`, `every_topic_has_retention_bytes`, `data_account_acls_are_minimal` (no CREATE, DELETE, ALTER, ALTER_CONFIGS on topics; no ACL operations; group and transactional-id ACLs PREFIXED to the configured prefixes), `no_api_key_resource_by_default`, `deletion_protection_on_unless_ci`, `ceiling_is_computed`; a static check `example_with_keys_enforces_encryption` (grep the example for `enforced = true` under `state` and `plan` when `create_api_keys = true`); `tofu fmt`, `tofu validate`, `tflint`.

Commit: `deploy/tofu: the confluent module and the aws-vm-confluent example (CC1 Task 7, D1823)`.

### Task 8 — `install.sh --kafka` and `loams byoc deploy --confluent`

Files: `deploy/release/byoc/install.sh`: `--kafka iggy|confluent|none` (default `none`; `iggy,confluent` allowed), `--confluent-bootstrap`, `--confluent-integration`; with `confluent` it turns on the `connectors` profile, writes the instance files from `deploy/release/connect/templates/`, and expects the secrets from Task 6. `loams byoc deploy` (when BY1's command exists; otherwise the guide's `tofu` steps): `--confluent [--confluent-cluster <lkc>] [--confluent-budget <usd>]` composes the example root and prints `estimated_monthly_usd`, `confluent_ceiling_monthly_usd` and `confluent_unbounded` before it applies.

Tests: `install_kafka_flag_matrix` (shell test over the rendered `.env` and instance files), `iggy_and_confluent_need_both_named`, shellcheck.

Commit: `byoc: install.sh --kafka and the Confluent deploy path (CC1 Task 8, D1820)`.

### Task 9 — Guides (D1834, part)

Files: `docs/guides/byoc/confluent.md`, `docs/guides/connectors/confluent-cloud.md`, a "Kafka endpoint: Iggy or Confluent" section in `docs/guides/kafka.md`, a link from `docs/guides/byoc/index.md` and `aws.md`, a row in `docs/design/networking-map.md`. Content per §80 §12; the hard/best-effort table copied from §80 §8.1 with the guard marked "CC2, not built yet" until Task 13 merges.

Commit: `docs: Confluent Cloud with BYOC guides (CC1 Task 9, D1834)`.

### Task 10 — The nightly AWS + Confluent row and the reaper (D1831, D1832)

Files:
- `scripts/byoc/confluent-ci.sh up|smoke|down|reap [--dry-run]`: `up` applies `deploy/tofu/examples/aws-vm-confluent` with `name = loams-ci-$GITHUB_RUN_ID`, `environment_id` = `loams-ci`, `cluster_type = basic`, `max_ecku = 1`, `region = us-east-1` (Q1334), keys out of band (Task 6) into the run's secret prefix, state on R2 under `loams-ci-state/confluent/<run-id>` (or local, destroyed in the job); `smoke` runs `tests/byoc/confluent_smoke.sh`; `down` destroys and checks the environment lists none of the run's clusters or keys; `reap` deletes `loams-ci-*` clusters and keys older than 3 hours and reports anything else older than a day.
- `tests/byoc/confluent_smoke.sh <engine-url>`: the round trip of §80 §11 (Postgres → stream → transactional Kafka sink → topic → Kafka source → collection; counts, order per key, no duplicates); a Debezium-envelope topic from a test producer (`kcat` with fixture files) into a table; a kill-restart of `connect`; a key rotation; latency p50/p99 printed; the exposure scan.
- `.github/workflows/byoc-confluent-nightly.yml`: `schedule` 04:15 UTC (after the AWS row) and `workflow_dispatch`; environments `byoc-aws` and `byoc-confluent`; first step exits when `vars.CONFLUENT_CI_ENABLED != 'true'`; `concurrency: byoc-confluent`; `timeout-minutes: 50`; `down` under `if: always()`.
- `.github/workflows/byoc-confluent-reaper.yml`: hourly `reap`.

Tests: `reaper_deletes_expired_clusters`, `reaper_keeps_oracle_and_demo`, `reaper_reports_unnamed` (dry-run fixtures in `tests/byoc/fixtures/confluent/`); `nightly_tears_down_in_always` (actionlint plus grep); shellcheck. **The cost check:** the first week's runs, read from the Metrics API by `confluent-ci.sh smoke`, must average under $0.10 a run.

Commit: `tests/byoc: the nightly AWS + Confluent round trip and its reaper (CC1 Task 10, D1831)`.

### Task 11 — CC1 exit

- The nightly green five nights in a row; the guides tested by someone following them on a fresh AWS account (or the nightly's account) end to end.
- `docs/guides/connectors/kafka.md`'s "Tested against" table gains Confluent Cloud (Basic, AWS `us-east-1`, nightly).
- A short exit note in "Rulings made during execution": measured latency, measured cost per run, the Task 0 rulings' effect.

## CC2 — Budget guard, spend, site (Tasks 12–16)

### Task 12 — Guard core in `loams-confluent` (D1829)

`src/guard.rs`: inputs (budget per integration, price table, metrics samples, costs lines, the connect role's byte counters), outputs (month to date, forecast, ceiling, ladder state, actions). Pure functions; the ladder of §80 §8.2 exactly; `original_amount` by default.

Tests: `ladder_50_80_alert_once_each`, `ladder_90_throttles_loams_only`, `ladder_100_pause_only_when_opted_in`, `guard_refuses_pause_past_retention`, `month_rollover_resumes`, `reconcile_records_drift_and_alerts_over_10pct`, `guard_never_calls_delete` (a client double that fails the test on any DELETE).

### Task 13 — The guard in `loams-fabric connect`

A periodic task (hourly estimate, daily reconcile when a billing reader is configured); state `_meta/connect/guard/<integration>.json`; actions through the admin API's own handlers; OTLP event `loams.confluent.budget`; optional webhook. Config: `guard.on = estimate|forecast`, `guard.floor_bytes_per_sec`, `guard.pause_at_limit` (default `false`), `guard.cluster_throttle` (default `false`, needs the ops key).

Tests: the Task 12 cases end to end against the Task 4 fakes and a running connect role; `guard_state_survives_restart`.

### Task 14 — Spend surfaces (D1830)

`loams confluent cost|budget set|budget resume|guard run --once`; `loams byoc status`'s line; `GET /v1/integrations/confluent/spend`; the console card (a `web/` plugin page under Settings → Integrations once the CF1 shell exists; until then a follow-up issue). Every number labelled.

### Task 15 — Ostrium's own guard and the CI stop

`deploy/ci-hosts/`: a `loams-confluent-guard.timer` on laptop A running `loams confluent guard run --once --org-wide --billing-env /etc/loams-ci/confluent-billing.env` daily; thresholds $60 alert, $80 alert, $100 stop; the stop sets `CONFLUENT_CI_ENABLED=false` with `gh variable set` (the host's `--gh-token-file`). Weekly Standard row: `byoc-confluent-weekly.yml` (`cluster_type = standard`, `max_ecku = 1`, ≤ 1 hour, plus the transactions check).

### Task 16 — Site (loams-site)

One PR in `ostrium-labs/loams-site`: "With Confluent Cloud" sections on `content/docs/(guides)/deploy/{aws,gcp,azure}.mdx`; `lib/confluent-prices.ts` (from `prices.toml`, with the source URL and read date); an optional Confluent group in `components/cost/calculator.tsx` (type, `max_ecku`, GB in, GB out, GB retained), list prices only. CodeRabbit reviews it.

**CC2 exit:** the guard observed walking the ladder in the nightly against a $0.10 test budget; Ostrium's guard running for two weeks; the site live.

## CC3 — Keyless, private networking, other clouds (milestone level)

Gate: CC2; for the cloud rows, BY1's Google Cloud and Azure owner actions. **2–3 engineer-weeks** (estimate).
- OAUTHBEARER in `loams-connector-kafka` through rdkafka's token-refresh callback, fed by the VM's cloud identity token (AWS IAM identity provider, Entra managed identity, Google ID token through a generic OIDC provider), per Task 0 ruling 5; the module's `auth = "oauth"` path with `confluent_identity_provider` and `confluent_identity_pool`; keys become optional.
- `confluent_networking = "privatelink"` with Enterprise: the interface endpoint, private DNS, the monthly row on credits (Q1339).
- `gcp-vm-confluent` and `azure-vm-confluent` examples and nightly rows.
- The container tier (BY1) and Kubernetes tier (BY2) reuse of `connect` and the guard (a CronJob).

## Effort

| Phase | Tasks | Effort (estimate) |
|---|---|---|
| CC0 | 0 | 3–4 days |
| CC1 | 1–11 | 4–5 engineer-weeks (Task 1 about 1.5 weeks; Tasks 2–8 about 2.5; 9–11 about 1) |
| CC2 | 12–16 | about 2 engineer-weeks |
| CC3 | milestone | 2–3 engineer-weeks |

## Rulings made during execution

None yet.
