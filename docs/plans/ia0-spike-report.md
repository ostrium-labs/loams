# IA0 — Zitadel and OpenFGA spike: report

Status: **Done** · 2026-10-21. Track IA, design [§76](../design/76-identity-and-authorization.md), plan [IA1](2026-10-21-ia1-identity-authz.md) (IA0 Tasks 0–6). The plan named this file `ia0-spike.md`; the owner's brief named it `ia0-spike-report.md`, which wins (plan ruling R0.1).

**Recommendation: go.** Zitadel v4.19.4 and OpenFGA v1.22.0 do what §76 needs, end to end, on one laptop. Nothing blocks IA2 or IA4. The spike found two things §76 should change before IA3 builds `identity-sync` (F1, F3) and five smaller ones; none changes the architecture, the `Authorizer` trait, the model or the agent flow (§7). **The owner approved all seven on 2026-10-21; they are D1622–D1628** (§7).

**Why now.** The owner, 2026-10-21: "current agent auth is too complex and friction". So §1 maps how agents authenticate today and counts the steps, and §2 counts them again on the new flow.

| | Today | With Zitadel + OpenFGA (measured in the spike) |
|---|---|---|
| Steps to give an agent access to one environment | **7** (8 for a Mastra agent), and it still cannot be limited to one environment | **3** (2 for an agent under agentd, which never sees a secret) |
| Credential the agent holds | a long-lived `lk_` key, often with no expiry, pasted into `.env` | a private-key JWT key; it trades it for a **15-minute** token (93 ms) |
| Scope | the key's org role (`member` or `admin`) over every environment | explicit grants only; the owner's rights are not inherited |
| Revocation | revoke the key; OIDC tokens cannot be revoked | delete the grant: next strict check; deactivate the agent: denied in **≤ 1.1 s** (10/10 rounds) |
| Credential kinds a user meets | 11 | 3 for agents (machine key, 15-minute token, agentd-vended token) |

## 1. Baseline: how agents authenticate today

Mapped from the code on `dev` at `3617814c8` (2026-10-21). "The old IdP" is the one IA1 Task 1 removes (§76 §13).

### 1.1 Credential kinds

| # | Credential | Issued by | Lifetime | Verified by | Who uses it |
|---|---|---|---|---|---|
| 1 | `lk_<ULID>_<secret>` in `LOAMS_API_KEYS_FILE` | `loams auth new-key [--append]` (prints the secret to stdout) | **none** (no expiry field) | argon2id, file re-read every 5 s (`crates/loams/src/auth/api_key.rs`) | SDKs, CLI, Mastra |
| 2 | `lk_…` in the metastore | console *Create API key* (`KeyService/CreateKey`) | `expires_in_days`, 0 = never | argon2id + revocation (`auth/directory.rs` `check_key`) | SDKs, apps |
| 3 | `lk_…` for `loams-pg-control` | `loams pg-api-key` | — | **SHA-256** in a JSON file, with its own `Agent`/`ServiceAccount` kinds and `act` (`crates/loams-pg-control/src/authz.rs`) | Postgres control plane |
| 4 | `ls_…` console session | `CreateSession`, `CompleteSetup` | 12 h, no refresh | SHA-256 digest | console |
| 5 | Any IdP's JWT | the configured issuer | the IdP's | `LOAMS_AUTH_MODE=oidc`, admitted by a subject, email or group allowlist | console, people |
| 6 | Tailscale identity header | the tailnet | ambient | loopback listener only | console |
| 7 | `agentd.token` | agentd, on every start | process lifetime | agentd (owner role) | Desktop → local agentd |
| 8 | An OIDC access token for `aud=loams-agentd`, `sub` pinned by `--subject` | the old IdP | the IdP's | `crates/loams-agentd-connect/src/oidc.rs` | browser Studio → remote agentd |
| 9 | `LOAMS_AGENTD_RUN_TOKEN` | agentd, per run | until revoked | agentd RPC (role `mcp`) | harness → agentd MCP |
| 10 | Loams Git tokens | the engine | — | loopback only | git |
| 11 | Static Postgres password (`cloud_admin:cloud_admin`) | the compose file | forever | Postgres | Mastra storage and vectors |

Plus each harness's own login (Claude Code, Codex), copied by agentd into plaintext JSON snapshots, which is out of scope here.

What does **not** exist, although the protos, console and SDKs suggest it does:

