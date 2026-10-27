# Networking map

> **Amendment (approved 2026-10-23) by [§78](78-loams-cloud-on-tidb-x.md)** (Loams Cloud on TiDB X; D1700–D1735). The public endpoints Ostrium runs become `loams.dev` (with `/cloud`), `auth.loams.dev` and the Loams Cloud control plane's API host; `console.loams.dev` is retired and `LOAMS_CORS_ORIGINS` loses it as a default in LC2 (D1705). Devices enrolled in Loams Cloud open no inbound port (D1714).

This is an index, not a new design. It points each kind of network traffic in Loams at the design
and plan that already own it, so a networking question starts in the right place. It was written on
2026-10-15, after the owner brought an external "networking planes" proposal. The proposal's access,
control, data and internal planes turned out to be designed here already. Its fifth plane, a public
"blind tunnel", is new, and is parked in §6.

Rules that hold across every plane:

- **The engine is never on the public internet.** Its listeners are reached on loopback, over the
  operator's tailnet, or by `loams-gateway` on an internal network. Callers off the tailnet, such as
  serverless functions, reach Loams only through the gateway's edge listener, over a private link or
  a brokered tunnel that authenticates every request before it reaches the stack ([§69](69-edge-reach.md)
  §4.2). Public endpoints are only the ones Ostrium runs: `loams.dev`, `auth.loams.dev` and
  `console.loams.dev`. *Amended 2026-10-21 by D1311 (approved by the owner); a public edge profile
  (D1312) was deferred.*
- **Open standards:** TLS, mTLS, OIDC, Connect, CloudEvents, and the Postgres and MySQL wire
  protocols.
- **Billing-grade metering stays out of this repository** (open core, §41). Limits,
  caps and backpressure belong here; usage billing does not.
- **Identity** is Zitadel behind `auth.loams.dev` (§76 D1600), an issuer of the operator's own choosing, or the
  tailnet's own identity (see §1).

## 1. Access: people and operators

How people sign in, and how operators reach machines.

