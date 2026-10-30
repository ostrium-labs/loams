# 83 — Cluster Transport: HTTP/3 over QUIC with mTLS

Status: **Approved.** The owner approved the coordinator's recommendation on 2026-10-29, as relayed by the coordinator: Connect RPC, mTLS on every hop, HTTP/3 by default with an HTTP/2+TLS fallback, and per-path defaults set by benchmark.
- Decisions **D2300–D2331** are in the [decision log](13-decision-log.md) with status **Approved**.
- Questions **Q1470–Q1485** are answered with their defaults (§17).
- D2332–D2349 and Q1486–Q1489 are kept free for the plan's rulings.

Plan: [CT1](../plans/2026-10-29-ct1-cluster-transport.md). **This document writes no transport code.**

Source: the owner, 2026-10-29: "plan all internal cluster use http/3 quic with mTLS etc for communication". The same day the owner asked about gRPC versus HTTP/3. The coordinator answered that the RPC layer and the transport are separate choices, and the answer is §3.

Markers, as in §75, §81 and §82:
- **(verify)**: not yet checked against a primary source. The task that depends on it checks it first.
- **(estimate)**: judged, not measured.
- **(measure)**: decided by CT1's benchmark (§10), not here.

Pins follow D1274: a release must be at least 14 days old when the PR opens.

---

## 1. Summary

Every call between Loams processes inside a deployment moves onto **one transport layer** with these properties:

