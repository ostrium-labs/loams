# IS1 — Loams Infra: OpenTofu Provider and State, Secrets, Config (plan stub)

> **For agentic workers:** REQUIRED SUB-SKILL: use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Work task by task, test first. Where this plan gives exact names, paths, branches or tests, use them verbatim. Record any deviation in "Rulings made during execution" at the end of this file.
>
> **Status: Stub** (2026-10-19). Track IS, design [§64](../design/64-loams-infra-stack.md) (D1150–D1171, Q970–Q983, approved by the owner on 2026-10-19 with every default). Owner request, 2026-10-19: **"I want all of the HashiCorp stack in Loams, in Rust."** This plan covers only the minimum viable slice, IS1a–IS1c (§64 §12). IS2–IS4 get their own plans once their gates are met (D1171). **Nothing here starts before v0.1.0 ships (Q983).** Each task lists its files, the tests that must exist and pass, and its gate. Interfaces are fixed in §64; Task 0 of each milestone checks the code as it stands and records rulings.

**Goal:**
- a team manages its Loams namespaces, Postgres projects, branches, roles, databases, collections, streams, API keys, secrets and config from OpenTofu, with no secret in state;
- OpenTofu state lives in Loams, locked, versioned and encrypted under the namespace's key;
- apps, CI and agents get short-lived Loams Postgres and Loams SQL credentials through one lease API, and CI authenticates with its own OIDC token;
- config changes reach running processes without a redeploy, and services register and resolve on BYOC nodes.

**Architecture** (§64 §10): three cargo features of the `loams` binary (`infra-state`, `secrets`, `config`), off in the default build; a separate provider binary. Hot state in `loams-kv` (TiKV, redb in `loams dev`); bodies in the bucket under `KeyProvider` (D96); Connect services `loams.infra.v1`, `loams.secrets.v1`, `loams.config.v1`; the OpenTofu `http` backend as a compatibility surface (D603); workflows on `loams-durable`.

**Tech stack:** Rust (tokio, tonic and connect-rust as in `crates/loams`, rustls, `aes-gcm`, `jsonwebtoken` or the crate `crates/loams` already uses), the pinned OpenTofu CLI (two minor versions, ≥ 1.11) in CI, stock Postgres 16–18 and MySQL 8.4 images for the external engines.

## Global Constraints

- **Worktrees and branches.** `~/Documents/Ostriumlabs/loams-wt/is1-<task>`, one branch and one PR per task into `dev` (CodeRabbit reviews; merge on a green `CI required`). `git commit -s`.
- **Builds.** Shared target directory, one cargo build at a time, touched crates only; never build in `/tmp`.
- **The default build does not change.** Test `default_features_exclude_infra`: `cargo tree -p loams -e normal` on default features lists none of `loams-tfplugin`, `loams-secrets`, `loams-config`.
- **Secrets.** Every token, issued credential, secret value, data key and signing key is `Secret<T>` (`[redacted]` in `Debug` and `Display`). Test `no_issued_value_in_logs` captures logs, errors, events and metrics for the whole IS1 suite and greps for every value issued.
- **Licences (§64 §11, D1169).** No code from any HashiCorp repository released after 2023-08-10; `tfplugin6.proto` vendored unmodified under `proto/vendor/tfplugin6/` with its MPL-2.0 header and a NOTICE line; no translation of OpenBao or OpenTofu Go code. `cargo deny check` passes with no new allow entry.
- **Pins.** New crates and the OpenTofu CLI at releases at least 14 days old, except security fixes.
- **OpenTofu only** in CI and docs (D1150).

## Review Focus