| What | Where it is decided | State |
|---|---|---|
| Two networks: Cloudflare is the public front door, a tailnet is the private network | [§43](43-private-networking.md) §1, §4 (D581) | Proposed |
| Pluggable tailnet: Tailscale on the operator's own account now, Headscale later | [§43](43-private-networking.md) §1, §6.5 (D580, D582); plan [NET1](../plans/2026-10-02-net1-private-networking.md) | Tailscale ships in the preview; Headscale is coming soon |
| Tailnet policy as code, tags, key expiry and approval | [§43](43-private-networking.md) §8 (D585, D589) | Proposed |
| Hosted console sign-in: the engine identifies the caller from the tailnet (Tailscale WhoIs through the sidecar's `tailscale serve`), with no password | [§44](44-unified-api-and-sdks.md); plan [API2](../plans/2026-10-10-api2-unified-api-sdks.md) ruling S6/7.13; `deploy/release/compose.yaml` (`LOAMS_AUTH_MODE=tailscale`, `LOAMS_TAILSCALE_LISTEN`) | Ships in v0.1.0-preview.2 (owner: "option A", 2026-10-14) |
| Other sign-in modes: OIDC (any issuer, `auth.loams.dev` by default) and API keys | [§19](19-console-identity-and-agents.md); [§10](10-operations.md) §4 | Ships in the preview |
| Browser origins allowed to call an engine | The engine's own origin (the console it serves at `/ui/`), plus the exact origins in `LOAMS_CORS_ORIGINS`, none by default. Loams Desktop sends the engine's own origin. *Until 0.1.2 the default was `https://console.loams.dev` only (D1705, amended)* | Ships in v0.1.2 (default changed); variable since v0.1.0-preview.2 |

## 2. Control: hosted control plane to customer deployments

| What | Where it is decided | State |
|---|---|---|
| The outbound-only BYOC agent (`loams-byoc-agent`): the customer opens no inbound port, and the control plane never holds cluster credentials | [§41](41-multitenant-byoc-control-plane.md) §7.1–§7.3 (D543); plan [CP1](../plans/2026-10-10-cp1-control-plane-byoc.md) | Planned |
| Connectivity modes: the outbound agent, or the customer's tailnet | [§41](41-multitenant-byoc-control-plane.md) §7.4, [§43](43-private-networking.md) §6 (D586, D587) | Proposed |
| Air-gapped: no outbound path, a Git mirror and carried-in bundles, shown as `disconnected` | [§41](41-multitenant-byoc-control-plane.md) §7.1 | Planned |
| Self-hosted preview: SSH to the operator's own VMs (`byoc/install.sh`), no control plane | `deploy/release/` | Ships in the preview |
| Upgrades delivered to deployments | [§54](54-upgrades.md); plan UP1 | Planned for preview.3 |

## 3. Data: applications to Loams

| What | Where it is decided | State |
|---|---|---|
| Postgres: routing by endpoint (SNI or `options=endpoint=`) through the proxy and `wake_compute` | [§28](28-loams-postgres.md) (routing table), [§46](46-loams-postgres-production.md) | Preview (compose stack) |
| Postgres front door in production: `loams-gateway` reads SNI (after `SSLRequest`, or from a direct-TLS ClientHello) and picks the namespace's PgDog; dedicated ports for clients without SNI; plaintext refused | [§57](57-loams-gateway.md) §7 (D1011, D1014–D1017); plan [GW1](../plans/2026-10-16-gw1-loams-gateway.md); PG2 Task 22 | Planned |
| API, LLM and MCP gateways on the same core | [§57](57-loams-gateway.md) §12 (D1022) | Designed, not planned |
| MySQL (Loams SQL): `loams-sqlgate`, TLS 1.2+ with SNI per database, branch from the role user or the SNI | [§47](47-loams-sql-production.md) §12 (D731) | Built; the release stack does not expose it yet |
| SQL router across shards | [§31](31-loams-router-and-verification.md) §4–§7 | In progress (RT) |
| Engine surfaces (native, Flight SQL, Qdrant, Elasticsearch, Connect) and their credentials | [§10](10-operations.md) §4 (the surface table); [§44](44-unified-api-and-sdks.md) | Preview |
| House (ClickHouse HTTP) on its own tailnet node | HS1; `deploy/release/compose.yaml` (profile `house-tailnet`; the House itself runs by default) | Preview; TLS between engine and House is HS1 Task 20 |
| Kafka to Confluent Cloud: outbound only from the BYOC VM to the cluster's public endpoint (TLS with full verification, SASL/PLAIN, ports 9092 and 443); nothing in Confluent connects in, so Confluent's managed connectors that pull from Loams are not supported | [§80](80-confluent-byoc.md) §6.2 (D1825); plan [CC1](../plans/2026-10-25-cc1-confluent-byoc.md) | Module and secrets built; the connect role is CC1 Task 1 |
| Per-tenant caps, rate limits and backpressure | [§10](10-operations.md) §3–§4; CP1 Task 6 (limits) | Partly built |

## 4. Internal: node to node

| What | Where it is decided | State |
|---|---|---|
| TLS on every listener; mTLS between nodes and to an external metastore | [§10](10-operations.md) §4 | Planned (M2) |
| Private network for internal routes, through a tailnet where needed | [§10](10-operations.md) §4, [§43](43-private-networking.md) | Proposed |
| Postgres: TLS on both hops (pg-control ↔ pg-runner, compute ↔ storage) | PG2 Task 23 | Planned |
| Postgres WAL service (`loams-wal`): TLS and auth on the Postgres and HTTP listeners | PG2 Task 38 (issue #148) | Planned |
| Engine ↔ House: TLS and authentication | HS1 Task 20 | In progress |
| Discovery and ownership: leases and fencing in the metastore and control store | [§18](18-metastore-backends-and-router.md); CP1 Task 2 (`loams-ctlstore`) | Built (metastore); planned (control store) |
| Events between components as CloudEvents | [§34](34-protocol-gateway-and-standards.md) | Built |

## 5. Hardening carried by existing plans

These items came out of the proposal and belong to plans that already exist, so they don't need a
plan of their own:

| Item | Owner |
|---|---|
| Fuzz every parser that reads untrusted bytes **before** authentication (TLS ClientHello and SNI, the Postgres startup message, SCRAM), with size and time limits before auth | The router (RT) and the gates; `loams-gateway`'s startup classifier and replay buffer (GW1 Task 14, D1020). Fuzzing already runs for the SQL gate and agentd RPC (CI) and for the Postgres fork (NF1 Task 6) |
| PROXY protocol v2 at the L4 load balancer, so client IPs survive to per-IP limits and audit | CP1 (the BYOC and cluster ingress); `loams-gateway` reads it (GW1 Task 12, Q901) |
| Short-lived internal mTLS certificates (hours), issued and rotated by the control plane | CP1, with NET1 for tailnet-reached nodes |

## 6. Exposure: public URLs for private deployments (parked)

A blind SNI tunnel would give a private deployment a public HTTPS URL:
- an edge reads only the TLS ClientHello and routes by SNI;
- an agent on the customer's side dials out and terminates TLS with keys it holds;
- certificates are wildcards per tenant.

OpenTunnel (`anomalyco/opentunnel`, 3 commits, dependent on a Cloudflare beta) is a design reference,
not a dependency.

It is parked for these reasons:
- **The engine never goes public** (the rule above). A tunnel could only ever expose things built on
  Loams, such as agent workspace previews or published apps, never an engine, admin or data endpoint.
- **No current need.** The tailnet plus the outbound agent cover today's cases.
- **The first real use is public preview URLs for agent workspaces**, beside the local preview proxy in
  `crates/loams-agentd-preview`. When that is wanted, try Tailscale Funnel or Cloudflare Tunnel first;
  build an edge only if they don't fit.
- **If it is ever built, keep these defaults from the proposal:**
  - TLS keys held on the customer side only;
  - wildcard certificates per tenant (fewer issuances, fewer hostnames in Certificate Transparency);
  - one fenced owner per tunnel, specified in TLA+;
  - a fuzzed ClientHello parser with hard limits before authentication;
  - an abuse process before the domain goes live.

Revisit after v0.1.0-preview.3 and DT1. Decision and question ids are allocated when it gets a design.
[§69](69-edge-reach.md) §10 (2026-10-20) records how a Loams-hosted relay would relate to it, and
proposes not to build one (D1317).
