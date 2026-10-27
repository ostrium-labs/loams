# 57 — Loams Gateway: One Rust Gateway, Postgres First

> **Updated 2026-10-19 ([D1119](13-decision-log.md)): Loams has no open core.** Where this document places metering, billing, the hosted control plane, commercial APIs or other components in the private `loams-platform` repository, or calls something private for open-core reasons, read that as superseded: everything is Apache-2.0, and the metering, billing and control-plane code is moving into the open repo now (design §63, Loams Meter). Ostrium Labs offers services (infrastructure efficiency, SLAs, support), not a paid edition. The rest of the design is unchanged.

Status: **Approved direction** (owner, 2026-10-16) · details **Proposed** · 2026-10-16. Source: the owner's decision of 2026-10-16 (§1.1), which is binding. Decisions **D1010–D1024** and open questions **Q895–Q905** are declared in the [decision log](13-decision-log.md). Rows that restate the owner's decision are *Approved*. Rows that add detail to it are *Proposed* until the owner confirms them. Plan: [GW1](../plans/2026-10-16-gw1-loams-gateway.md).

> **Amendment (approved 2026-10-21; [§77](77-kv-gateways.md), D1646).** Two more modules, `resp` (Redis) and `cql` (Cassandra), behind cargo features of the same names. They terminate their protocols and run on `loams-kv` in process, like §74's `temporal` module, in their own Deployment `loams-gateway-kv`; their listeners are never public (D1648).

This document builds on these earlier designs, and §13 says where it amends each one:
- [§46](46-loams-postgres-production.md) §8 (PgDog as the front door, D708–D711) and the [PG2](../plans/2026-10-08-pg2-postgres-production.md) rulings R18–R21. The SNI listener of PG2 Task 22 is the first thing this gateway does.
- [§25](25-clever-cloud-stack.md) §3 (D176, D184: Envoy as the edge, Pingora as the Rust L7 library) and [§34](34-protocol-gateway-and-standards.md) (D360–D363). The commercial protocol gateway moved to the private `loams-platform`; this one is a different, open component.
- [§15](15-agent-workspaces.md) §10.2 (the MCP gateway with tool retrieval) and [§43](43-private-networking.md) (the tailnet and the public front door).
- The [networking map](networking-map.md), which gains rows for this gateway.

---

## 1. Summary

### 1.1 The owner's decision (2026-10-16, binding)

> "Anyway we are going to build things like API gateway, LLM gateway, MCP gateway, so [instead of adding] Envoy or Traefik, build a crate."

Loams builds its own gateway in Rust, the crate `loams-gateway`, instead of deploying Envoy or Traefik for PG2's SNI listener. Its first job is the Postgres front door. The API, LLM and MCP gateways come later, on the same core.

### 1.2 Decisions