1. **A secret reaches OpenTofu state or a plan file.** Expected: never. Tests: `provider_secret_not_in_state`, `ephemeral_credential_not_in_plan`.
2. **Two writers both commit state.** Expected: the stale lock holder is refused. Tests: `state_write_without_lock_refused`, `state_stale_epoch_refused`, `state_serial_must_increase`.
3. **A lease outlives its revocation.** Expected: the credential stops working by the lease end, and on `RevokeByRun` within one workflow step. Tests: `lease_expiry_drops_role`, `revoke_by_run_drops_all`, `revocation_resumes_after_crash`.
4. **An external OIDC token is accepted without a matching trust.** Expected: refused with `trust_not_matched`. Tests: `trust_requires_all_claims`, `trust_rejects_wrong_issuer`, `trust_ttl_capped`.
5. **A principal reads a secret, state or config it has no relation to.** Expected: `permission_denied`. Tests per service: `*_openfga_denied`.
6. **A watch misses a config revision.** Expected: never; a watch resumes from its revision. Tests: `config_watch_resumes`, `catalog_lease_expiry_removes_instance`.

## Milestones and tasks

### IS1a — OpenTofu provider and state backend (Tasks 0–9)

| Task | Content | Files | Tests | Gate |
|---|---|---|---|---|
| 0 | Rulings: the `Authorizer` trait and principal kinds to use (§64 §10.2), the existing JWT and KMS code, whether GT1 Task 59 has landed | this file | — | Rulings recorded |
| 1 | `tfplugin6.proto` vendored; `loams-tfplugin` crate skeleton | `proto/vendor/tfplugin6/`, `crates/loams-tfplugin/` | `handshake_line_format`, `automtls_uses_client_cert` | `cargo deny` green |
| 2 | **Spike gate (Q970):** a one-resource provider passes plan, apply, refresh, import and destroy under `tofu`, including an ephemeral resource and a write-only attribute | `crates/loams-tfplugin/examples/` | `spike_lifecycle_tofu` | Pass, or the Go fallback is chosen and recorded |
| 3 | Proto options `(loams.options.v1.resource)` and `(loams.options.v1.secret)`; annotations on the IS1 RPCs | `proto/loams/options/v1/`, the services' protos | `buf breaking` clean; `resource_annotations_complete` | — |
| 4 | Generator: provider schemas and CRUD glue from the annotations | `crates/loams-facade-gen/` | `generated_schema_snapshot` | Drift check in CI |
| 5 | Provider binary with the 13 resources and their data sources (§64 §4.2) | `tools/terraform-provider-loams/` | per-resource `acc_<resource>_lifecycle` against `loams dev` | All green on two OpenTofu versions |
| 6 | State service: versions, KMS wrapping, fenced lock leases, `StateService` | `crates/loams/src/infra/`, `proto/loams/infra/v1/` | Review Focus 2; `state_rollback_writes_new_version`; `state_diff_redacts_sensitive` | — |
| 7 | The `http` backend surface at `/tofu/v1/state/...` | `crates/loams/src/api/` | `tofu_init_http_backend`, `tofu_lock_conflict_423` | — |
| 8 | CloudEvents `io.loams.dev.infra.state.*.v1`; console list of workspaces and versions | `crates/loams/src/infra/`, `web/` | `state_events_have_no_values` | — |
| 9 | Release: mirror repository workflow, signing key (Q978), OpenTofu registry submission (owner action), docs | `.github/workflows/`, `docs/` | `provider_release_dry_run` | Q974 answered for the repository name |

### IS1b — Loams Secrets (Tasks 10–21)

