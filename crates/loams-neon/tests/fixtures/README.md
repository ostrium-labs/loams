# loams-neon fixtures (PG2 Task 2)

Recorded on 2026-10-09 from `deploy/neon` at its pinned digests:
- `ghcr.io/neondatabase/neon@sha256:7a4f1249…` (Neon `77e22e4b`);
- `compute-node-v17@sha256:13ab146d…`;
- `loams-wal` from this repository.

The fork `ostrium-labs/neon` publishes no images yet (PG2 ruling R2.1). For these APIs the pinned build is the fork's code: none of the 12 commits from `77e22e4b` to the fork's tag `loams-decoder-trim-1` (`1218fb7a`) touches `libs/pageserver_api/src/{models,controller_api}.rs`, `pageserver/src/http/routes.rs`, `storage_controller/src/http.rs`, `libs/compute_api`, `compute_tools/src/http`, `libs/http-utils/src/error.rs` or `safekeeper/src/http` (`git log 77e22e4b..loams-decoder-trim-1 -- <those paths>` is empty; checked 2026-10-09). The pinned `pageserver --version` reports `git-env:77e22e4bf09d88b70b4a83a38c2f6de6301816b4`.

## Files

- **`*.request.json`** are what `loams-neon` sends. `capture.sh` sends them as written, so a request the component refuses fails the capture. `tests/client.rs` checks that the client produces them byte for byte after canonical JSON (keys sorted).
- **`*.response.json`** are the components' answers. `statuses.txt` has the method, the path and the HTTP status of each.
- **`src_*.response.json`** are hand-written from the fork's source, for answers the capture cannot provoke on one pageserver without a storage controller or a replica. Each is the exact text the source formats:
  - `src_branch_gc_cutoff`: `pageserver/src/tenant.rs` ("invalid branch start lsn: less than latest GC cutoff {}"), answered as 406 by `routes.rs` `timeline_create_handler` (`{err:#}`);
  - `src_create_in_progress`: `tenant.rs` `CreateTimelineError::AlreadyCreating`, answered as 429;
  - `src_promote_not_prewarmed`: `compute_tools/src/compute_promote.rs` (`bail!("compute {status}")` with `LfcPrewarmState::NotPrewarmed`'s `Display`), answered as a 500 `PromoteState::Failed` by `routes/promote.rs`;
  - `src_storcon_delete_timeout`: `storage_controller/src/http.rs` `deletion_wrapper`'s 409 with a `null` body;
  - `src_storcon_create_timeline`: `create_timeline.response.json` plus `safekeepers` (`models.rs` `TimelineCreateResponseStorcon`, `SafekeepersInfo`).
- **`spec_main.json`** is `ComputeSpecBuilder`'s golden output (`spec_builder_golden`).

  Its shape was checked by running the pinned `compute-node-v17` with it. Only the runtime's port, listen address and safekeeper were changed, a `cloud_admin` role was added, and the config file was wrapped as `{spec, compute_ctl_config}`. `compute_ctl` took the basebackup from the pageserver, created role `app` and database `app`, and set `neon.max_cluster_size` to 10 GB.

| Fixture | Source of the shape (fork, tag `loams-decoder-trim-1`) |
|---|---|
| `attach.request.json` | `libs/pageserver_api/src/models.rs` `LocationConfig` (`TenantLocationConfigRequest`) |
| `create_timeline.request.json`, `branch.request.json` | `models.rs` `TimelineCreateRequest` + `TimelineCreateRequestMode` |
| `tenant_config.request.json` | `models.rs` `TenantConfigRequest` / `TenantConfig` (`pitr_interval` in humantime) |
| `*_timeline*.response.json`, `branch.response.json` | `models.rs` `TimelineInfo` |
| `lsn_by_timestamp.response.json` | `pageserver/src/http/routes.rs` `get_lsn_by_timestamp_handler` |
| `conflict.response.json`, `not_found.response.json` | `libs/http-utils/src/error.rs` `HttpErrorBody` |
| `wal_*` | `crates/loams-safekeeper/src/http.rs` (this repository) |
| `branch_below_ancestor.*`, `delete_with_children`, `delete_tenant_missing` | `routes.rs` `timeline_create_handler` (406), `From<DeleteTimelineError>` and `timeline_delete_handler` (412) |
| `compute_status`, `compute_unauthorized`, `prewarm_state`, `promote_primary` | `compute_api/src/responses.rs` (`ComputeStatusResponse`, `GenericAPIError`, `LfcPrewarmState`, `PromoteState`); recorded from `deploy/neon`'s `compute1` |
| `spec_main.json` | `libs/compute_api/src/spec.rs` `ComputeSpec` |
| (no fixture: no controller in `deploy/neon`) `NeonClient` through the storage controller | `controller_api.rs` `TenantCreateRequest`; `storage_controller/src/http.rs` routes |
| (no fixture) `ComputeCtlClient` | `compute_api/src/{requests,responses}.rs` (`ConfigurationRequest`, `PromoteConfig`, `PromoteState`, `ComputeStatusResponse`, `GenericAPIError`); `compute_tools/src/http/server.rs` routes |

## Re-recording

From the repository root, on a fresh stack:

```sh
SP=$(mktemp -d)   # any scratch directory
export DOCKER_HOST=unix:///run/user/$UID/podman/podman.sock   # with Podman
(cd deploy/neon && docker compose up -d rustfs create-bucket storage_broker pageserver safekeeper1)
target/debug/loams-wal --listen-pg 127.0.0.1:55701 --listen-http 127.0.0.1:57701 &
# compute_ctl with a key of the capture's own: compute-jwt.py adds it to the
# compute config and prints a token for compute id compute-capture.
export COMPUTE_JWT=$(python3 -I crates/loams-neon/tests/fixtures/compute-jwt.py deploy/neon/compute/config.json "$SP")
export CAPTURE_CONFIG=$SP/config.json
crates/loams-neon/tests/fixtures/capture.sh http://127.0.0.1:9898 http://127.0.0.1:57701 http://127.0.0.1:3080
```

The capture creates the tenant and timeline `compute1` attaches to, then says "waiting for compute_ctl" and waits up to 4 minutes. Start `compute1` then, from a second shell with the same `CAPTURE_CONFIG`:

```sh
(cd deploy/neon && TENANT_ID=4c6f616d734e656f6e54656e616e7431 TIMELINE_ID=4c6f616d734e656f6e54696d656c6e31 \
  docker compose -f compose.yaml -f ../../crates/loams-neon/tests/fixtures/compose.capture.yaml up -d compute1)
```

Afterwards: `kill %1` and `(cd deploy/neon && docker compose -f compose.yaml -f ../../crates/loams-neon/tests/fixtures/compose.capture.yaml down -v)`.

The ids are fixed ("LoamsNeonTenant1", "LoamsNeonTimeln1", "LoamsNeonBranch1" and "LoamsNeonBranch2" in hex). Start from a fresh stack (`down -v`), because a second capture against the same pageserver records the existing tenant's answers.

Two behaviours the capture showed:
- `loams-wal`'s create answers the existing timeline (200) where the pageserver answers `409` for a conflicting create.
- `DELETE` of a timeline is `202` with a `null` body; `PATCH /v1/tenant/config` is `200` with `null`.
