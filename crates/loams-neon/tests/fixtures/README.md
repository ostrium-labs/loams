# loams-neon fixtures (PG2 Task 2)

Recorded on 2026-10-09 from `deploy/neon` at its pinned digests:
- `ghcr.io/neondatabase/neon@sha256:7a4f1249…` (Neon `77e22e4b`);
- `compute-node-v17@sha256:13ab146d…`;
- `loams-wal` from this repository.

The fork `ostrium-labs/neon` publishes no images yet (PG2 ruling R2.1). For these APIs the pinned build is the fork's code: none of the 12 commits from `77e22e4b` to the fork's tag `loams-decoder-trim-1` (`1218fb7a`) touches `libs/pageserver_api/src/{models,controller_api}.rs`, `pageserver/src/http/routes.rs`, `storage_controller/src/http.rs`, `libs/compute_api`, `compute_tools/src/http`, `libs/http-utils/src/error.rs` or `safekeeper/src/http` (`git log 77e22e4b..loams-decoder-trim-1 -- <those paths>` is empty; checked 2026-10-09). The pinned `pageserver --version` reports `git-env:77e22e4bf09d88b70b4a83a38c2f6de6301816b4`.

## Files

- **`*.request.json`** are what `loams-neon` sends. `capture.sh` sends them as written, so a request the component refuses fails the capture. `tests/client.rs` checks that the client produces them byte for byte after canonical JSON (keys sorted).
- **`*.response.json`** are the components' answers. `statuses.txt` has the method, the path and the HTTP status of each.
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
| `spec_main.json` | `libs/compute_api/src/spec.rs` `ComputeSpec` |
| (no fixture: no controller in `deploy/neon`) `NeonClient` through the storage controller | `controller_api.rs` `TenantCreateRequest`; `storage_controller/src/http.rs` routes |
| (no fixture) `ComputeCtlClient` | `compute_api/src/{requests,responses}.rs` (`ConfigurationRequest`, `PromoteConfig`, `PromoteState`, `ComputeStatusResponse`, `GenericAPIError`); `compute_tools/src/http/server.rs` routes |

## Re-recording

```sh
(cd deploy/neon && docker compose up -d rustfs create-bucket storage_broker pageserver)
target/debug/loams-wal --listen-pg 127.0.0.1:55701 --listen-http 127.0.0.1:57701 &
crates/loams-neon/tests/fixtures/capture.sh http://127.0.0.1:9898 http://127.0.0.1:57701
kill %1
(cd deploy/neon && docker compose down -v)
```

The ids are fixed ("LoamsNeonTenant1", "LoamsNeonTimeln1" and "LoamsNeonBranch1" in hex). Start from a fresh stack (`down -v`), because a second capture against the same pageserver records the existing tenant's answers.

Two behaviours the capture showed:
- `loams-wal`'s create answers the existing timeline (200) where the pageserver answers `409` for a conflicting create.
- `DELETE` of a timeline is `202` with a `null` body.