- **mTLS on every hop**, with no exceptions inside the cluster. Each process proves who it is with a short-lived certificate from the cluster's own CA. The certificate carries a SPIFFE-style identity: `spiffe://<cluster>/node/<id>` for a node, `spiffe://<cluster>/role/<role>` for a product role, and `spiffe://<cluster>/svc/<service>` for a separate service.
- **Authorization per route on the peer's identity.** Every internal route lists the identities that may call it. Everything else is refused.
- **HTTP/3 over QUIC is the default transport**, with an automatic fallback to **HTTP/2 over TLS 1.3** where UDP is blocked. Both run on the same port number, UDP and TCP.
- **The RPC layer does not change.** It stays Protobuf over Connect (`connectrpc` in Rust), plus the plain HTTP routes Loams already has. Connect is transport-agnostic and also speaks gRPC, so moving a call from HTTP/2 to HTTP/3 changes no `.proto` file and no handler.
- **gRPC over HTTP/2 + mTLS** is kept only where an external system requires it: TiKV and PD (and TiDB's status port). Those hops still use the cluster CA.
- **Per-path defaults are rulings decided by measurement.** For each path class (cross-region WAN, intra-DC bulk data plane, many small RPCs), CT1 benchmarks HTTP/3 against HTTP/2+TLS on CPU per byte, p99 and throughput. The default for that class is whichever wins by the bar in §10. Until a class is measured, HTTP/3 is attempted with fallback.
- **The stack**: `quinn` plus `h3`/`h3-quinn` on `rustls`. It is Rust only, licensed Apache-2.0 or MIT, and `quinn` is already in the lockfile through Iggy.
- **The first slice closes issue #1560.** The cluster listener stops trusting the network: the meter's `LeaseService` and service ingest get per-role identities instead of the blanket `loams-cluster` identity (§12).

### 1.1 Decisions

| # | Decision (short) | § | Status |
|---|---|---|---|
| D2300 | **Scope**: every hop between Loams processes inside one deployment (node to node, role to role, gateway to engine, engine to engine, agentd to engine, services to the cluster listener). Client-facing protocols (Postgres, MySQL and RESP wire, Kafka wire, public HTTPS) and hops to systems Loams does not run are out of scope except where §9 names them | 2 | Approved |
| D2301 | **The RPC layer and the transport are separate choices.** Protobuf over Connect (`connectrpc`) and Loams' plain HTTP routes stay the RPC layer on every internal hop. The transport under them is HTTP/3 or HTTP/2+TLS. No hop moves to raw gRPC, Cap'n Proto or a custom framing | 3 | Approved |
| D2302 | **gRPC over HTTP/2 + mTLS only where an external system requires it**: TiKV, PD and TiDB's status port (and their client libraries, `client-rust` and `client-go`). Loams' own services never require a gRPC-only client. Connect handlers keep answering gRPC and gRPC-Web for tools | 3, 9 | Approved |
| D2303 | **mTLS is mandatory on every internal hop**, cluster mode or not. A cluster node refuses to start with a plain internal listener, except `--insecure-dev` on loopback (dev and tests). This replaces the "private network until MT1" rule (server.rs `validate_auth`) | 6, 11 | Approved |
| D2304 | **HTTP/3 over QUIC (RFC 9114, RFC 9000) is the default transport.** HTTP/2 over TLS 1.3 is the fallback on the same port number (TCP), chosen automatically when UDP is blocked (§8). A per-path ruling (D2322) can make HTTP/2 the default for a class that measures better on it | 8, 10 | Approved |
| D2305 | **Stack: `quinn` (QUIC) + `h3` + `h3-quinn` (HTTP/3) on `rustls` 0.23**, with `hyper` 1 for HTTP/2. `s2n-quic` is not used (§5.2). Everything is Rust, Apache-2.0 or MIT. No C QUIC library (quiche, msquic, lsquic, ngtcp2) | 5 | Approved |
| D2306 | **Crypto provider: `aws-lc-rs`**, which `rustls` already uses in the workspace, for TLS 1.3 and QUIC. Only TLS 1.3 cipher suites. Certificates use ECDSA P-256 keys | 5, 6 | Approved |
| D2307 | **One crate, `loams-transport`**: the endpoint (client and server), the fallback, the connection pool, the identity extractor, the route-authorization layer and the metrics. Every Loams binary uses it for internal hops; nobody else opens internal sockets | 5.3 | Approved |
| D2308 | **Identities are SPIFFE IDs in a URI SAN**: `spiffe://<cluster-id>/node/<node-id>`, `spiffe://<cluster-id>/role/<role>`, `spiffe://<cluster-id>/svc/<service>`. A node certificate may carry its node ID plus the roles it runs. The SPIFFE Workload API and SVID fetch are not implemented. The ID format is SPIFFE's, so SPIRE can issue them later (D2314) | 6.1 | Approved |
| D2309 | **An internal CA per deployment**: a root (P-256, 10 years) kept offline in the deployment's secret store, and an intermediate (1 year) that the CA service signs with. Leaf certificates last **24 hours**, are renewed at two-thirds of their life, and have no CRL or OCSP | 6.2 | Approved |
| D2310 | **The CA service runs in the `meta` role**: `loams.ca.internal.v1.CertificateService` (`Issue`, `Renew`, `TrustBundle`), backed by the metastore for issued serials, join tokens and the deny list | 6.3 | Approved |
| D2311 | **Join flow**: an operator-issued **join token** that works once, expires in 1 hour and is bound to a node ID and role set. The joining node generates its key locally, sends a CSR with the token over TLS to a pinned trust-bundle hash, and receives a 24-hour leaf. Renewal is authenticated by the node's current certificate. Compose stacks get their tokens from `migrate` (one per service); Kubernetes gets them from the operator (§65) | 6.4 | Approved |
| D2312 | **Revocation**: short lifetimes plus a **deny list** of serials and identities in the metastore. Every node watches the list and checks it at handshake and on every new stream. Revoking a node also stops its renewals. There is no CRL or OCSP | 6.5 | Approved |
| D2313 | **Rotation**: leaves renew automatically. The intermediate rotates yearly, with both intermediates in the bundle for 30 days. The root rotates by publishing a cross-signed bundle. `loams ca rotate` drives the intermediate and root rotations | 6.6 | Approved |
| D2314 | **An external CA is optional**: cert-manager (Kubernetes) or SPIRE can issue the same SPIFFE IDs instead of the CA service, configured with `[transport.identity] source = "files"` plus a watched directory. The route policy is the same either way | 6.7 | Approved |
| D2315 | **Authorization per route on the peer identity**: each internal route (Connect method or HTTP path) declares the identities it accepts in one table, `transport/routes.toml`, checked into the repository and compiled in. Anything unlisted is refused (deny by default), and the caller becomes `Caller::Service(<role or svc>)` or `Caller::Node(<id>)` | 7 | Approved |
| D2316 | **Peer identity is not user identity.** A user's or principal's credentials, when a hop carries them (forwarded reads, House queries), still ride in the request and are still checked by `loams-authz`. mTLS answers "which Loams process is calling", and authz answers "on whose behalf" | 7.3 | Approved |
| D2317 | **Fallback**: a client races QUIC against TCP+TLS to a peer it has not reached yet (QUIC starts first, and TCP starts after 250 ms). It remembers per peer that UDP fails for 15 minutes, and re-probes after that. `[transport] mode = "auto" \| "h3" \| "h2"` per deployment, with per-path overrides | 8 | Approved |
| D2318 | **0-RTT is off for every non-idempotent call.** 0-RTT is accepted only on routes `routes.toml` marks `idempotent = true`, and only from an identity already seen on that connection's resumption ticket. It is off for everything until CT1 measures it (Q1474) | 9.1 | Approved |
| D2319 | **Connection pooling**: one QUIC connection per (local process, peer, path class), with many streams. HTTP/2 uses one connection per (peer, path class), plus a second one when stream limits are reached. Bulk data-plane transfers get their own connection, so they never block small RPCs on the same peer (head-of-line isolation) | 9.2 | Approved |
| D2320 | **Pipelining and batching stay on**: many concurrent streams per connection, batched requests where the RPC has a batch form (as BQ1 did: RESP, relay, meter `check_many`), and no per-call connection | 9.3 | Approved |
| D2321 | **Three path classes, benchmarked separately**: (A) cross-region/WAN, (B) intra-DC bulk data plane, (C) many small RPCs | 10 | Approved |
| D2322 | **The per-path default is decided by measurement.** For each class, HTTP/3 becomes the default only if it is no worse than HTTP/2+TLS by more than 10 % on CPU per byte and on p99, and not worse on throughput. Otherwise HTTP/2+TLS is that class's default. The result is recorded as a CT1 ruling and rechecked each minor release | 10 | Approved |
| D2323 | **QUIC's costs are accepted with mitigations**: user-space CPU (UDP GSO/GRO, larger socket buffers, batching), UDP blocked by networks or CNIs (fallback, plus a preflight check), immature L4 load balancers and tooling (no load balancer on internal hops; qlog and metrics), and no 0-RTT for writes | 11 | Approved |
| D2324 | **No load balancer between internal peers.** Peers dial each other's advertised addresses from metastore membership. Kubernetes uses a headless Service. QUIC connection migration is on, and no connection-ID-aware load balancer is needed | 8.3 | Approved |
| D2325 | **Exceptions keep TLS with the same CA**: TiKV and PD gRPC (their `security.ca-path`, `cert-path` and `key-path` use certificates from the CA service). Neon's protocols (compute to pageserver and safekeeper, libpq and HTTP) get TLS and mTLS through PG2 Task 23 and Task 38 using these certificates. PgDog and Lakekeeper keep TLS with the CA's certificates | 9 | Approved |
| D2326 | **Migration order** (§13): T0 identity and the CA; T1 mTLS on HTTP/2 for the cluster listener (closes #1560); T2 HTTP/3 with fallback on the cluster listener and metastore Raft; T3 gateway to engines and engine to engine; T4 the data-plane bulk paths; T5 agentd and the services. Each step stays compatible with the previous release | 13 | Approved |
| D2327 | **Mixed versions**: during an upgrade a node accepts both HTTP/2+mTLS and HTTP/3+mTLS. Plain HTTP is accepted only while `transport.compat_plain_until` (an upgrade window of at most 24 hours, stamped by `migrate`) has not passed, and only from addresses already in membership | 13.2 | Approved |
| D2328 | **Issue #1560**: T1 replaces `Caller::Service("loams-cluster")` with the caller's role identity on `LeaseService` and on `POST /internal/v1/meter/partitions/{p}/records`. Only product roles may lease, only `meter-agent` and product roles may ingest, and the billing exporter has its own identity. This amends D2203 and D2206's trust model | 12 | Approved |
| D2329 | **Observability**: per-hop metrics (`transport_connections`, `streams`, `handshakes{result}`, `fallbacks{reason}`, `0rtt{accepted}`, `bytes`, `cpu_seconds` by class), qlog on demand (`LOAMS_TRANSPORT_QLOG_DIR`), and the peer identity in every internal-request span. `SSLKEYLOGFILE` works only with `--insecure-dev` | 14 | Approved |
| D2330 | **Ports**: each internal listener binds the same port number on UDP (h3) and TCP (h2). Compose publishes nothing new, because internal hops stay on the compose networks. Kubernetes Services list both protocols. The tailnet path is unchanged (Tailscale carries UDP) | 8.4 | Approved |
| D2331 | **Track CT, plan CT1**; status file `loams-wt/.he0-shared/ct1-status.md` | 16 | Approved |

---

## 2. Scope

**In scope** (D2300) is every hop between processes Loams runs, inside one deployment:
- node to node: the metastore Raft, forwarded reads, and the hot, graph and git internal routes;
- role to role: the meter's leases and ingest, and counter owners;
- the gateway to the engines it fronts;
- engine to engine (the House, Durable/Resonate, the Temporal gateway, the managed engines' controllers);
- agentd and `loams-fabric connect` to the engine;
- the node agent to the cluster.

**Out of scope:**
- **Client-facing protocols.** Postgres, MySQL, RESP, Kafka and CQL wire stay what clients speak. The gateway's front door is §57's.
- **Public HTTPS and the edge** (§69).
- **The tailnet itself** (§43). mTLS runs inside it, not instead of it.
- **Hops to systems Loams does not run**, such as Confluent Cloud, Zitadel at `auth.loams.dev` and object storage. They keep their own TLS.
- **The Loams Cloud control plane to customer deployments** (§41, §78), which is outbound only with its own credentials.

## 3. The RPC layer is not the transport

The owner asked whether the cluster should use gRPC or HTTP/3. These are different layers (D2301, D2302).

| Layer | Choice | Why |
|---|---|---|
| **Schema** | Protobuf (`proto/loams/**`), unchanged | Already shared by every SDK and service. `buf breaking` guards it |
| **RPC protocol** | **Connect** (`connectrpc` 0.9 in Rust, `@connectrpc` in TypeScript), plus the existing plain HTTP routes (`/internal/v1/*`, the metastore's Raft RPC) | Connect works over HTTP/1.1, HTTP/2 and HTTP/3 alike, and the same handler also answers gRPC and gRPC-Web. Choosing Connect does not choose a transport |
| **Transport** | **HTTP/3 over QUIC** by default; **HTTP/2 over TLS 1.3** as fallback and where measured better | §8, §10 |
| **Security** | **mTLS on every hop**, with the cluster CA's SPIFFE identities | §6 |
| **gRPC (HTTP/2) specifically** | Only to TiKV, PD and TiDB's status port, which require it | Their clients and servers speak only gRPC over HTTP/2. Loams does not fork them for this |

Consequences:
- **No handler changes.** A Connect service mounted on the HTTP/2 router is mounted on the HTTP/3 router unchanged. `loams-transport` hands each request to the same `axum`/`tower` service whichever transport it arrived on (verify: `connectrpc` serves through `tower::Service`; CT1 Task 2 checks this).
- **No new RPC framework.** gRPC's own HTTP/3 mapping is not standardised and is not needed: Connect's unary and streaming protocols are plain HTTP semantics. Raw gRPC is never required between Loams processes.
- **External tools still work.** `grpcurl` and `buf curl` reach any Connect service over HTTP/2 with a client certificate. `buf curl --http3` is used where it is supported (verify).

## 4. Inventory: every internal hop today

"Today" means dev at the v0.1.3 tag, 2026-10-29. The **Auth today** column describes the hop's own authentication, not the network around it. "Network" means the hop trusts that it is reachable only on a private network or a compose network.

| # | Hop | Where | Protocol today | Auth today | Class | Target |
|---|---|---|---|---|---|---|
| H1 | Metastore Raft and the metastore client (`loams-meta` `HttpTransport`) | `crates/loams/src/cluster.rs`, `loams-meta` | HTTP/1.1 JSON on the cluster listener | Network | C | h3+mTLS, `role/meta` only |
| H2 | Forwarded reads (`/internal/v1/reads/{op}`), hot status and warm, node stats | `crates/loams/src/api/internal.rs` | HTTP on the cluster listener | Network | C (reads), B (split files) | h3+mTLS, `node/*` |
| H3 | Graph and Git internal routes | `api/graph.rs` `internal_routes`, `api/git_internal.rs` | HTTP on the cluster listener | Network | B and C | h3+mTLS, by role |
| H4 | Meter `LeaseService` (`loams.meter.internal.v1`) | `api/meter.rs` (D2203) | Connect on the cluster listener | Network; the caller is the blanket `loams-cluster` | C | **T1**: mTLS, product roles only (#1560) |
| H5 | Meter service ingest `POST /internal/v1/meter/partitions/{p}/records` | `api/meter.rs` (D2206) | HTTP on the cluster listener | Network | C (batched) | **T1**: mTLS, `svc/meter-agent` and product roles (#1560) |
| H6 | The node agent (`loams-meter-agent`) to the cluster | `crates/loams-meter-agent` | HTTP (`StreamSink`) | Network or API key (verify) | C, batched | h3+mTLS, `svc/meter-agent` |
| H7 | `loams-gateway` to the engines: RisingWave (PG wire), Spark Connect (gRPC), Flink SQL Gateway and REST, the engines facade, and the browser service (CDP over WebSocket) | `crates/loams-gateway`, design 81, D2118 | Each engine's own protocol, TLS where the engine has it | Engine keys, bearer tokens and `ENGINE_GATEWAY_KEY` | B and C | The gateway's hop to Loams-run sidecars uses mTLS. Third-party engines keep their protocol, wrapped in TLS with the CA's certificates where they support it (§9) |
| H8 | `loams-gateway` to PgDog and computes (Postgres) | §57 §7 | Postgres wire over TLS (spliced or re-originated) | SCRAM, TLS | — | Unchanged: client-facing protocol. The gateway's certificate comes from the CA (D2325) |
| H9 | Engine to House (`loams-fabric` front, workers) | HS1 Task 20 | ClickHouse HTTP | House user and password | B and C | h3+mTLS, `role/house`, with the House user still checked (D2316) |
| H10 | House front to its chDB workers | `loams-house-ipc` | A Unix socket, inside one host or pod | Socket permissions | — | Unchanged: not a network hop |
| H11 | Engine to its embedded Resonate (Durable) | `crates/loams-durable` | In process or loopback HTTP | — | — | Unchanged on loopback; h3+mTLS when Resonate runs as its own service |
| H12 | Temporal gateway (`loams-temporal`) to the engine's Resonate and to Postgres | design 74, `compose.compute.yaml` | HTTP to Resonate (in the engine's network namespace); Postgres wire | API key; Postgres password | C | h3+mTLS, `svc/temporal-gateway`, to Resonate. Postgres TLS uses the CA's certificates |
| H13 | agentd to the engine (Connect RPC, previews, MCP) | `loams-agentd-*`, `loams-agentd-link` | Connect over HTTP/1.1 and HTTP/2, WebSocket | API keys, OIDC or tailnet identity | C | h3+mTLS with `svc/agentd` when agentd runs inside the deployment. A user's agentd on a laptop stays a client (§2) |
| H14 | `loams-fabric connect` to the engine | the connect socket (CC1 R1.1) | Connect over a Unix socket, or TCP | API key | C | Unix socket unchanged; TCP gets h3+mTLS, `svc/connect` |
| H15 | pg-control to pg-runner; computes to pageserver and safekeeper (`loams-wal`) | PG2 Tasks 23 and 38 | HTTP and libpq (Neon) | None or JWT (verify) | B | TLS and mTLS with the CA's certificates (D2325); the protocols stay Neon's |
| H16 | Engine to TiKV and PD; TiDB to TiKV and PD | `loams-tikv`, `client-rust`, TiDB | gRPC over HTTP/2 | None by default (verify) | B and C | **gRPC over HTTP/2 + mTLS** with the CA's certificates (D2302, D2325) |
| H17 | Engine to OpenFGA (`authz` profile); identity sync to Zitadel | §76 | HTTP and gRPC | Preshared key | C | mTLS to OpenFGA with `svc/openfga`, keeping its preshared key as well. Zitadel is external when hosted |
| H18 | The managed engines' controllers to their engines (RisingWave meta, the Flink session cluster, Spark drivers) | design 81 | Each engine's protocol | Varies | B and C | Third-party protocols, wrapped in TLS where supported (§9) |
| H19 | Iggy (the `kafka` profile) between its gateway and the server | the Iggy fork | Iggy TCP (or QUIC; Iggy has a QUIC transport) | Iggy user | B | Iggy's QUIC transport with the CA's certificates (verify) |
| H20 | Lakekeeper (Iceberg REST) from engines and the managed engines | §81 | HTTP | Auth off in compose (R9) | C | TLS with the CA's certificates, keeping Lakekeeper's own auth |
| H21 | Desktop and console to the engine | §19, §44 | HTTPS (tailnet), WebSocket | Tailnet, OIDC, API keys | — | Unchanged: these are clients (§2) |

The inventory is the checklist for CT1 Task 1. It must find any hop missing from this table (`grep` for listeners and clients across `crates/`, `fabric/` and `deploy/`) before T1 starts.

## 5. The stack

### 5.1 Chosen: quinn + h3 + rustls (D2305, D2306)

| Piece | Crate | Licence | Why |
|---|---|---|---|
| QUIC | `quinn` 0.11 (already in Cargo.lock through `iggy`) | MIT OR Apache-2.0 | Pure Rust and mature. It uses `rustls` for QUIC's TLS, supports GSO, GRO and ECN, connection migration and 0-RTT, and runs on tokio |
| HTTP/3 | `h3` + `h3-quinn` (hyperium) | MIT | The hyper project's HTTP/3. Used by `reqwest`'s `http3` feature (verify: version maturity and API stability, CT1 Task 2) |
| TLS | `rustls` 0.23 with `aws-lc-rs` | Apache-2.0, ISC, MIT | Already the workspace's TLS. One configuration builder serves TCP and QUIC |
| HTTP/2 | `hyper` 1 + `tokio-rustls` | MIT | Already used by `crates/loams/src/tls.rs` |
| Certificates | `rcgen` (CA and CSR), `x509-parser` | MIT OR Apache-2.0 | Both already in Cargo.lock |
| RPC | `connectrpc`, `axum`, `tower` | Apache-2.0, MIT | Unchanged |

### 5.2 Not chosen

- **`s2n-quic`** (Apache-2.0, AWS). It is strong on performance and formally checked. It was not chosen for three reasons. First, its HTTP/3 support (`s2n-quic-h3`) is less used than `h3-quinn` (verify). Second, it carries its own TLS provider choice (s2n-tls by default, or rustls), which would mean two TLS stacks in one binary. Third, `quinn` is already in the dependency tree. Q1471 records a re-measurement: if `quinn` misses D2322's bar on class B and `s2n-quic` meets it, CT1 may switch, behind `loams-transport`'s interface.
- **C libraries** (quiche, msquic, lsquic, ngtcp2): they need a C toolchain and their own memory-safety review. They are not needed.
- **gRPC everywhere** (tonic): it is HTTP/2 only and would force every hop onto one RPC style. See §3.

### 5.3 `loams-transport` (D2307)

The crate has these parts:
- `Endpoint`: binds UDP and TCP on one port, serves HTTP/3 and HTTP/2 into one `tower::Service`, and extracts the peer identity from the verified client certificate into request extensions.
- `Client`: a pool keyed by (peer, class), with the race and fallback of §8, the 0-RTT policy and per-route timeouts.
- `Identity`: loads, watches and renews this process's certificate. It talks to the CA service, or watches files for an external CA.
- `RoutePolicy`: the compiled `routes.toml` (§7), plus a `tower` layer that refuses unlisted calls with Connect `permission_denied` or HTTP 403.
- `metrics`: the D2329 metrics.

Every internal client and listener in `loams`, `loams-gateway`, `loams-fabric`, `loams-temporal`, `loams-meter-agent` and agentd uses it. Code review rejects a new internal `TcpListener`.

## 6. Identity and certificates

### 6.1 SPIFFE-style IDs (D2308)

```
spiffe://<cluster-id>/node/<node-id>          a loams cluster node (with roles in the cert, see below)
spiffe://<cluster-id>/role/<role>             meta | gateway | writer | reader | house | meter | ...
spiffe://<cluster-id>/svc/<service>           meter-agent | temporal-gateway | connect | agentd | openfga | tikv | pd | ...
```

- `<cluster-id>` is the deployment's UUID, the one `migrate` stamps.
- A node certificate carries one `node/` URI SAN plus one `role/` URI SAN for each role it runs. Changing a node's roles means renewing its certificate.
- The DNS and IP SANs are the node's advertised addresses, so ordinary TLS hostname checks also pass.
- The URI SANs, not the DNS names, are what authorization reads.

### 6.2 The CA (D2309)

- **Root**: an ECDSA P-256 key valid for 10 years. It is created by `loams ca init` (run by `migrate` on a new stack) and kept in the deployment's secret store: a compose secret file, a Kubernetes Secret, or the cloud secret store for BYOC (§70). The root signs only intermediates.
- **Intermediate**: valid for 1 year, held by the CA service and stored encrypted in the metastore. It is sealed with age, the way D2205 seals the meter's secrets (verify: key management with §63's `LOAMS_METER_SECRETS_KEY_FILE` pattern).
- **Leaves**: valid for 24 hours and renewed at two-thirds of their life. They carry `serverAuth` and `clientAuth`, and their name constraints are limited to the cluster's DNS suffix.
- **Trust bundle**: the root plus the active intermediates, served by `TrustBundle` and pinned by hash in join tokens.

### 6.3 The CA service (D2310)

`loams.ca.internal.v1.CertificateService` runs on `role/meta` nodes, next to the metastore.

| Method | Called by | Checks |
|---|---|---|
| `Issue(join_token, csr)` | a joining process, before it has a certificate, over server-authenticated TLS | the token is valid, unused and unexpired; the CSR's requested identities fit the token's grant |
| `Renew(csr)` | an existing process, over mTLS | the presented identity matches the CSR, and the identity is not on the deny list |
| `TrustBundle()` | anyone, on any listener | none; the bundle is public |

Issued serials, used tokens and the deny list live in the metastore. The service is the only writer.

### 6.4 Bootstrap and join (D2311)

1. **New stack.** `migrate` runs `loams ca init`, which creates the root and the intermediate. It then issues one join token per compose service from the compose file's service list (each bound to that service's identities) and writes the tokens as files under `secrets/transport/`, mode 0600. It also writes the trust bundle.
2. **Each process at start** reads its token, generates a P-256 key in memory (never on disk) and sends `Issue`. It then serves with the leaf it gets back. If the token is spent, the process uses the certificate it already holds; if that has expired, the operator issues a new token.
3. **A Kubernetes node or pod.** The operator (§65) issues tokens per pod. Alternatively, cert-manager or SPIRE issues the certificates directly (D2314).
4. **A BYOC VM.** `install.sh` gets its tokens from the cloud's secret store, as it already does for the tailnet key and the setup token (§70).
5. **Adding a cluster node.** `loams cluster join` gains `--join-token`. The metastore membership change and the certificate grant are the same operator act.

### 6.5 Revocation (D2312)

- `loams ca revoke --node <id> | --serial <s> | --identity <spiffe-id>` adds an entry to the deny list.
- Nodes watch the list through the metastore and drop matching connections. They check each new handshake and each new stream against it.
- A revoked identity cannot renew, so it dies within 24 hours even on a node that missed the update. That is the bound when the metastore is unreachable.
- There is no CRL and no OCSP: the leaves are short-lived and the deny list reaches every node in seconds.

### 6.6 Rotation (D2313)

- **Leaves**: automatic.
- **Intermediate**: `loams ca rotate intermediate` creates the new one, publishes a bundle holding old and new for 30 days, and then retires the old one.
- **Root**: `loams ca rotate root` cross-signs the new root with the old one, publishes both, and retires the old root after every leaf has renewed under the new chain.
- An expiry alarm fires 30 days ahead (D2329's metrics).

### 6.7 External CAs (D2314)

`[transport.identity] source = "files"` makes `Identity` watch a directory holding `tls.crt`, `tls.key` and `ca.crt`. That is cert-manager's Secret layout and also what SPIRE's helper writes. The issuer must put the same SPIFFE URIs in the certificates. The route policy does not change.

## 7. Authorization per route (D2315, D2316)

### 7.1 One table

`crates/loams-transport/routes.toml` lists every internal route, and the build compiles it into `RoutePolicy`:

```toml
[[route]]
match = "loams.meter.internal.v1.LeaseService/*"
allow = ["role/gateway", "role/writer", "role/house"]   # product roles that lease (D2328)

[[route]]
match = "POST /internal/v1/meter/partitions/*/records"
allow = ["svc/meter-agent", "role/gateway", "role/writer", "role/house", "svc/temporal-gateway"]
idempotent = false

[[route]]
match = "loams.ca.internal.v1.CertificateService/Renew"
allow = ["node/*", "role/*", "svc/*"]       # any authenticated identity; the CA then checks the CSR

[[route]]
match = "POST /meta/raft/*"
allow = ["role/meta"]
```

(The patterns are illustrative. CT1 Task 4 writes the real table from the §4 inventory.)

### 7.2 Behaviour

- **Deny by default.** A route that is served but not listed answers `permission_denied`, and CI fails on an unlisted route. A test walks every router and checks each route against the table.
- **The caller** becomes `Caller::Node(<id>)` or `Caller::Service(<role|svc>)`. Handlers that check callers today (the meter's `LeaseService` takes `Caller::Service`) receive the precise identity in place of `loams-cluster`.
- **Audit**: refusals are logged with the peer identity, the route and the reason, and counted.

### 7.3 Peer identity versus principal (D2316)

mTLS identifies the calling process. A request made on behalf of a user still carries that user's principal: a forwarded read, a House query, or an agentd call for a person. `loams-authz` checks the principal as it does today. A node identity never gains a user's permissions, and a user's token never stands in for a node identity.

## 8. Transport selection and fallback

### 8.1 Same port, two protocols (D2304, D2330)

- Each internal listener binds `addr:port` on UDP (QUIC, ALPN `h3`) and on TCP (TLS 1.3, ALPN `h2`, plus `http/1.1` for old internal clients during T1 and T2 only).
- Both use the same certificate.
- Peers' addresses come from metastore membership, and no `Alt-Svc` discovery is needed.

### 8.2 The race and the memory (D2317)

The client's behaviour:
- For a peer with no recent result, open QUIC first; if no handshake completes within 250 ms, open TCP+TLS in parallel and use whichever finishes first.
- After 3 consecutive QUIC failures to a peer while TCP works, mark UDP blocked for that peer for 15 minutes.
- Count each fallback by reason (timeout, ICMP unreachable, handshake failure).

`mode = "h3"` disables the TCP path (for tests). `mode = "h2"` disables QUIC (for networks known to block UDP). `mode = "auto"` is the default.

**Preflight**: `loams doctor transport` probes every peer on both protocols and prints which one works.

### 8.3 No internal load balancer (D2324)

Internal peers dial each other directly. In Kubernetes, a headless Service gives pod IPs. QUIC connection migration is left on, so a pod whose IP changes keeps its connection where the CNI allows it; otherwise it simply reconnects. There is no connection-ID-aware L4 load balancer, so the immaturity of load balancers for QUIC does not affect internal hops. The gateway's public listeners are §57's, not this design's.

### 8.4 Ports and environments (D2330)

| Environment | What changes |
|---|---|
| Compose (one host) | Nothing is published. Internal services talk on the compose networks, and UDP works there |
| Kubernetes | Services list `protocol: UDP` beside TCP. NetworkPolicies allow UDP on the internal ports. CNIs that drop UDP, or have MTU trouble, trigger the fallback, and `loams doctor transport` reports it |
| BYOC VM | One host, the same as compose |
| Tailnet between nodes | Tailscale and WireGuard carry UDP. Tailscale's MTU (1280) is below QUIC's minimum of 1200 plus overhead (verify); quinn's path MTU discovery handles it, and the fallback covers the rest |

## 9. Hops that keep another protocol (D2302, D2325)

| Hop | Protocol kept | What this design adds |
|---|---|---|
| TiKV and PD, from the engine, TiDB and `tikv-ctl` | gRPC over HTTP/2 | mTLS with leaves from the CA service: TiKV and PD's `[security] ca-path`, `cert-path` and `key-path`, plus `cert-allowed-cn` set to the TiKV, PD and client identities (verify: TiKV's checks on URI SANs versus CNs; CT1 Task 9). `client-rust`'s TLS options get the same files |
| Neon (compute to pageserver and safekeeper, pg-control to pg-runner) | libpq and HTTP | TLS and mTLS through PG2 Tasks 23 and 38, using these certificates and identities |
| PgDog | Postgres wire | Its server certificate comes from the CA. Client-facing TLS is unchanged |
| Third-party engines (RisingWave, Flink, Spark, OpenFGA, Lakekeeper) | Their own protocols | TLS with the CA's certificates where each supports it. Where one does not, the hop stays on the compose or pod network and the plan lists it as an exception, with a sidecar as the fallback (Q1478) |
| Iggy | Iggy TCP or QUIC | Iggy's own QUIC transport with the CA's certificates (verify), or TCP+TLS |
| Unix sockets (the House IPC, the connect socket) | — | Unchanged. Socket permissions are the boundary |

## 10. Performance and the per-path ruling (D2318–D2322)

### 10.1 0-RTT (D2318)

- 0-RTT data can be replayed, so it is **off for every non-idempotent route**.
- Routes marked `idempotent = true` (reads, `TrustBundle`, node stats) may accept 0-RTT once CT1 has measured the gain (Q1474). Until then it is off everywhere.
- The server tracks resumption tickets per identity, and a 0-RTT request from an identity other than the ticket's is refused.

### 10.2 Pools, streams and pipelining (D2319, D2320)

- **One connection per (process, peer, class).**
- **Stream limits**: 256 concurrent bidirectional streams for class C, and 32 for class B (estimate; tuned by measurement).
- **Flow-control windows**: class B uses large windows (16 MiB per stream and 64 MiB per connection, estimate), so one transfer fills a 10 Gbit/s link at intra-DC RTT.
- **Head-of-line isolation**: class B transfers never share a connection with class C calls to the same peer.
- **Batching stays in the RPC** where it exists: `check_many`, RESP batches, the relay's concurrent groups, and the meter's binary ingest.
- **UDP tuning**: GSO and GRO on, `SO_RCVBUF`/`SO_SNDBUF` at 8 MiB (estimate), and ECN on.

### 10.3 The benchmark plan (D2321, D2322)

CT1 Task 11 builds `bench/transport/` in the BQ1 style:
- quiet window and pinned cores;
- JSON lines `{class, transport, cpu_s_per_gib, p50_ms, p99_ms, p999_ms, gbps, rps, errors}`.

| Class | Represents | Workload | Network | Measures |
|---|---|---|---|---|
| **A — cross-region / WAN** | BYOC spread across regions, a remote metastore, inter-region replication (future) | Unary 1 KiB, a 1 MiB stream, mixed | `tc netem`: 40 ms and 120 ms RTT, 0.1 % and 1 % loss, jitter. Then two real regions on the CB1 accounts, within CB1's caps | p99, throughput under loss, CPU/byte, handshake time (1-RTT, then resumption) |
| **B — intra-DC bulk data plane** | Split files and forwarded reads, House result streams, Git fetches, pageserver pages | Streams of 1, 16 and 256 MiB; 1–64 concurrent | Loopback, then two hosts (the second laptop, and a cloud pair in CB1) at 10 Gbit/s where available | **CPU per byte (both ends)**, throughput, p99 per transfer |
| **C — many small RPCs** | Metastore Raft, leases, ingest, node stats, OpenFGA checks | Unary 100–1,000 B, 1–1,024 in flight per connection, open and closed loop | Loopback and two hosts | p50, p99 and p999, RPS per core, CPU per request |

Every class runs over HTTP/3+mTLS against HTTP/2+TLS 1.3+mTLS, on the same hardware, the same payloads and the same `connectrpc` handlers. Plain HTTP/2 without TLS is reported as a reference line only and is never a candidate.

**The ruling (D2322).** HTTP/3 becomes a class's default only if all of these hold:
- CPU per byte is no more than 10 % worse than HTTP/2+TLS;
- p99 is no more than 10 % worse;
- throughput is not worse.

Otherwise HTTP/2+TLS is that class's default, and HTTP/3 stays available under `mode = "h3"`.
- Class A is expected to favour HTTP/3: no TCP head-of-line blocking under loss, and faster resumption (estimate).
- Class B may favour HTTP/2, because kernel TCP with TSO is cheaper per byte than user-space QUIC (estimate).
- Class C could go either way (measure).

The result goes in the decision log as a CT1 ruling (from the range reserved for CT1), and the bench reruns at every minor release.

## 11. QUIC's costs (D2323)

| Cost | Mitigation |
|---|---|
| **User-space CPU.** QUIC's encryption, packetisation and ACKs run in the process, not the kernel; kernel TCP has TSO and GRO and is often cheaper per byte | GSO and GRO (quinn supports both), AES-GCM through aws-lc-rs, batching, and large streams. D2322 lets measurement put class B back on HTTP/2 |
| **UDP blocked** by enterprise networks, some cloud security groups, or CNIs (and UDP rate limits on some hosts) | Automatic fallback (§8.2), `loams doctor transport`, and `mode = "h2"` |
| **Load-balancer and tooling maturity**: few L4 load balancers route QUIC by connection ID, and tcpdump or Wireshark need key logs to read QUIC | No internal load balancer (D2324); qlog on demand; peer identity in spans; `SSLKEYLOGFILE` in dev only |
| **0-RTT replay** | Off for non-idempotent routes (D2318) |
| **Amplification and DoS on a UDP port** | QUIC's address validation (Retry) when under load; internal ports never published; rate limits per source address before the handshake |
| **MTU** | quinn's PLPMTUD and an initial 1,200-byte datagram; tailnet MTU is covered by the fallback |
| **Library maturity** (`h3` is 0.0.x) | Pinned versions, CT1's conformance suite (§15), and `loams-transport`'s interface, which leaves room for `s2n-quic` (Q1471) |

## 12. Issue #1560 (D2328)

Issue #1560 notes that the cluster listener trusts its network. Anything that can reach it can lease meter capacity (`LeaseService`, with caller `loams-cluster`) and append to `_system/_meter` (the service ingest). The issue offered two options: mTLS with per-role identities, or a shared cluster credential.

This design takes **option 1 (mTLS)**, and makes it the first slice to ship (**T1**):
- The cluster listener requires client certificates from the cluster CA. On T1 it serves HTTP/2 only; HTTP/3 comes in T2.
- `LeaseService` accepts `role/gateway`, `role/writer` and `role/house`, the product roles that lease. The billing exporter gets `svc/meter-exporter`, which may read but not lease.
- Service ingest accepts `svc/meter-agent` plus the product roles that emit usage.
- `Caller::Service("loams-cluster")` is removed. Handlers see the exact identity.
- The D2203 and D2206 rows are amended to point here, and issue #1560 is closed by T1's PR.

## 13. Migration and compatibility (D2326, D2327)

### 13.1 Phases

| Phase | What ships | Compatible with |
|---|---|---|
| **T0** | `loams-transport` (identity, CA service, `routes.toml`, metrics); `loams ca init\|rotate\|revoke`; `migrate` issues tokens. No hop changes | Every 0.1.x stack |
| **T1** | The cluster listener on HTTP/2 + mTLS + route policy; closes #1560. Plain is accepted during the upgrade window only (D2327) | Mixed versions within the window |
| **T2** | HTTP/3 with fallback on the cluster listener, including metastore Raft (H1–H5) | T1 nodes, over HTTP/2 |
| **T3** | Gateway to engines and engine to engine (H7, H9, H11, H12, H17, H20) | T2 |
| **T4** | Data-plane bulk paths (H2 split files, H3 Git fetches) on the class-B ruling. TiKV and PD mTLS (H16). Neon TLS through PG2 | T3 |
| **T5** | agentd, connect over TCP and the node agent (H6, H13, H14) | T4 |

### 13.2 Upgrades (D2327)

- An upgrade from a plain-HTTP release stamps `transport.compat_plain_until` (now plus 24 hours). Until then a new node accepts plain HTTP from addresses already in membership, and its clients speak plain HTTP to peers that do not offer TLS.
- After the window, plain is refused and the stack is all mTLS. `loams upgrade` checks that every node has renewed at least once before it reports success.
- A single-node stack has no peers: its internal listeners move to mTLS in one restart.

### 13.3 Developer experience

- `loams dev` runs everything on loopback with an ephemeral in-memory CA, so mTLS is on even in dev. `--insecure-dev` (loopback only) turns it off for debugging, and the process then logs a warning every minute.
- Tests use `loams-transport`'s test CA helpers.

## 14. Observability (D2329)

- **Metrics**: labelled by `class`, `transport` (`h3` or `h2`) and `peer_identity`:
  - `transport_connections`, `transport_streams`;
  - `transport_handshakes_total{result}`, `transport_fallbacks_total{reason}`, `transport_0rtt_total{accepted}`;
  - `transport_bytes_total{dir}`, `transport_cpu_seconds_total`;
  - `transport_cert_expiry_seconds`.
- **qlog**: written per connection to `LOAMS_TRANSPORT_QLOG_DIR` when that is set.
- **Traces**: every internal request span carries `peer.identity`, `net.transport` and `tls.resumed`.
- **Debugging**: `loams doctor transport` (§8.2) and `loams ca status` (expiries, the deny list, the bundle).

## 15. Testing

- **Conformance**:
  - every route in every router is in `routes.toml`;
  - every allowed identity reaches its route, and every other identity is refused (generated tests);
  - the same Connect suite passes over `h3` and `h2`.
- **Fault injection**:
  - dropped UDP (fallback within 500 ms);
  - expired leaves;
  - a revoked identity mid-connection;
  - CA unreachable during renewal (a 24-hour bound);
  - clock skew of ±5 minutes.
- **Fuzzing**: the CSR and join-token parsers, and `routes.toml` matching, under cargo-fuzz.
- **Interop**: `curl --http3` and `buf curl` against an internal listener with a client certificate. TiKV and PD started with CA leaves.
- **Upgrade gate**: a plain-HTTP stack upgraded to T1 and T2 inside the compat window.

## 16. Plan

[CT1](../plans/2026-10-29-ct1-cluster-transport.md) (D2331) has these tasks:

| # | Task |
|---|---|
| 0 | Rulings and pins |
| 1 | Inventory audit |
| 2 | Stack spike |
| 3 | `loams-transport` identity and the CA service |
| 4 | `routes.toml` and `RoutePolicy` |
| 5 | Join tokens in `migrate` and compose |
| 6 | **T1**: mTLS on the cluster listener (#1560) |
| 7 | **T2**: HTTP/3 with fallback on the cluster listener and Raft |
| 8 | **T3**: gateway and engine hops |
| 9 | TiKV and PD mTLS |
| 10 | **T4**: bulk paths |
| 11 | The benchmark and the per-path rulings |
| 12 | **T5**: agentd, connect and the node agent |
| 13 | Kubernetes and BYOC |
| 14 | Docs and the upgrade gate |

The status file is `loams-wt/.he0-shared/ct1-status.md`.

## 17. Questions (answered with their defaults)

| # | Question | Default (approved) |
|---|---|---|
| Q1470 | Should a pre-shared cluster credential ship before mTLS, as a stopgap for #1560? | **No.** T1 (mTLS on HTTP/2) is the stopgap. It is small enough to ship first |
| Q1471 | Should CT1 benchmark `s2n-quic` as well? | **Only if quinn misses D2322's bar on class B**, behind the `loams-transport` interface |
| Q1472 | Leaf lifetime? | **24 hours**, renewed at 16 hours. It can be configured down to 1 hour |
| Q1473 | Should the root key stay in the cluster or offline? | **In the deployment's secret store** (not on any node's disk). Offline roots are an option for operators |
| Q1474 | 0-RTT for idempotent reads? | **Off until CT1 measures it.** It is then turned on only for routes marked idempotent, if the class-C or class-A p99 gain is at least 10 % |
| Q1475 | Should the CA service be a separate binary? | **No.** It runs in the `meta` role (D2310) |
| Q1476 | Support RSA leaves for old tools? | **No.** P-256 only. TiKV and PD accept ECDSA (verify) |
| Q1477 | HTTP/1.1 on internal listeners after T2? | **No.** It is accepted only during T1 and T2 for compatibility |
| Q1478 | Third-party engines with no TLS on some hop? | **List them as exceptions on the internal network, and add a TLS sidecar only if one is needed** (no default sidecar) |
| Q1479 | Should agentd on a user's laptop get a cluster identity? | **No.** It stays a client with user credentials (§2). Only agentd running inside the deployment gets `svc/agentd` |
| Q1480 | Make the tailnet optional between nodes once mTLS exists? | **Yes.** mTLS does not depend on the tailnet. The tailnet stays the operator access path (§43) |
| Q1481 | Who issues certificates on Kubernetes: the CA service or cert-manager? | **The CA service by default**, with cert-manager or SPIRE through D2314 |
| Q1482 | Should the benchmark run on cloud hardware too? | **Yes**, for classes A and B on the CB1 accounts within their caps, after the local runs |
| Q1483 | WebTransport for agentd or the console? | **Not in this design.** Clients are out of scope (§2) |
| Q1484 | Use QUIC datagrams (RFC 9221) for anything? | **No.** Only streams are used |
| Q1485 | Should the rulings be re-measured on every minor release? | **Yes** (D2322) |

## 18. What this amends

- [Networking map](networking-map.md) §4: "TLS on every listener; mTLS between nodes" is now this design, and the "Private network for internal routes" row is superseded by D2303. §5's row "Short-lived internal mTLS certificates (hours), issued and rotated by the control plane" is now D2309–D2313, issued by the deployment's own CA service; a control plane may still act as an external CA (D2314).
- [§10](10-operations.md) §4: the mTLS between nodes planned for M2 is now CT1.
- D2203 and D2206 (the meter's trust model): their callers are identified per role from T1 (D2328).
- The `validate_auth` rule "a cluster node's listeners stay on a private network until MT1" in server.rs is replaced by D2303 at T1.
- PG2 Tasks 23 and 38 and HS1 Task 20: they use this design's CA and identities (D2325).