| # | Decision | § | Status |
|---|---|---|---|
| D1010 | **One Rust gateway, `loams-gateway`** (Apache-2.0): a core of listeners, protocol sniffers, a hot-reloaded route table and upstream pools, with one module per protocol. `pg` first, then `http`, `llm` and `mcp` | 1.1, 5 | Approved direction (owner, 2026-10-16) |
| D1011 | **Amends D708:** the L4 SNI listener in front of the per-namespace PgDog is `loams-gateway`, not Envoy, Kourier or Traefik | 7, 13 | Approved direction (owner, 2026-10-16) |
| D1012 | **The core's shape:** listeners (fixed and dynamic), sniffers that peek without consuming, an immutable route table swapped atomically on reload, upstream sets with ordered failover, TLS and mTLS from rustls, an admin listener, OTel and `/metrics` | 5 | Proposed |
| D1013 | **Frameworks:** the `http`, `llm` and `mcp` modules build on **Cloudflare Pingora 0.9** (Apache-2.0), pinned exactly and wrapped behind Loams' own traits. The `pg` module is Loams' code on tokio and rustls, run by its own listener manager because its port set changes at runtime; when Pingora enters the binary, that manager runs as a Pingora background service. rama is the recorded alternative. Sōzu and linkerd2-proxy are rejected | 6 | Proposed (owner's default, 2026-10-16) |
| D1014 | **TLS for Postgres:** a client that sends `SSLRequest` is **spliced**. The gateway answers `S`, reads the ClientHello's SNI, sends its own `SSLRequest` to the namespace's PgDog and forwards the client's TLS bytes unchanged, so TLS runs end to end and PgDog holds the certificate (D710 unchanged). A **direct-TLS** client (`sslnegotiation=direct`) is **terminated** at the gateway with ALPN `postgresql`, then re-encrypted to PgDog after an `SSLRequest`, verifying PgDog's certificate; the gateway presents **PgDog's own certificate** to the client, because SCRAM channel binding hashes it. Pinned PgDog negotiates no ALPN, and libpq refuses a direct connection without it (§7.3) | 7.2, 7.3 | Proposed; amends D710 for direct-TLS clients |
| D1015 | **What is refused before TLS:** a plaintext `StartupMessage` gets `FATAL 28000` and is closed; `GSSENCRequest` gets `N`; a ClientHello without a known SNI on the shared port gets the TLS alert `unrecognized_name`; a direct ClientHello without ALPN `postgresql` gets `no_application_protocol` | 7.4 | Proposed |
| D1016 | **Dedicated namespace ports** for clients without SNI: on request per namespace (`dedicated_port`), allocated by `pg-control` from a range (default 16000–17999) and kept until the namespace's last project is deleted. The port alone picks the namespace. A single-node stack may name a `default_namespace` for the shared port instead | 7.5 | Proposed |
| D1017 | **Upstream choice per namespace is ordered** (active, then standby): the first healthy PgDog replica takes every connection, and the next takes over on a connect failure. A plaintext `CancelRequest` is sent to every replica of the namespace on a dedicated port and is dropped on the shared port | 7.6, 7.7 | Proposed |
| D1018 | **Routes come from `pg-control`'s records,** rendered deterministically into one routes file per region with a generation digest. The gateway polls it (1 s) and re-reads on `SIGHUP`. A new table is validated whole and swapped atomically; an invalid file keeps the old table; dynamic listeners are reconciled to the table; live connections are never cut by a reload | 8 | Proposed |
| D1019 | **Deployment:** its own binary and image (`loams-gateway`, `ghcr.io/ostrium-labs/loams-gateway`), not a role of `loams`. On Kubernetes a regional Deployment of two replicas behind an L4 load balancer, with NetworkPolicy that lets it reach only PgDog's port 6432; in compose, one service beside PgDog | 9 | Proposed |
| D1020 | **Hard limits before authentication:** 10 s to finish the pre-TLS exchange and the ClientHello, 8 bytes for `SSLRequest`, 10 KiB for a plaintext startup packet, 64 KiB for a ClientHello, a per-process connection cap; every pre-auth parser is fuzzed | 10 | Proposed |
| D1021 | **Observability:** `loams_gateway_*` metrics labelled by module, listener and outcome, **never by tenant**; OTel spans per connection; per-namespace numbers leave the process only through a `ConnectionObserver` hook with no wire format (the usage observer seam, D548) | 11 | Proposed |
| D1022 | **The later modules:** `http` (API routes, h1/h2, gRPC, Connect), `llm` (OpenAI- and Anthropic-compatible routing, keys, rate limits, usage observer hooks), `mcp` (§15 §10.2 on MCP 2026-07-28). Commercial adapters stay in `loams-platform` (§34); the open gateway exposes hooks only | 12 | Proposed |
| D1023 | **Licence:** Apache-2.0, like every crate here. No AGPL, GPL or SSPL dependency (`deny.toml`), no PgDog code, and PgDog stays an unmodified separate process (§46 §8.3) | 14 | Proposed |
| D1024 | **The plan is GW1** in this repository. The name GW1 was last used for the protocol gateway's plans, which left with §34 (D440) and are numbered in `loams-platform` now; this GW1 is unrelated to them | 15 | Proposed |

### 1.3 What it looks like (Postgres, Kubernetes)

```
 psql / JDBC / Prisma / ...                         (old client, no SNI)
   │ TLS, SNI = acme.eu1.pg.<base>                     │ TLS to <region-ip>:16004
   ▼                                                   ▼
 cloud or MetalLB L4 load balancer  :5432 and the dedicated ports
   │
   ▼
 loams-gateway ×2 (loams-system)      pg module
   ├─ sniff: SSLRequest │ direct TLS │ plaintext (refused) │ GSSENC (N) │ Cancel
   ├─ SSLRequest: answer S ─ peek ClientHello ─ SNI → namespace ─ SSLRequest to PgDog ─ splice
   └─ direct TLS: terminate (ALPN postgresql) ─ SSLRequest + TLS to PgDog (verify) ─ relay
   │                                       routes.toml ◄── rendered by pg-control (D1018)
   ▼
 PgDog (loams-pg-acme, ×2, unmodified)  TLS ─ SCRAM ─ pools ─ route by database name
   ▼
 computes (§46 §7)
```

## 2. Scope

**In scope (GW1):** the crate, the core pieces the `pg` module needs, the `pg` module, the routes file and its renderer in `pg-control`, dedicated ports, the image, the compose and Helm wiring that PG2 Task 22 needs, and the integration test against the pinned PgDog.

**Designed here, built later:** the `http`, `llm` and `mcp` modules (§12), Pingora in the binary, PROXY protocol v2 from the load balancer (Q901), IP allow-lists per project (§46 §8.6), and a route push over mTLS (§8.4).

**Not in scope:**
- **The commercial protocol gateway** (OpenRTB and partner adapters, §34's moved decisions): it stays in `loams-platform`, and nothing here depends on it.
- **Anything billing-grade** (D548, D202): the gateway counts for limits and operations only.
- **Authentication of Postgres users:** PgDog does SCRAM (D711). The gateway never sees a Postgres password on the splice path, and does not parse the startup message on the terminate path either; it only relays.
- **Replacing PgDog:** pooling, routing by database name, and read/write splitting stay PgDog's (D708).
- **Replacing `loams-sqlgate`:** Loams SQL's MySQL front door (§47 §12) stays where it is. A `mysql` module could host it later; nothing is planned.

## 3. Why now: what blocked PG2 Task 22

PG2 Task 22 needs one listener that routes `<ns>.<region>.pg.<base-domain>` to that namespace's PgDog (D708). Three facts make a generic L4 SNI router fail:

1. **Postgres clients send a plaintext `SSLRequest` first.** In the standard negotiation the client sends the 8-byte `SSLRequest`, waits for `S`, and only then starts TLS. A TCP proxy that reads SNI from the first bytes (Envoy's `tls_inspector`, Traefik's `HostSNI`) sees `SSLRequest`, not a ClientHello, so it never finds an SNI. Envoy has a Postgres filter that can answer `S` itself (`envoy.filters.network.postgres_proxy` with `terminate_ssl`), but only by terminating TLS in Envoy, which moves the certificate out of PgDog and adds a component the owner has now ruled out.
2. **PgDog does TLS only after `SSLRequest` and reads no SNI.** Pinned PgDog v0.1.60 answers `SSLRequest` with `S` and then runs a rustls acceptor (`pgdog/src/frontend/listener.rs`, `Startup::Ssl`). It does not detect a ClientHello as the first bytes, and it never reads the server name (R18.4). A direct-TLS ClientHello passed through to it fails.
3. **PgDog negotiates no ALPN.** Its rustls `ServerConfig` sets no `alpn_protocols` (`pgdog/src/net/tls.rs` at v0.1.60, read 2026-10-16). libpq 17 with `sslnegotiation=direct` requires the server to select ALPN `postgresql` and closes the connection otherwise ("direct SSL connection was established without ALPN protocol negotiation extension", `fe-secure-openssl.c`, `REL_17_STABLE`). So even a proxy that replayed an `SSLRequest` to PgDog and then passed the client's direct ClientHello through would fail at the client. This is new since R18.4, and it is why direct clients are terminated (D1014).

And old clients without SNI (libpq before 14, some JDBC configurations, tools that connect by IP) need a port per namespace (§46 §8.1, "dedicated port").

## 4. Where things are today (checked on `dev` at `dc7aae4d`, 2026-10-16)

| Piece | State |
|---|---|
| PgDog per namespace | Rendering (Task 20) and the push through `pgdog-config-sync` (Task 21) are built. Nothing deploys PgDog yet: Task 22 does |
| PgDog's client TLS | `tls_certificate`, `tls_private_key`, `tls_client_required` (R18.5); `SSLRequest` only; no ALPN; no SNI; no PROXY protocol; it sees the peer address of the TCP connection |
| PgDog's cancel handling | `Startup::Cancel` is served when the key is its own; otherwise it calls its Enterprise broadcast, which the open build does not provide |
| The Helm chart `deploy/helm/loams-postgres` and the single-node bundle `deploy/loams-postgres-single` | Not built (PG2 Tasks 48 and 56) |
| An L7 or L4 gateway in this repository | None. `loams-sqlgate` terminates TLS for MySQL; `loams-agentd-preview` is a local preview proxy; the Envoy edge of D184 is not built |
| Workspace dependencies the `pg` module needs | `tokio`, `rustls` 0.23 (ring), `tokio-rustls` 0.26, `hyper` 1, `toml`, `serde`, `clap`, `tracing`, `sha2`, `rcgen` 0.14 (tests): all present, so GW1's first tasks add no new crate to `Cargo.lock` |

## 5. The core (D1012)

```
 crates/loams-gateway/
   config      static config: listeners, certificates, upstream trust, admin
   routes      the route table: parse, validate, digest; one section per module
   reload      route sources (file poll + SIGHUP today); a watch channel of Arc<RouteTable>
   listeners   fixed listeners from config; dynamic ones reconciled to the table
   sniff/      peek-only classifiers: Postgres startup packets, TLS ClientHello (SNI, ALPN)
   tls         server configs with an SNI certificate resolver; upstream client configs
   io          a stream that replays peeked bytes before the socket
   pg/         the Postgres module: front door, upstream connect, relay
   admin       /healthz, /readyz, /v1/routes (generation), /metrics (later)
   observe     the ConnectionObserver hook, metrics, spans
```

- **Listeners.** A listener is an address plus the module that serves it. Fixed listeners come from the static config and change only on restart. Dynamic listeners come from the route table (the `pg` module's dedicated ports) and are bound and unbound by the reconciler on every reload. A listener that fails to bind is reported on `/readyz` and retried each reload; it never blocks other listeners.
- **Sniffers** read only what classification needs, keep every byte, and hand the module a stream that replays them (`io::Prefixed`). Postgres: the first 8 bytes say `SSLRequest` (`80877103`), `GSSENCRequest` (`80877104`), `CancelRequest` (`80877102`, 16 bytes), or a protocol-3 `StartupMessage`; a first byte of `0x16` is a TLS handshake. TLS: rustls' `server::Acceptor` parses the ClientHello (SNI, ALPN, across records) from the kept bytes, so Loams writes no TLS parser.
- **The route table** is immutable. A reload builds a new one, validates it whole (unique names, unique hostnames, unique ports, parseable upstreams), computes its generation, and swaps it into a `tokio::sync::watch` channel. Each connection reads the table once, at routing time.
- **Upstream sets** are ordered lists of `host:port`. A host is resolved at connect time to all of its addresses, sorted, so a headless Kubernetes Service lists every replica. A failed connect marks that address down for 2 s (passive ejection) and the next is tried; there are no active health checks, because a PgDog health query must not keep anything awake (R18.9).
- **TLS** is rustls with the `ring` provider, as elsewhere in the workspace. Server certificates are chosen by SNI from a list of `(names, cert, key)`, where a name may be one wildcard label. Upstream TLS verifies against a configured CA with the route's server name. mTLS is the admin listener's and the route push's (§8.4).
- **Admin listener:** `/healthz` (the process runs), `/readyz` (a route table is loaded and every fixed listener is bound; dedicated ports that failed to bind are listed as `degraded`, and the replica stays Ready because its shared port still serves every SNI client: taking it out of the load balancer would cut those too, so a bind failure raises an alert instead, GW1 Task 13), `GET /v1/routes` (the generation and counts, no secrets). `/metrics` arrives with GW1's metrics task. Bound to the pod network only, never exposed by the Service.

## 6. Frameworks (D1013)

The owner's default (2026-10-16) is Pingora for the HTTP side, with rama as the alternative. This section records the evaluation.

| Option | Licence | Fit | Verdict |
|---|---|---|---|
| **Cloudflare Pingora** 0.9.0 (crates.io 2026-09-09) | Apache-2.0 | Production HTTP/1.1 and HTTP/2 proxying at Cloudflare's scale; upstream connection pools; graceful restart with listener hand-over; per-request filters (`ProxyHttp`) that fit API, LLM and MCP routing; a `ServerApp` for raw TCP streams and `BackgroundService` for side tasks; rustls, BoringSSL or OpenSSL. Pre-1.0: minor versions break, so it is pinned exactly and kept behind Loams traits. Its server owns the runtime and process lifecycle, and its listeners are fixed at start | **Chosen for `http`, `llm`, `mcp`** (D184 already named it "the library if a Rust L7 component is needed") |
| **hyper 1 + rustls + tokio directly** | MIT / Apache-2.0 | Already in the workspace; complete control; no pre-1.0 framework. Loams would write the pools, retries, graceful restart and filter chain itself, which Pingora has and has tested | Kept for the `pg` module and the admin listener; not chosen for the HTTP modules |
| **plabayo/rama** 0.4.0 (2026-08-19) | MIT OR Apache-2.0 | Modular, tower-style TCP, TLS and HTTP stacks with SNI peeking and proxy building blocks. Pre-1.0, and a much smaller team than Pingora's | **The alternative** if Pingora's fixed listeners or runtime model get in the way (Q898) |
| **Sōzu** | AGPL-3.0 | An application, not a library, and AGPL cannot link into an Apache-2.0 crate (D184 rejected it for the edge already) | Rejected |
| **linkerd2-proxy** | Apache-2.0 | Tied to Linkerd's control plane and not published as a library | Rejected |
| **Envoy / Traefik / Kourier** | Apache-2.0 / MIT | The owner's decision replaces them for this job (§1.1). Envoy stays the HTTP edge of D184 until §12.1 decides otherwise | Not used for Postgres |

**Why the `pg` module does not run on Pingora's listeners.** Its dedicated ports appear and disappear with namespaces, at runtime, through a reload. Pingora binds listeners once, at server start, and changes them only through a graceful upgrade of the whole process. So the `pg` module keeps its own listener manager on tokio. When the `http` module brings Pingora into the binary, the manager runs as a Pingora `BackgroundService` on the same process, and the shared port could become a Pingora `ServerApp` if that proves cleaner. GW1's first tasks add no Pingora dependency (YAGNI); the `http` plan adds it.

## 7. The `pg` module

### 7.1 Two ways in

| Entry | Picks the namespace by | Listener |
|---|---|---|
| **Shared port** (5432) | The ClientHello's SNI, matched exactly (case-insensitive) against the routes' hostnames; `default_namespace` when there is no SNI and one is set | Fixed |
| **Dedicated port** (D1016) | The port | Dynamic, one per namespace that asked for one |

### 7.2 The connection, step by step (D1014, D1015)

1. **Sniff** the first 8 bytes (within the 10 s handshake deadline, D1020).
2. **`SSLRequest`** → answer `S`. If any byte arrived with the 8 (data sent before the server's answer, the injection pattern of CVE-2021-23214), close. Then read the ClientHello (up to 64 KiB), keeping every byte.
   - Find the namespace (SNI or port). Unknown → send the TLS alert `unrecognized_name` and close.
   - Connect to the namespace's first healthy PgDog (§7.6), send `SSLRequest`, and require `S`. Anything else → close (never downgrade to plaintext).
   - Write the kept ClientHello bytes to PgDog, then copy both directions until either side closes. **This is the splice:** the TLS session is between the client and PgDog.
   - In `tls_mode = "terminate"` (an option, §7.3), this step terminates instead, as for direct TLS.
3. **Direct TLS** (first byte `0x16`) → read the ClientHello.
   - Without ALPN `postgresql` → alert `no_application_protocol` and close (Postgres 17's own rule for direct connections).
   - Find the namespace as above; unknown, or a name no configured certificate covers → `unrecognized_name`.
   - Connect to PgDog **first**: `SSLRequest`, require `S`, and open TLS verifying PgDog's certificate for the route's `upstream_server_name` (default: the route's first hostname) against the upstream CA.
   - Accept the client's TLS with **the very certificate PgDog presented** (the configured certificate whose leaf is byte-identical to PgDog's), selecting ALPN `postgresql`. If the gateway holds no copy of it → alert `internal_error`, and an error log that names the fix. See §7.3 for why.
   - Relay plaintext between the two TLS sessions.
4. **`GSSENCRequest`** → answer `N` and go back to step 1 once (libpq then sends `SSLRequest` or a startup). A second `GSSENCRequest` closes.
5. **A protocol-3 `StartupMessage`** (plaintext) → `ErrorResponse` with severity `FATAL`, SQLSTATE `28000` and "Loams Postgres requires TLS (sslmode=require or stronger)", then close (D710: plaintext refused).
6. **`CancelRequest`** (plaintext, 16 bytes) → §7.7.
7. **Anything else** → close.

### 7.3 Splice or terminate (D1014)

| | Splice (SSLRequest clients) | Terminate (direct clients) |
|---|---|---|
| Who holds the client-facing key | PgDog only | The gateway too: a copy of **PgDog's own certificate and key** (SCRAM channel binding, below) |
| D710 ("PgDog terminates client TLS") | Unchanged | Amended: the gateway terminates and re-encrypts to PgDog, verifying PgDog's certificate |
| What the gateway can read | The ClientHello only | Everything, but it parses nothing after TLS: it relays |
| CPU | Copy only | Two TLS sessions |
| ALPN `postgresql` | PgDog's (none, §3) | The gateway's, so libpq's direct mode works |
| `sslmode=verify-full` | Against PgDog's certificate | Against the same certificate, presented by the gateway |
| SCRAM-SHA-256-PLUS (`channel_binding`) | Binds to PgDog's certificate | Binds to the same certificate, so PgDog accepts it |

**Why splice is the default for `SSLRequest` clients:** it keeps the property D708 and D710 were written for. The key that protects a namespace's traffic stays with that namespace's PgDog, a compromised gateway reads no SQL or password on that path, and the gateway's cost is a copy. Every libpq before 17, every JDBC, .NET, Go, Node and Python driver, and libpq 17 by default (`sslnegotiation=postgres`) use `SSLRequest`, so almost all traffic is spliced.

**Why direct clients are terminated:** splicing a direct ClientHello to PgDog cannot satisfy libpq's ALPN check (§3, fact 3), because ALPN is decided inside PgDog's handshake. Termination is the only way to serve them without changing PgDog. If PgDog gains ALPN upstream (Q897), `auto` splices direct clients too and the gateway stops holding client-facing keys for Postgres.

**Why the terminate path presents PgDog's own certificate (found by GW1's integration test).** Over TLS, libpq authenticates with SCRAM-SHA-256-PLUS when the server offers it (`channel_binding=prefer`, the default), and PgDog offers it. Its `tls-server-end-point` binding is a hash of the server certificate the **client** saw, which PgDog checks against the certificate **it** holds. A gateway certificate of its own, however valid for the hostname, makes PgDog answer "password … is wrong" to every direct client. So the gateway connects to PgDog first and presents the certificate PgDog presented, which it must hold a copy of; without one it refuses direct clients with a clear log line rather than letting them fail at authentication. This is one more reason the splice is the default.

**What this means for certificates (Q905).** Each namespace's PgDog holds **only its own** certificate and key, for `<ns>.<region>.pg.<base-domain>` (PG2 Task 23's "a `Certificate` per namespace hostname"), and the gateway holds a copy of each, chosen by matching PgDog's leaf. So a compromised PgDog can present only its own namespace's name; the gateway, which fronts every namespace anyway, is the one place that holds them all. The routes file names each namespace's certificate files, and they are loaded with the table on every reload (GW1 Task 9). A regional wildcard shared by every PgDog is simpler but lets one leaked key impersonate every namespace, so it is kept for single-node and compose stacks, which have one trust domain.

**The modes:** `tls_mode = "auto"` (the default: splice `SSLRequest`, terminate direct), `"terminate"` (both terminated; for operators who want one certificate place), `"passthrough"` (both spliced; direct clients refused, so the gateway needs no key at all).

### 7.4 Refusals (D1015)

| Client sends | The gateway answers | Why |
|---|---|---|
| A plaintext `StartupMessage` | `FATAL 28000`, closed | D710; a clear message instead of a reset |
| `GSSENCRequest` | `N` | Loams does not offer GSSAPI encryption; libpq falls back to `SSLRequest` |
| A ClientHello with an unknown SNI, or none on the shared port without `default_namespace` | TLS alert `unrecognized_name` | Nothing to route to. The alert is sent in clear, before any key exchange, so no certificate is needed |
| Direct TLS without ALPN `postgresql` | TLS alert `no_application_protocol` | Postgres 17's own rule; it also keeps ALPN free for the `http` module on a shared port later |
| An `SSLRequest` followed by bytes before the gateway's `S` | Closed | The buffered-data injection pattern |
| PgDog answers `N` to the gateway's `SSLRequest` | Closed | Never downgrade |

### 7.5 Dedicated ports (D1016)

- **On request.** A namespace gets a port when its settings ask for one (`dedicated_port: true`, a namespace-level Postgres setting, Q896). A port per namespace by default would put hundreds of listeners on a cloud load balancer, and those have small listener quotas (an AWS NLB allows 50 by default). SNI covers every libpq since 14 and every current driver.
- **Allocation.** `pg-control` takes the lowest free port in the region's range (default 16000–17999) when the setting is turned on, records it with the namespace (a `G/<ns>` record in `PgControlStore`), and frees it when the setting is turned off or the namespace's last project is deleted. Ports are never reused within 24 hours, so a stale client cannot reach another namespace.
- **Routing.** A connection on a dedicated port belongs to that namespace whatever its SNI says. The rest of §7.2 is the same, and plaintext is refused the same way.
- **Exposure.** The rendered Service of the gateway lists the shared port and every allocated port (§9.1).
- **`default_namespace`** (single node, §9.2): one namespace may own the shared port's no-SNI connections, so `psql "host=localhost sslmode=require"` works on a laptop.

### 7.6 Upstream choice (D1017)

A namespace has two PgDog replicas (D708). The gateway tries them **in order**: the first healthy address takes every connection, and the second takes over when a connect fails. Two reasons:
- **Cancel requests.** A Postgres cancel opens a new connection that must reach the PgDog that issued the key. PgDog's open build does not forward a cancel it did not issue (§4), so random spreading would lose half the cancels, whether they are encrypted (libpq 17's `PQcancelCreate`) or not. Ordered choice puts every connection and every cancel on the same replica while it is healthy.
- **Load.** One PgDog replica is multi-threaded and serves thousands of clients; a namespace that outgrows one replica is a reason for random spreading (`upstream_order = "random"`, Q902) or a larger replica, not for the default.

A failed connect marks that address down for 2 s and the next is tried within the client's handshake deadline. When every address is down the client gets a closed connection after the TLS alert `internal_error` on the terminate path, or a closed socket on the splice path.

### 7.7 Cancel requests (D1017)

- **Inside TLS** (libpq 17's new cancel API, `sslmode` honoured): routed like any connection, so with ordered choice it reaches the issuing replica.
- **Plaintext on a dedicated port:** the 16 bytes are sent to **every** replica of that namespace. Only the one that issued the key acts; the others ignore it. Postgres itself accepts plaintext cancels whatever `pg_hba` says, and a cancel carries no SQL or password.
- **Plaintext on the shared port:** it has no SNI, so it cannot be routed. It is dropped and counted (`loams_gateway_pg_cancel_dropped_total`). Q895 asks whether to broadcast instead.

### 7.8 What the client and PgDog see

- PgDog sees the gateway's address as the client's. PgDog has no allow-list setting (R18.5), so nothing in PgDog depends on it. Per-project IP allow-lists (§46 §8.6) move to the gateway, where the real client address is known (from the socket, or from PROXY protocol v2 behind a load balancer that sends it, Q901).
- Wake-on-connect is unchanged: the gateway connects to PgDog at once, and PgDog holds the client in its queue while the waker starts the compute (R18.9). The gateway has no timeout on an established relay, so a 20 s checkout wait passes through. TCP keepalive is on both sides.

## 8. Routes and hot reload (D1018)

### 8.1 The routes file

`pg-control` renders it from its records, deterministically (byte-identical for identical records, under golden tests, like R20's PgDog render):

```toml
# Rendered by pg-control. Do not edit.
generation = "sha256:…"            # the digest of everything below

[pg]
# default_namespace = "dev"         # single node only

[[pg.namespaces]]
name = "acme"
hostnames = ["acme.eu1.pg.example.com"]
port = 16004                       # only when dedicated_port is on
upstreams = ["pgdog.loams-pg-acme.svc.cluster.local:6432"]   # headless: every replica
upstream_server_name = "acme.eu1.pg.example.com"            # optional; default hostnames[0]
```

- The file carries no secret: names, hosts and ports only. TLS material stays in the static config's files.
- The generation is **required** (`routes.require_generation = true`, the default) and checked, so a truncated or hand-edited file is refused rather than accepted with a new digest. A hand-written file for a local stack may turn the requirement off; the gateway then computes the digest.

### 8.2 Reload

- **Sources.** The file is polled every second (by content digest, which also catches a Kubernetes ConfigMap's symlink swap) and re-read at once on `SIGHUP`.
- **Validation.** The whole file is parsed and checked (names `[a-z0-9-]{1,63}`, unique names, unique hostnames, unique ports inside the dynamic range, at least one upstream each). An invalid file is logged, counted (`loams_gateway_reload_total{result="invalid"}`) and **ignored**: the old table stays in force.
- **The swap** is atomic. New connections route on the new table; existing connections keep their upstream until they close. A namespace removed from the table therefore stops taking new connections at once and keeps its live ones; its PgDog going away closes them.
- **Dynamic listeners** are reconciled after the swap: new ports are bound, removed ports are closed (in-flight connections on them are not cut), and a moved port is rebound.
- **Readiness** waits for the first valid table, so a new replica takes no traffic before it can route.

### 8.3 Delivery

- **Compose and single node:** `pg-control` writes the file into a directory the gateway mounts, atomically (temporary file, fsync, rename), as `DirSink` does for PgDog (R21.6).
- **Kubernetes:** a ConfigMap in the gateway's namespace that `pg-control` updates. The kubelet's refresh delay (up to a minute) only delays a **new** namespace's first reachability; existing routes never wait on it.
- **Later (§8.4):** a push over mTLS for sub-second propagation, reusing `pgdog-config-sync`'s protocol (`PUT /v1/config`, `GET /v1/generation`), if namespace creation latency matters.

### 8.4 Why a file, not a watch on `pg-control`'s API

A file makes the gateway independent of `pg-control`'s availability: a `pg-control` outage freezes the routes but never takes the front door down, and a gateway restart serves the last file. The core's `RouteSource` trait allows a Connect watch source later without changing modules.

## 9. Deployment (D1019)

### 9.1 Kubernetes

- **Workload:** `loams-gateway` Deployment, two replicas, anti-affinity across zones, PodDisruptionBudget `minAvailable: 1`, in the platform's system namespace. Its own image, built from `deploy/release/Containerfile.loams-gateway` beside the others.
- **Service:** `type: LoadBalancer` with `externalTrafficPolicy: Local` (client addresses kept where the load balancer supports it), port 5432 plus each allocated dedicated port, rendered by `pg-control` into the tenants repository with the namespace (MT4's renderer hook), or into the `loams-postgres` chart's values before MT4 lands.
- **NetworkPolicy:** egress only to PgDog's port 6432 in `loams-pg-*` namespaces and to DNS; each PgDog namespace admits 6432 only from the gateway's pods (and `pg-control` for admin). The gateway is the only way in from outside.
- **Certificates:** one cert-manager `Certificate` per namespace hostname in that namespace (PG2 Task 23), mounted by its PgDog only; the gateway reads a copy of each (its ServiceAccount may read those Secrets, or a replication controller copies them into its namespace; GW1 Task 11), because the terminate path must present PgDog's exact certificate (§7.3). The gateway also trusts the issuer's CA for upstream verification. With `tls_mode = "passthrough"` it needs none of this.
- **Rolling restarts:** a replica stops accepting, lets live relays finish for up to the drain timeout (default 60 s), then exits. The load balancer moves new connections to the other replica.

### 9.2 Compose and single node

One `loams-gateway` service beside PgDog in `deploy/loams-postgres-single` (PG2 Task 56), publishing 5432 and mounting the routes directory that `pg-control` writes. The single node sets `default_namespace`, so clients that connect to `localhost` work.

### 9.3 Configuration

The static config (`/etc/loams-gateway/gateway.toml`; every field also a flag):

```toml
[admin]
listen = "0.0.0.0:9187"

[routes]
path = "/etc/loams-gateway/routes/routes.toml"
poll_interval_ms = 1000

[pg]
listen = "0.0.0.0:5432"
dedicated_bind = "0.0.0.0"
dedicated_ports = "16000-17999"
tls_mode = "auto"               # auto | terminate | passthrough
handshake_timeout_ms = 10000
max_connections = 20000
drain_timeout_ms = 60000

[[pg.certs]]                    # the terminate path only
names = ["*.eu1.pg.example.com"]
cert = "/etc/loams-gateway/tls/tls.crt"
key = "/etc/loams-gateway/tls/tls.key"

[pg.upstream_tls]
ca = "/etc/loams-gateway/upstream-ca/ca.crt"
```

## 10. Security (D1020)

- **Before authentication** the gateway parses only untrusted bytes: Postgres startup packets and a ClientHello. Each has a size limit and the shared handshake deadline (D1020). rustls parses the ClientHello. The startup classifier and the replay buffer are fuzzed (`cargo fuzz` targets in the crate, run by the nightly fuzz job), as the networking map's §5 asks.
- **No plaintext** reaches PgDog through the gateway, and none leaves it except the protocol's own pre-TLS messages (§7.4).
- **Keys:** with `auto`, the gateway holds a copy of every namespace PgDog's key (Q905), used for direct clients only; each PgDog holds only its own. With `passthrough`, it holds none. Keys are read from files (mounted Secrets) and never logged.
- **Connection caps:** a per-process cap and a per-source-address cap on unfinished handshakes (default 64), so a slow-loris client cannot exhaust handshake slots.
- **The admin listener** is not on the Service. Its route push, when built (§8.3), is mTLS only, like `pgdog-config-sync`.
- **Threat model** additions go into PG2 Task 53's review, with the gateway in scope.

## 11. Observability (D1021)

- **Metrics** (`/metrics`, a GW1 task): `loams_gateway_connections_total{module,listener,outcome}` (outcome: `spliced`, `terminated`, `refused_plaintext`, `unknown_sni`, `no_alpn`, `upstream_failed`, `timeout`), `loams_gateway_active_connections{module,listener}`, `loams_gateway_handshake_seconds{module,path}`, `loams_gateway_upstream_connect_seconds{module}`, `loams_gateway_reload_total{result}`, `loams_gateway_route_generation_info{generation}`, `loams_gateway_pg_cancel_dropped_total`. **No tenant or namespace label**, so no series can be read as a per-tenant account (D548).
- **The `ConnectionObserver` hook:** `on_open(namespace, listener, path)` and `on_close(namespace, bytes_in, bytes_out, duration)`, a trait with no wire format and a no-op default. Operators who want per-namespace numbers implement it; this is the usage observer seam, as `ComputeLifecycleObserver` is for computes (D703).
- **Traces:** one span per connection (module, listener, path, upstream address, outcome) through `tracing`, exported by OTel where the binary is configured for it. Postgres has no trace context on the wire, so the span is a root.
- **Logs:** no bytes after the ClientHello, no startup parameters, no certificate private material.

## 12. The later modules (D1022)

### 12.1 `http`: the API gateway

- On Pingora (D1013): HTTP/1.1 and HTTP/2 (ALPN `h2`), gRPC and Connect, WebSocket and SSE (D176's phase-1 protocols, except HTTP/3).
- Routes by host and path prefix to Loams services; TLS from the same SNI resolver; the `x-loams-tenant` and `traceparent` rules of §34 §2 at the first hop; limits from the limits API (D546).
- **Relation to Envoy (D184):** Envoy remains the planned HTTP edge until the `http` module is built and measured against it; replacing Envoy at the edge is Q900, decided at the `http` plan, not here. Kourier stays Knative's internal ingress (D442) either way.

### 12.2 `llm`: the LLM gateway

- OpenAI-compatible (`/v1/chat/completions`, `/v1/responses`, `/v1/embeddings`) and Anthropic-compatible (`/v1/messages`) faces; routing by model name to providers or self-hosted servers, with fallbacks.
- Provider keys held by the gateway (from the credential store), never by the agent; Loams keys or tokens in, provider keys out.
- Rate and token limits per key, namespace and model, enforced in-process (D546's model).
- **Usage observer hooks:** per request, the model, token counts from the provider's response and the latency go to an observer trait with no wire format. What anyone does with them is outside this repository (D548).

### 12.3 `mcp`: the MCP gateway

- §15 §10.2's design, on the MCP 2026-07-28 spec: stateless requests; routing on the `Mcp-Method` and `Mcp-Name` headers without parsing JSON; the `find_tools` and `call_tool` meta-tools over a tool catalogue in a collection and a tool graph; `traceparent` from `_meta`; older 2025-11-25 servers behind a held upstream session.
- **agentgateway** (Linux Foundation, Apache-2.0, Rust; MCP, A2A and LLM routing; the 1.5.x line) is a candidate to run unmodified beside `loams-gateway`, to fork with a patch queue (as Grafeo), or to borrow designs from. Q899 decides it at the LLM and MCP plan; the default is to prefer running it unmodified if it fits.

### 12.4 Commercial adapters

The protocol gateway's commercial adapters (OpenRTB, partner APIs, §34) stay in `loams-platform`. The open gateway offers hooks (route filters, observers) that a private build can implement; it never links private code, and no open module depends on one.

## 13. Relation to earlier decisions

| Decision | Effect |
|---|---|
| **D708** (PgDog is the front door; an L4 SNI-passthrough listener) | **Amended (D1011):** the listener is `loams-gateway`. PgDog stays the front door for Postgres semantics; the gateway only picks the namespace |
| **D710** (TLS on both hops; PgDog terminates client TLS) | **Kept** for `SSLRequest` clients (spliced); **amended** for direct-TLS clients, terminated at the gateway and re-encrypted to PgDog with verification (D1014). Plaintext is still refused |
| **§46 §8.1** (the diagram; "the namespace's dedicated port on the same listener") | The diagram's listener is this gateway; dedicated ports are on request (D1016) |
| **§46 §8.6** (allow-lists "enforced at the L4 listener") | They are this gateway's, after Q901 |
| **D184** (Envoy as the edge; Pingora if a Rust L7 component is needed) | Consistent: Pingora is that library (D1013). Envoy at the HTTP edge is unchanged until Q900 |
| **D176** (phase-1 protocols) | The `http` module's target list |
| **§34** (the protocol gateway moved to `loams-platform`) | Unchanged: this is a different, open gateway, with hooks only (D1022) |
| **D548, D202** (no billing in this repository) | Kept: no tenant labels, observer hooks only (D1021) |
| **PG2 Task 22** | Re-scoped by ruling R22.0 in the PG2 plan: its SNI listener is `loams-gateway` (GW1 Tasks 1–8) |
| **[Networking map](networking-map.md)** | Rows added in §3 (data) and §5 (hardening) |

## 14. Licence (D1023)

`loams-gateway` is Apache-2.0 (`license.workspace`). Its dependencies pass `cargo deny` (no AGPL, GPL or SSPL). PgDog is not a dependency and no PgDog text is copied: §3's facts are behaviour and file names. Pingora is Apache-2.0; rama is MIT or Apache-2.0; agentgateway, if run, is a separate unmodified process unless Q899 rules for a fork.

## 15. The plan (D1024)

[GW1](../plans/2026-10-16-gw1-loams-gateway.md): the crate and its core, the `pg` module with splice and terminate, dedicated ports, reload, the PgDog integration test, the image, `pg-control`'s routes renderer and port records, metrics, fuzzing, and the compose and Helm wiring for PG2 Task 22. The `http`, `llm` and `mcp` modules get plans of their own.

## 16. Risks

| Risk | Likelihood | Mitigation |
|---|---|---|
| A driver sends something before waiting for `S` (pipelined `SSLRequest`) | Low | Postgres refuses it too; the gateway closes the same way, and Task 29's driver matrix runs through the gateway |
| Pingora's pre-1.0 breaks a minor release | Medium | Exact pin, Loams traits around it, upgrade under `ci:upgrade` |
| Ordered choice overloads one PgDog replica | Low | Per-namespace `upstream_order = "random"` (Q902); the replica is sized from the namespace's pool sizes |
| A cloud load balancer's listener quota limits dedicated ports | Medium | Dedicated ports on request only (D1016); a second load balancer when one fills |
| Plaintext cancels on the shared port are lost | Medium for old drivers | Q895; libpq 17's encrypted cancels route; dedicated ports broadcast |

## 17. Open questions (Q895–Q905)

| # | Question | Default |
|---|---|---|
| Q895 | A plaintext `CancelRequest` on the shared port has no SNI. Drop it, or broadcast it to every namespace's PgDog in the region? | Drop and count it; broadcasting sends one tenant's cancel key to every tenant's PgDog |
| Q896 | Dedicated ports: on request, or for every namespace? And the range? | On request (`dedicated_port: true`); range 16000–17999 per region |
| Q897 | ~~Ask PgDog upstream to negotiate ALPN `postgresql` (and to accept a direct ClientHello), so direct clients can be spliced too? Contacting upstream needs the owner~~ **Answered 2026-10-16 by the owner: yes; asked upstream in pgdogdev/pgdog#1840.** | Yes, propose it upstream with the owner's approval; terminate direct clients until a pinned PgDog has it |
| Q898 | Pingora (default) or rama for the HTTP modules? | Pingora 0.9, pinned exactly, wrapped (owner's default, 2026-10-16) |
| Q899 | agentgateway for the LLM and MCP modules: run unmodified beside `loams-gateway`, fork it, or borrow designs only? | Evaluate at the LLM/MCP plan; prefer running it unmodified if it fits |
| Q900 | Does the `http` module replace Envoy as the HTTP edge (D184)? | Decide at the `http` plan, after measuring; Envoy stays the plan until then |
| Q901 | PROXY protocol v2 from the load balancer? | On where the load balancer sends it (CP1's ingress), off in compose; the gateway refuses a PROXY header on a listener not configured for it |
| Q902 | Upstream order per namespace: ordered (active and standby) or random? | Ordered; random as a per-namespace option |
| Q903 | `tls_mode` default: `auto` (holds a key for direct clients) or `passthrough` (no key; direct clients refused)? | `auto`, so libpq 17's direct mode works |
| Q904 | Where the gateway runs in single-node and desktop stacks: always, or only when Postgres is enabled? | Only with Postgres (`loams dev --postgres`, the desktop's Postgres stack) |
| Q905 | PgDog's client certificate: one per namespace, each copied to the gateway (the terminate path must present PgDog's exact certificate for channel binding, §7.3), or one regional wildcard shared by every PgDog and the gateway? | One per namespace (a leaked PgDog key then impersonates only its own namespace); the wildcard only in single-node and compose stacks |

## 18. Sources

- PgDog v0.1.60: `pgdog/src/frontend/listener.rs` (startup handling) and `pgdog/src/net/tls.rs` (no ALPN), read 2026-10-16 at the tag; PG2 rulings R18.1–R18.9.
- PostgreSQL `REL_17_STABLE`: `src/interfaces/libpq/fe-secure-openssl.c` (ALPN required for `sslnegotiation=direct`) and `src/backend/libpq/be-secure-openssl.c` (the server's ALPN callback); the protocol's `SSLRequest`, `GSSENCRequest` and `CancelRequest` codes.
- Pingora 0.9.0 (crates.io, 2026-09-09, Apache-2.0); `pingora_core::apps::ServerApp`; rama 0.4.0 (2026-08-19, MIT OR Apache-2.0).
- MCP specification 2026-07-28 (§15 §10).


> **See also (2026-10-17):** [§60](60-places-and-harness-composition.md) D1091 proposes cascades for the LLM module: typed triage with a confidence, threshold escalation, a verifier, and deterministic routes.

> **See also (2026-10-21):** [§69](69-edge-reach.md) (Edge Reach, approved by the owner 2026-10-21) makes the `http` module's first slice an **edge listener** for serverless runtimes: allow-listed routes, no admin, `access` or `network` admission, on hyper behind an `EdgeProxy` trait until this module moves to Pingora (D1303, D1306). Under the amended rule (D1311), callers off the tailnet reach Loams only through that listener, over a private link or a brokered tunnel; the engine stays loopback, tailnet or gateway-only. The listener is never public: the opt-in public profile (D1312) was deferred, and the `pg` listener stays private (Q992).