- **No token endpoint.** The SDKs' OIDC exchange (Go, Rust, Python, Swift, TypeScript, Java) posts to `/oauth/token`, which the server does not serve. The request bodies differ by language.
- **Agents hold nothing.** The console creates, suspends and resumes agent records (`AgentRec`). `ListAgentTokens` always answers an empty list ("minted by MT1's issuer; none exist before it", `crates/loams/src/api/connect_admin.rs:1212`). `TokenFlow` and `TrustPolicy` are stored and never read.
- **Revocation of tokens is recorded, not enforced.** `RevokeToken` appends a `jti` to `revoked_tokens`, and nothing checks that list.
- **No agent principal in the engine.** `PrincipalKind` has no `Agent`; a key made for a service account authenticates as `apikey:<id>`.

### 1.2 The steps a user takes

**A. An SDK, CLI or Mastra agent calling Loams** (the only working path for an agent):

1. Set `LOAMS_AUTH_MODE=apikey` in `.env`.
2. Set `LOAMS_API_KEYS_FILE` (or use the console's keys instead).
3. Restart the engine.
4. Mint a key: `loams auth new-key --append "$LOAMS_API_KEYS_FILE"`, or the console's *Create API key*.
5. Copy the secret, which is shown once.
6. Paste it into the agent's environment as `LOAMS_API_KEY`.
7. Set `LOAMS_ENDPOINT` (the Go, Java, Kotlin and C# clients throw without it).
8. Mastra only: set `DATABASE_URL` with the shared static Postgres password, and `LOAMS_NAMESPACE`.

**7 steps (8 for Mastra), 4 values copied by hand.** The result is a long-lived secret with an org-wide role. The admin RPCs ignore key scopes, so it cannot be limited to one environment.

**B. Browser Studio driving a remote agentd** (`docs/guides/studio/remote-agentd.md`):

1. Create an OAuth provider and application `loams-agentd` in the IdP by hand.
2. Note the issuer URL.
3. Find your own `sub` (a hashed id), which needs a token first; the guide does not say how to get one.
4. Get a TLS certificate and mount it.
5. Write `config.toml` `[remote]` with eight keys, or pass the `serve --remote` flags.
6. Start agentd (the compose service passes no flags, so it needs the file).
7. For the hosted console, set `LOAMS_AGENTD_ORIGINS` and redeploy.
8. In Studio, add the host and **paste an access token**. A reload drops it, and an expiry means pasting again.

**8 steps**, unverified against a live IdP (the guide says so).

**C. Desktop → local agentd:** 0 steps (`agentd.token` is automatic). This is the one path that works without friction, and the new design keeps it.

### 1.3 Where it fails or confuses

1. **"Agents never need a key"** says the console's empty state (`web/plugins/projects/src/environment.tsx:275`); the sample code on the same page sets `LOAMS_API_KEY`. The Mastra starter's agent skill tells a coding agent to run `loams auth new-key`, which prints the key into the transcript.
2. **The same `lk_` prefix means two systems** (argon2id in the engine, SHA-256 in `loams-pg-control`), with different principal models.
3. **The SDKs' OIDC path cannot work** (no server endpoint), and empty-string and missing-key handling differ across languages.
4. **Agent records are decoration**: an agent created in the console can do nothing, and suspending it changes nothing an agent holds.
5. **Nothing is short-lived.** Keys default to no expiry; sessions are 12 h; remote-agentd tokens are pasted.
6. **No environment scoping** for keys on the admin surface; an agent's key is as strong as its role in the whole org.

## 2. The new agent flow, as the spike ran it

Outside a sandbox (§76 §10):

1. **Create the agent.** One Zitadel machine user with JWT access tokens and a private-key JWT key; Loams links `(issuer, sub)` to `agent:<sub>` and writes `agent#owner` and `agent#project`. The spike does it with three API calls in `bootstrap.sh`; IA4 makes it one console action or `loams agents create`.
2. **Grant it an environment.** One tuple, `agent:<id>#editor@environment:<id>`.
3. **Give the agent its key file.** The agent signs a 5-minute assertion and trades it at `/oauth/v2/token` (RFC 7523) for a 15-minute JWT access token: one HTTP call, 93 ms. An SDK does this itself and repeats it before expiry.

**3 steps, 1 file.** The key never travels: only signed assertions do. Inside a sandbox (D1618), agentd vends a 15-minute Loams token whose `sub` is the machine user's id, so the user does steps 1 and 2 only and the agent never holds a Zitadel key (IA4).

Revocation is one step: remove the grant, or deactivate the machine user.

## 3. What was built

All on loopback, under opt-in compose profiles, in `conformance/identity/`:

| Profile | Services |
|---|---|
| `identity` | Zitadel API, Zitadel Login, `zitadel-db` (Postgres 17) |
| `authz` | OpenFGA on SQLite (`openfga-migrate`, then `openfga`) |
| `spike` | the spike service `spikes/ia0-authz` |

- `up.sh` generates the git-ignored `.env` (masterkey, database and admin passwords, cookie secret, admin token) and starts `identity` and `authz`. `bootstrap.sh` creates org `acme`, project `loams` (the audience), OIDC app `loams-console` (code + PKCE, JWT access tokens), person `alice`, agent `agent-1` with its key, 15-minute tokens, and an OpenFGA store with model v1. **From nothing to bootstrapped: 26 s** (images cached).
- `deploy/openfga/core.fga` (model v1), `fga.mod` (schema 1.2, `core.fga` + `graph.fga`) and `core.fga.yaml` (model tests). `graph.fga` now hangs off `environment` (Q1207).
- `spikes/ia0-authz`: a standalone Cargo package outside the workspace (nothing enters the default build). An Axum service: JWT verification (JWKS from discovery, cached 5 min, refetch on unknown `kid`, `iss`, `aud`, `exp` with 60 s skew, RS256 only), a principal link table, a declared route table with deny by default, `Check`/`BatchCheck`/`ListObjects` through `openfga-client` 0.6.1, an allow-only decision cache (5 s), and `POST /zitadel/events` (Actions v2 with `ZITADEL-Signature`). Plus drivers: `scenario`, `revocation`, `events`, `bench`.
- `smoke.sh` and `.github/workflows/identity.yml` (jobs `openfga-model` and `identity-smoke`, outside `CI required`).
- `cross_org_scan.py`: `no_cross_org_tuples` over a store or a fixture, with a self-test.

Zitadel runs as its official image, unmodified; no Zitadel code is copied or linked (D1602).

**Provenance note.** While explaining the Actions v2 measurements, the IA0 agent read a few files of Zitadel's AGPL source on GitHub (the execution queue worker and target caller, the webhook signing helper, `cmd/defaults.yaml`). Nothing from them is in this PR. This report states only what was measured, what Zitadel's documented configuration says, and what its public docs say. Under the owner's clean-room rule for `loams-identity`, this report, the spike and its worktree are not inputs for that work.

## 4. Results

### 4.1 Pins (Task 0)

The 14-day rule on 2026-10-21 means a release from 2026-10-07 or earlier.

| Image | Release | Digest |
|---|---|---|
| `ghcr.io/zitadel/zitadel:v4.19.4` | 2026-10-01 | `sha256:90a3045b80a2b5b395e1e770302076906af3a5911225af7c464b4f3bd00d0a1b` |
| `ghcr.io/zitadel/zitadel-login:v4.19.4` | 2026-10-01 | `sha256:93ffd1395b827f685ba29c02fa70aee881fc21102519c4e2bbb0b89c0e00f42c` |
| `docker.io/library/postgres:17.10-alpine` (the tag Zitadel's own `deploy/compose/.env.example` pins) | 2026-07-07 | `sha256:742f40ea20b9ff2ff31db5458d127452988a2164df9e17441e191f3b72252193` |
| `docker.io/openfga/openfga:v1.22.0` | 2026-10-06 | `sha256:9cf9a20af32a40434cd3a788c429879439ee9ea9abf65c9c200ee17cd43f769e` |
| `docker.io/openfga/cli:v0.8.1` (v0.8.2 is from 2026-10-15, too new) | 2026-09-24 | `sha256:1b8035ea4aeb8ad739cbea972488cdda37886644ed004737880b63403bae0fe8` |

Zitadel v4.19.5 (2026-10-15) is the newest release and too new.

### 4.2 Allow and deny, end to end (Task 3)

`ia0-authz scenario`, against the stack, every row a real HTTP call with a real Zitadel token:

| # | Principal | Request | Status | Result |
|---|---|---|---|---|
| 1 | alice, no tuple | GET /deployments/dev-web | 403 | pass |
| 2a | alice, viewer on p1 | GET /deployments/dev-web | 200 | pass |
| 2b | alice, viewer on p1 | DELETE /deployments/dev-web | 403 | pass |
| 3a | agent-1, no grant (owner alice, a project viewer) | GET /deployments/dev-web | 403 | pass |
| 3b | agent-1, editor on dev | GET /deployments/dev-web | 200 | pass |
| 3c | agent-1, editor on dev | POST /deployments/dev-web/restart | 200 | pass |
| 3d | agent-1, editor on dev | GET /deployments/prod-web (ungranted environment) | 403 | pass |
| 3e | agent-1, editor on dev | DELETE /deployments/dev-web | 403 | pass |
| 3f | agent-1, editor on dev | GET /deployments (ListObjects) | 200 | `["deployment:dev-web"]` |
| 3g | agent-1 | GET /secrets (undeclared route) | 403 | pass |
| 4a | agent-1, deployer on dev | DELETE /deployments/dev-web | 200 | pass |
| 4b | agent-1, grant deleted | DELETE /deployments/dev-web (HIGHER_CONSISTENCY, no cache) | 403 | pass |
| 4c | agent-1, grant deleted | GET /deployments/dev-web (cached allow) | 200 | stale allow within the 5 s TTL |
| 4d | agent-1, grant deleted | GET until denied | 403 | after 5010 ms (the cache TTL) |
| 5a | agent-1, token without the project audience | GET /whoami | 401 | pass |
| 5b | a forged token | GET /whoami | 401 | pass |
| 6 | write `billing_admin`, `meter_admin`, `operator` for `agent:…` | model write | refused | pass (`agent_cannot_write_grants`) |

Tokens, as issued: alice's access token has `aud = [client id, project id]`; the agent's has `aud = [project id]`. Both carry `urn:zitadel:iam:user:resourceowner:id` (the org) and live 900 s. Alice's code + PKCE login through the session API took 1051 ms (password hashing); the agent's token took 93 ms.

Integration tests (`IA0_E2E=1 cargo test --release --manifest-path spikes/ia0-authz/Cargo.toml`): `rejects_wrong_aud`, `rejects_wrong_iss`, `allow_after_grant_and_deny_after_revoke`, `idempotent_duplicate_add_and_missing_delete_is_success`, `rejects_bad_signature_at_the_endpoint`, `signature_scheme`: 6/6 pass.

Model tests (`fga model test`, CLI v0.8.1): **8/8 tests, 93/93 checks, 2/2 ListObjects** pass, including `deny_by_default_without_tuple`, `agent_does_not_inherit_owner`, `agent_reaches_only_its_grant` and `agent_session_expires` (allowed before `expires`, denied one second after, denied for another session). `cross_org_scan.py` finds 0 cross-org tuples in the spike's store (64 tuples) and catches all three planted leaks in its self-test.

### 4.3 Agent revocation

`ia0-authz revocation`: grant, take a token, call (200), then deactivate the machine user in Zitadel and poll every 20 ms with the **old** token. Ten rounds:

| | min | median | max |
|---|---|---|---|
| Deactivate API call | 21 ms | 29 ms | 38 ms |
| Old token denied after | 710 ms | 1021 ms | 1059 ms |
| Grant tuple deleted after | 737 ms | 1043 ms | 1087 ms |

A new token was refused at once in 10/10 rounds. The old token keeps a valid signature until `exp`; it is denied because the sync marks the principal disabled (F5). Deleting the grant directly takes effect on the next `HIGHER_CONSISTENCY` check (row 4b) and within the cache TTL for cached checks (row 4d).

### 4.4 Actions v2 delivery (Task 4)

A `Webhook` target (`interruptOnError: false`) with event executions for org members, user grants and user state, into `POST /zitadel/events`:

| Run | Events | Received | Out of order | Duplicates | Missed | Call → receipt |
|---|---|---|---|---|---|---|
| 100 add/remove pairs, back to back | 200 | 200 | 0 | 0 | 0 (after draining) | p50 8.3 s, p95 15.1 s, max 15.9 s |
| 20 pairs, one per second | 40 | 40 | 0 | 0 | 0 | p50 0.77 s, p95 1.08 s, max 1.18 s |
| 3 deliveries answered 500, then 2 answered 200 | 5 | 5 | 0 | 0 | **3 lost: never redelivered** (90 s watched) | — |

After each run the final state matched Zitadel's member listing (drift 0), except when deliveries were refused.

What Zitadel v4.19.4 did in these runs, with its documented configuration:

- Event executions are jobs on an internal queue with **one worker** by default (`Executions.Workers: 1`). Delivery was in sequence order in every run, but a burst queues: about 12 events per second drained, so the 200th event arrived 16 s after its API call.
- **No retry.** A failed call is logged ("error calling target") and never delivered again (with `interruptOnError: false`, the setting an event execution should use). The documented `Executions.MaxTtl` (5 min) cancels jobs older than that.
- The payload carries `aggregateID`, `aggregateType`, `resourceOwner`, `sequence` (per aggregate), `event_type`, `created_at`, `userID` and `event_payload` (for members and grants, `userId`; grants also `projectId` and `roleKeys`).
- The signature is `ZITADEL-Signature: t=<unix>,v1=<hex HMAC-SHA256(key, "<t>." + body)>`; the spike accepts five minutes of skew and rejects a wrong key, a tampered body and a stale timestamp.
- Zitadel's event API (`POST /admin/v1/events/_search`, filtered by type, ascending) answered in 32 ms with every event, in order, including the three the webhook lost.

### 4.5 `Check` latency (Task 5)

Measured from the spike container to OpenFGA in the same compose network (container-to-container), 10 000 sequential checks per row for SQLite, 5 000 for the others. Milliseconds.

| Call | Store | Consistency | p50 | p95 | p99 |
|---|---|---|---|---|---|
| Check, agent via environment grant (3 hops) | SQLite | MINIMIZE_LATENCY | 1.94 | 4.51 | 5.71 |
| Check, person via org owner (5 hops) | SQLite | MINIMIZE_LATENCY | 1.54 | 3.03 | 3.77 |
| Check, deny (no tuple) | SQLite | MINIMIZE_LATENCY | 2.44 | 4.09 | 4.81 |
| Check, agent | SQLite | HIGHER_CONSISTENCY | 1.75 | 4.09 | 5.25 |
| Check, agent | Postgres 17 | either | 0.74 | 1.42 | 1.84 |
| Check, person (5 hops) | Postgres 17 | either | 1.25 | 2.32 | 2.93 |
| Check, agent | memory | either | 0.35 | 0.93 | 1.29 |
| Check, agent, OpenFGA check cache on | SQLite | MINIMIZE_LATENCY | 0.13 | 0.58 | 0.84 |
| Check, agent, OpenFGA check cache on | SQLite | HIGHER_CONSISTENCY | 1.83 | 4.32 | 5.44 |
| Check, the spike's in-process allow cache | — | — | < 0.001 | < 0.001 | 0.009 |
| BatchCheck of 50 | SQLite | either | 159–163 | 186–205 | 259–291 |
| 50 concurrent Checks | SQLite | either | 166–174 | 225–262 | 323–349 |
| BatchCheck of 50 | Postgres 17 | either | 8.8–9.4 | 11–20 | 12–27 |
| 50 concurrent Checks | Postgres 17 | either | 9.1 | 11.2–11.7 | 12.1 |

**Against Q1214's estimate** (p99 ≤ 10 ms cached or co-located, ≤ 25 ms uncached): a single uncached `Check` is already at p99 ≤ 6 ms on SQLite and ≤ 3 ms on Postgres, and a cached one is sub-millisecond. The estimate holds with room; this is one client on an idle laptop, so IA5's load test still sets the exit. The one miss is **batches on SQLite** (F4).

`HIGHER_CONSISTENCY` bypasses OpenFGA's check cache (the cache-on rows), as §3.2 says.

### 4.6 Footprint

Idle, five minutes after start, one `podman stats --no-stream` sample (CPU is the share of one core averaged since the container started), rootless Podman 6.1 on a 14-thread laptop:

| Container | Memory | CPU | Image | Data |
|---|---|---|---|---|
| `zitadel` | 85 MB | 5.8 % | 181 MB | — |
| `zitadel-login` | 96 MB | 1.2 % | 214 MB | — |
| `zitadel-db` (Postgres 17.10) | 62 MB | 3.8 % | 300 MB | 72 MB after the runs |
| `openfga` (SQLite) | 30 MB | 0.2 % | 84 MB | 0.6 MB |
| **`identity` profile** | **243 MB** | | **695 MB** | |
| **`authz` profile** | **30 MB** | | **84 MB** | |
| `ia0-authz` (the spike) | 2.6 MB | — | — | — |

So the `identity` profile costs about a quarter of a gigabyte of memory and 0.7 GB of disk, which is why D1613 keeps it off by default; `authz` alone is small. The stack was healthy 25 s after `up` with cached images.

## 5. The (verify) items (Task 0)

| # | Item | Answer |
|---|---|---|
| 1 | Actions v2 delivery: ordering, retries, redelivery, event names | In order with the default single worker; **no retry and no redelivery**; jobs dropped after 5 min. Names: `org.member.added`, `org.member.removed`, `org.member.cascade.removed`, `user.grant.added`, `user.grant.removed`, `user.grant.cascade.removed`, `user.deactivated`, `user.reactivated`, `user.locked`, `user.removed`, `user.machine.added`, `org.removed`. §4.4, F1 |
| 2 | The `aud` an engine requires; JWT access tokens | **The project id.** It is in a person's token (with the client id) and in an agent's token when the agent asks for scope `urn:zitadel:iam:org:project:id:<project id>:aud`; without that scope the token lacks it and is refused (row 5a). JWT access tokens: `accessTokenType: OIDC_TOKEN_TYPE_JWT` on the app, `ACCESS_TOKEN_TYPE_JWT` on the machine user |
| 3 | Modular models at v1.22 | Work. `fga model validate` and `fga model test` (CLI v0.8.1) take `fga.mod`, and `fga model write --file fga.mod` writes it to OpenFGA v1.22.0. The CLI refuses a test's `model_file` outside its own directory, so the tests sit beside `fga.mod` |
| 4 | `smartive/zitadel-rust` licence | `MIT OR Apache-2.0` (its `Cargo.toml`, with `LICENSE-MIT` and `LICENSE-APACHE`); GitHub shows NOASSERTION only because there are two files. Usable, but the spike did not need it: JWT bearer and the management calls are a few HTTP requests |
| 5 | Lakekeeper's store-name setting (D1604) | `LAKEKEEPER__OPENFGA__STORE_NAME`, default `lakekeeper` (v0.14.1 `docs/docs/configuration.md`), with `LAKEKEEPER__OPENFGA__ENDPOINT` (gRPC) |
| 6 | Clerk and WorkOS as OIDC IdPs in Zitadel | Not tested; Q1201's default (neither) does not need it |
| 7 | What is built | `auth::authorizer`, `graph_auth.rs`, `loams-graph/src/authz.rs` and `loams-pg-control/src/authz.rs` are unchanged since 2026-10-21; CP1 Task 3 has not started (no `crates/loams-tenancy`, no `core.fga` before this PR) |

## 6. Findings

**F1 (amends D1608): Actions v2 is at-most-once, so it cannot be the primary feed.** A delivery that fails is gone, and a burst queues for seconds. With webhooks primary and a nightly reconciler, a dropped "member removed" leaves access in place for up to a day, against §8.1's "within seconds". Proposal: `identity-sync` reads Zitadel's event API with a stored cursor (creation date plus `(aggregate, sequence)` to dedupe) as its primary feed, and treats a webhook as a wake-up that makes it read now. The nightly reconciler stays as the backstop. IA3 only; nothing in IA2 or IA4 depends on it.

**F2: Zitadel's HTTP client deny list blocks loopback, link-local and every private range by default.** A webhook to `identity-sync` on the same network is refused until `ZITADEL_HTTPCLIENT_DENYLIST` is narrowed. The compose file keeps `0.0.0.0/8` and `169.254.169.254/32` blocked. Helm values need the same care.

**F3 (amends §8.1's table): Zitadel's "org members" are its org managers, not an org's ordinary users.** Mapping `org.member.added` to `org:O#member` would make every Loams member a Zitadel administrator. Loams membership should come from **user grants** (Zitadel "authorizations") on the Loams project, inside the org: `user.grant.added|changed|removed` with coarse role keys (`member`, `admin`, `owner`) map to `org:O#member|admin|owner`. Org manager roles stay Zitadel's own. The spike handles both; grants round-trip in under 2 s.

**F4 (refines D1613): OpenFGA's SQLite datastore serializes.** A 50-check batch takes about 160 ms on SQLite and 9 ms on Postgres, while single checks differ by only 2–3×. SQLite stays right for the single-user `authz` profile; a multi-user install, or any list that uses `BatchCheck`, should use Postgres. Lists on SQLite should prefer `ListObjects`.

**F5 (adds to §10): a deactivated agent's JWT stays valid until `exp`.** Zitadel stops issuing at once, but a directly verified JWT is stateless. The auth layer must check the principal's status on each request (the spike's link table did; denied in about 1 s) as well as deleting tuples. With 15-minute tokens the window without that check would be 15 minutes.

**F6 (refines D1611): two caches stack.** OpenFGA's check cache (off by default, 10 s TTL when on) under Loams' 5 s decision cache gives up to 15 s of stale allows on `MINIMIZE_LATENCY` calls. Use one: Loams' `CachedAuthorizer` with OpenFGA's cache off, or OpenFGA's cache with `--check-query-cache-ttl` at 5 s and no in-process cache. Destructive, admin and credential calls use `HIGHER_CONSISTENCY`, which bypasses both.

**F7: a JWT does not say whether its subject is a person or a machine.** The principal kind comes from Loams' link table `(issuer, sub) → kind`, written when Loams creates the agent and on `user.machine.added`. §5 already has the link; IA2 must make it the source of the kind. A missing link fails closed: the subject is checked as `user:<sub>`, which holds none of the agent's grants.

Smaller notes:

- Access-token lifetime is per instance (OIDC settings), not per app or user; 15 minutes then applies to people too, who refresh.
- RFC 8693 token exchange is off by default in Zitadel (not in the discovery document), which fits D1606: Loams' own endpoint stays for workloads.
- A container cannot reach the host's loopback under rootless Podman, so the spike service runs inside the compose network; Zitadel needs `Host: localhost:18080` on internal calls (the spike's verifier sets it).
- The tree pins `jsonwebtoken` 10.4.0 and has no `openidconnect`; the spike used 10.4.0 and 40 lines of discovery code (plan ruling R0.4).

## 7. Does design 76 need changes?

Yes, small ones; none is a blocker and none changes the architecture, the trait, the model or the agent flow.

| Finding | Where | Change | Decision | Phase |
|---|---|---|---|---|
| F1 | D1608, §8.1 | The event list from a stored position is the primary feed; webhooks wake it | D1622 | IA3 |
| F2 | D1617, §12.5 | Zitadel's outbound deny list narrowed only for the sync webhook | D1627 | IA3 |
| F3 | §8.1 table, §5 | Membership from user grants on the Loams project; org managers stay in Zitadel | D1623 | IA3 |
| F4 | D1613, §12.1 | SQLite for one user; Postgres for shared installs and batch lists | D1626 | IA2 |
| F5 | §10, §9.2 | The auth layer checks principal status per request | D1624 | IA2, IA4 |
| F6 | D1611, §9.3 | One cache layer, ≤ 5 s | D1625 | IA2 |
| F7 | §5, §9.2 | The link table is the source of the principal kind | D1628 | IA2 |
| (verify) markers | §6.2, §8.1, §12.3, §14, §16 | Settled by §5 of this report | — | — |

**Approved by the owner on 2026-10-21 ("approve all").** F1 is D1622, F3 D1623, F5 D1624, F6 D1625, F4 D1626, F2 D1627 and F7 D1628, recorded in §76 and the decision log, with IA2, IA3 and IA4 updated in plan IA1.

## 8. Go / no-go

**Go.** End-to-end allow and deny work for a person and an agent; the agent reaches only its granted environment, never its owner's; revocation takes effect in about a second; `Check` is well inside Q1214; the footprint is modest and off by default. Next, agent path first: IA2's `loams-authz` crate and the deny-by-default middleware, then IA4's machine-user flow. F1 and F3 are approved as D1622 and D1623 and shape IA3.

## Appendix: reproducing

```sh
cd conformance/identity && ./up.sh
cargo build --release --manifest-path spikes/ia0-authz/Cargo.toml   # from the repository root
export IA0_SPIKE_BIN=<target>/release/ia0-authz
docker compose --profile identity --profile authz --profile spike up -d --wait ia0-authz && ./actions.sh
set -a; source .env; set +a; cd ../..
export IA0_FGA_GRPC=http://127.0.0.1:18082
"$IA0_SPIKE_BIN" scenario; "$IA0_SPIKE_BIN" revocation; "$IA0_SPIKE_BIN" events 100
docker exec loams-ia0-ia0-authz-1 ia0-authz bench 10000
```
