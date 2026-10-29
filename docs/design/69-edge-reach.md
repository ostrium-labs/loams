# 69 — Edge Reach: Serverless Runtimes to a Private Loams

> **Amendment (approved 2026-10-23) by [§78](78-loams-cloud-on-tidb-x.md)** (Loams Cloud on TiDB X; D1700–D1735). D1317 (no hosted relay) stands; its rationale's hosting list gains the Loams Cloud control plane (D1701). Devices enrolled in Loams Cloud only dial out; device-to-device traffic uses the user's tailnet, `tsnet` or their own Tunnel (D1722).

Status: **Approved** · 2026-10-21. The owner approved the direction on 2026-10-20 and the design with every default on 2026-10-21.

Source: on 2026-10-20 the owner approved designing an "edge reach" path. It lets serverless runtimes call a self-hosted Loams without exposing the engine on the public internet.

Decisions **D1300–D1322** and open questions **Q1060–Q1075** are declared in the [decision log](13-decision-log.md):
- Every decision is *Approved (owner, 2026-10-21)*, and every question is answered with its default.
- **Part A of the never-public amendment (D1311) is approved:** callers off the tailnet reach Loams only through the gateway's edge listener, over a private link or a tunnel that authenticates every request first. The engine stays loopback, tailnet or gateway-only. The networking map, §43, §57 and §65 carry the amended wording.
- **Part B (D1312), the opt-in public edge profile, is deferred** per Q1060's default, and its requirements (D1313) with it. It is revisited after ER2. Until then the edge listener is never public.

Plan: [ER1](../plans/2026-10-20-er1-edge-reach.md) (approved; ER0 is specified task by task).

