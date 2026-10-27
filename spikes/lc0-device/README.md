# LC0 Task 5 (partial): OpenFGA's MySQL datastore on Starter

Spike code for [LC1](../../docs/plans/2026-10-23-lc1-loams-cloud.md) LC0 Task 5. It is not product code.

**Scope.** The plan's Task 5 runs device enrolment on IA0's Zitadel stack. It was **not run** in LC0, by the owner's direction of 2026-10-23 ("Zitadel isn't needed now"). The hosted try-out's device identity moved to WorkOS CLI Auth plus device keys issued by the control plane (D1755, design 78 §25.14), and self-hosted control planes keep D1714 as written. So the Rust device binary and the tests `device_flow_issues_tokens`, `deactivated_device_is_denied` and `device_cannot_act_for_another_org` wait for whoever builds the self-hosted path. See the plan's rulings.

What did run is the TiDB half of the task: **does OpenFGA's MySQL datastore work on a free Starter instance** (design 78 §17, the research report §5)?

## How to run

```bash
set -a; . ~/tidbcloud-lc0.env; set +a
./openfga_on_starter.sh <cluster-id> <user_prefix>
```

- `openfga_setup.py` creates database `lc0p_openfga` and a dedicated SQL user. The datastore URI goes into a 0600 env file for `podman --env-file`, and is never printed.
- OpenFGA runs at IA0's pin, `openfga/openfga:v1.22.0@sha256:9cf9a20a…`, and listens on **127.0.0.1 only**.
- `openfga_smoke.py` creates a store and a model (`org#admin` and `device#owner` through `org`), writes 50 × 2 tuples, runs 100 checks (50 that must allow and 50 that must deny), then a `list-objects` call, a revoke and a re-check.
- On exit, the script drops the database and the user.

## Result (2026-10-23, the `lc0-cells` instance, client about 206 ms from `aws-us-east-1`)

| Step | Result |
|---|---|
| `openfga migrate` (MySQL engine, `tls=true`) | **Works:** every migration applied in about 16 s |
| `openfga run` | Serving; no datastore errors in the log |
| 100 checks | **All correct** (50 allow, 50 deny) |
| `list-objects` for `user:u7` | `["device:d7"]` only |
| Revoke, then re-check | Denied |
| Write latency p50 / p95 | 2,052 / 2,452 ms |
| Check latency p50 / p95 | 924 / 1,155 ms |

- The latencies are dominated by the 206 ms round trip from this workstation. A check is a few sequential SQL round trips, and a write is a transaction of several.
- An OpenFGA in `aws-us-east-1` next to the instance would see single-digit milliseconds per round trip (estimate).
- But OpenFGA needs a long-running host, and the try-out's control plane is a Worker. So §17's default, OpenFGA on Postgres beside Zitadel, stays for self-hosted control planes, and the try-out doesn't need OpenFGA at all (§25.8: the admin check is WorkOS role plus allow-list plus step-up, through `loams-authz`).

**Answer to §78 §17's (verify):** yes, OpenFGA v1.22.0's MySQL datastore runs on TiDB Cloud Starter (TiDB v8.5.3). Migrations, tuple writes, checks and deletes all work. It is usable only from a host near the instance.