| Task | Content | Files | Tests | Gate |
|---|---|---|---|---|
| 10 | Rulings; `loams-vault` core and `KeyProvider` if GT1 Tasks 59–60 have not landed (D1154) | `crates/loams-vault/` | GT1's vault tests, run here if built here | — |
| 11 | `loams-secrets` crate: folders, secrets, versions over `loams-vault`; `SecretService` | `crates/loams-secrets/`, `proto/loams/secrets/v1/` | `secret_versions_create_only`, `secret_destroy_keeps_record` | — |
| 12 | Leases in `loams_secrets` (TiKV, redb in dev); `LeaseService`; revocation, renewal and expiry workflows on `loams-durable` | `crates/loams-secrets/src/lease/` | Review Focus 3 | — |
| 13 | Engine `loams-postgres` over `IssueConnectCredential` (D711) | `crates/loams-secrets/src/engines/` | `pg_lease_connects_through_pgdog`, `lease_expiry_drops_role` | — |
| 14 | Engine `loams-sql`, with the user-issuing RPC Loams SQL lacks today (no `loams.sqldb.v1` proto on `dev` at `034c8cb4`) | same, plus §47's control plane | `sql_lease_connects_through_sqlgate`, `sql_lease_expiry_drops_user` | — |
| 15 | Engines `postgres` and `mysql` (external) with SQL templates; `random`; `webhook` (D843) | same | `external_pg_lease_lifecycle`, `external_mysql_lease_lifecycle` | Q973 |
| 16 | `IdentityService`: trust policies and the RFC 8693 exchange for external OIDC tokens | `crates/loams/src/auth/`, `proto/loams/secrets/v1/identity.proto` | Review Focus 4; `github_actions_token_exchanged` (recorded fixture) | — |
| 17 | The Loams workload issuer: SPIFFE-shaped JWTs, JWKS under `/.well-known/`, key rotation | same | `workload_jwt_verifies_with_jwks`, `jwks_rotation_overlap` | Q975 |
| 18 | OpenFGA types `secret_folder`, `state_workspace`, `config_env`; read audit as CloudEvents plus hash-chained log | `deploy/openfga/`, `crates/loams-secrets/src/audit.rs` | `secret_read_audited`, `audit_chain_verifies` | Q976 |
| 19 | Delivery: `loams env run` reads the secrets service; the Dapr `loams` component; an ESO webhook example; SDK modules `secrets`, `config` | `crates/loams/src/cli/`, `deploy/dapr/`, `docs/` | `env_run_injects_live_set`, `eso_webhook_example_resolves` | — |
| 20 | Provider wiring: `loams_secret` write-only, `ephemeral "loams_secret"`, `ephemeral "loams_postgres_credential"`, `loams_identity_trust` | `tools/terraform-provider-loams/` | Review Focus 1 | — |
| 21 | Security pass: fuzz targets for token and request parsers before authorisation; `no_issued_value_in_logs`; threat model note | `crates/loams-secrets/fuzz/`, `docs/security/` | fuzz smoke in CI | External review scheduled before preview exit (D1157) |

### IS1c — Loams Config and Catalog (Tasks 22–26)

| Task | Content | Files | Tests | Gate |
|---|---|---|---|---|
| 22 | `loams-config` crate; `ConfigService` with typed entries, revisions, CAS, `Watch`, `Render` | `crates/loams-config/`, `proto/loams/config/v1/` | `config_cas_conflict`, `config_watch_resumes`, `config_render_dotenv` | — |
| 23 | `# loams:plain` env lines promote into config entries (with GT1's `EnvPromoter`, if built) | `crates/loams-config/src/env.rs` | `plain_env_line_becomes_entry` | GT1 Task 15 or a stub |
| 24 | `CatalogService`: lease-based registration, optional checks by the node agent, `Resolve`, `Watch` | `crates/loams-config/src/catalog/` | `catalog_lease_expiry_removes_instance`, `catalog_resolve_by_tag` | — |
| 25 | `loams-gateway` `http` upstreams from the catalog (when the `http` module exists; otherwise a documented hook) | `crates/loams-gateway/` | `gateway_upstreams_follow_catalog` | GW `http` plan |
| 26 | Provider resource `loams_config_entry`; CLI `loams config`; console page | `tools/terraform-provider-loams/`, `crates/loams/src/cli/`, `web/` | `acc_loams_config_entry_lifecycle` | — |

## Owner actions

- Q974: counsel on the `terraform-provider-` repository name and nominative use of HashiCorp marks, before Task 9's registry submission.
- Create the mirror repository `ostrium-labs/terraform-provider-loams` and submit the provider and its signing key to the OpenTofu registry (Task 9).

## Rulings made during execution

(none yet)
