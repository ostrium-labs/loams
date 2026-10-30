# The `ErrorInfo.reason` registry

Status: **API1 Task 2** (2026-10-03), with later services' rows appended as they land (latest: Loams Git, GT1 Task 12, 2026-10-12): Task 1's registry plus `not_found`'s metadata, which `loams.collection.v1` fills from `ServiceError::NotFound`'s `kind` and `name` (`crates/loams/src/api/errors.rs` rule 3). Design [§44](../design/44-unified-api-and-sdks.md) §7.4, decision **D611**; plan [API1](../plans/2026-10-02-api1-unified-connect.md).

`loams.collection.v1` adds **no** row: every RPC it serves reuses the `error`
code the REST surface already raises as its `reason` (API1 plan ruling 1.6),
through the one helper in `crates/loams/src/api/connect_collections.rs`.
Three of `api::errors`'s codes are not registry rows and fold into the row
they mean — `schema_violation` → `invalid_argument` (the offending field
rides in `metadata`), `conflict` → `aborted` (a fenced lease or a version
mismatch is a concurrent write that won), `timeout` → `deadline_exceeded` —
and a code with no mapping at all is a bug in a layer that grew one, so it
answers `internal` and is logged rather than inventing a reason.

Every failed RPC carries a Connect code (`unimplemented`, `not_found`, ...) and one `loams.errors.v1.ErrorInfo` in its details. Callers branch on **`reason`**, a stable `snake_case` string, never on `code` alone and never on `message` (which may change). This page is the registry of every `reason` the server can return, so a caller can look one up instead of guessing a string.

The rules, in the order they bite:

- A reason is added here **before** an RPC returns it.
- Within an API major version a reason is **never renamed and never removed**. A bug fix is a new reason; a deprecated one keeps working and stops being returned.
- `snake_case`, `[a-z0-9_]` only, unique. The test `reasons_are_snake_case_and_unique` (`crates/loams/tests/connect_api.rs`) reads this page and fails on a duplicate, on a name outside `snake_case`, and on a registry that has lost one of the reasons it promised.
- The mapping from `reason` to a typed error class is **generated** by SDK1 from this page plus the protos; until then each SDK maps the codes of its own language.

`metadata` carries structured context (`{"variant": "standard"}`), never secrets. `hint` is a short next step in the caller's locale.

## Registry