**Numbering.** §67 (PR #945) uses D1228–D1250 and Q1015–Q1029. Another design, on open-protocol compute, was being written at the same time and had no ids yet. To stay well clear of both, this document reserves **D1300–D1339** and **Q1060–Q1079** and takes the number 69, leaving 68 to that design.

This document builds on the designs below; §13 says where it amends each one.
- [§57](57-loams-gateway.md): `loams-gateway`, its `pg` module (GW1) and its planned `http`, `llm` and `mcp` modules (D1010–D1024). The edge listener is the first slice of the `http` module.
- [§43](43-private-networking.md) and the [networking map](networking-map.md): the tailnet, Cloudflare as the public front door for Ostrium's own sites (D581), the rule that the engine is never public, and the parked public URLs for private deployments (networking map §6).
- [§65](65-loams-operator.md) §11.4: exposure is `tailnet` or `internal`, and the gateway's Service is internal only (D1195, which amended D1019 through Q992).
- [§66](66-loams-mastra.md) §11.1: a Mastra server authenticates with an environment-scoped `lk_` key plus `LOAMS_NAMESPACE` (D1208).
- [§19](19-console-identity-and-agents.md) §5.2–§5.5 and [§44](44-unified-api-and-sdks.md) §7.4: workload-identity federation by RFC 8693 exchange, trust policies, Loams access tokens, and the SDKs' `TokenSource` (D449, D608).
- [§46](46-loams-postgres-production.md) §8.6: short-lived Postgres login roles by credential exchange (D711).
- D603: the compatibility surfaces stay unchanged, and the edge carries some of them (§6.3).

Markers, as in §64–§67:
- **(verify)** means the claim has not been checked against a primary source. The task that depends on it checks it first.
- **(estimate)** means the number was judged, not measured.
- **(unverified)** means a search did not settle it.

Vendor facts in §5 were read on 2026-10-20 from the source linked beside each one. A date after a link is the page's own last-updated or published date; "read 2026-10-20" means the page shows none.

---

## 1. Summary

### 1.1 The problem

Loams' standing rule is that the engine is never on the public internet: it is reached on loopback or over a tailnet (the networking map, §43, D1195). The rule is why a self-hosted Loams is safe to run with little operations work.

It also means the runtimes where TypeScript agents run cannot reach Loams:
- **Cloudflare Workers, Vercel Functions, AWS Lambda and Google Cloud Run** are not on the user's tailnet.
- Workers and Vercel cannot join a tailnet at all (§5.2).
- These are the runtimes Mastra and the AI SDK deploy to (§66), and the ones the partner decks pitch Loams for.

### 1.2 The answer

Loams gets **one door for the edge**: the edge listener of `loams-gateway`. Several **private paths** lead to that door, and the engine stays where it is.

```
 Worker ── Workers VPC binding (no public hostname) ───────────────┐
 Worker, Vercel, anything with fetch ── Access-protected hostname ─┤  Cloudflare checks a service token first
                                                                   ▼
                                    Cloudflare edge ◄── cloudflared (dials out from the user's stack)
                                                                   │
 Lambda, Cloud Run ── VPC: internal LB, PrivateLink, VPN ──┐        │
 Fly.io, Lambda, Cloud Run ── a tailnet node ──────────────┤        │
                                                           ▼        ▼
                          loams-gateway, edge listener: allow-listed routes, no admin,
                          checks the broker's assertion, limits, header hygiene
                                                           │  internal network only
                                                           ▼
                          engine, edge upstream listener: environment-bound credentials only
```

- **Cloudflare Workers:** Cloudflare Tunnel from the user's stack, reached by a **Workers VPC** binding (beta). Where the user does not want a beta, the Worker uses an Access-protected hostname with a service token (§8).
- **Vercel:** the Access-protected tunnel hostname with a service token. When Loams runs on AWS, Vercel's PrivateLink (Pro and Enterprise) or Secure Compute (Enterprise) instead. From ER2, the function's **Vercel OIDC** token is exchanged for a Loams token.
- **Lambda and Cloud Run:** when Loams runs in the same cloud, the cloud's private networking to an internal load balancer in front of the gateway (Lambda in a VPC, Cloud Run Direct VPC egress). Elsewhere, the tunnel hostname. From ER2, tokens from AWS or Google workload identity.
- **Fly.io:** a tailnet node in the Fly Machine, or the tunnel.
- **Two credentials on every request:**
  - the path admits the caller: a Cloudflare service token, a VPC, a tailnet;
  - Loams still checks its own credential: an **environment-bound `lk_` key** now (ER0), and a **short-lived Loams token exchanged from the runtime's workload identity** from ER2.
- **A public gateway profile was proposed and deferred** (D1312, owner 2026-10-21, per Q1060): only the edge listener on the internet, with mTLS or short-lived tokens. None of the defaults above needs it. It is revisited after ER2; until then the edge listener is never public.
- **No Loams-hosted relay** (D1317). Cloudflare Tunnel does the job, and a relay would make Ostrium an operator of customers' data traffic.

### 1.3 Decisions

| # | Decision | § | Status |
|---|---|---|---|
| D1300 | **Edge reach is designed** (owner, 2026-10-20): serverless runtimes reach a self-hosted Loams without the engine becoming public | 1.1 | Approved (owner, 2026-10-20 direction; 2026-10-21 design) |
| D1301 | **Reach classes define "public" precisely:** *loopback*, *tailnet*, *private link* (a cloud VPC, PrivateLink, a Workers VPC binding), *brokered* (a tunnel whose broker authenticates every request before it reaches the user's stack), and *public* (anyone on the internet can open a connection to a Loams process). Only *public* is public | 4.2 | Approved (owner, 2026-10-21) |
| D1302 | **One edge door:** off-tailnet traffic enters through the edge listener of `loams-gateway` only, never the engine's own ports. This holds on every edge path, including a serverless node that joined the tailnet | 6.1 | Approved (owner, 2026-10-21) |
| D1303 | **The edge listener** is the first slice of the `http` module. It speaks HTTP/1.1 and h2c, serves an allow-listed route set, has no admin routes, no console sessions and no Tailscale identity headers, cleans headers, and enforces body and time limits and per-credential rate limits. ER0 builds it on hyper behind a trait; the Pingora `http` module (D1013) takes it over later | 6.2 | Approved (owner, 2026-10-21) |
| D1304 | **The engine's edge upstream listener:** an engine listener whose only client is the gateway, on an internal network. It accepts only environment-bound credentials and refuses sessions, cookies, operator keys and org keys. It applies the allow-list again and marks each request `via=edge` for audit | 6.4 | Approved (owner, 2026-10-21) |
| D1305 | **The default path is Cloudflare Tunnel:** `cloudflared` runs in the user's stack and dials out, in the user's own Cloudflare account. For Workers, a Workers VPC binding is first, and an Access application with a service-token policy is the fallback | 6.5, 8 | Approved (owner, 2026-10-21) |
| D1306 | **Admission is per edge listener.** An `access` listener requires a valid `Cf-Access-Jwt-Assertion` for the configured team and application `aud`. A `network` listener admits what reached it: VPC Services, internal load balancers, tailnet nodes. A tunnel public hostname may target only an `access` listener, so a misconfigured Access application fails closed | 6.5 | Approved (owner, 2026-10-21) |
| D1307 | **Two credentials on every edge request:** admission by the path, and a Loams credential that the engine checks. ER0 accepts environment-bound `lk_` keys only | 7.1 | Approved (owner, 2026-10-21) |
| D1308 | **Workload-identity exchange** (ER2): the engine's token endpoint (`POST /oauth/token`, RFC 8693) exchanges a runtime's OIDC token for a Loams access token bound to one environment. A trust policy (issuer, audience, subject pattern) maps the caller to a service account and an environment. The token lives 15 minutes by default and 1 hour at most (§19 §5.3). The first issuers are Vercel, AWS outbound identity federation, Google, Fly.io, GitHub Actions and Kubernetes | 7.2 | Approved (owner, 2026-10-21) |
| D1309 | **What the SDKs need:** a `fetch` override (done for TypeScript in #947; it also carries a Workers VPC binding's `fetch`), extra headers for an Access service token, and token providers for workload identity (`vercelOidc`, `awsWebIdentity`, `gcpIdToken`, `flyOidc`, `githubActions`) behind one `exchange()` source | 7.3 | Approved (owner, 2026-10-21) |
| D1310 | **Postgres from the edge** goes only through the gateway's `pg` module, on a dedicated namespace port (D1016). Hyperdrive reaches it through a TCP VPC Service; other runtimes use a private link. Login roles are short-lived (D711) | 6.6 | Approved (owner, 2026-10-21) |
| D1311 | **The never-public rule, part A:** reword it for private-link and brokered paths to the gateway's edge listener (§9.1). The engine stays loopback, tailnet or gateway-only | 9.1 | Approved (owner, 2026-10-21) |
| D1312 | **The never-public rule, part B:** "the engine is never public; the gateway's edge listener may be, in an explicit opt-in edge profile" (§9.2) | 9.2 | **Deferred** (owner, 2026-10-21, per Q1060) |
| D1313 | **The public edge profile**, only if D1312 is approved: TLS 1.3, mTLS or short-lived tokens only (no static `lk_` keys), per-namespace scopes, rate limits, WAF-friendly, the same allow-list, no admin. The operator renders a public Service only for the edge port, and only on an explicit opt-in | 9.3 | Deferred with D1312 (owner, 2026-10-21) |
| D1314 | **Tailscale-native paths** are documented recipes where a runtime can run `tailscaled` in userspace (Lambda, Cloud Run, Fly.io), with ephemeral tagged nodes and workload identity federation. They are never offered for Workers or Vercel. Funnel stays off (`AllowFunnel: false`) and counts as *public* | 5.2, 8 | Approved (owner, 2026-10-21) |
| D1315 | **Cloud private networking** is a supported *private link* path to the gateway's internal load balancer: Lambda in a VPC with PrivateLink or VPN, Cloud Run Direct VPC egress, Vercel PrivateLink and Secure Compute. D1195 is unchanged | 5.3, 8 | Approved (owner, 2026-10-21) |
| D1316 | **Operator and compose:** the operator's `spec.exposure.mode` gains `tunnel`, which renders a `cloudflared` Deployment and no Service, so D1195 holds. Compose gains an opt-in `edge` profile with `loams-gateway` and `cloudflared` on internal networks | 6.7 | Approved (owner, 2026-10-21) |
| D1317 | **No Loams-hosted relay** now; Cloudflare Tunnel instead. Revisit on the criteria of §10 | 10 | Approved (owner, 2026-10-21) |
| D1318 | **Edge audit and metrics:** every edge request is audited with `via=edge`, its path class and the broker's subject. Gateway metrics carry no tenant label (D1021) | 6.8 | Approved (owner, 2026-10-21) |
| D1319 | **Tests:** a refusal suite, a local `workerd` → gateway → engine test on every relevant PR, and a nightly test on real Cloudflare: a deployed Worker reaches `cloudflared` on the runner through Workers VPC and through Access | 11 | Approved (owner, 2026-10-21) |
| D1320 | **Phases ER0–ER3** (§12); plan [ER1](../plans/2026-10-20-er1-edge-reach.md) specifies ER0 task by task | 12 | Approved (owner, 2026-10-21) |
| D1321 | **Not built:** a Tailscale client inside Workers or Vercel, a hosted relay, IP allow-lists as the only control, and durable (Resonate), Live admin, Git and Flight SQL over the edge in ER0 | 2 | Approved (owner, 2026-10-21) |
| D1322 | **The allow-list is data.** It is generated from a method option `(loams.options.v1.edge) = true` and the REST route map into `api/edge-allowlist.txt`, which the gateway and the engine both embed. A CI check fails when the file is stale | 6.3 | Approved (owner, 2026-10-21) |

### 1.4 Phases

| Phase | What | Gate | Effort (estimate) |
|---|---|---|---|
| **ER0** | The gateway's edge listener; the engine's edge upstream listener; the compose `edge` profile with `cloudflared`; Access-JWT admission; environment-bound `lk_` keys only; the refusal suite; the local `workerd` test; an SDK helper; a guide per runtime | GW1a merged (#663, done) | 3–4 engineer-weeks |
| **ER1** | The operator's `tunnel` exposure mode; the nightly real-Cloudflare test; Hyperdrive to the `pg` module; the Tailscale and cloud private-link recipes, tested by hand | ER0; OP1's gateway component | 2–3 engineer-weeks |
| **ER2** | Workload-identity exchange: the token endpoint, trust policies, Loams access tokens on the edge, and SDK token providers (TypeScript, then Python and Go) | ER0; §19 §5.3's signing key and JWKS | 4–5 engineer-weeks |
| **ER3** | Deferred with D1312 (owner, 2026-10-21); if it is approved after ER2, the public edge profile: mTLS and token-only admission, an external security review, the operator's opt-in public Service | Owner approval of D1312; ER2 | 3 engineer-weeks plus the review |

## 2. Scope

**In scope:** how a request from a serverless function reaches Loams' data plane, and how it is authenticated. That covers:
- the HTTP surfaces: Connect and gRPC-Web, the native REST, the Qdrant and Elasticsearch REST APIs, and MCP over streamable HTTP;
- Postgres through the gateway's `pg` module.

**Not in scope:**
- **Browsers.** The console reaches the engine over the tailnet (§44, the owner's "option A"). Edge reach is server to server, and the edge listener answers no CORS preflight.
- **Durable (Resonate), Live admin, Git and Flight SQL in ER0.** Durable is instance-wide and needs an operator or org key on shared deployments (§66 §7.5), and the edge refuses those keys (D1304). Live's app-scoped principals (D686) come with ER2.
- **Ostrium hosting anything new.** Ostrium still runs only `loams.dev`, `auth.loams.dev` and `console.loams.dev` (D1317).
- **A Tailscale client in Workers or Vercel** (§5.2).
- **IP allow-lists as the only control.** Serverless egress addresses are shared or dynamic (§5.3). Allow-lists stay an extra layer where a provider offers static IPs.
- **Commercial adapters and anything billing-grade** (D1021, D548). The edge counts requests for limits only.

## 3. Where things are today (checked on `dev` at `342179be3`, 2026-10-20)

| Piece | State |
|---|---|
| Engine HTTP surfaces | The main port serves Connect, gRPC, gRPC-Web, the native REST and the House proxy. Qdrant, Elasticsearch and Flight SQL have their own listeners. One `Auth` guards them all (`crates/loams/src/auth/mod.rs`) |
| Engine auth modes | `LOAMS_AUTH_MODE` is `apikey`, `oidc`, `tailscale`, a comma list of those, or `off` (loopback only). In every mode the engine also takes its own sessions (`ls_`) and `KeyService` keys (`lk_`). `oidc` accepts any configured issuer, including a Cloudflare Access team domain (`/cdn-cgi/access/certs`), but maps a token to a `User` principal on an allowlist, not to an environment |
| Engine wire protocols | Postgres, MySQL, native stream gRPC and durable refuse every address but loopback. Applications reach Postgres through Loams Postgres: PgDog behind the gateway |
| Namespace reach | A credential bound to an environment reaches only that environment's namespace (`crates/loams/src/api/namespace_reach.rs`). A service account's key reaches its own project's environments |
| Compose | Nothing binds a host port on `0.0.0.0`. The engine listens on loopback in a network namespace it shares only with the Tailscale sidecar, because its Tailscale listener (`127.0.0.1:7781`) trusts identity headers. `serve-https.json` sets `AllowFunnel: false` |
| `loams-gateway` | GW1 Tasks 1–7 merged (#663): the core and the `pg` module. There is no `http` module and no Pingora yet (D1013: the `http` plan adds it) |
| Operator | `spec.exposure.mode` is `tailnet` or `internal`. A CEL rule and a ValidatingAdmissionPolicy refuse public Services (D1195) |
| Workload-identity exchange | Designed (§19 §5.2 flow 1, D449, D608) but not built: no token endpoint, no trust policies, no Loams access tokens |
| TypeScript SDK | `TokenSource` with `apiKey`, `staticToken`, `envToken` and `refreshing` (`runtime/token-source.ts`), and `TransportOptions.headers`. Open PR #947 adds `TransportOptions.fetch` and sends `redirect: "manual"` automatically in workerd |
| Method options | `loams.options.v1` owns the extension numbers 50001–50099. `ModuleOptions module = 50001` (service) and `FacadeOptions facade = 50002` (method) are used |

## 4. Threat model

### 4.1 Assets and trust boundaries

**Assets:**
- namespace data: collections, tables, streams, graphs, Postgres databases;
- credentials: `lk_` keys, Loams tokens, Access service tokens, tunnel tokens, PgDog certificates;
- the control surfaces: admin RPCs, `KeyService`, first-run setup, Live admin;
- the engine's availability for its tailnet users.

**Trust boundaries:**
1. the runtime's provider network to the broker or private link;
2. the broker or link to `cloudflared` or the load balancer in the user's stack;
3. the gateway's edge listener;
4. the gateway to the engine's edge upstream listener;
5. the engine's authorizer.

### 4.2 Reach classes (D1301)

| Class | Who can open a connection to a Loams process | Examples | What may be reached this way |
|---|---|---|---|
| **Loopback** | Processes on the host | `loams dev` | Everything |
| **Tailnet** | Nodes the tailnet policy admits | Tailscale, Headscale | Everything the tailnet serves today (the console, operators); serverless nodes reach only the edge listener (D1302) |
| **Private link** | Workloads inside a network the user controls or has bound | Lambda in a VPC, Cloud Run Direct VPC egress, a Workers VPC binding, Vercel PrivateLink or Secure Compute | The gateway's edge listener only (D1302) |
| **Brokered** | Only requests a third-party broker has authenticated against the user's policy; the user's stack dials out | Cloudflare Tunnel behind an Access application | The gateway's edge listener only, which checks the broker's assertion (D1306) |
| **Public** | Anyone on the internet | A public load balancer, Tailscale Funnel, a relay on a public VPS | Nothing; the edge listener only if D1312, deferred on 2026-10-21, is approved later |

The brokered class is close to the tailnet in what it admits: a third party (Tailscale's coordination server, or Cloudflare Access) decides who may connect, under the user's policy. Unauthenticated requests stop at the broker and never reach a Loams process. The difference is that Cloudflare terminates TLS (T10).

### 4.3 Threats

| # | Threat | Boundary | Mitigation |
|---|---|---|---|
| T1 | An admin, setup or key-management RPC is called from the edge | 3, 4 | The allow-list is enforced at the gateway (D1303) **and** at the engine's edge upstream listener (D1304). The refusal suite calls every method in the descriptor set (D1319) |
| T2 | A leaked `lk_` key is used | 1–5 | Admission by the path is a second factor on every default path (D1307). The key reaches one environment. From ER2, 15-minute tokens replace stored keys. The audit names the broker subject (D1318) |
| T3 | A leaked Access service token | 1–2 | Useless without a Loams credential. The user rotates it in Cloudflare, which alerts before expiry. The guide's Access policy is Service Auth for that token only, with no Bypass or Everyone rule |
| T4 | A misconfigured Access application (Bypass, a wrong hostname) lets unauthenticated requests through the tunnel | 2–3 | A public hostname may target only an `access` listener, which refuses a request without a valid `Cf-Access-Jwt-Assertion` for the configured `aud` (D1306). A tunnel without Access fails closed |
| T5 | Spoofed identity headers: `Tailscale-User-Login`, `Cf-Access-Authenticated-User-Email`, `X-Forwarded-For`, `Loams-Via` | 3–4 | The edge listener strips them. The engine trusts no identity header from a client on the edge upstream listener. The client address comes from the broker's assertion or PROXY v2 (Q901), never from a header |
| T6 | `cloudflared` placed in the engine's network namespace reaches the Tailscale listener and forges identity | Compose | `cloudflared` runs in its own container on an `edge` network with the gateway only. A compose test asserts its networks (ER0 Task 5) |
| T7 | Denial of service of the engine through the edge | 3 | Rate and concurrency limits per credential and per broker subject; body and time limits; a cap on open streams. Cloudflare's own limits sit in front (§5.1) |
| T8 | Reach into another namespace | 5 | Only environment-bound credentials on the edge (D1304). `Loams-Namespace` must match the credential's environment, checked by the engine |
| T9 | A stolen tunnel token runs a rogue connector that receives the user's traffic | 2 | The token is mounted only into `cloudflared`. A rogue connector receives requests, not the engine's data. The credentials it captures reach one environment and, from ER2, expire in minutes. The runbook rotates the tunnel |
| T10 | A broker sees traffic in clear | 1–2 | On the brokered and Workers VPC paths, Cloudflare terminates the client's TLS; the user accepts this by choosing them, and the guides say so. Cloud private links keep traffic in the user's network. The public profile (if approved) runs TLS end to end to the gateway |
| T11 | A parser bug in the gateway before authentication | 3 | hyper's parsers; header and body limits before routing; the edge's routing and header code is fuzzed (networking map §5) |
| T12 | The public profile (if approved) widens T1, T2, T7 and T11 to the whole internet | 3 | §9.3: no static keys, mTLS or short-lived tokens only, a WAF in front recommended, an external review before release |

## 5. Options evaluated

### 5.1 Cloudflare Tunnel, Access service tokens and Workers VPC

**Tunnel**
- `cloudflared` connects outbound only, so all inbound traffic can be blocked ([Cloudflare Tunnel](https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/), 2026-08-04).
- It needs egress on port 7844: UDP for QUIC, TCP for HTTP/2 fallback ([firewall](https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/configure-tunnels/tunnel-with-firewall/), 2026-10-12).
- Each replica opens 4 connections to at least 2 data centres. Replicas are for availability only and are not load-balanced ([Tunnel](https://developers.cloudflare.com/tunnel/), 2026-09-11; [Workers VPC tunnel](https://developers.cloudflare.com/workers-vpc/configuration/tunnel/), 2026-04-22).
- "Available on all plans"; an account includes 1,000 tunnels ([Tunnel](https://developers.cloudflare.com/tunnel/), 2026-09-11; [changelog](https://developers.cloudflare.com/changelog/post/2026-10-16-tunnels-for-platforms-and-50k-routes/), 2026-10-16).
- A remotely managed tunnel runs from a token: `cloudflared tunnel run --token`, `TUNNEL_TOKEN` or `--token-file` ([run parameters](https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/configure-tunnels/run-parameters/), 2026-04-17).
- Native gRPC works over private subnet routing, not over a public hostname ([gRPC](https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/use-cases/grpc/), 2026-10-12). The SDKs' default transport (Connect and gRPC-Web over `fetch`) does not need native gRPC.

**Access service tokens**
- A non-browser client sends `CF-Access-Client-Id` and `CF-Access-Client-Secret` to an application whose policy action is Service Auth ([service tokens](https://developers.cloudflare.com/cloudflare-one/access-controls/service-credentials/service-tokens/), 2026-10-02).
- The token's duration is chosen at creation. Rotation keeps the old secret valid for 1 hour to 30 days, and an alert fires a week before expiry (same page).
- The origin receives `Cf-Access-Jwt-Assertion` and should validate it: keys at `https://<team>.cloudflareaccess.com/cdn-cgi/access/certs`, `aud` equal to the application's AUD tag ([validating the JWT](https://developers.cloudflare.com/cloudflare-one/access-controls/applications/http-apps/authorization-cookie/validating-json/), 2026-05-06). Signing keys rotate every 6 weeks, and old keys stay valid for 7 days (same page).
- mTLS policies exist, on Enterprise and pay-as-you-go Zero Trust plans only ([mTLS](https://developers.cloudflare.com/cloudflare-one/access-controls/service-credentials/mutual-tls-authentication/), 2026-10-12).
- **No Access selector accepts a third-party workload JWT** such as Vercel's or AWS's. "External Evaluation" calls the user's own API, which could check one ([policies](https://developers.cloudflare.com/cloudflare-one/access-controls/policies/), 2026-10-01). So workload identity is checked by Loams (D1308), not by Access.

**Workers to a tunnel**
- **A Worker can `fetch()` an Access-protected hostname with service-token headers;** Access evaluates the subrequest ([Workers and Access](https://developers.cloudflare.com/workers/configuration/cloudflare-access/), 2026-10-12).
- **The same-zone caveat:** without the `global_fetch_strictly_public` flag, a same-zone `fetch()` goes to the zone's origin and bypasses Cloudflare security settings ([compatibility flags](https://developers.cloudflare.com/workers/configuration/compatibility-flags/), 2026-09-18). The guide puts the Loams hostname on a zone with no Workers routes, or sets that flag. How Access behaves on the bypass path is (unverified).
- **Workers VPC (beta)** lets a Worker reach a service behind a tunnel with **no public hostname and no Access application**. It is free on all Workers plans while in beta, and "features and APIs may change before general availability" ([Workers VPC](https://developers.cloudflare.com/workers-vpc/), 2026-10-12; [open beta](https://blog.cloudflare.com/workers-vpc-open-beta/), 2025-11-05).
  - **`vpc_services`** binds one host and port behind a tunnel. The service type is `http` or `tcp` (`app_protocol` `postgresql` or `mysql`). TLS verification is `verify_full`, `verify_ca` or `disabled`. The URL's host only sets `Host` and SNI; the service config decides the destination ([VPC Services](https://developers.cloudflare.com/workers-vpc/configuration/vpc-services/), 2026-10-12). An account has up to 1,000 ([limits](https://developers.cloudflare.com/workers-vpc/platform/limits/), 2026-09-16).
  - **`vpc_networks`** binds a whole tunnel or Cloudflare Mesh, with `fetch()` to any address and, since 2026-06-16, plaintext `connect()` ([VPC Networks](https://developers.cloudflare.com/workers-vpc/configuration/vpc-networks/), 2026-10-15; [changelog](https://developers.cloudflare.com/changelog/post/2026-06-16-tcp-connect-vpc-networks/)). It is broader than one service, so Loams' guides use `vpc_services`.
  - A tunnel used only by Workers VPC needs no ingress rules ([Workers VPC tunnel](https://developers.cloudflare.com/workers-vpc/configuration/tunnel/), 2026-04-22).
- **Hyperdrive** reaches a private Postgres through a `tcp` VPC Service, with no Access application. The older path needs a TCP public hostname, an Access application and a service token, and requires TLS ([private database](https://developers.cloudflare.com/workers-vpc/examples/private-database/), 2026-04-30; [Hyperdrive](https://developers.cloudflare.com/hyperdrive/configuration/connect-to-private-database/), 2026-10-12). Hyperdrive supports only Postgres, MySQL and MariaDB ([supported databases](https://developers.cloudflare.com/hyperdrive/reference/supported-databases-and-features/), 2026-09-30).
- **Plain `connect()`** cannot reach private or Cloudflare addresses ([TCP sockets](https://developers.cloudflare.com/workers/runtime-apis/tcp-sockets/), 2026-10-13), so it cannot reach a tunnel.

**Worker identity and mTLS**
- **A Worker has no workload identity.** Cloudflare issues no JWT that identifies a Worker to third parties; `ctx.access.getIdentity()` describes the inbound caller only ([Workers and Access](https://developers.cloudflare.com/workers/configuration/cloudflare-access/), 2026-10-12). Asked for in the community; not a product (unverified).
- **The `mtls_certificates` binding** presents a client certificate on `env.CERT.fetch()`. It cannot be used towards a Cloudflare-proxied zone (520) ([mTLS binding](https://developers.cloudflare.com/workers/runtime-apis/bindings/mtls/), 2026-04-23), so it fits only the public profile with a DNS-only name.

**WAF:** the Free plan has custom rules and one rate-limiting rule (per IP, 10 s period); Pro and above get the managed rulesets ([WAF](https://developers.cloudflare.com/waf/), 2026-08-19).

**Verdict:**
- Tunnel plus Workers VPC is the best path for Workers. Nothing is public, the Worker holds a Loams credential only, and Hyperdrive covers Postgres. Its weakness is the beta label.
- Tunnel plus Access with a service token is GA and works from **any** runtime that can send two headers. It is the universal fallback and the default for Vercel.

### 5.2 Tailscale-native paths

- **tsnet** is a Go library that embeds a node in the program on a userspace network stack. It supports `Ephemeral`, `AdvertiseTags`, `ControlURL` (Headscale) and workload identity federation fields ([tsnet](https://tailscale.com/kb/1244/tsnet), 2026-07-24; [pkg.go.dev v1.104.1](https://pkg.go.dev/tailscale.com/tsnet), 2026-10-07).
- **Userspace `tailscaled`** offers a SOCKS5 proxy and an HTTP proxy for HTTP and HTTPS ([userspace networking](https://tailscale.com/kb/1112/userspace-networking), 2025-11-12).
- Tailscale publishes guides for:
  - **AWS Lambda:** a container image whose bootstrap starts `tailscaled` with SOCKS5 and an ephemeral key ([Lambda](https://tailscale.com/kb/1113/aws-lambda), 2026-01-05);
  - **Cloud Run** ([Cloud Run](https://tailscale.com/kb/1108/cloudrun), 2026-01-05);
  - **Fly.io** ([Fly.io](https://tailscale.com/kb/1132/flydotio), 2025-12-04).
- **Cold-start cost** of joining is not published (unverified). ER1 measures it.
- **Ephemeral nodes** are removed 30–60 minutes after they go idle ([ephemeral nodes](https://tailscale.com/kb/1111/ephemeral-nodes), 2025-12-04).
- **Pricing:** ephemeral use is billed in minutes: 1,000 a month on Personal and Standard, 10,000 on Premium. 50 tagged resources are included, and each extra one costs $1 a month ([pricing](https://tailscale.com/pricing), read 2026-10-20).
- **Workers: not feasible.** No guide or project runs a Tailscale node inside a Worker. Tailscale's browser client (`@tailscale/connect`, Go to WASM, DERP over WebSocket) shows that a relay-only client is possible in principle. Running it under a Worker's CPU, memory and state limits is (unverified) and is not pursued (D1321).
- **Vercel: no guide.** Vercel's "Tailscale" connector fetches Tailscale credentials at runtime; it does not join the function to a tailnet ([Vercel Connect](https://vercel.com/connect/tailscale), read 2026-10-20). Running userspace `tailscaled` in a Vercel Node function is (unverified).
- **Funnel** publishes a local service on ports 443, 8443 or 10000 under `<tailnet>.ts.net`, with TLS terminated on the node. Bandwidth limits are not configurable. It is beta and needs the `funnel` node attribute ([Funnel](https://tailscale.com/kb/1223/funnel), 2026-01-20; [CLI](https://tailscale.com/kb/1311/tailscale-funnel), 2026-01-26). Headscale does not support Funnel ([headscale#1040](https://github.com/juanfont/headscale/issues/1040), closed 2022-12; [features](https://headscale.net/stable/about/features/), read 2026-10-20). **Funnel is public** by D1301, so it is not used.
- **Workload identity federation** is GA since 2026-02-19 on all plans. A node joins with an OIDC token instead of a stored auth key: `tailscale up --client-id=… --id-token=… --advertise-tags=…` (v1.90.1 or later), or tsnet's fields. Built-in issuers are AWS, Google Cloud, Azure and GitHub Actions, and any public OIDC issuer can be added as a custom issuer ([workload identity federation](https://tailscale.com/kb/1581/workload-identity-federation), 2026-01-30; [GA](https://tailscale.com/blog/workload-identity-ga), 2026-02-19). Vercel's and Fly's issuers as custom issuers are (unverified). Headscale has no equivalent (unverified).
- **Identity:** `tailscale serve`'s user headers are not set for tagged devices ([serve](https://tailscale.com/kb/1312/serve), 2026-01-20). WhoIs returns the node's name and tags ([apitype](https://pkg.go.dev/tailscale.com/client/tailscale/apitype), 2026-10-07). A serverless node is therefore authorised by its tag, and the Loams credential is still required (D1307).
- **App connectors** are for egress, so they do not help here. **Peer relays** could cut latency for runtimes with UDP ([app connectors](https://tailscale.com/kb/1281/app-connectors), 2026-10-07; [peer relays](https://tailscale.com/kb/1591/peer-relays), 2026-02-04).

**Verdict:**
- Tailscale is a good fit for **Fly.io** (long-lived VMs) and a workable recipe for **Lambda and Cloud Run**, at the cost of a cold join and ephemeral-minute billing.
- It cannot serve **Workers or Vercel**.
- Funnel would make the stack public.

### 5.3 Cloud private networking

**Vercel**
- **Secure Compute** is Enterprise only, at a custom price. It gives a dedicated VPC with static egress IPs, AWS VPC peering (up to 50 peerings) and site-to-site VPN. It covers Node, Python, Go and Ruby functions, not the Edge runtime ([Secure Compute](https://vercel.com/docs/networking/secure-compute), 2026-09-23). Self-serve for Enterprise since 2026-01-07; the same post says a Pro offering is planned ([changelog](https://vercel.com/changelog/secure-compute-is-now-self-serve), 2026-01-07).
- **Static IPs** (Pro and Enterprise, 2025-10-02) are egress only, from a VPC shared with a few other customers, with no peering. They cost $100 a month per project ([Static IPs](https://vercel.com/docs/networking/static-ips), 2026-06-30; [changelog](https://vercel.com/changelog/static-ips-are-now-available-for-more-secure-connectivity), 2025-10-02). Shared addresses make an allow-list weak, and Vercel itself says the IPs alone are not enough.
- **AWS PrivateLink** has been on Pro and Enterprise since 2026-09-01. It reaches internal services behind an AWS Network Load Balancer through an interface endpoint, and the provider allow-lists a dedicated IAM role. The first connection is included, each further one costs $30 a month, and data is $0.04/GB ([PrivateLink](https://vercel.com/docs/networking/privatelink), 2026-09-15; [changelog](https://vercel.com/changelog/aws-privatelink-is-now-available-on-pro-and-enterprise), 2026-09-01). The price of the "Advanced Networking" product it belongs to is (unverified).
- **Without these, Vercel's egress addresses are dynamic** and unpublished ([fixed IP](https://vercel.com/kb/guide/can-i-get-a-fixed-ip-address), 2026-09-28).
- **The Edge runtime** has only `fetch`. Vercel recommends moving to Node.js, and Next.js 16.3 no longer supports `runtime = 'edge'` ([Edge runtime](https://vercel.com/docs/functions/runtimes/edge), 2026-08-03). Node functions open TCP connections (pools for `pg` and others: [connection pools](https://vercel.com/kb/guide/efficiently-manage-database-connection-pools-with-fluid-compute), 2026-06-17).

**AWS Lambda**
- **A function in a VPC** uses shared Hyperplane ENIs, created when the function is configured. It stays Pending until they are ready. After 14 idle days the function goes Inactive, and the next invoke fails while it returns to Pending. It needs NAT to reach the internet ([Lambda VPC](https://docs.aws.amazon.com/lambda/latest/dg/configuration-vpc.html), read 2026-10-20). The 2019 change removed the per-invoke cold-start penalty ([AWS blog](https://aws.amazon.com/blogs/compute/announcing-improved-vpc-networking-for-aws-lambda-functions), 2019-09-03).
- **PrivateLink** puts an NLB in front of the service as an endpoint service, allowed for specific principals ([PrivateLink](https://docs.aws.amazon.com/vpc/latest/privatelink/privatelink-share-your-services.html), read 2026-10-20).
- Transit Gateway or VPN from a Lambda VPC follows the VPC's routes (unverified, inference).

**Google Cloud Run**
- **Direct VPC egress** is recommended over Serverless VPC Access connectors; both are GA ([connecting to a VPC](https://docs.cloud.google.com/run/docs/configuring/connecting-vpc), 2026-10-13).
- Direct egress can add a minute or more at instance start, and 30 s or more with Cloud NAT ([Direct VPC](https://docs.cloud.google.com/run/docs/configuring/vpc-direct-vpc), 2026-10-20). On-premises reach over VPN or Interconnect follows the VPC's routes (unverified, inference).

**Fly.io**
- **6PN** joins an organisation's apps over WireGuard, and custom private networks isolate apps within an organisation ([private networking](https://docs.fly.io/networking/private-networking), read 2026-10-20).
- `fly wireguard create` lets an external network reach the organisation's apps (same page). Whether a Fly app can dial a service on that peer is (verify), so the guide uses Tailscale or the tunnel.

**Verdict:** when Loams runs in the same cloud as the function, private networking is the strongest path. Nothing leaves the user's network, and the gateway keeps its internal load balancer (D1195). It is not the default for the preview's typical install, compose on a VM of the user's choosing, because it needs the stack in that cloud.

### 5.4 A hardened public gateway profile

Only the edge listener is on the internet, and the engine stays private behind it. What it would take is in §9.3.

**What it gives:**
- any runtime, with no broker and no Cloudflare account;
- TLS end to end to the gateway;
- the lowest latency;
- the only path for a Workers mTLS binding (§5.1).

**What it costs:**
- the gateway's pre-auth surface becomes internet-wide;
- scanners can find and fingerprint Loams installations;
- every user becomes responsible for an internet-facing service;
- D1195's admission policy needs an exception.

Every runtime in §8 can use the brokered path instead: any runtime that can send two headers can use Access. So the profile's value is convenience and independence from Cloudflare, not reach. The owner deferred it on 2026-10-21 (§9.2).

### 5.5 A Loams-hosted relay

See §10.

### 5.6 Comparison

| | Tunnel + Workers VPC | Tunnel + Access | Cloud private link | Tailscale in the function | Public edge profile | Loams-hosted relay |
|---|---|---|---|---|---|---|
| Reach class | Private link | Brokered | Private link | Tailnet | Public | Brokered (by Ostrium) |
| Workers | Yes (beta) | Yes | No | No | Yes | Yes |
| Vercel | No | Yes | PrivateLink (Pro+), Secure Compute (Enterprise) | No | Yes | Yes |
| Lambda | No | Yes | Yes | Recipe | Yes | Yes |
| Cloud Run | No | Yes | Yes | Recipe | Yes | Yes |
| Fly.io | No | Yes | No | Yes | Yes | Yes |
| Postgres | Hyperdrive | Hyperdrive (legacy path) | Yes | SOCKS only | Yes | Yes |
| Who sees plaintext | Cloudflare | Cloudflare | Nobody | Nobody | Nobody | Ostrium, unless SNI-blind |
| Extra cost to the user | Free in beta | Free tier (verify for service tokens) | Cloud and Vercel prices | Ephemeral minutes | None | Ostrium's price |
| Ostrium operates | Nothing | Nothing | Nothing | Nothing | Nothing | A global relay |

## 6. The design

### 6.1 One door (D1302)

Every edge path ends at the gateway's edge listener, including the tailnet and private-link ones. That gives one allow-list, one place for limits and one audit point. The engine's main port, its Tailscale listener and its compatibility listeners are never the target of a tunnel, a VPC load balancer or Funnel.

### 6.2 The edge listener (D1303)

- **A listener of `loams-gateway`.** In compose it is on an internal network; on Kubernetes, on the pod network behind a `ClusterIP` or internal load balancer.
  - It speaks HTTP/1.1 and h2c from `cloudflared`.
  - It terminates TLS when the path ends at the gateway itself: an internal load balancer with TLS passthrough, a tailnet node, or the public profile.
  - The default SDK transport (Connect and gRPC-Web over `fetch`) works over HTTP/1.1, which is what Workers and the tunnel carry.
- **On hyper in ER0.** hyper is already in the workspace for GW1's admin listener, and the listener sits behind an `EdgeProxy` trait. When the `http` module arrives on Pingora (D1013), the edge becomes one of its route sets and the hyper version is removed. GW1's "no Pingora yet" rule holds (Q1062).
- **Admission per listener** (D1306): `access` or `network`.
- **Route allow-list** (§6.3). It is matched on the exact path before any body is read. Anything else gets a `404` with no body naming a Loams service.
- **Header hygiene:**
  - It strips `Tailscale-*`, `Cf-Access-Authenticated-User-Email`, `Forwarded`, `X-Forwarded-*`, `Cookie`, `Loams-Via`, `Loams-Edge-*` and hop-by-hop headers.
  - It adds `Loams-Via: edge`, the admission listener's name and the verified broker subject on the upstream request.
  - It forwards `CF-Access-Client-*` to no one.
- **Limits** (defaults in Q1068, estimates):
  - 10 s to receive the request head;
  - an 8 MiB request body (the Elasticsearch `_bulk` route has its own cap);
  - 60 s for a unary call;
  - a cap on open server streams per credential;
  - token-bucket rate limits per credential and per broker subject, held in the process.
- **Credential presence:** a request with no `Authorization`, Qdrant `api-key` or Elasticsearch `ApiKey` credential is refused with `401` at the gateway, without reaching the engine. The token endpoint (ER2) is the one exception. The engine does the real check (D1304).
- **No admin and no probes on this listener.** `/healthz` and `/readyz` stay on the admin listener (§57 §5).

### 6.3 The route allow-list (D1322)

| Surface | Allowed on the edge | Refused |
|---|---|---|
| Connect and gRPC-Web (`/loams.<pkg>.v1.<Service>/<Method>`) | Methods marked `(loams.options.v1.edge) = true`: the data-plane calls an environment-bound key may make. That covers collections and documents, search and query, SQL query, stream produce and fetch, graph query, Memory, and the app modules of #884 | `loams.admin.v1` (keys, namespaces, members), `loams.auth.v1` (setup, sessions), `loams.internal.v1`, `LiveAdminService`, `loams.instance.v1` except `GetInstance`, House administration |
| Native REST | The routes that map to allowed methods in `docs/api/route-map.md` | Everything else |
| Qdrant REST (D603) | Points, search, query, reading collections; creating and deleting a collection (Q1066) | Snapshots, cluster, telemetry, service routes |
| Elasticsearch REST (D603) | `_search`, `_msearch`, the document APIs, `_bulk`, index create and delete (Q1066), `_cat/indices` | `_cluster`, `_nodes`, other `_cat` routes, index settings writes |
| MCP (D603), streamable HTTP | `loams-mcp`'s tools for the credential's environment | — |
| `POST /oauth/token` | From ER2: the token exchange (§7.2), unauthenticated by design, with its own lower rate limit | — |
| Postgres | Through the `pg` module, not this listener (§6.6) | — |
| Resonate, Live admin, Git, Flight SQL | Not in ER0 (D1321) | All |

- **Generated, not hand-kept.** A new method option `bool edge = 50003` in `loams.options.v1` marks methods. `scripts/api/gen-edge-allowlist` combines the descriptor set with the REST, Qdrant and Elasticsearch route tables into `api/edge-allowlist.txt`. The gateway and the engine both embed the file, and CI fails when it is stale.
- **Applied twice.** The engine's edge upstream listener applies the same list (D1304), so a gateway bug alone opens no route.
- **New methods are off the edge by default:** a method is on the edge only when someone marks it.

### 6.4 The engine's edge upstream listener (D1304)

- **Where it listens:** `--edge-upstream-listen=<addr>` (`LOAMS_EDGE_UPSTREAM_LISTEN`).
  - In compose, it is on the `edge-link` network, which only the gateway joins.
  - On Kubernetes, a NetworkPolicy admits only the gateway's pods, with mTLS from the operator's CA (§65 §11.3).
  - In ER0 the gateway also presents a link key (Q1065).
- **What the engine refuses on it:**
  - no credential;
  - `ls_` sessions and the session cookie;
  - keys from `LOAMS_API_KEYS_FILE` (operator keys);
  - org-wide keys, and owner or admin keys;
  - any key or token not bound to exactly one environment;
  - a `Loams-Namespace` other than that environment;
  - Tailscale identity headers, which are never honoured here.
- **What it accepts:** an environment-bound `lk_` key (ER0), or a Loams access token whose `env` claim is that environment (ER2). The normal authorizer then runs.
- **Audit:** it records `via=edge`, the gateway's `Loams-Edge-Listener` and `Loams-Edge-Subject`, and the path class (D1318). It trusts those headers only on this listener, and only from an authenticated gateway.

### 6.5 Cloudflare Tunnel in the user's stack (D1305, D1306)

All of it is in **the user's own Cloudflare account**. Ostrium holds no tunnel, no Access application and no token (D1317).

**Workers VPC (the Workers default)**
1. Create a tunnel with no public hostname and run `cloudflared` in the stack (§6.7).
2. Create a VPC Service of type `http` that targets the gateway's `network` edge listener (`loams-gateway:8444`) through the tunnel.
3. Bind it in `wrangler.jsonc` and pass the binding's `fetch` to the SDK:
   ```jsonc
   { "vpc_services": [{ "binding": "LOAMS", "service_id": "<id>", "remote": true }] }
   ```
   ```ts
   const loams = new Loams({
     endpoint: 'http://loams.internal',                // sets Host only; the service config routes
     auth: apiKey(env.LOAMS_API_KEY),                  // an environment-bound lk_ key, a Worker secret
     transport: { fetch: (input, init) => env.LOAMS.fetch(input, init) },
   });
   ```
   - Traffic from the tunnel to the gateway stays on the compose network.
   - The edge between the Worker and Cloudflare is Cloudflare's.
   - The VPC Service's TLS mode is `disabled` towards the gateway's plaintext internal listener. With `verify_ca`, the gateway's edge certificate is used (Q1074).

**Access hostname (the fallback, and the Vercel default)**
1. Give the same tunnel a public hostname on a zone the user owns, pointed at the gateway's `access` edge listener (`loams-gateway:8443`).
2. Create a self-hosted Access application for that hostname with one Service Auth policy for one service token.
3. Copy the application's AUD tag into the gateway config.
4. The function sends `CF-Access-Client-Id` and `CF-Access-Client-Secret` (through `accessServiceToken()`, §7.3) plus its Loams credential.

The gateway verifies `Cf-Access-Jwt-Assertion`:
- RS256 against the team's `certs` endpoint, cached and refetched on an unknown `kid`;
- `aud` must be the configured tag;
- `iss` must be the team domain;
- `exp` must not have passed.

It forwards the token's service-token identity (`common_name`, (verify) the claim name for service tokens) as `Loams-Edge-Subject`.

**Why two listeners.** The Workers VPC path carries no Access assertion, so its listener cannot require one. If a public hostname could reach that listener, a missing Access policy would open it. So the two are separate ports:
- the guide's tunnel configuration maps public hostnames only to `:8443`;
- the gateway refuses, on `:8444`, any request whose `Host` is a configured public name;
- `scripts/edge/check-tunnel-config` reads the tunnel's ingress through the Cloudflare API and fails if a hostname points at `:8444` (ER1).

### 6.6 Postgres from the edge (D1310)

- **Workers:** Hyperdrive on a `tcp` VPC Service (`app_protocol: postgresql`) targeting the gateway's `pg` module on the namespace's **dedicated port** (D1016). The port alone picks the namespace, so Hyperdrive's SNI does not matter.
  - Hyperdrive negotiates TLS with `SSLRequest`. The gateway splices it to PgDog (D1014), and PgDog's certificate is verified with Hyperdrive's `verify_ca` and an uploaded CA ((verify) Hyperdrive's custom-CA option).
  - Plaintext is refused (D1015), so `vpc_networks.connect()`, which is plaintext only, cannot be used.
- **Vercel Node, Lambda, Cloud Run, Fly.io:** the `pg` driver connects over the private link or tailnet to the gateway's SNI port (`<ns>.<region>.pg.<base>`) or a dedicated port. The Access path does not carry Postgres: a TCP public hostname needs `cloudflared` on the client.
- **Credentials:** a Postgres role and password from `pg-control`'s connection string (§66 §11.1) in ER0. From ER2, the function exchanges its Loams token for a `VALID UNTIL` login role through `IssueConnectCredential` (D711), so no long-lived database password sits in the function.
- **Pools:** Hyperdrive and Vercel's `attachDatabasePool` pool on the client side, and PgDog pools behind the gateway. The guides size the client pool small (Q1068).

### 6.7 Configuration surface (D1316)

**Compose.** `deploy/release/compose.edge.yaml`, profile `edge`; nothing starts without the profile:

```yaml
services:
  loams-gateway:
    profiles: [edge]
    image: ${LOAMS_REGISTRY:-ghcr.io/ostrium-labs}/loams-gateway:${LOAMS_VERSION}
    command: [--config=/etc/loams-gateway/gateway.toml]
    networks: [edge, edge-link]          # never the engine's network namespace, never `default`
  cloudflared:
    profiles: [edge]
    image: docker.io/cloudflare/cloudflared:${CLOUDFLARED_TAG}@${CLOUDFLARED_DIGEST}
    command: [tunnel, --no-autoupdate, run, --token-file, /run/secrets/cloudflared-token]   # (verify) flag order
    networks: [edge]
  loams:
    networks: [edge-link]                # added to its existing networks
networks:
  edge: {}                               # cloudflared dials out from here; nothing is published
  edge-link: { internal: true }          # gateway and engine only
```

- The engine adds `--edge-upstream-listen` on its `edge-link` address.
- The Tailscale sidecar arrangement is unchanged.
- The file has no `ports:` (test `compose_edge_has_no_ports`), and `cloudflared` is never on `edge-link` (test `cloudflared_networks_are_edge_only`).

**Gateway config:**

```toml
[[edge.listeners]]
name = "access"
listen = "0.0.0.0:8443"
admission = "access"                     # requires Cf-Access-Jwt-Assertion
access.team_domain = "https://<team>.cloudflareaccess.com"
access.aud = ["<application AUD tag>"]

[[edge.listeners]]
name = "network"
listen = "0.0.0.0:8444"
admission = "network"                    # Workers VPC, internal LB, tailnet
refuse_hosts = ["loams.example.com"]     # the Access hostname, never on this port

[edge]
upstream = "http://loams:8090"           # the engine's edge upstream listener
link_key_file = "/run/secrets/edge-link-key"
max_body_bytes = 8388608
[edge.limits]
per_credential_rps = 50
per_credential_burst = 100
per_credential_streams = 8
```

**Operator.**
- `spec.exposure.mode: tunnel` renders:
  - a `cloudflared` Deployment: two replicas, the tunnel token from a referenced Secret, egress only to Cloudflare's 7844 and the gateway's edge Service, and no Service of its own;
  - the gateway's edge listeners as a `ClusterIP` Service.
- D1195's CEL rule and admission policy are unchanged, because `tunnel` creates no public Service.
- `internal` also renders the edge listeners, on the internal load balancer, for VPC callers.
- An `edge-public` mode exists only after D1312 (§9.3).

### 6.8 Audit and metrics (D1318)

- **Audit:** `via=edge`; `path_class` (`brokered`, `private_link`, `tailnet`, `public`); the broker subject (an Access service token's client id, the VPC listener's name, a tailnet node's tag); the Loams principal; the route.
- **Metrics:** `loams_gateway_edge_requests_total{listener,route_class,outcome}`, `loams_gateway_edge_refusals_total{listener,reason}` and `loams_gateway_edge_active_streams{listener}`. No tenant or namespace label (D1021).

## 7. Authentication

### 7.1 Two credentials (D1307)

| Path | Admission (layer 1) | Loams credential (layer 2), ER0 | ER2 |
|---|---|---|---|
| Workers VPC | Only Workers bound to the VPC Service in the user's account | `lk_` environment key, a Worker secret | Same: Workers have no workload identity (§5.1) |
| Access hostname | Access service token, checked by Cloudflare and again by the gateway (D1306) | `lk_` environment key | A Loams token from the runtime's OIDC token (Vercel, AWS, Google, Fly.io); Workers keep the key |
| Cloud private link | The VPC, the security group, PrivateLink's allowed principals | `lk_` environment key | A Loams token from AWS or Google workload identity |
| Tailnet node | Tailnet policy on the node's tag | `lk_` environment key | A Loams token; the node itself joins with Tailscale's workload identity federation |
| Public profile (if approved) | None at the network; mTLS at the gateway | Not allowed | A Loams token only, or mTLS plus a token |

- **Layer 2 is never skipped.** The broker's assertion is recorded, not trusted as a Loams identity.
- Mapping an Access service token directly to an environment, so that it is the only credential, is Q1064. The default is no.

### 7.2 Workload-identity exchange (D1308)

This builds §19 §5.2's flow 1 for the edge.

- **The endpoint:** `POST /oauth/token` on the engine, reachable through the edge listeners. It takes `grant_type=urn:ietf:params:oauth:grant-type:token-exchange`, `subject_token` (the runtime's OIDC JWT), `subject_token_type=urn:ietf:params:oauth:token-type:jwt` and `audience` (the instance's edge audience, `loams:<instance-id>`).
- **Trust policies** belong to a service account. They are created through an admin RPC, which is never on the edge, and kept in the metastore:

  ```toml
  issuer   = "https://oidc.vercel.com/acme"                      # exact; discovery and JWKS fetched from it
  audience = "loams:01J9Z8Y7X6"                                  # must equal the token's aud
  subject  = "owner:acme:project:support-agent:environment:production"   # exact or a trailing *
  claims   = { }                                                 # optional exact-match claims
  environment = "support-agent/production"
  ttl = "15m"                                                    # at most 1h
  ```

- **Verification:**
  - The issuer must be configured exactly, and the JWKS is cached as `oidc.rs` does today.
  - Only RS256, ES256, ES384 and EdDSA are accepted, with `alg` matched to the key.
  - `exp`, `nbf` and `iat` are checked; a subject token older than its issuer's documented maximum is refused.
  - A `jti` replay cache holds tokens for their lifetime.
- **The result:** a Loams access token as in §19 §5.3. It is Ed25519-signed and published at `/.well-known/jwks.json`, carries `sub` (the service account), `env` (one environment), `scp`, `exp` and `jti`, and is **never longer-lived than the subject token**.

**Issuers at ER2:**

| Runtime | Subject token | Issuer and subject |
|---|---|---|
| Vercel | `getVercelOidcToken({ audience })` from `@vercel/oidc` (4.0 or later reads the request context first); function tokens last 2 hours ([OIDC reference](https://vercel.com/docs/oidc/reference), 2026-09-17) | `https://oidc.vercel.com/<team>` (team mode); `sub` = `owner:<team>:project:<project>:environment:<env>` ([OIDC](https://vercel.com/docs/oidc), 2026-08-04) |
| AWS (Lambda, ECS, EC2) | `sts:GetWebIdentityToken` with `Audience`, 60–3600 s (default 300), after the account enables outbound identity federation ([announcement](https://aws.amazon.com/blogs/aws/simplify-access-to-external-services-using-aws-iam-outbound-identity-federation), 2025-11-19; [API](https://docs.aws.amazon.com/STS/latest/APIReference/API_GetWebIdentityToken.html), read 2026-10-20) | `https://<uuid>.tokens.sts.global.api.aws`; `sub` = the role ARN, plus `aws_account` in a namespaced claim. The older signed `GetCallerIdentity` pattern ([Vault AWS auth](https://developer.hashicorp.com/vault/docs/auth/aws), read 2026-10-20) is Q1072 |
| Google (Cloud Run, GKE) | The metadata server's `identity?audience=…`; valid for one hour ([service identity](https://docs.cloud.google.com/run/docs/securing/service-identity), 2026-10-13; [token types](https://docs.cloud.google.com/docs/authentication/token-types), 2026-10-13) | `https://accounts.google.com`; `sub` = the service account's numeric id, with `email` matched as a claim |
| Fly.io | `POST /v1/tokens/oidc` on `/.fly/api` with `aud` ([OIDC](https://docs.fly.io/security/openid-connect), read 2026-10-20) | `https://oidc.fly.io/<org>`; `sub` = `<org>:<app>:<machine>`, matched with a trailing `*` |
| GitHub Actions | The Actions OIDC token with `audience` | `https://token.actions.githubusercontent.com`; `sub` = `repo:<owner>/<repo>:…` |
| Kubernetes | A projected service-account token | The cluster's issuer |

**Workers** have no subject token. They keep an `lk_` key in Workers secrets. Two later options exist:
- an `mtls_certificates` client certificate on the public profile;
- Q1064's service-token mapping.

### 7.3 What the SDKs need (D1309)

**TypeScript (`@loams/client`)**
- **`transport.fetch`** (PR #947) is the hook for:
  - a Workers VPC binding: `env.LOAMS.fetch`;
  - Vercel and Node proxies, such as undici's `ProxyAgent` for Tailscale's HTTP proxy on Lambda;
  - instrumentation.

  No new transport is needed.
- **`accessServiceToken({ clientId, clientSecret })`** returns the two headers for `transport.headers`. By default it reads `CF_ACCESS_CLIENT_ID` and `CF_ACCESS_CLIENT_SECRET`. ER0 Task 10 adds it.
- **`exchange({ endpoint, subjectToken, audience })`** (ER2) is a `TokenSource` built on `refreshing()`:
  - it refreshes 60 s before `exp` as well as on `token_expired`;
  - one exchange is in flight per source.
- **Subject-token providers** (ER2) live in a subpath `@loams/client/identity`, with optional peer dependencies, so the core stays dependency-free and Workers bundles do not grow:
  - `vercelOidc({ audience })` uses `@vercel/oidc`;
  - `awsWebIdentity({ audience, region })` uses `@aws-sdk/client-sts`;
  - `gcpIdToken({ audience })` calls the metadata server with plain `fetch`;
  - `flyOidc({ audience })` is Node only (a Unix socket);
  - `githubActions({ audience })` reads `ACTIONS_ID_TOKEN_REQUEST_*`.
- **The Postgres helper** `pgConnectCredential()` (ER2) wraps `IssueConnectCredential` for `pg`'s `password` callback.

**`@loams/mastra`** (MA2) passes `transport` and `auth` through unchanged. Its docs gain an edge section; no Mastra-specific code is needed.

**Python and Go** (ER2, after TypeScript) need:
- the header helper;
- `exchange()`;
- providers for AWS, Google, Fly.io and GitHub Actions.

They are the languages used on Lambda, Cloud Run and Fly. The other SDKs need only the static `headers` option, which they already have through D608's transport options (verify per SDK in ER2 Task 0).

**Environment names:**
- `LOAMS_ENDPOINT`, `LOAMS_API_KEY` and `LOAMS_NAMESPACE` stay as they are (D1208).
- `CF_ACCESS_CLIENT_ID` and `CF_ACCESS_CLIENT_SECRET` are added.
- `LOAMS_TOKEN_EXCHANGE=vercel|aws|gcp|fly|github` makes `envToken()` pick a provider (ER2).

## 8. Per-runtime recommendations

| Runtime | Default | Alternatives | Credential now (ER0) | Credential from ER2 | Postgres |
|---|---|---|---|---|---|
| **Cloudflare Workers** | Tunnel + **Workers VPC** `vpc_services` binding, passed as `transport.fetch` | Access hostname + service token (GA, no beta); the public profile with an `mtls_certificates` binding (only if D1312) | `lk_` environment key as a Worker secret | Unchanged; Workers have no workload identity | Hyperdrive on a `tcp` VPC Service to a dedicated port |
| **Vercel Functions** (Node; Edge runtime the same, `fetch` only) | Tunnel + **Access hostname** with a service token in Vercel's sensitive environment variables | **PrivateLink** (Pro and Enterprise) to an NLB in front of the gateway when Loams runs on AWS; **Secure Compute** (Enterprise) with VPC peering or VPN | `lk_` environment key | **Vercel OIDC** → Loams token; the service token stays as layer 1 | Node only: over PrivateLink or Secure Compute; otherwise not from Vercel in ER0 |
| **AWS Lambda** | Loams on AWS: **Lambda in a VPC** → internal NLB (or a PrivateLink endpoint service) → the gateway. Elsewhere: **Access hostname** | Tailscale recipe: container image, userspace `tailscaled`, HTTP proxy, tagged ephemeral node joined by workload identity federation | `lk_` environment key from Secrets Manager | **`GetWebIdentityToken`** → Loams token | Over the VPC to the gateway's SNI or dedicated port |
| **Google Cloud Run** | Loams on GCP: **Direct VPC egress** → internal load balancer → the gateway. Elsewhere: **Access hostname** | Tailscale recipe (Tailscale's own guide) | `lk_` environment key from Secret Manager | **Metadata ID token** → Loams token | Over the VPC |
| **Fly.io** | **Tailscale in the Machine** (Tailscale's Fly guide), joined to the user's tailnet as a tagged node, reaching the gateway's `network` listener | Access hostname | `lk_` environment key as a Fly secret | **Fly OIDC** → Loams token; the node joins by workload identity federation where Fly's issuer works as a custom issuer (unverified) | Over the tailnet |
| Others with only `fetch` (Netlify, Supabase Edge Functions; Deno Deploy removed by D1790, as it shuts down) | **Access hostname** | — | `lk_` environment key | Their OIDC, where they issue one | Not in ER0 |

**One stack can offer several paths at once.** A compose install that already joins a tailnet adds the `edge` profile once and gets Workers VPC, Access and tailnet nodes, all into the same two edge listeners.

## 9. The never-public rule

### 9.1 Part A: private and brokered paths (D1311, approved by the owner 2026-10-21)

**The rule today** (networking map):
> **The engine is never on the public internet.** Self-hosted stacks are reached over the operator's tailnet. Public endpoints are only the ones Ostrium runs: `loams.dev`, `auth.loams.dev` and `console.loams.dev`.

The second sentence is too narrow for this design: a Workers VPC or a Lambda VPC reaches the stack without the tailnet. The amended text, approved on 2026-10-21 and applied to the networking map:
> **The engine is never on the public internet.** Its listeners are reached on loopback, over the operator's tailnet, or by `loams-gateway` on an internal network. Callers off the tailnet, such as serverless functions, reach Loams only through the gateway's edge listener, over a private link or a brokered tunnel that authenticates every request before it reaches the stack (§69 §4.2). Public endpoints are only the ones Ostrium runs: `loams.dev`, `auth.loams.dev` and `console.loams.dev`.

**Why this fits the approved direction:**
- Nothing in it lets an unauthenticated party on the internet open a connection to a Loams process.
- The owner's direction of 2026-10-20 asks for exactly this reach without public exposure.
- The owner approved the wording on 2026-10-21.

### 9.2 Part B: an opt-in public edge profile (D1312, deferred by the owner 2026-10-21)

**Deferred on 2026-10-21** per Q1060's default; revisited after ER2. The text that was proposed, kept for that review, would be appended to part A:
> **The engine is never public; the gateway's edge listener may be, in an explicit opt-in edge profile** (§69 §9.3). The profile is off by default, admits only mTLS or short-lived tokens, and exposes nothing but the edge allow-list.

The decision also answers Q992 ("May the gateway's Postgres data listener ever be public? … revisited only by an owner ruling"). The proposed answer is: **not the `pg` listener**, only the HTTP edge listener; Postgres stays private-link only.

**Arguments for part B:**
- It works from any runtime and network with no Cloudflare account and no broker.
- TLS runs end to end, so no third party sees plaintext (T10).
- It has the lowest latency and no per-vendor setup.
- It is the only path for a Workers mTLS binding.
- It lets users who will not use Cloudflare, or whose regulator forbids TLS termination by a third party, still reach Loams from serverless.

**Risks of part B:**

| Risk | Likelihood | Effect | Mitigation if approved |
|---|---|---|---|
| A pre-auth bug in the gateway is exploitable from the whole internet | Medium over time | Remote compromise of the gateway; it holds no data keys, but it sits next to the engine's edge listener | Fuzzing (networking map §5); minimal pre-auth parsing; the engine re-checks everything (D1304); an external review before ER3 ships |
| Scanners find Loams installations and fingerprint versions | High | Targeted attacks after a disclosure | No version in any response; a generic `404`; a prominent advisory process; users told to keep the gateway patched |
| Credential stuffing and token-endpoint abuse | High | Load; lockouts | No static keys on this profile (D1313); per-source limits; a WAF recommended in front (Cloudflare proxied or a cloud WAF), which is WAF-friendly by design: plain HTTPS, stable paths, no WebSocket |
| Users turn it on casually, as "the easy way" | Medium | The safety story erodes; support load | Off by default; an explicit `exposure.mode: edge-public` plus an acknowledgement field; the console shows a banner; the docs lead with the private paths |
| Message and reputation: "Loams says never public" | Medium | Trust | Precise wording: the engine is still never public. Only a gateway with an allow-list is, and only when the user opts in |
| D1195's admission policy needs an exception | Certain | A weaker guard | The exception is narrow: one Service, one port, `loams.dev/edge-public: "acknowledged"`, only the gateway's pods; LOC tests that nothing else gets through |
| Denial of service of a self-hosted engine | Medium | Loss of availability for tailnet users too | Gateway limits; concurrency caps towards the engine; a WAF or CDN in front recommended |

**Recommendation (Q1060's default, adopted by the owner on 2026-10-21): defer part B.**
- Part A serves every runtime in §8. The Access path works from any runtime that can send headers.
- So part B adds convenience and independence from Cloudflare, not reach.
- Revisit when a design partner or user cannot use Cloudflare and has no private link, or when ER2 has shipped, so the profile can be token-only from its first day.

### 9.3 The public edge profile's requirements (D1313, deferred with D1312)

- **TLS 1.3 only** at the gateway, with certificates from the user's ACME or cert-manager. HSTS on, and no HTTP listener.
- **Admission by one of these:** mTLS against a user-configured client CA (the Workers `mtls_certificates` binding, a function's client certificate); or a Loams access token from §7.2. **No static `lk_` keys and no Access assertions** on this listener.
- **Per-namespace scopes:** tokens carry `env` and `scp`, and the allow-list can be narrowed per environment (`edge.public.allowed_methods`).
- **Rate limits** per source address (from the WAF's or load balancer's PROXY v2, Q901) and per subject, plus a global concurrency cap towards the engine.
- **WAF-friendly:** stable paths, no WebSocket, JSON or protobuf bodies within the body cap, and a documented ruleset for Cloudflare's Free WAF and AWS WAF.
- **The same allow-list, and no admin.** The token endpoint is on it, with its own strict limit.
- **The operator** renders a public Service for the edge port only, when `exposure.mode: edge-public` and `exposure.edgePublic.acknowledged: true` are both set. A narrow VAP exception covers it. Postgres is never on this profile.
- **External security review** before release, with the result published (as the networking map asks for pre-auth parsers).

## 10. A Loams-hosted relay (D1317)

A relay run by Ostrium would work like Tunnel: the user's stack dials out to it, and runtimes call a hostname under a Loams domain.

| | Cloudflare Tunnel (+ Workers VPC, Access) | A Loams-hosted relay |
|---|---|---|
| Who operates the edge | Cloudflare, in the user's account | Ostrium |
| Cost to the user | Free tier; Workers VPC free in beta | Ostrium's price; Ostrium pays bandwidth and a global footprint |
| Who sees plaintext | Cloudflare | Ostrium, unless it is an SNI-blind relay (networking map §6) with TLS keys only on the user's side |
| Identity at the edge | Access policies, service tokens, mTLS (paid plans) | Built by Ostrium |
| Workers integration | Native: Workers VPC, Hyperdrive | None: Workers would fetch a public hostname |
| Availability | Cloudflare's network | An availability promise Ostrium would have to staff (it offers no SLA, D1323) |
| Abuse handling | Cloudflare's | Ostrium needs an abuse process before the domain goes live |
| Fit with the preview model ("we host only the console and auth") | Keeps it | Breaks it |
| Open source | `cloudflared` is Apache-2.0; the edge is Cloudflare's | Could be Apache-2.0 (frp and rathole are Apache-2.0 Rust and Go prior art; zrok and OpenZiti are Apache-2.0 overlays), but the service still needs operating |

Prior art, read 2026-10-20 ([ngrok Traffic Policy](https://ngrok.com/docs/traffic-policy/actions); [ngrok pricing](https://ngrok.com/pricing); licences from GitHub):
- **ngrok** validates JWTs at its edge (`jwt-validation`) and forwards to internal endpoints. Free with limits, Pay-as-you-go from $20 a month.
- **Open-source and source-available projects:**
  - frp (Apache-2.0);
  - rathole (Apache-2.0, Rust);
  - zrok and OpenZiti (Apache-2.0);
  - chisel (MIT);
  - Pangolin (AGPL-3.0 and a commercial licence, so unsuitable to embed);
  - inlets, which is commercial.
- **A relay on the user's own public VPS** (frp, rathole) makes the edge listener **public** by D1301, so it falls under D1312.

**Decision: no hosted relay now.**

**Revisit when one of these holds:**
1. Three or more design partners or users cannot use Cloudflare and have no private link.
2. ~~Ostrium offers a managed-reach service with an SLA as part of its services business. The relay would then be operated as a service, with the abuse and availability work priced in.~~ Superseded by D1323: the services offering is removed, so this trigger no longer applies.
3. The SNI-blind tunnel of networking map §6 is built anyway for agent preview URLs. Edge reach could then reuse it, with TLS keys on the user's side only.

## 11. Tests (D1319)

**On every PR that touches the gateway's edge module, the engine's auth or the allow-list (GitHub-hosted runners):**
- **`edge_allowlist_is_fresh`:** regenerate `api/edge-allowlist.txt` and diff it.
- **The refusal suite** (`crates/loams/tests/edge_refusals.rs`, with the gateway in process):
  - every method in the descriptor set that is not on the list is refused at the gateway (`404`);
  - the same call sent straight to the engine's edge upstream listener is refused too;
  - every refused credential kind gets `401` or `403`: no credential, `ls_`, cookie, an operator key from the keys file, an org key, an owner key, a key for another environment, a `Loams-Namespace` mismatch;
  - spoofed `Tailscale-User-Login` and `Cf-Access-*` headers have no effect.
- **Access admission:** a local JWKS (rcgen keys) signs assertions.
  - Accepted: a valid assertion.
  - Refused: wrong `aud`, wrong `iss`, expired, unknown `kid`, `alg: none`, HS256, a missing header on the `access` listener, a public `Host` on the `network` listener.
- **Limits:** body cap, head timeout, rate limit, stream cap.
- **A fuzz target** for the edge's request-head handling and header stripping (`cargo fuzz`, nightly job).
- **`workerd` end to end** (job `edge-workerd`, path-filtered):
  - pinned `workerd` from npm runs a test Worker that uses `@loams/client` with `transport.fetch` set to a service binding. That is the same code path as a Workers VPC binding: `env.X.fetch` with the host ignored.
  - A small stub in front of the gateway plays Access and adds a signed assertion.
  - The Worker creates a collection, writes, searches, and calls a refused admin method.
  - The engine runs `loams dev`-style with `LOAMS_AUTH_MODE=apikey` and an environment key.
- **Compose static tests:** `compose_edge_has_no_ports`, `cloudflared_networks_are_edge_only`, `engine_edge_listener_on_edge_link_only`.

**Nightly, in a GitHub environment `edge-nightly` with secrets, on the owner's Cloudflare test account (Q1069):**
- the runner starts the compose stack with the `edge` profile and `cloudflared` from the tunnel token;
- a test Worker, deployed by pinned `wrangler` with a `vpc_services` binding to that tunnel, is called on its `workers.dev` URL with a test secret;
- it runs the same scenario through Workers VPC and through the Access hostname;
- a Hyperdrive config on a `tcp` VPC Service runs `SELECT 1` and a write through the gateway's `pg` dedicated port (ER1);
- the run tears the Worker down;
- a failure opens an issue labelled `edge-nightly`.

**What CI cannot run, done by hand and recorded in the plan before each release claims support:**
- Vercel through Access and PrivateLink;
- Lambda in a VPC and with the Tailscale recipe;
- Cloud Run Direct VPC egress;
- Fly.io with Tailscale.

## 12. Phases (D1320)

**ER0: the edge door, keys only.** Gate: GW1a (done). The plan's Tasks 0–12:
- the allow-list generator and the method option;
- the engine's edge upstream listener and its credential policy;
- the gateway's `edge` module with both admission modes, header hygiene, limits, the link key and metrics;
- the compose `edge` profile;
- the refusal suite, the fuzz target and the `workerd` test;
- `accessServiceToken()` in the TypeScript SDK;
- guides under `docs/guides/edge/`;
- the networking-map wording of part A.

Exit:
- every test of §11's PR list passes;
- one manual run from a real Worker (Workers VPC and Access) and one from a Vercel function is recorded.

**ER1: operations and Postgres.** Gate: ER0; OP1's gateway component.
- The operator's `tunnel` mode, and the edge listeners in `internal` mode;
- `scripts/edge/check-tunnel-config`;
- the nightly real-Cloudflare job;
- Hyperdrive to the dedicated port;
- recipes for Lambda (VPC and Tailscale), Cloud Run and Fly.io, with the cold-join cost measured.

**ER2: workload identity.** Gate: ER0; §19 §5.3's instance signing key and JWKS.
- The token endpoint, trust policies (the admin RPC, the console page), and Loams access tokens on the edge listener;
- `exchange()` and the providers in TypeScript, then Python and Go;
- `pgConnectCredential()` with D711.

**ER3: the public profile.** Only if D1312 is approved; gate: ER2.
- mTLS and token admission, the operator's `edge-public` mode with the narrow VAP exception, the WAF guide, and the external review.

## 13. Relation to earlier decisions

| Decision | Effect |
|---|---|
| **Networking map rule** ("the engine is never on the public internet") | **Amended by D1311 (part A), approved 2026-10-21:** the wording names private-link and brokered paths to the gateway's edge listener; the networking map, §43 §4, §57 and §65 §11.4 carry it. The engine itself is unchanged. Part B (D1312) was deferred |
| **D581** (Cloudflare is the public front door for Ostrium's sites; the tailnet is the private network) | **Extended:** Cloudflare Tunnel also carries users' serverless traffic, in the **user's own** account, never Ostrium's |
| **D1019, D1195** (gateway Service internal only; exposure `tailnet` or `internal`) | **Kept.** D1316 adds `tunnel`, which renders no Service. There is no `edge-public` mode while D1312 is deferred |
| **Q992** (may the gateway's Postgres listener ever be public?) | Unchanged: no. D1312 concerns only the HTTP edge listener |
| **D1013, D1022** (the `http` module on Pingora) | **Amended in order only:** ER0's edge listener is a narrow hyper proxy behind a trait, and the `http` plan moves it onto Pingora. Q900 (Envoy at the HTTP edge) is unaffected |
| **D1014–D1016** (Postgres splice, refusals, dedicated ports) | Used as they are; Hyperdrive uses a dedicated port (D1310) |
| **§19 §5.2–§5.5, D449, D608** | ER2 builds flow 1 (workload identity) and the SDK sources D608 named |
| **D711** | ER2 uses `IssueConnectCredential` for edge Postgres |
| **D1208** (Mastra: `lk_` keys plus `LOAMS_NAMESPACE`; never public) | Kept. The Mastra docs gain an edge section; quickstarts stay on loopback or a tailnet |
| **Networking map §6** (public URLs for private deployments, parked) | Still parked. §10 records how a relay would relate to it |
| **§43 §8** (tailnet policy, tags, key expiry) | Serverless tailnet nodes are tagged, ephemeral and join by workload identity federation where possible (D1314) |
| **D1021** (no tenant labels) | Kept for the edge metrics (D1318) |

## 14. Risks

| Risk | Likelihood | Mitigation |
|---|---|---|
| Workers VPC changes before GA, or its terms change | Medium | The Access hostname is a GA fallback with the same gateway; nightly tests catch breaks; the guide labels it beta |
| Users wire a public hostname to the `network` listener | Medium | Separate ports, `refuse_hosts`, `check-tunnel-config` (§6.5) |
| A data-plane method that should not be on the edge gets marked | Low | Marking is reviewed; the refusal suite lists every marked method in its snapshot, so a change shows in the diff |
| The edge listener on hyper diverges from the later Pingora module | Medium | The `EdgeProxy` trait and the refusal suite are the contract; the suite runs against both during the move |
| Cloudflare terminating TLS is unacceptable for some users | Medium | Cloud private links; tailnet nodes; part B if approved |
| Ephemeral tailnet nodes from Lambda exhaust minutes or tagged-resource quotas | Medium | Recipes recommend the tunnel for bursty Lambdas; tailnet nodes for long-lived Fly Machines |
| Exchanged tokens outlive a revoked trust policy | Low | 15-minute TTL; the revocation set of §19 §5.4 |

## 15. Open questions (Q1060–Q1075), answered

The owner answered every question with its default on 2026-10-21.

| # | Question | Default (answered 2026-10-21 by the owner) |
|---|---|---|
| Q1060 | Approve part B, the opt-in public edge profile (D1312)? | **Defer.** Approve part A now. Revisit part B when a user cannot use Cloudflare or a private link, and only after ER2, so the profile is token-only from its first day |
| Q1061 | Workers default: Workers VPC (beta) or the Access hostname (GA)? | Workers VPC first in the guide, labelled beta; the Access hostname documented beside it; the nightly job tests both |
| Q1062 | The edge listener on hyper now, or start the `http` module on Pingora now? | hyper now, behind `EdgeProxy`; Pingora with the `http` plan |
| Q1063 | After ER2, are static `lk_` keys still accepted on the edge? | Yes on private-link and brokered paths, environment-bound only; never on the public profile; the console flags a key used through the edge when a trust policy exists for that service account |
| Q1064 | May an Access service token be the only credential, mapped to an environment by a trust policy? | No in ER0; in ER2, an explicit opt-in trust policy whose issuer is the Access team domain, off by default |
| Q1065 | Gateway-to-engine link authentication in compose: a link key or mTLS? | A link key file (32 random bytes made by setup, constant-time compare) in ER0; mTLS from the operator's CA on Kubernetes (ER1) |
| Q1066 | May the edge create and delete Qdrant collections and Elasticsearch indexes? | Yes for environment-bound keys, since Mastra's adapters create indexes; settings writes stay refused |
| Q1067 | Durable (Resonate) over the edge? | Not until per-namespace durable (D142) exists |
| Q1068 | Default limits? | 50 requests/s and burst 100 per credential, 8 open streams, 8 MiB body, 10 s head, 60 s unary, the token endpoint at 5/s per source; client Postgres pools of 5 (estimates) |
| Q1069 | The nightly real-Cloudflare job: whose account, which secrets? | A dedicated Ostrium Cloudflare account on the Free plan with a test zone; secrets in the `edge-nightly` environment; creating them is an owner action |
| Q1070 | Ever build a Loams-hosted relay? | No, until a criterion of §10 holds |
| Q1071 | Tailscale recipes for Lambda and Cloud Run: supported or community? | "Recipe, tested each release by hand"; Fly.io supported once tested |
| Q1072 | AWS identity: outbound identity federation only, or also signed `GetCallerIdentity`? | Outbound federation only; add `GetCallerIdentity` if a user cannot enable it |
| Q1073 | Loams token lifetime on the edge? | 15 minutes default, 1 hour maximum, never longer than the subject token |
| Q1074 | TLS between `cloudflared` or a VPC Service and the gateway on the compose network? | Plaintext on the internal `edge` network in ER0 (TLS mode `disabled`); TLS with the gateway's edge certificate and `verify_ca` on Kubernetes and wherever the hop crosses hosts |
| Q1075 | Where do guides live: the monorepo's `docs/guides/edge/` or loams.dev Learn? | `docs/guides/edge/` as the source; loams.dev renders them |

## 16. Sources

All read on 2026-10-20. The page's own date, where shown, is beside each link in §5 and §7.2.

- **Cloudflare:**
  - [Tunnel](https://developers.cloudflare.com/tunnel/), [connector](https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/), [firewall](https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/configure-tunnels/tunnel-with-firewall/), [run parameters](https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/configure-tunnels/run-parameters/), [gRPC](https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/use-cases/grpc/), [Tunnels for Platforms changelog](https://developers.cloudflare.com/changelog/post/2026-10-16-tunnels-for-platforms-and-50k-routes/);
  - [service tokens](https://developers.cloudflare.com/cloudflare-one/access-controls/service-credentials/service-tokens/), [JWT validation](https://developers.cloudflare.com/cloudflare-one/access-controls/applications/http-apps/authorization-cookie/validating-json/), [mTLS](https://developers.cloudflare.com/cloudflare-one/access-controls/service-credentials/mutual-tls-authentication/), [policies](https://developers.cloudflare.com/cloudflare-one/access-controls/policies/);
  - [Workers VPC](https://developers.cloudflare.com/workers-vpc/), [open beta post](https://blog.cloudflare.com/workers-vpc-open-beta/), [VPC Services](https://developers.cloudflare.com/workers-vpc/configuration/vpc-services/), [VPC Networks](https://developers.cloudflare.com/workers-vpc/configuration/vpc-networks/), [tunnel for Workers VPC](https://developers.cloudflare.com/workers-vpc/configuration/tunnel/), [limits](https://developers.cloudflare.com/workers-vpc/platform/limits/), [private database example](https://developers.cloudflare.com/workers-vpc/examples/private-database/);
  - [Hyperdrive private database](https://developers.cloudflare.com/hyperdrive/configuration/connect-to-private-database/), [supported databases](https://developers.cloudflare.com/hyperdrive/reference/supported-databases-and-features/);
  - [Workers and Access](https://developers.cloudflare.com/workers/configuration/cloudflare-access/), [compatibility flags](https://developers.cloudflare.com/workers/configuration/compatibility-flags/), [TCP sockets](https://developers.cloudflare.com/workers/runtime-apis/tcp-sockets/), [mTLS binding](https://developers.cloudflare.com/workers/runtime-apis/bindings/mtls/), [WAF](https://developers.cloudflare.com/waf/), [Mesh](https://blog.cloudflare.com/mesh/) (2026-04-14).
- **Tailscale:**
  - [tsnet](https://tailscale.com/kb/1244/tsnet), [tsnet API](https://pkg.go.dev/tailscale.com/tsnet), [userspace networking](https://tailscale.com/kb/1112/userspace-networking);
  - guides for [Lambda](https://tailscale.com/kb/1113/aws-lambda), [Cloud Run](https://tailscale.com/kb/1108/cloudrun) and [Fly.io](https://tailscale.com/kb/1132/flydotio);
  - [ephemeral nodes](https://tailscale.com/kb/1111/ephemeral-nodes), [pricing](https://tailscale.com/pricing), [Funnel](https://tailscale.com/kb/1223/funnel), [Funnel CLI](https://tailscale.com/kb/1311/tailscale-funnel), [serve](https://tailscale.com/kb/1312/serve), [Tailscale Services](https://tailscale.com/kb/1552/tailscale-services);
  - [workload identity federation](https://tailscale.com/kb/1581/workload-identity-federation), [beta](https://tailscale.com/blog/workload-identity-beta), [GA](https://tailscale.com/blog/workload-identity-ga);
  - [app connectors](https://tailscale.com/kb/1281/app-connectors), [peer relays](https://tailscale.com/kb/1591/peer-relays);
  - Headscale [features](https://headscale.net/stable/about/features/) and [#1040](https://github.com/juanfont/headscale/issues/1040).
- **Vercel:** [Secure Compute](https://vercel.com/docs/networking/secure-compute), [self-serve changelog](https://vercel.com/changelog/secure-compute-is-now-self-serve), [Static IPs](https://vercel.com/docs/networking/static-ips), [Static IPs changelog](https://vercel.com/changelog/static-ips-are-now-available-for-more-secure-connectivity), [PrivateLink](https://vercel.com/docs/networking/privatelink), [PrivateLink changelog](https://vercel.com/changelog/aws-privatelink-is-now-available-on-pro-and-enterprise), [fixed IPs](https://vercel.com/kb/guide/can-i-get-a-fixed-ip-address), [Fluid compute](https://vercel.com/docs/fluid-compute), [Edge runtime](https://vercel.com/docs/functions/runtimes/edge), [connection pools](https://vercel.com/kb/guide/efficiently-manage-database-connection-pools-with-fluid-compute), [OIDC](https://vercel.com/docs/oidc), [OIDC reference](https://vercel.com/docs/oidc/reference), [Vercel Connect: Tailscale](https://vercel.com/connect/tailscale).
- **AWS:** [Lambda VPC](https://docs.aws.amazon.com/lambda/latest/dg/configuration-vpc.html), [cross-account VPC](https://docs.aws.amazon.com/lambda/latest/dg/configuration-vpc-cross-account.html), [2019 VPC networking](https://aws.amazon.com/blogs/compute/announcing-improved-vpc-networking-for-aws-lambda-functions), [PrivateLink endpoint services](https://docs.aws.amazon.com/vpc/latest/privatelink/privatelink-share-your-services.html), [outbound identity federation](https://aws.amazon.com/blogs/aws/simplify-access-to-external-services-using-aws-iam-outbound-identity-federation), [`GetWebIdentityToken`](https://docs.aws.amazon.com/STS/latest/APIReference/API_GetWebIdentityToken.html), [outbound providers](https://docs.aws.amazon.com/IAM/latest/UserGuide/id_roles_providers_outbound.html); HashiCorp [Vault AWS auth](https://developer.hashicorp.com/vault/docs/auth/aws).
- **Google:** [connecting Cloud Run to a VPC](https://docs.cloud.google.com/run/docs/configuring/connecting-vpc), [Direct VPC egress](https://docs.cloud.google.com/run/docs/configuring/vpc-direct-vpc), [service identity](https://docs.cloud.google.com/run/docs/securing/service-identity), [ID tokens](https://docs.cloud.google.com/docs/authentication/get-id-token), [token types](https://docs.cloud.google.com/docs/authentication/token-types).
- **Fly.io:** [private networking](https://docs.fly.io/networking/private-networking), [Flycast](https://docs.fly.io/networking/flycast), [OpenID Connect](https://docs.fly.io/security/openid-connect).
- **Relays:** [ngrok Traffic Policy](https://ngrok.com/docs/traffic-policy/actions), [ngrok pricing](https://ngrok.com/pricing); [frp](https://github.com/fatedier/frp), [rathole](https://github.com/rapiz1/rathole), [zrok](https://github.com/openziti/zrok), [OpenZiti](https://github.com/openziti/ziti), [Pangolin](https://github.com/fosrl/pangolin), [chisel](https://github.com/jpillora/chisel) (licences from the GitHub API).
- **This repository at `342179be3`:** `crates/loams/src/auth/{mod,oidc}.rs`, `crates/loams/src/api/namespace_reach.rs`, `deploy/release/compose.yaml`, `deploy/release/tailscale/serve-https.json`, `proto/loams/options/v1/options.proto`, `sdks/typescript/packages/client/src/runtime/{token-source,transports}.ts`, and PR #947.
