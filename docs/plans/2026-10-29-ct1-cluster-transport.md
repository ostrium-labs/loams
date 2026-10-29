# CT1 — Cluster Transport: HTTP/3 over QUIC with mTLS — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Work task by task, test first. Where this plan gives exact names, paths, flags or tests, use them verbatim. Record any deviation in the task's rulings file, `docs/plans/ct1-rulings/t<N>.md`, indexed in "Rulings made during execution" at the end of this file.
>
> **Status: Approved.** The owner approved the coordinator's recommendation on 2026-10-29: Connect RPC, mTLS everywhere, HTTP/3 by default with HTTP/2+TLS fallback, per-path defaults by benchmark.
> - Track CT; design [§83](../design/83-cluster-transport-http3-mtls.md).
> - Rulings made during execution take decision IDs from **D2332–D2349** and questions from **Q1486–Q1489**.
> - **Nothing here is built yet.** Tasks 0–6 (T0 and T1, which close issue #1560) are specified task by task. Tasks 7–14 are specified at the level needed to start; each one's rulings file refines it.

**Goal (T1, first to ship):** the cluster listener requires client certificates from the deployment's CA. Every route on it is authorized on the caller's SPIFFE identity, and the meter's `LeaseService` and service ingest see per-role callers instead of `loams-cluster`. This closes #1560.

**Goal (T2–T5):** every internal hop in §83 §4 runs over HTTP/3 with mTLS. HTTP/2+TLS is the automatic fallback, and also the default for any path class where the benchmark (Task 11) says so. TiKV and PD keep gRPC over HTTP/2, with mTLS.

**Architecture** (§83 §5.3): a new crate `crates/loams-transport` provides:
- the endpoint (UDP and TCP on one port; HTTP/3 and HTTP/2 into one `tower::Service`);
- the client pool with race and fallback;
- `Identity` (certificate load, watch and renew);
- the compiled `routes.toml` with its `RoutePolicy` layer;
- metrics.

The CA service, `loams.ca.internal.v1.CertificateService`, runs in the `meta` role of `crates/loams`. Every internal listener and client in `loams`, `loams-gateway`, `loams-fabric`, `loams-temporal`, `loams-meter-agent` and agentd moves onto `loams-transport`, phase by phase.

**Tech stack:**
- Rust 1.97.1, edition 2024.
- `quinn` 0.11 (already in Cargo.lock), `h3` and `h3-quinn`, pinned by Task 0 to releases at least 14 days old.
- `rustls` 0.23 with `aws-lc-rs`, `hyper` 1, `tokio-rustls`, `rcgen` and `x509-parser`, all already in Cargo.lock.
- `connectrpc` 0.9 unchanged.

**Spec:** [§83](../design/83-cluster-transport-http3-mtls.md), all of it; [§63](../design/63-loams-meter.md) §11–§13 (D2203, D2206); issue #1560; [networking map](../design/networking-map.md) §4–§5; `crates/loams/src/tls.rs` (the existing main-port TLS and client-CA verifier).

## Global Constraints

- **Worktrees and branches:**
  - `~/Documents/Ostriumlabs/loams-wt/ct1-t<N>-<slug>`, from `origin/dev`;
  - one PR per task into `dev`, labelled `review:greptile` (security);
  - commit with `git commit -s`; never `git stash`.
- **Light builds:**
  - only through `~/Documents/Ostriumlabs/loams-wt/.he0-shared/buildslot.sh`, with `-j 4`, one package at a time; never `--workspace` locally;
  - never set `CARGO_TARGET_DIR`, and never build in `/tmp`;
  - delete your own artefacts.
- **No plain internal listener after T1** except under `--insecure-dev` on loopback (D2303). Every new internal route gets a `routes.toml` entry in the same PR; the route-coverage test (Task 4) fails CI otherwise.
- **The RPC layer does not change** (D2301). No `.proto` changes are needed for transport work. `buf breaking` must stay clean.
- **Compatibility** (D2327): every phase works against the previous release inside the upgrade window. The upgrade gate (`scripts/release/upgrade-gate.sh`) runs on each phase's PR.
- **Pins** at least 14 days old (D1274). No AGPL, BSL, FSL, SSPL or ELv2 code. No C QUIC library.
- **Honest status labels:** "preview" until the per-path rulings (Task 11) are recorded and the upgrade gate passes T2.

## Review Focus

- **Certificate handling:**
  - keys are generated in memory and never logged;
  - join tokens are single-use and expire;
  - the deny list is checked at handshake and on every new stream;
  - no CRL or OCSP fetches.
- **Route policy:** deny by default, and every served route is listed.
- **0-RTT:** off for non-idempotent routes, with a test for each such route.
- **Fallback:** it never silently downgrades to plaintext, only to HTTP/2+TLS.

## T0 — Identity and the CA (Tasks 0–5)

### Task 0 — Rulings and pins (lead)

Record in `docs/plans/ct1-rulings/t0.md`:
- the pinned versions of `h3` and `h3-quinn`, and confirmation that `quinn` 0.11.12 is reused;
- confirmation that `connectrpc` serves through `tower::Service`, so one service can sit under both HTTP/2 and HTTP/3 (§83 §3, verify);
- whether `rustls` with `aws-lc-rs` covers QUIC's TLS in `quinn` without `ring` (verify);
- TiKV and PD's TLS options: which accept a URI SAN identity, and what `cert-allowed-cn` needs (verify, used in Task 9).

Exit: the rulings file is merged. Effort: half a day.

### Task 1 — The inventory audit

Grep `crates/`, `fabric/`, `deploy/` and `bench/` for every listener (`TcpListener::bind`, `axum::serve`, `tonic::transport::Server`, `UdpSocket`) and every internal client (`reqwest`, `hyper` clients, `tonic` channels, `MetaClient`, `HttpTransport`). Compare the result with §83 §4's table (H1–H21). Add any missing hop to `docs/plans/ct1-rulings/t1.md`, with its class (A, B or C), its current auth and its target phase.

Exit: the rulings file lists every hop, and each hop has a phase. Effort: 1 day.

### Task 2 — Stack spike (report only)

In a scratch crate under `bench/transport/spike`, which is not a workspace member, serve one Connect unary method and one server-streaming method over HTTP/3 (`quinn` + `h3-quinn`) and over HTTP/2+TLS from the same `tower::Service`, with mTLS on both. Measure on loopback, briefly: handshake time, a 1 KiB unary at 64 in flight, and a 64 MiB stream. This is a smoke test, not Task 11's benchmark.

Report: `docs/plans/ct1-rulings/t2.md`, covering the API friction, the `h3` maturity issues found, and whether `s2n-quic` needs a look (Q1471). Effort: 2 days.

### Task 3 — `loams-transport`: identity and the CA service

- **New crate `crates/loams-transport`:**
  - `Identity`: holds the key and certificate chain; renews at two-thirds of the leaf's life; watches files when `source = "files"` (D2314);
  - `SpiffeId` parse and format (`node/`, `role/`, `svc/`);
  - the `rustls` `ServerConfig` and `ClientConfig` builders: TLS 1.3 only, P-256, the client-certificate verifier pinned to the trust bundle, and a deny-list hook.
- **New proto `proto/loams/ca/internal/v1/ca.proto`**: `CertificateService` with `Issue`, `Renew` and `TrustBundle`.
- **In `crates/loams`, on `meta` nodes:**
  - the CA service;
  - the root and intermediate (sealed with age, as D2205 does);
  - issued serials, join tokens and the deny list in the metastore.
- **CLI:** `loams ca init|status|token|revoke|rotate`.

Tests:
- `identity_renews_before_two_thirds`;
- `issue_refuses_spent_token`;
- `issue_refuses_identity_outside_token_grant`;
- `renew_requires_matching_identity`;
- `revoked_identity_cannot_renew`;
- `deny_list_drops_live_connection`;
- `trust_bundle_rotation_keeps_old_intermediate_30_days`;
- `keys_never_written_to_disk` (checks the file system after issue).

Exit: the crate and CA service are merged, behind the `transport` feature (default off). Effort: 5 days.

### Task 4 — `routes.toml` and `RoutePolicy`

- `crates/loams-transport/routes.toml`, with every route from Task 1's inventory and its allowed identities. The meter routes are as §83 §12 lists them.
- A `RoutePolicy` `tower` layer:
  - matches a Connect method or an HTTP method and path;
  - refuses unlisted routes with `permission_denied` or HTTP 403;
  - sets `Caller::Node` or `Caller::Service`.
- The coverage test walks every internal router (`api::internal::routes`, `meta_rpc::router`, meter internal, graph internal, git internal, CA) and fails on any route not in the table.

Tests:
- `every_internal_route_is_listed`;
- `unlisted_route_is_refused`;
- `lease_service_refuses_meter_agent`;
- `ingest_accepts_meter_agent`;
- `raft_accepts_only_role_meta`;
- generated: for each route, every allowed identity passes and one disallowed identity is refused.

Exit: the layer is merged; it is not yet mounted. Effort: 3 days.

### Task 5 — Join tokens in `migrate`, compose and BYOC

- `migrate` runs `loams ca init` on a new stack. It writes one token per compose service to `secrets/transport/<service>.token` (mode 0600) and writes `secrets/transport/bundle.pem`.
- `deploy/release/compose*.yaml` mounts each service's token and the bundle.
- `byoc/install.sh` gets the tokens from the cloud secret store.
- An upgrade from 0.1.x stamps `transport.compat_plain_until` (D2327).

Tests:
- `migrate_issues_one_token_per_service`;
- `tokens_are_single_use`;
- the compose `config -q` checks;
- `tests/byoc/static` goldens.

Exit: merged. A fresh compose stack has a CA and tokens, and nothing uses them yet. Effort: 3 days.

## T1 — The cluster listener on HTTP/2 + mTLS (Task 6, closes #1560)

### Task 6 — mTLS and the route policy on the cluster listener

- The cluster node's listener (`start_cluster`) serves TLS 1.3 with client certificates (HTTP/2 and, during the window, HTTP/1.1) through `loams-transport`, with `RoutePolicy` mounted.
- The metastore client (`loams-meta` `HttpTransport`), forwarded reads, and the meter's lease and ingest clients present the process's identity.
- `Caller::Service("loams-cluster")` is removed: the `LeaseService` and ingest handlers take the identity from `RoutePolicy`.
- `validate_auth`'s "private network until MT1" error becomes D2303's rule.
- Amend the D2203 and D2206 rows. The PR closes #1560.

Tests:
- `cluster_listener_refuses_without_client_cert`;
- `cluster_listener_refuses_foreign_ca`;
- `lease_from_meter_agent_is_permission_denied`;
- `ingest_from_meter_agent_appends`;
- `plain_peer_accepted_only_inside_compat_window`;
- the three-node cluster tests (`crates/loams/tests/cluster*`), all on mTLS;
- the upgrade gate from v0.1.3.

Exit: merged; the cluster tests and the upgrade gate are green. Effort: 5 days.

## T2–T5 — HTTP/3 and the remaining hops (Tasks 7–14)

### Task 7 — T2: HTTP/3 with fallback on the cluster listener and metastore Raft

- `loams-transport`'s `Endpoint` binds UDP and TCP on the cluster port.
- The `Client` gets the race, the 15-minute memory and `mode = auto|h3|h2` (D2317).
- `loams doctor transport`.
- Metastore Raft and H2–H5 move onto the pool, with class C connections and class B connections kept separate (D2319).
- 0-RTT is off (D2318).

Tests:
- `h3_and_h2_serve_the_same_router`;
- `udp_blocked_falls_back_within_500ms`;
- `udp_memory_expires_after_15m`;
- `zero_rtt_refused_on_non_idempotent_route`;
- `bulk_and_small_rpc_use_separate_connections`;
- the cluster tests in `h3` mode and in `h2` mode.

Effort: 5 days.

### Task 8 — T3: gateway and engine hops

Move H7 (Loams-run sidecars), H9 (engine to House, keeping the House user), H11 (Resonate as a service), H12 (Temporal gateway to Resonate), H17 (OpenFGA, keeping its key) and H20 (Lakekeeper TLS) onto `loams-transport` or CA certificates, per Task 1's ruling. Third-party hops without TLS are listed as exceptions (Q1478). Effort: 5 days.

### Task 9 — TiKV and PD over gRPC + mTLS

- Leaves for `svc/tikv` and `svc/pd` from the CA service, plus client leaves for the engine and TiDB.
- TiKV and PD `[security]` settings in the `tikv` profile and the playground scripts.
- The TLS options of `client-rust` and `client-go`.

Tests: the `tikv` profile's smoke over mTLS; `loams-tikv`'s integration tests, run once with `scripts/tikv/playground.sh --tls`. Effort: 3 days.

### Task 10 — T4: the bulk data-plane paths

H2 split files and H3 Git fetches move onto class B connections with large windows (§83 §10.2), using the transport that Task 11 rules for class B. The Neon hops (H15) get CA certificates through PG2 Tasks 23 and 38; coordinate with those lanes. Effort: 3 days.

### Task 11 — The benchmark and the per-path rulings (D2321, D2322)

`bench/transport/` in the BQ1 style: quiet window, pinned cores, and `native-load.sh`-style JSON lines `{class, transport, cpu_s_per_gib, p50_ms, p99_ms, p999_ms, gbps, rps, errors}`.

| Class | Workload |
|---|---|
| A (WAN) | Under `tc netem`: 40 and 120 ms RTT, 0.1 % and 1 % loss. Then two cloud regions on the CB1 accounts, within their caps |
| B (intra-DC bulk) | Streams of 1, 16 and 256 MiB, at 1–64 concurrent: loopback, the second laptop, then a cloud pair |
| C (many small RPCs) | Unary 100–1,000 B, 1–1,024 in flight, open and closed loop |

Each class compares HTTP/3+mTLS with HTTP/2+TLS 1.3+mTLS on the same Connect handlers.

Record one ruling per class in the decision log (from the CT1 range above), applying D2322's bar:
- HTTP/3 must be within 10 % on CPU per byte and on p99;
- HTTP/3 must be no worse on throughput.

Set the per-class defaults in `loams-transport`. Report: `docs/benchmarks/2026-1x-ct1-transport.md`. Effort: 5 days, plus cloud time.

### Task 12 — T5: agentd, connect over TCP, and the node agent

- H6: the node agent gets `svc/meter-agent`.
- H13: agentd running inside the deployment gets `svc/agentd`.
- H14: connect over TCP gets `svc/connect`.

Unix-socket paths are unchanged, and user-run agentd stays a client (Q1479). Effort: 3 days.

### Task 13 — Kubernetes and BYOC

- The Helm charts and operator (§65): UDP and TCP Service ports, NetworkPolicies for UDP, and token Secrets per pod or a cert-manager `Certificate` (D2314).
- BYOC cloud-init: tokens from the secret store.
- `loams doctor transport` in the BYOC checks.

Effort: 3 days.

### Task 14 — Docs, the upgrade gate and exit

- `docs/guides/operations/cluster-transport.md`: the CA, tokens, rotation, revocation, `mode`, the doctor command and the exceptions.
- The release README's known gaps.
- The CHANGELOG.
- The upgrade gate covers 0.1.x → T1 → T2 inside the compat window.
- Close the tracking issue.

Effort: 2 days.

## Starting order (2026-10-29)

1. Task 0, then Tasks 1 and 2 in parallel.
2. Then Tasks 3 → 4 → 5 → 6. **T1 ships #1560's fix first.**
3. After T1, Task 7 next. Task 11's local runs can start once Task 7 has merged.
4. Tasks 8, 9, 10, 12 and 13 follow Task 11's class rulings where a class matters (8, 10), or run in parallel where it does not (9, 12, 13).

## Rulings made during execution

| Task | File | Summary |
|---|---|---|
| 0 | [ct1-rulings/t0.md](ct1-rulings/t0.md) | Pins (quinn 0.11.12 with aws-lc-rs, h3 0.0.8, h3-quinn 0.0.10); h3 has had no release since 2025-05, so watch for #360 and #365; connectrpc serves through `tower::Service`; TiKV and PD authorize by CN |
| 1 | [ct1-rulings/t1.md](ct1-rulings/t1.md) | Inventory audit: every listener and client is classed; new hops H22–H27 (sqlgate to TiDB, the pgdog-sync sidecar, house-cache, the meter host socket, Live workers, Cloud admin) |
| 2 | [ct1-rulings/t2.md](ct1-rulings/t2.md) | Spike: one `tower::Service` over h3 and h2 with mTLS works; foreign CA refused on both; h3 #360 not hit; loopback smoke shows h3 near h2 for small RPCs and about half for bulk before tuning (Task 11 decides) |
| 3 | [ct1-rulings/t3.md](ct1-rulings/t3.md) | Part 1: `loams-transport` with `SpiffeId`, TLS 1.3 mTLS configs on an explicit aws-lc-rs provider, and `peer_identity`; R3.1: `crates/loams`' existing TLS stays on ring until Task 6 |