| reason | Connect code | Raised by | Metadata |
|---|---|---|---|
| `approval_expired` | failed_precondition | DecideApproval | |
| `approval_already_decided` | failed_precondition | DecideApproval | |
| `approval_stale_revision` | failed_precondition | DecideApproval | |
| `requester_cannot_approve` | permission_denied | DecideApproval | |
| `decision_proof_invalid` | permission_denied | DecideApproval | |
| `step_up_required` | unauthenticated | DecideApproval, CreatePairing | |
| `reason_required` | invalid_argument | DecideApproval | |
| `invalid_decision` | invalid_argument | DecideApproval | |
| `pairing_expired` | failed_precondition | the pairing grant (MT) | |
| `pairing_used` | failed_precondition | the pairing grant (MT) | |
| `device_revoked` | unauthenticated | every RPC from a revoked device | |
| `push_target_unknown` | not_found | UnregisterPushTarget | |
| `not_implemented` | unimplemented | a stub handler whose service has not landed yet; ObjectService Blame for a file whose history the server's blame walk cannot follow (in `gix-blame` 0.18, a path that was a directory, or below one, in an ancestor), so the shim runs `git blame` instead (GT1 Task 21, ruling T21.1) | |
| `feature_not_in_variant` | unimplemented | any RPC of a catalogue package whose engine is not in this build variant (§44 §4, §30 §9) | `variant` |
| `house_not_configured` | unimplemented | any `loams.house.v1` RPC on a server with no `[house] endpoint` (`--house-endpoint`): the main port has no Loams House front to proxy to (HS1 Task 7, §49 §18.1). A front that is configured but unreachable answers `unavailable` | |
| `house_syntax_error` | invalid_argument | `loams.house.v1` ExecuteQuery, ExplainQuery, PreviewTable: ClickHouse's `62 SYNTAX_ERROR` — a statement that does not parse, more than one statement, or a form outside the House's surface (HS1 Task 8, §49 §18.2) | `clickhouse_code`, `clickhouse_name` |
| `house_unknown_table` | not_found | `loams.house.v1` statements, DescribeTable, PreviewTable: ClickHouse's `60 UNKNOWN_TABLE` | `clickhouse_code`, `clickhouse_name` |
| `house_unknown_database` | not_found | `loams.house.v1` statements and the browsing RPCs: ClickHouse's `81 UNKNOWN_DATABASE` (HS1 Task 8; §49 §18.2 lists no reason for it) | `clickhouse_code`, `clickhouse_name` |
| `house_access_denied` | permission_denied | `loams.house.v1`: ClickHouse's `497 ACCESS_DENIED`; a read-only user's write (`164`); CancelQuery of another caller's query; `ListQueryHistory` with scope `NAMESPACE` without `house:admin` | `clickhouse_code`, `clickhouse_name` when ClickHouse raised it |
| `house_read_only` | failed_precondition | `loams.house.v1` ExecuteQuery with `read_only` absent or true and a statement that writes or changes state: refused before it runs (ClickHouse's `164 READONLY`); the desktop asks to confirm and runs it again with `read_only = false` (§49 §18.3) | `clickhouse_code`, `clickhouse_name` |
| `house_timeout` | deadline_exceeded | `loams.house.v1` statements: ClickHouse's `159 TIMEOUT_EXCEEDED` — past `timeout_ms` (or 300 s), or the worker killed 2 s after it | `clickhouse_code`, `clickhouse_name` |
| `house_memory_limit` | resource_exhausted | `loams.house.v1` statements: ClickHouse's `241 MEMORY_LIMIT_EXCEEDED`, or the worker killed over its memory | `clickhouse_code`, `clickhouse_name` |
| `house_too_many_queries` | resource_exhausted | `loams.house.v1` statements: ClickHouse's `202 TOO_MANY_SIMULTANEOUS_QUERIES` — no worker free within the acquire timeout | `clickhouse_code`, `clickhouse_name` |
| `house_quota_exceeded` | resource_exhausted | `loams.house.v1` statements past a namespace quota (raised from HS1 Task 22, §49 §14) | `quota`, `limit` |
| `house_disabled_function` | permission_denied | `loams.house.v1` statements: ClickHouse's `344 SUPPORT_IS_DISABLED` — a table function, function, engine, statement or `system` table the House's deny list refuses (§49 §13) | `clickhouse_code`, `clickhouse_name` |
| `house_cancelled` | canceled | `loams.house.v1` ExecuteQuery and PreviewTable: ClickHouse's `394 QUERY_WAS_CANCELLED` — CancelQuery, or the statement's worker stopped for it | `clickhouse_code`, `clickhouse_name` |
| `house_query_not_found` | not_found | `loams.house.v1` CancelQuery (no running query of that id) and GetQuery (no logged query of the caller's with that id) (HS1 Task 8) | `query_id` |
| `house_query_failed` | unknown | `loams.house.v1` statements: any other error the engine raised whose ClickHouse HTTP status is not 4xx (one whose status is 4xx answers `invalid_argument`), with its ClickHouse code and name (HS1 Task 8) | `clickhouse_code`, `clickhouse_name` |
| `project_not_found` | not_found | every `loams.postgres.v1` RPC that names a project | `project` |
| `branch_has_children` | failed_precondition | DeleteBranch | `children` (count) |
| `branch_protected` | failed_precondition | DeleteBranch, RestoreBranch (an agent's goes through `loams.approvals.v1` instead) | `branch` |
| `lsn_out_of_retention` | failed_precondition | CreateBranch, RestoreBranch | `oldest_lsn`, `history_retention` |
| `endpoint_exists_for_branch` | already_exists | CreateEndpoint (a second read-write endpoint) | `endpoint` |
| `compute_start_failed` | unavailable | StartEndpoint, RestartEndpoint (the Operation's error) | `compute` |
| `unsupported_pg_version` | invalid_argument | CreateProject: a `pg_version` new projects may not use, either one Loams Postgres does not serve or a maintenance major (design 62 §9.2, D1114) | `field`, `offered` (the offered majors, a beta one marked `(beta)`) |
| `beta_major_requires_opt_in` | failed_precondition | CreateProject: a beta major without `allow_beta_major` | `field`, `pg_version` |
| `compute_major_mismatch_refused` | failed_precondition | StartEndpoint, RestartEndpoint (the Operation's error): a compute image of another major than the branch's timeline (§51 §9) | |
| `import_checksums_enabled` | failed_precondition | UpgradeProject and imports: a data directory with data checksums on, which the pageserver refuses (D1111; Loams Postgres pages carry no checksums) | |
| `quota_exceeded` | resource_exhausted | any `loams.postgres.v1` create or resize past a limit (§46 §14); any `loams.repos.v1` create past a limit (GT1 Task 75) | `quota`, `limit` |
| `storage_unavailable` | unavailable | any `loams.postgres.v1` RPC that needs the pageserver, `loams-wal` or the storage controller while it is down | `component` |
| `secret_already_issued` | failed_precondition | CreateRole, ResetRolePassword, IssueConnectCredential (`loams.postgres.v1`) and `loams.admin.v1` CreateKey (API2 Task 7) replayed with the same `idempotency_key`: the secret is answered once, and the idempotency ledger keeps only that it was issued, never the secret | `role` (postgres); `hint`: ResetRolePassword for a new password, IssueConnectCredential or CreateKey with a new key for a new credential |
| `repo_not_found` | not_found | every `loams.repos.v1` RPC that names a repository, and `ForkRepo`'s `source`; also a name in another namespace than the credential's (D182) | `repo` |
| `repo_already_exists` | already_exists | CreateRepo, ForkRepo: the name is taken, by a repository created under another `idempotency_key` (a retry with the same key and request answers the first repository) | `repo` |
| `repo_deleting` | failed_precondition | any `loams.repos.v1` RPC but GetRepo on a repository that was deleted and is waiting to be purged, and CreateRepo or ForkRepo of its name until the purge is done; the purge waits while a fork of it lives (GT1 Task 13) | `repo` |
| `ref_stale` | aborted | a ref write (UpdateRepoConfig's commit, PushWorkspace, the API's ref writes) whose expected old value no longer holds: another write won | `ref` |
| `ref_protected` | failed_precondition | a ref write a protection rule refuses (a delete under `deny_delete`, a force update under `deny_force`) | `ref` |
| `out_of_scope` | permission_denied | a write outside the credential's branch prefixes or cones (GT1 Task 16, D397) | `ref` or `path` |
| `object_format_unsupported` | unimplemented | CreateRepo with `OBJECT_FORMAT_SHA256`: SHA-1 only for now (Q388) | `object_format` |
| `fork_too_deep` | failed_precondition | ForkRepo of a repository that is already 8 forks deep (`MAX_FORK_DEPTH`); ForkWorkspace of a workspace that is (GT1 Task 25, ruling T24.6) | `max` |
| `sequencer_unavailable` | unavailable | a `loams.repos.v1` write whose repository sequencer is shutting down, lost its lease, or could not learn whether the write committed: retry with the same `idempotency_key`, which answers the first outcome if it did | `repo` |
| `idempotency_key_reused` | failed_precondition | a `loams.repos.v1` mutation whose `idempotency_key` was used inside its window for a different request; PushWorkspace, whose key binds the caller and the request as asked, so a retry that finds the refs moved answers them up to date (GT1 Task 27); CreateWorkspace and ForkWorkspace, whose key names the new workspace, for any other request under the same key (GT1 Task 25) | |
| `workspace_not_found` | not_found | every `WorkspaceService` RPC that names a workspace the repository does not have or that was deleted, a snapshot it does not hold (GetSnapshot, ForkWorkspace) or a salvage record (RestoreSalvage) (GT1 Task 25, §36 §19.10), and `ObjectService`'s reads that name one the repository does not have (GT1 Task 18) | `workspace`; `n` for a snapshot |
| `workspace_fenced` | aborted | SaveSnapshot, ApplyEdits, CommitWorkspace: another writer appended snapshot `n` first (§19.4.3); the caller reads the new head, merges or salvages. SaveSnapshot keeps the refused state as salvage under its `writer` and names it (GT1 Task 25); DeleteWorkspace and RestoreSalvage answer it after 16 lost races | `n`, `writer` (the winner's); `mount`, `salvage` when salvaged |
| `batch_partially_applied` | aborted | **client-side**: `documents.write_batch` stopped at a failed chunk (runtime contract R13); the chunks before it stay applied, and the failed chunk's own error is the cause | `chunks_applied`, `rows_applied` |
| `workspace_read_only` | failed_precondition | a write to a workspace mounted or opened read-only, or to a past snapshot | `workspace` |
| `workspace_name_taken` | already_exists | CreateWorkspace, ForkWorkspace: the owner already has a workspace of that name | `workspace_name`, `workspace` (the holder) |
| `object_missing` | failed_precondition | SaveSnapshot, PushWorkspace: a snapshot or push names an object that is neither uploaded nor in the repository (§19.5.2), or SaveSnapshot a pack the workspace does not hold (metadata `pack`; GT1 Task 25); CreateWorkspace: `at` names no commit the repository holds; PutWorkspacePack refuses a thin pack with `invalid_argument` instead (GT1 Task 24, ruling T24.2); GetTrees: the root, or an object a tree names, is not held (GT1 Task 18) | `oid` |
| `secret_detected` | failed_precondition | ApplyEdits, PushWorkspace, receive-pack, PutWorkspacePack (its scanner hook, GT1 Task 24; a no-op until Task 67): the leak scanner found a credential outside `env/` (§19.13.9) | `path`, `rule` |
| `env_unsealed` | invalid_argument | ApplyEdits, PushWorkspace, receive-pack: an `env/` file holds a plaintext value (§19.13.2) | `path` |
| `env_access_denied` | permission_denied | `EnvService` Open, GetLiveSet, Seal: the credential has no grant for the environment (§19.13.7) | `environment` |
| `env_foreign_reference` | permission_denied | ApplyEdits, PushWorkspace, receive-pack: a sealed `env/` reference names a secret of another repository or path (§19.13.4) | `path` |
| `memory_store_not_found` | not_found | every `loams.memory.v1` RPC that names a store (by name or `ms_` id) the namespace does not have | `store` |
| `memory_space_not_found` | not_found | every `loams.memory.v1` RPC that names a space by id, or by a (kind, external_id) key that does not exist when the call may not create it | `space` |
| `memory_space_limit` | resource_exhausted | `CreateSpace`, and `Add` creating a space on first use, past the store's spaces limit (§52 §13.1) | `limit` |
| `memory_managed_object` | failed_precondition | a direct write through any other API (Connect, Qdrant, Elasticsearch) to a `_mem.*` or `_code.*` collection or graph, which only Loams Memory writes (§52 §5.1, D891) | `object` |
| `memory_block_version_mismatch` | aborted | `UpdateBlock` whose `if_version` is not the block's version: another write won; re-read, merge and retry (§52 §6.5) | `label`, `version` |
| `memory_block_too_large` | invalid_argument | `UpdateBlock` with a value over the block's `limit`, or a `limit` over 16 384, or past the space's 128 blocks | `label`, `limit` |
| `memory_core_budget_exceeded` | resource_exhausted | `UpdateBlock` that would take the space's blocks past its core-memory cap (default 8 000 characters, §52 §6.5) | `limit` |
| `memory_content_too_large` | invalid_argument | `Add`, `AddBatch` item: content over 1 MiB (§52 §13.1) | `limit` |
| `memory_secret_detected` | failed_precondition | `Add`, `AddBatch` item: the content holds a secret and the store's secret policy is `refuse` (§52 §7.1) | `rule` (never the secret) |
| `memory_pii_refused` | failed_precondition | `Add`, `AddBatch` item: the content holds personal data and the store's PII policy is `refuse` | `kind` |
| `memory_model_not_configured` | failed_precondition | a call that needs a model the store has none of: `Search` with `rerank: model` and no reranker, `Consolidate` steps that need a chat model, `RetryExtraction` with no chat model | `model` (chat, embedder or reranker) |
| `memory_model_not_local` | failed_precondition | `CreateStore`, `UpdateStore`, and any model call of a `local_only` store, whose model host is neither loopback nor in `private_hosts` (§52 §10.2) | `host` |
| `memory_token_budget_exhausted` | resource_exhausted | a model call refused before it is made because the store's daily, run or per-call token budget is spent (§52 §10.3); also an episode's `extraction.reason` | `budget`, `retry_after_ms` |
| `memory_extraction_failed` | failed_precondition | `Add` with `wait_ms` whose extraction failed within the wait (the episode is written and searchable); also an episode's `extraction.reason` after invalid model output or a provider error (§52 §7.1.2) | `episode`, `cause` |
| `memory_too_many_spaces` | invalid_argument | `Search`, `GetProfile`: more than 8 spaces (§52 §4.2) | `limit` |
| `memory_cross_space_write` | invalid_argument | a write that would touch two spaces, or link entities of two spaces (I-ISO, §52 §5.3) | |
| `memory_not_yet_available` | unimplemented | a `loams.memory.v1` RPC whose MM1 task has not landed, on a server built with the feature `memory` (MM1 plan "Shared contracts"; no longer raised from MM1 Task 56, but never removed) | `rpc` |
| `code_repo_not_indexed` | failed_precondition | a `CodeMemoryService` query of a repository with no code index (§52 §8; MM1 Task 27) | `repo` |
| `code_index_building` | unavailable | a `CodeMemoryService` query while the repository's first index is being built; retry later | `repo`, `retry_after_ms` |
| `code_change_too_large` | invalid_argument | `ReviewContext`, `ImpactRadius`, `RiskScore`: a change over the file limit (default 500, at most 2 000; §52 §13.1) | `limit` |
| `code_language_unsupported` | failed_precondition | a code query on a file whose language has no parser or resolver | `language` |
| `code_scip_invalid` | invalid_argument | `UploadScipIndex`: the SCIP index does not parse, or names another repository or commit | `cause` |
| `live_bad_cursor` | invalid_argument | `loams.live.v1` Query, Mutate and the session's queries: a function's `paginate` got a cursor that is malformed, altered, or from another app, table or index (LV1 Task 4) | |
| `live_worker_crashed` | internal | `loams.live.v1` Query, Mutate and the session's queries: the isolated worker process running the function crashed or was killed; the call's transaction never committed, and a mutation is not retried (LV1 Task 5, design §45 §3.1). Live's own wire code is `ERROR_CODE_FUNCTION_ERROR`; on Connect it is `internal`, since the crash is the server's (LV1 Task 19) | |
| `live_deploy_conflict` | failed_precondition | `loams.live.v1` Deploy (LiveAdminService's Deploy and ActivateDeployment from LV1 Task 10): the deploy's `expected_active` is not the app's active deployment, because another deploy activated in between; the message names both. The deployment's record is `failed` (LV1 Task 6, design §45 §3.3) | |
| `live_schema_violation` | invalid_argument | `loams.live.v1` Mutate (and every write a function makes, `_system:*` included): the document does not match its table's validator, or a stricter validator a deploy is checking; the message names the table and the field paths, never the values (LV1 Task 8, design §45 §4.1) | `field_violations` (path, description) |
| `filter_conflict` | invalid_argument | `loams.collection.v1` ScrollDocuments, CountDocuments, DeleteByFilter and PatchByFilter with both `filter` and `where` set (SDKP P0a) | `field` (`where`) |
| `search_after_needs_tiebreak` | invalid_argument | **client-side**: `search.query_all` with a sort that does not end in the primary key, which could skip or repeat a hit across pages | `field` (`sort`) |
| `invalid_consistency_token` | invalid_argument | **client-side**: a consistency token that is not the `v1:` encoding, refused before it is sent (runtime contract R4) | `field` |
| `namespace_required` | invalid_argument | **client-side**: an app-package request with no `namespace` and a client with no default namespace (runtime contract R11); never sent | `field` (`namespace`) |
| `sql_param_count` | invalid_argument | `loams.sql.v1` Query and QueryArrow: `params` and the statement's `$n` placeholders differ in number (SDKP P0b) | `expected`, `got` |
| `live_index_not_ready` | failed_precondition | `loams.live.v1` Query, Mutate and the session's queries: the function read an index a deploy is still backfilling; it becomes readable when that deploy activates (LV1 Task 9, design §45 §4.2). The message names the index and the table | |
| `live_app_disabled` | failed_precondition | `loams.live.v1` every `LiveService` and `LiveAdminService` call that addresses an app (the `loams-live-app` header, or the server's own app): the app is disabled (`UpdateApp`) or deleted (`DeleteApp`, until it is purged) (LV1 Task 13, design §45 §7) | |
| `live_app_exists` | failed_precondition | `loams.live.v1` LiveAdminService CreateApp: another app of the cluster has that name (LV1 Task 13) | |
| `live_function_error` | unknown | `loams.live.v1` Query, Mutate and the session's queries: the deployed function threw; the message is the function's own error message (LV1 Task 19) | |
| `live_function_timeout` | deadline_exceeded | `loams.live.v1` Query, Mutate and the session's queries: the function ran past its JavaScript CPU limit (LV1 Task 19) | |
| `client_deadline_exceeded` | deadline_exceeded | **client-side**: an SDK's own deadline for one call ran out, retries included (runtime contract R12); never retried | `timeout_ms` |
| `live_function_out_of_memory` | resource_exhausted | `loams.live.v1` Query, Mutate and the session's queries: the function ran past its runtime's memory limit (LV1 Task 19) | |
| `live_limit_exceeded` | resource_exhausted | `loams.live.v1` every call: one of the app's limits was exceeded (a document's size or fields, a mutation's writes, a function's scans or result); `limit` names it (LV1 Task 19, design §20 §4.1 and §5.1) | `limit` |
| `result_too_large` | resource_exhausted | `loams.sql.v1` QueryArrow: the Arrow answer is over `--sql-arrow-max-bytes` (API2 Task 1) | `use` (`flight`) |
| `live_app_mismatch` | permission_denied | `loams.live.v1` every call: the credential is bound to an environment, and the addressed Live app belongs to another (or the call creates or lists apps, which needs a credential bound to none) (LV1 Task 15, design §45 §6.2) | |
| `live_origin_not_allowed` | permission_denied | `loams.live.v1` `LiveService` from a browser: the request's `Origin` is not among the addressed Live app's allowed origins (a platform credential on the engine's own CORS origins, the console's, passes) (LV1 Task 18, design §45 §6.4) | |
| `session_rotate` | unavailable | `loams.live.v1` Watch: the session reached its maximum age (24 h) and closed; the client resumes it, and its credential is checked again (LV1 Task 17, design §45 §5.4, Q637) | |
| `session_gone` | not_found | `loams.live.v1` ModifyQuerySet and Authenticate: no Live node holds the session any more (its owner stopped, died or ended it); the client resumes it on any node (LV1 Task 24, design §45 §9) | |
| `node_draining` | unavailable | `loams.live.v1` LiveService: the Live node is shutting down; its Watch streams close with it and new calls are refused; the client resumes on another node (LV1 Task 24, design §45 §9) | |
| `role_required` | permission_denied | every `loams.admin.v1` RPC whose caller's org role is below the one it needs (reads `member`, mutations `admin`, inviting or removing an owner `owner`), or who is not in the org (API2 Task 7, `member_cannot_create_key`); every `loams.repos.v1.RepoService` RPC likewise (reads `member`; create, fork, delete, configure and compact `admin`; GT1 Task 16) | `role` (the role needed) |
| `setup_required` | failed_precondition | every `loams.admin.v1` RPC, and every `loams.repos.v1.RepoService` RPC of a caller who is not the operator (GT1 Task 16), before first-run setup (`AuthService/CompleteSetup`) has created the org | |
| `token_missing` | unauthenticated | any main-port RPC outside `PUBLIC_RPCS` called without a credential (the auth layer, API2 Task 6); on `LiveService`, Live's own refusal when the addressed app takes no anonymous calls (LV1 Task 16) | |
| `token_malformed` | unauthenticated | a credential that is neither an API key, a session token nor a JWT, or more than one `Authorization` header | |
| `token_revoked` | unauthenticated | a revoked API key (`KeyService/RevokeKey`), or a console session that was signed out or whose person left the org | |
| `token_not_yet_valid` | unauthenticated | an OIDC token whose `nbf` is in the future | |
| `token_bad_signature` | unauthenticated | an OIDC token whose signature does not verify | |
| `token_wrong_audience` | unauthenticated | an OIDC token for another audience (on `LiveService`, none of the issuer's audiences on the addressed Live app) | |
| `token_wrong_issuer` | unauthenticated | an OIDC token from an issuer this engine does not trust (on `LiveService`, nor the addressed Live app, LV1 Task 16) | |
| `token_unknown_key` | unauthenticated | an OIDC token signed with a key its issuer's JWKS does not have | |
| `token_algorithm_not_allowed` | unauthenticated | an OIDC token signed with an algorithm other than RS256 or EdDSA | |
| `api_key_invalid` | unauthenticated | an API key no key file or metastore record admits | |
| `credential_not_accepted` | unauthenticated | a credential kind this engine's `LOAMS_AUTH_MODE` does not take | |
| `credential_in_query_string` | unauthenticated | any request with `access_token`, `token`, `api_key`, `apikey` or `api-key` in its query string (D608) | |
| `csrf_token_invalid` | permission_denied | a call authenticated by the `loams_session` cookie that is not a `GET` (nor `GetSession`) without the session's `X-CSRF-Token` | |
| `principal_not_allowed` | permission_denied | a valid OIDC or tailnet identity that is not on the engine's allowlist | |
| `cors_origin_not_allowed` | permission_denied | a browser call from an origin outside `LOAMS_CORS_ORIGINS` | |
| `auth_unavailable` | unavailable | a credential that cannot be checked now (the issuer's JWKS or the principal store is unreachable) | |
| `join_token_unknown` | unauthenticated | `loams.ca.internal.v1` Issue: no join token with that id, or its secret does not match (the two are not told apart) | |
| `join_token_spent` | unauthenticated | `loams.ca.internal.v1` Issue: the join token was already used (tokens work once, D2311) | |
| `join_token_expired` | unauthenticated | `loams.ca.internal.v1` Issue: the join token is past its hour (D2311) | |
| `identity_not_granted` | permission_denied | `loams.ca.internal.v1` Issue and Renew: the CSR asks for an identity the token's grant, or the caller's current certificate, does not hold, or one outside this CA's cluster | `identity` |
| `identity_revoked` | permission_denied | `loams.ca.internal.v1` Issue and Renew: the caller's certificate, or an identity asked for, is on the deny list (`loams ca revoke`, D2312) | |
| `ca_not_initialized` | failed_precondition | `loams.ca.internal.v1`: the deployment has no CA yet (`loams ca init`), or this node cannot open its intermediate key | |
| `invalid_argument` | invalid_argument | any RPC: a malformed field, an unparseable value | `field` |
| `not_found` | not_found | any RPC: the named resource does not exist | `kind`, `name` |
| `already_exists` | already_exists | any RPC that creates a named resource | |
| `permission_denied` | permission_denied | any RPC the caller's role may not make | |
| `token_expired` | unauthenticated | a rejected access token | `hint` says whether to refresh or sign in again |
| `unauthenticated` | unauthenticated | any RPC with no or an unusable credential | |
| `failed_precondition` | failed_precondition | any RPC whose preconditions do not hold | |
| `resource_exhausted` | resource_exhausted | a backpressure or quota refusal | `retry_after_ms` |
| `unavailable` | unavailable | a dependency is down or this node cannot serve the read | |
| `deadline_exceeded` | deadline_exceeded | the caller's deadline passed | |
| `aborted` | aborted | a concurrent write won; the caller retries | |
| `internal` | internal | a bug; the message and `request_id` go to the log | |

The `loams.postgres.v1` rows (`project_not_found` to `secret_already_issued`) are design §46 §4.1's, registered by PG2 Task 1 before `pg-control` raises them (PG2 Task 9); `reasons_registered` (`crates/loams-proto/tests/postgres.rs`) fails if one §46 §4.1 names is missing. A replayed create under a taken name with another key answers the generic `already_exists`, and a `RestartEndpoint` with `promote` while the branch's read-write endpoint runs the generic `failed_precondition` (PG2 Task 1 ruling R1.8).

The `loams.memory.v1` rows (`memory_store_not_found` to `code_scip_invalid`) are Loams Memory's (design §52 §9.1, D884), registered by MM1 Task 1 before any RPC raises them, with the codes of MM1 ruling R1.4; `reasons_registered` and `reasons_have_their_connect_codes` (`crates/loams-proto/tests/memory.rs`) fail if one §52 §9.1 names is missing or its code changes. The `code_*` rows are `CodeMemoryService`'s (MM1 Task 27); `memory_not_yet_available` stays registered after MM1 Task 56, which stops raising it.

The `loams.repos.v1` rows (`repo_not_found` to `env_foreign_reference`) are Loams Git's (design §36 §19.10, plan GT1's "Public API" contract), registered by GT1 Task 12 before any RPC raises them; `reasons_registered` (`crates/loams-proto/tests/repos.rs`) fails if one the proto or §19.10 names is missing. `quota_exceeded` is shared with `loams.postgres.v1`.

`live_bad_cursor` is LV1 Task 4's, `live_worker_crashed` LV1 Task 5's, `live_deploy_conflict` LV1 Task 6's, `live_schema_violation` LV1 Task 8's, `live_index_not_ready` LV1 Task 9's, `live_app_disabled` and `live_app_exists` LV1 Task 13's, the function and limit reasons LV1 Task 19's, `live_app_mismatch` LV1 Task 15's, `session_rotate` LV1 Task 17's, `session_gone` and `node_draining` LV1 Task 24's, and `live_origin_not_allowed` LV1 Task 18's: `loams_live::LiveError::wire_reason` names them, and every Live error carries one in its `ErrorInfo` on both the main port and Live's own listener (Task 19), an internal one with a `request_id` the server log carries.

Seven rows are SDKP's (2026-10-19). `sql_param_count` and `result_too_large` are `loams.sql.v1`'s (P0b). The other five are **client-side**: an SDK raises `namespace_required` (R11), `invalid_consistency_token` (R4), `search_after_needs_tiebreak` (`search.query_all`), `client_deadline_exceeded` (R12) and `batch_partially_applied` (R13) itself, before or instead of a server answer, and no server returns them. They are registry rows so every SDK's typed error carries them the way it carries a server's, and `error.json` lists them as unproducible with that reason.

The `loams.ca.internal.v1` rows (`join_token_unknown` to `ca_not_initialized`) are the deployment CA's (design §83 §6.3–§6.5, D2310–D2312), registered by CT1 Task 3 part 2b. The package is internal: only the cluster's own processes call it.

The generic rows (`invalid_argument` and below) are the code-to-class mapping of D611's error hierarchy: they are what an SDK turns into its own `InvalidArgument`, `NotFound`, ... types. They carry no Loams-specific semantics, so they were already the native REST API's `error` values (`crates/loams/src/api/errors.rs`) and become the RPC `reason` unchanged. The rows above them are the specific causes AP0 registered; the specific causes API1 Tasks 2–8 add are appended as their services land, and `unavailable_service_reports_reason` covers the variant case.

## Not reasons

- **`cursor_expired`** is not an error: a watch stream whose cursor the server no longer has resets with a fresh snapshot instead (§37 §8.3).
- A **`Deprecated`** RPC (a removed REST route behind `--legacy-rest`, §44 §5.3) answers `308`/`410` with `{"error":"moved","rpc":…}`, not an `ErrorInfo`.

## agentd reasons (daemon-only)

`loams.agentd.v1` (design §53 §6.1, plan SD1 Task 4) is served by `loams-agentd`, the per-user daemon, and never by the Loams server or an SDK (Q816). Its errors carry the same `loams.errors.v1.ErrorInfo`, whose `reason` is the daemon's ndjson `{err}` prefix (`loams_agentd_proto::rpc::ERROR_REASONS`) or one of SD1's additions, under the Connect code below (`crates/loams-agentd-connect/src/error.rs`). An error without a known prefix answers `unknown` with no `ErrorInfo`.

These are **not registry rows**: the names are written without backticks, so neither `reasons_are_snake_case_and_unique` nor the SDK generator reads them, and no SDK grows a type for a daemon it cannot call. `unauthenticated`, `not_found` and `limit_exceeded` mean what the rows above mean.

| Reason | Connect code | When |
|---|---|---|
| unauthenticated | unauthenticated | No token, or a wrong one, on a listener |
| forbidden_role | permission_denied | The caller's role may not call the method (§50 §7); also the remote `studio` role (SD1 Task 7) |
| protocol_mismatch | failed_precondition | ndjson only: a `Hello` with another protocol |
| agentd_draining | unavailable | The daemon is draining before a stop or an upgrade |
| unknown_method | unimplemented | ndjson only |
| bad_params | invalid_argument | A request the daemon cannot parse or map (an unset required field, an unknown enum value) |
| key_required | failed_precondition | A harness needs a key first |
| unknown_provider | invalid_argument | |
| invalid_url | invalid_argument | |
| tool_unavailable_remote_server | failed_precondition | |
| not_found | not_found | |
| engine_not_ready | unavailable | The sessions engine is still starting |
| keyring_unavailable | unavailable | The OS keyring cannot be reached |
| limit_exceeded | resource_exhausted | Too many streams on one connection |
| harness_unsupported | failed_precondition | A harness this daemon does not ship |
| not_implemented | unimplemented | A §53 §6.5 gap (G1–G23) before its SD1 task lands |
| stale_diff | failed_precondition | A checksum does not match (commit, discard, restore; SD1 Tasks 9, 10) |
| merge_conflict | aborted | `MergeBranch` stopped on conflicts (SD1 Task 9) |
| nothing_to_commit | failed_precondition | SD1 Task 9 |
| push_rejected | failed_precondition | A non-fast-forward or a lease failure (SD1 Task 9) |
| provider_not_configured | failed_precondition | No Forgejo or GitHub credential for `CreateChangeRequest` (SD1 Task 9) |
| checkpoint_missing | not_found | SD1 Task 10 |
| group_not_ready | failed_precondition | `ResolveRunGroup` while members still run (SD1 Task 12) |
| approval_not_pending | failed_precondition | SD1 Task 13 |
| origin_not_allowed | permission_denied | The remote listener, for an origin it was not given (SD1 Task 7) |

The desktop's protocol proxy answers two codes of its own as a JSON body, not as an `ErrorInfo`: `forbidden_method` (403, a method the renderer may not call) and `agentd_unavailable` (503, no daemon running).

## Adding one

1. Add the row here, with the Connect code it is raised under and the RPC that raises it.
2. Raise it through the one helper that builds the error (`crates/loams/src/api/connect.rs`), so the code and the `reason` cannot drift apart.
3. Run `cargo test -p loams --test connect_api`. `reasons_are_snake_case_and_unique` fails on a duplicate or a name that is not `snake_case`.