# LC0 — Loams Cloud on TiDB X: spike report

Status: **Done** · 2026-10-23. Track LC, design [§78](../design/78-loams-cloud-on-tidb-x.md), plan [LC1](2026-10-23-lc1-loams-cloud.md) (LC0 Tasks 0–7). The rulings and pins are in the plan's "Rulings made during execution" (R-LC0-1 to R-LC0-7 and R-LC2a-1 to R-LC2a-11, #1116). The spike code is in `spikes/lc0-cells/` and `spikes/lc0-device/` (#1115), `spikes/lc0-fs/` (#1117) and `spikes/lc0-worker/` (#1120).

Everything ran on 2026-10-23, on the owner's no-card TiDB Cloud organization (`aws-us-east-1`), the WorkOS **staging** environment, Workers **Free**, and a Linux workstation about 206 ms from `us-east-1`. Nothing was spent, and everything created was deleted (§12).

## 1. Summary

| Task | Result | The one-line reason |
|---|---|---|
| 0: rulings and pins | Done | `ti` 0.2.10, `@tidbcloud/serverless` 0.3.0, IA0's pins; no limit changed since the research report |
| 1: Starter tenant cells | **Go, with corrections** | Isolation held at 50, 200 and 500 tenants, and DDL cost stayed flat. But Starter shows **no per-tenant bytes and no per-tenant RUs**, and **ignores `MAX_USER_CONNECTIONS`** |
| 2: Filesystem workspaces and handoff | **Go for BYOG; no-go for the hosted cohort** | Handoff after a drain is complete, and an undrained handoff is detectable and replays on wake. Scoped tokens hold (27/27). A session costs about **77 writes ($0.04)**. Limits can't be set and writes can't be counted per file system without a card |
| 3: the control plane on a Worker | **Go on Workers Free** | Twenty devices fit Free. With auto-response pings a device costs about **112 DO requests a day**, so the account holds about 890 online devices. Writes from a DO placed near `aws-us-east-1` take 19–25 ms. Leases stay exclusive. Idempotency keys are required, because aborted writes commit |
| 4: TiDB Cloud Zero | **No for now** (Q1264 → cells) | There's an unauthenticated API, but the terms don't cover third-party provisioning |
| 5: device enrolment | **Zitadel part not run** (D1755); OpenFGA on Starter **works** | The try-out uses WorkOS CLI Auth plus control-plane device keys |
| 6: company device | **Not run** | Needs a managed-like Mac or Windows laptop; runbook in §7 |
| LC2a Task 0 | Done; **five items for the owner** | Resend key scope, root-domain DMARC shared with Brevo and Cloudflare, Resend tracking and region, bounce source, an unlicensed list source (§11) |

**Can LC2a start?** **Yes.** LC2a's prerequisite, a go from Task 3, holds. See §13 for what LC2a needs from the owner first.

## 2. Task 1: Starter tenant cells

Spike: [`spikes/lc0-cells/`](../../spikes/lc0-cells/README.md). Setup: one free Starter instance (`lc0-cells`, TiDB `v8.5.3-serverless`, spending limit 0), with N = 50, 200 and 500 tenants. Each tenant had its own database and its own SQL user, granted only on that database.

| Measure | Result |
|---|---|
| **Isolation** (`cell_grants_isolate_tenants`) | **Pass** at every N. Every one of 25 probes per pair was denied or filtered: `SELECT`/`INSERT`/`USE` on another tenant's database, `SHOW DATABASES`, `information_schema` (`SCHEMATA`, `TABLES`, `COLUMNS`, `STATISTICS`, `TABLE_STORAGE_STATS`, `DDL_JOBS`), `PROCESSLIST`, `SHOW STATS_META`, `ADMIN SHOW DDL JOBS`, `mysql.*`, `GRANT` to self. Tenants see only their own database and sessions |
| **Per-tenant storage** (`cell_storage_is_visible_per_tenant`) | **Fail for bytes:** <br>• `TABLES.TABLE_ROWS` is right per tenant, about 60–90 s after the writes. <br>• `DATA_LENGTH`/`INDEX_LENGTH` are `rows × 16` and `rows × 8`, an estimate rather than bytes (320 B reported for 4 KB of rows). `ANALYZE` doesn't help: auto-analyze is read-only on Starter. <br>• `TABLE_STORAGE_STATS` reports the **shared region**, so every tenant shows the same figure. <br>• The API's `usage` field is `null`. <br>• The billing API is per cluster |
| **Per-tenant activity** | `PROCESSLIST`/`CLUSTER_PROCESSLIST` (sessions per user) and `CLIENT_ERRORS_SUMMARY_BY_USER` work for the admin user. `SLOW_QUERY`, `STATEMENTS_SUMMARY` and `RESOURCE_GROUPS` are denied, and console only |
| **RU attribution** | **None server-side.** Only `@@tidb_last_query_info.ru_consumption` (and `EXPLAIN ANALYZE`) inside the session that ran the statement |
| **Connection caps** | **`MAX_USER_CONNECTIONS` is accepted and ignored:** a user limited to 1 opened 4 sessions, and nothing stores the limit. Workaround measured: count sessions in `CLUSTER_PROCESSLIST` and `KILL` the extras |
| **Credential rotation** | `ALTER USER … IDENTIFIED BY`: the old password is refused 1.1 s later. `PASSWORD EXPIRE`: the next login gets 1862. `ACCOUNT LOCK`: new logins get 3118 at once, but **existing sessions survive until killed**, and `KILL` from `PROCESSLIST` ends them |
| **The spending limit** | Not driven to exhaustion: 50M RUs of synthetic load wasn't worth it. A free instance reports `spending_limit.monthly = 0`; the documented behaviour (deny new connections, throttle existing ones) stands. LC2a Task 11's budget week watches it |
| **DDL cost** | Flat to 500 tenants: `CREATE DATABASE`, `CREATE USER` and `GRANT` each take about 0.3 s p50 (one round trip plus about 100 ms), so about 0.9 s to provision a tenant serially. Dropping 500 tenants took about 6 minutes |
| **Connections** | A new TLS connection takes about 1.3 s from 206 ms away. Bursts of new connections had an 8 s p95 at the start of a phase, which looks like gateway pacing. Devices should pool connections |

**The cell size:** the tenant count isn't what limits a cell; the instance's free share is (5 GiB, 50M RUs a month, 400 connections).
- At §15.3's quotas (100 MiB, 1M RUs, 10 connections), a free cell holds about **50** tenants.
- At D1717's 200, each tenant gets about 25 MiB, 250k RUs and 2 connections.
- **Recommendation:** keep **200 as the maximum** and set the try-out quota to fit (§9).
- With 4 free cells at $0 (D1756), the try-out holds **800 tenants at 25 MiB each, or 200 at 100 MiB**.

**Go for cells**, with three corrections:
- the Meter measures bytes itself (row counts from `TABLE_ROWS` times a sampled row size);
- the connection cap is a watcher that kills sessions;
- the kill switch is `ACCOUNT LOCK` plus a kill sweep.

**Also checked (Task 5's TiDB question):** OpenFGA v1.22.0's MySQL datastore runs on Starter. Migrations, writes, 100 correct checks, `list-objects` and a revoke all worked ([`spikes/lc0-device/`](../../spikes/lc0-device/README.md)). Latency is usable only from a host near the instance.

## 3. Task 2: Filesystem workspaces and handoff

Spike: [`spikes/lc0-fs/`](../../spikes/lc0-fs/README.md). Setup:
- one no-card file system, `lc0-fs`;
- machine A: this Linux host;
- machine B: a rootless podman container with its own `/dev/fuse`, `HOME` and `~/.ti`;
- writes counted by a local logging proxy, because nothing else counts them.

The macOS leg wasn't run; it needs the owner's Mac (runbook in the spike's README).

| Measure | Result |
|---|---|
| **Writes per agent session** (30 edits, `npm ci` + `tsc`, a commit, one drain, in a Git workspace) | **77 writes** and 42 reads. That is **$0.04** a session, so about 125 sessions fit in the $5 monthly allowance |
| **Files created** | 0: Git-workspace content lives in an overlay and doesn't count toward the 2,000-file limit |
| **Drain latency** | Layer mount: 0.02–0.08 s. A mount holding a Git workspace: 4.9–6.4 s, and **14 writes per drain even when idle**. After a 20 MiB write: 26.5 s |
| **Propagation** | B sees a closed file about **2.5 s** after the close, with or without a drain (drive9 uploads 2 s after close) |
| `handoff_after_drain_is_complete` | **Pass:** drain → checkpoint (0.4 s) → fork on B (0.9 s) → mount (6–8 s); all 17 files matched. Critical path **about 10–12 s** |
| `undrained_handoff_is_detected` | **Pass:** after `kill -9`, B's fork lacks the unsynced edits. A keeps them in a local shadow cache and replays them into **its own** layer on remount. `diff-layer` shows the divergence. Nothing was lost silently |
| `scoped_token_cannot_escape_prefix` | **Pass, 27/27:** `/t/b`, prefix confusion (`/t/ab`), `..`, listing `/`, mounting `/`, delete without the grant, moves and copies across prefixes, minting tokens and forking are all denied. **Disable takes 0.41 s** to bite after the call returns (the docs say about 10 s). A live mount fails at once with `EACCES` |
| **Limits** | **Files:** the counter lags 30–60 s, so the limit overshot to 2,012; then 507. **Bytes:** checked before upload against the exact delta, so no overshoot; 507 at 2 GiB, and a 501 MiB file was refused. Reads kept working. **Through the mount, `close()` returns 0 while the upload fails**: only the drain counters and the mount log show it |
| **Usage by API** | Per file system: storage bytes, file count and limits, through the undocumented `ti-drive9 admin tenant get` with the org key. `set-quota` returns **402** without a card. **No per-file-system read or write counts anywhere**; the billing API is per organization and lags by an hour or more |
| `teardown_leaves_nothing` | **Pass** (6/6) |

**What this changes in §78:**
- **Layers and Git workspaces don't compose.** Edits in a layer mount of a Git workspace go to the workspace's shared overlay, so the layer isolates nothing. Handoff therefore uses:
  - a layer over a plain tree, for the working files;
  - Git (a WIP ref, or hydrating the workspace) for history.

  §9.4 steps 2–3 and D1713's "a layer per run" need that split.
- **Scoped tokens can't fork layers or call the Git-workspace APIs.** With device-scoped tokens, the control plane (or the user's own CLI session, for BYOG) does the fork, and the device fetches the WIP ref.
- **D1713's 5-minute drain:**
  - Drop the timer.
  - Keep the drains at handoff and at approval.
  - Drain only when the mount is dirty.

  Uploads already happen about 2 s after close, and a timed drain on a Git workspace costs 14 writes for nothing (168 writes, or $0.08, an active hour).
- **Agents' `ls`, `find` and `git status` on a layer mount** often get `EAGAIN` right after writes. agentd retries.
- **A drain error isn't a durability verdict** (`uploader_cached` or `commit_queue_conflicts` can stick). Check what the counters say.

**Q1263, the hosted cohort: no-go at $0**, as §25.13 already rules.
- Without a card, limits can't be set per file system, and only one file system is allowed per region, so the cohort can't exist.
- With a card there is no spending cap and no per-file-system write counter. D1718's 2,000-write budget would need Loams to count writes itself.

**BYOG workspaces: go.**

## 4. Task 3: the control plane on a Worker

Spike: [`spikes/lc0-worker/`](../../spikes/lc0-worker/README.md), #1120. Setup:
- the Worker `loams-cloud-lc0` on `workers.dev`, on **Workers Free**;
- `@tidbcloud/serverless` 0.3.0 against a free Starter instance (`lc0-worker`);
- one SQLite-backed Durable Object per user, in three variants: plain hibernation, hibernation with `setWebSocketAutoResponse`, and no hibernation;
- 20 simulated devices per user, pinging every 30 s, for 60 minutes per variant. The DO figures come from Cloudflare's analytics.

| Measure | Result |
|---|---|
| **Single-statement write latency**, p50 / p95 | From the edge Worker (colo MAA, the driver in India): 274–286 / 286–300 ms. **From a DO created with `locationHint: "enam"`: 19–25 / 24–29 ms.** The first request costs up to 1.6 s from the Worker, and 111 ms from the DO |
| **Idempotency keys under retries** | **Correct:** 60 keys gave 60 rows, through lost responses, client aborts and concurrent duplicates. A reused key with a different payload gets `409`. The pattern is one statement, `INSERT … ON DUPLICATE KEY UPDATE`, with `rowsAffected` telling new from duplicate |
| **The driver on transient errors** | No timeout and no retries of its own. A wrong password gives 401; a wrong host gives 530; a timeout needs an injected `fetch` with `AbortSignal.timeout`. **A write aborted at 5–250 ms committed 8 times out of 8, so a timeout never means "not written"** |
| **DO requests a device a day** | Plain hibernation: **2,960** (every ping is a request). **Auto-response: 112** (2,390 pings an hour caused 0 invocations). No hibernation: 68 HTTP requests plus 2,884 messages, and **11,428 GB-s a day for one user**, 88 % of Free's daily cap |
| **A device a month** | Auto-response: about **3,350 DO requests and 1,000 Worker requests** (WebSocket upgrades after reconnects) |
| **Worker CPU** | p50 1.1–1.5 ms, p99 3.4 ms, against Free's 10 ms; no CPU-limit errors |
| **Do 20 devices fit Free?** | **Yes**, in every variant but no-hibernation. One 20-device user on plain hibernation takes about 59 % of the account's free daily DO requests, if Free counts messages 1:1 (the docs don't say). The account's free requests hold about **890 online devices with auto-response pings**, shared with the loams.dev site Worker |
| `lease_is_exclusive_under_races` | **Pass:** exactly one winner in every round, both in the DO and in TiDB's conditional single statements |
| `device_reconnects_after_sleep` | **Pass:** a 36–40 s sleep keeps the lease. After 80 s the device is `asleep` and the lease can move (epoch 2), and the sleeper sees it lost the run |
| `teardown_leaves_nothing` | **Pass.** The Worker, its DO namespaces and the instance are gone, checked through both APIs |
| `connect()` to Starter's MySQL port (an input to Q1304) | **Works on Free without Hyperdrive:** 413–588 ms to set up, then 61–88 ms per `SELECT 1` from CDG. There's no pooling across invocations |

**Surprises:**
1. **Hibernation alone saves nothing on requests.** Only auto-response pings do. Presence and lease renewal then read `getWebSocketAutoResponseTimestamp`.
2. **No hibernation means one user per account**, because duration is billed for the whole connection.
3. **Placement dominates latency.** The per-user DO has to be created with a `locationHint` near the control DB, and the writes go through it.
4. **Half-dead WebSockets are common.** Sockets stay `OPEN` with nothing flowing: 75 % within a minute on this host's IPv6 path, and about 1.25 reconnects per device an hour on IPv4. Presence must come from last-seen times, never from "socket open". Clients reconnect on a missed pong, and upgrades retry.
5. **Aborted writes commit,** so idempotency keys or fences on every write are mandatory.

**Requests per device per month:** about 3,350 DO requests and 1,000 Worker requests, with auto-response pings.

**Workers Free or Paid (D1727 against D1756):** **Free**, up to about 890 online devices across the account. Q1292's trigger (Workers Paid at $5 a month) is reached by device count, not by users. **Go.**

## 5. Task 4: TiDB Cloud Zero for try-out SQL

Research only. No agent created a Zero database. The one manual creation and claim is the owner's (steps below).

| Question | Answer | Source |
|---|---|---|
| Is there an API, or only the SKILL? | **An API.** `POST https://zero.tidbapi.com/v1beta1/instances`, with an optional body `{"tag":"…"}`. "No API key, token, or Authorization header is required." It returns `connection{host, port 4000, username, password}`, a `connectionString`, `claimInfo.claimUrl` and `expiresAt`. There is no renewal call. The SKILL is a prompt that tells an agent to call this API | [SKILL.md](https://zero.tidbcloud.com/SKILL.md) v0.1.2; [zero.tidbcloud.com](https://zero.tidbcloud.com/) FAQ (V) |
| Can a third party create Zero databases for its users, programmatically? | **Technically, yes:** one unauthenticated `fetch`, and PingCAP pitches it for "automation", "MCP tooling" and "your app or agent". **The terms don't cover it.** There are no Zero-specific terms. The public-preview terms allow use "solely for internal evaluation purposes", if Zero counts as a preview (it isn't labelled one). The Cloud Services Agreement (updated 2026-09-01) bars "service bureau, outsourcing" use (§3.2(e)) and resale (§3.2(g)) | [Preview terms](https://www.pingcap.com/legal/tidb-cloud-public-preview-terms-and-conditions/); [CSA](https://www.pingcap.com/legal/tidb-cloud-services-agreement/) (V) |
| Rate limits | **None published.** Each database gets the Starter free quota plus a hard $1 spending limit, then refuses new connections. It lives 30 days | FAQ, SKILL.md (V); the rate limits are not verified |
| How does a claim work? | The user opens `claimUrl`, signs in to TiDB Cloud, and clicks Claim. The database becomes a Starter instance in their organization, with "schema, and data … exactly the same", and uses one of that organization's 5 free instances. **The claim link is a bearer link:** whoever opens it first claims the database. After expiry the link stops working and the data is removed | [Launch blog](https://www.pingcap.com/blog/tidb-cloud-zero-public-preview/) (V) |

**Is Q1264's Zero option allowed and practical?** **It isn't allowed as things stand. Q1264 resolves to Ostrium's cells** (§2).
- The terms don't cover third-party provisioning, and the service-bureau clause may apply.
- There are no published rate limits and no SLA. The API is v1beta1.
- Ostrium would hold each user's credentials and a bearer claim link until the user claims.

Zero stays attractive, because Ostrium would hold no tenant SQL at all. Revisit if PingCAP confirms in writing (the product page invites partnership questions at zero@pingcap.com).

**The owner's manual run** (pending):
1. On your own machine: `curl -sS -X POST https://zero.tidbapi.com/v1beta1/instances -H 'Content-Type: application/json' -d '{"tag":"loams-lc0-owner"}' > zero.json`. Note `expiresAt`.
2. Connect with `mysql --ssl-mode=REQUIRED` and the `connectionString`. Create a table, insert a row, and run `SELECT tidb_version()` and `SHOW GRANTS`.
3. Open `claimInfo.claimUrl`, sign in to the no-card LC0 organization, and click **Claim**. Run the activation instruction it gives.
4. Confirm the instance shows up as Starter and the row survived. Record whether the host or credentials changed, whether the claim used one of the 5 free slots, and the spending limit it got.
5. Delete the instance. Optionally, email zero@pingcap.com to ask whether Loams Cloud may create Zero databases for its users and hand them the claim link, and what rate limits apply.

## 6. Task 5: device enrolment

- **The Zitadel part was not run** (owner, 2026-10-23: "Zitadel isn't needed now"; R-LC0-5).
  - On the hosted try-out, device identity is WorkOS CLI Auth for the person, plus device keys and 15-minute tokens issued by the control plane (D1755).
  - WorkOS staging's device endpoint answers: `POST /user_management/authorize/device` returns a device code and a user code, with `expires_in: 300` and `interval: 5`.
  - The three Zitadel tests (`device_flow_issues_tokens`, `deactivated_device_is_denied`, `device_cannot_act_for_another_org`) move to whichever phase builds the self-hosted device path (D1714 as written), on IA0's compose.
  - IA0's report measured machine-user deactivation (denied within 1.1 s). It didn't exercise the OIDC device grant, so §9.1's "(verify)" stays open for that phase.
- **OpenFGA's MySQL datastore on Starter: works** (§2, last paragraph).


## 7. Task 6: the company-device check

**Not run.** It needs a managed-like macOS or Windows laptop: a user account without admin rights, behind an HTTPS proxy with a re-signing CA. No agent has one. The table is §78 §10's, row by row, with what LC0 could establish without that laptop and the check the owner runs. Every row's verdict waits for that run.

| §10 row | Established without the laptop | The check on the laptop |
|---|---|---|
| Admin rights: none, a per-user service (D784) | — | Install Loams Desktop's per-user daemon as the standard user. Pass: no UAC, sudo or Authorization Services prompt, and a LaunchAgent or HKCU Run entry rather than a system service |
| Network client: outbound 443 only | Task 3: the control plane is a Worker on `workers.dev`, reached over HTTPS and WSS on 443 only | Set `HTTPS_PROXY` to the re-signing proxy (for example mitmproxy, its CA in the OS store). Pass: the daemon's WebSocket and HTTPS calls go through the proxy and succeed |
| MySQL over TLS on 4000 through the proxy, or the HTTP fallback | Task 1: MySQL/TLS on 4000 needs a raw TCP path. An HTTP proxy carries it only by `CONNECT` to port 4000, which most corporate proxies refuse (they allow 443). Task 3: the HTTP driver (`@tidbcloud/serverless`) works over 443 | Run `mysql --ssl-mode=VERIFY_IDENTITY -h gateway01.us-east-1.prod.aws.tidbcloud.com -P 4000` through the proxy, then the HTTP-driver call. Expected: MySQL fails and HTTP passes. Then the fallback must be automatic |
| The corporate CA from the OS store | — | With the proxy re-signing, the daemon must trust its CA from the OS store (Keychain, the Windows store) without a flag, and pin only `auth.loams.dev`'s issuer, never a leaf |
| Inbound: none, except loopback | — | `lsof -iTCP -sTCP:LISTEN` (macOS) or `Get-NetTCPConnection -State Listen` (Windows). Pass: only `127.0.0.1` or `::1` |
| Existing corporate Tailscale untouched; `tsnet` joins a second tailnet | Report §4.3: a device is on one tailnet at a time (V) | With the system Tailscale client on tailnet A, start a test daemon with `tsnet` on tailnet B (`TS_AUTHKEY` for B). Pass: the system client stays on A, the daemon's node appears in B, and the two reach their own peers only |
| Data leaving the device: off for `--work` | — | Enrol with `--work`. Pass: no transcript, workspace or SQL upload is attempted (proxy log) |
| Background activity, telemetry | — | No wake locks; the tray icon shows running agents; no telemetry hosts in the proxy log |
| Removal | — | `loams cloud device remove`, then uninstall. Pass: the per-user files listed in §50 §5 are gone and the login item is removed |
| MDM key `CloudSync=off` | — | Deferred to LC2 (§10) |

**What blocks it:** the per-user daemon exists in Loams Desktop (D784), but `loams cloud device add` and the device link are LC2 work. Before LC2 the laptop run can check only the rows that need no control plane: admin rights, the proxy and CA for HTTPS, MySQL versus HTTP through the proxy, the listeners, `tsnet`, and removal. **Recommendation:** run those six on the owner's work-like laptop before LC2's device link is built, and the rest as part of LC2's exit.

## 8. Design 78's (verify) items

The items Task 0 of LC0 and LC2a own. The WorkOS, Resend and domain-list items are answered in the plan's R-LC2a-1 to R-LC2a-11 and only summarised here.

| § | Item | Answer | How |
|---|---|---|---|
| 9.1 | Zitadel supports the OIDC device grant | **Open**: Task 5's Zitadel part not run (§6). The try-out uses WorkOS CLI Auth, which works on staging | Live (WorkOS) |
| 9.2, 25.13 | Hibernating WebSockets make an idle device cheap; does the 20:1 ratio apply to the free daily limit? | **Measured.** Plain hibernation still bills every ping as a DO request: about 2,960 a day per device at 30 s pings. The docs don't say whether Free's daily limit counts messages at 20:1, and the analytics count them 1:1, so plan for 1:1. **`setWebSocketAutoResponse` pings cost nothing** (0 invocations for 2,390 pings): about 112 requests a day per device. Without hibernation, one user uses 88 % of the daily GB-s | Task 3, measured |
| 10 | Proxies allow MySQL on 4000; the HTTP driver is the fallback | MySQL over 4000 needs a `CONNECT` to port 4000, which corporate proxies usually refuse. **The HTTP driver works from a Worker over 443** (Task 3). The proxy row itself is in Task 6's runbook | Task 3; Task 6 pending |
| 13.2 | `meter`: the billing API's scope | Works with the org key: monthly totals by project and service, daily rows **per cluster**, and a cost explorer. Nothing below a cluster | Live |
| 13.3 | TiDB Cloud Zero: terms for third-party provisioning | **Not covered by any terms** (§5) | Docs and terms |
| 13.3 | Durable Objects' free limits | 100,000 requests a day (WebSocket messages included), 13,000 GB-s, 5M rows read and 100k written a day, 5 GB; SQLite-backed only | Docs (R-LC2a-9); the rate-limiting binding deploys and enforces on Free (Task 3, measured) |
| 13.3 | Tunnel at $0 | "Available on all plans", with no price | Docs |
| 13.3 | Access free up to 50 users | Zero Trust Free has 50 users, but **onboarding asks for payment details**, so Access isn't used at $0 | Docs |
| 13.2 | `cloudflare` `workspace` slot (Sandbox / Containers disk + R2) | Not LC0's: LC3 | — |
| 13.3 | Vercel Blob and Cron; Google's e2-micro; Azure's B1s | Not LC0's: LC4 and LC5 | — |
| 14.1 | Per-device credentials expire with `ALTER USER … PASSWORD EXPIRE` | **Works** (1862 on the next login). Rotation by `IDENTIFIED BY` takes about 1.1 s | Live |
| 14.6 | Per-database storage from `information_schema.TABLES` | **Row counts only.** `DATA_LENGTH` isn't bytes, and `TABLE_STORAGE_STATS` is per region (§2) | Live |
| 14.6 | Billing API granularity | Per cluster and per day (above) | Live |
| 15.1 | `MAX_USER_CONNECTIONS` enforced | **No:** accepted and ignored | Live |
| 15.2 | Per-file-system limits by API | Readable through the undocumented `ti-drive9 admin tenant get`. Setting them returns 402 without a card. No write counters | Live |
| 16 | The TiDB Terraform provider on the OpenTofu registry | Listed: `tidbcloud/tidbcloud` 0.4.11 | Registry API |
| 17 | OpenFGA's MySQL datastore on TiDB X | **Works** on Starter | Live |
| 25.2 | Resend: commercial use on Free | Allowed (ToS §6). 100 emails a day, 3,000 a month, one webhook | Docs |
| 25.2 | AuthKit branding is free | Yes; only a custom domain is billed | Docs |
| 25.4 | Disabling password and passkeys on the free tier | Per-environment toggles with no tier gate. Whether the dashboard insists on at least one method wasn't checked | Docs |
| 25.4 | Does the action context name the method? | **Yes:** `authentication_method`, and the registration context carries the `invitation` | SDK |
| 25.4 | `authentication_method` for social sign-ins | `GoogleOAuth`, `GitHubOAuth`, `MicrosoftOAuth`, `MagicAuth`; the union is larger, so the policy is an allow-list | SDK |
| 25.5 | GitHub `idp_id` is the numeric user id | Yes (SDK fixture resolved on GitHub); LC2a Task 11 confirms live | SDK, GitHub API |
| 25.6 | Resend's region and subprocessors | Account data in the US; this domain sends from `ap-northeast-1`; 22 subprocessors, all in the USA | API, docs |
| 25.7 | Google or Microsoft give an account-age signal | No | Live, reference |
| 25.8 | The MFA API is free | Yes, inside AuthKit's free tier; use the user-scoped factors API | Docs, live |
| 25.9 | Does resend change the expiry? | **No:** resend keeps the token, the URL and `expires_at`. There is no `invitation.expired` event | Live (staging) |
| 25.11 | WorkOS's current data location | USA, no selectable region; the DPA uses SCCs | Docs |
| 25.11 | Commercial use of WorkOS Free | Not banned, but it may change at any time. §2(a) "internal business purposes" and §5 "service bureau" go to counsel | ToS |
| 25.13 | Cron Triggers on Free | 5 per account, shared with the site Worker | Docs |

## 9. Recommended values

- **The cell size (D1717):** at most **200** tenants a cell. The quota decides how many a cell really holds.
- **Q1263, the hosted workspace cohort:** **off.** This confirms Q1291 and D1756 with measurements: it can't be built without a card, and with one there is no spending cap and no per-file-system write counter. Revisit only with a budget **and** Loams' own write counter. Each user's own TiDB organization (BYOG) gets its own $5, about 125 sessions a month.
- **Q1264, try-out SQL:** **Ostrium's cells.** Zero isn't covered by any terms (§5).
- **Q1265, quotas and the ceiling, at the approved $0:**

  | Per personal org | §15.3 default | Recommended at $0 | Why |
  |---|---|---|---|
  | Try-out SQL databases | 2, 100 MiB in total | **2, 25 MiB in total** | 200 tenants in a free cell's 5 GiB |
  | RUs | 1M a month | **250k a month** | 200 tenants in a free cell's 50M |
  | Concurrent SQL connections | 10 | **2, by watcher** | 400 a cell; `MAX_USER_CONNECTIONS` isn't enforced |
  | Hosted workspace | 1 GiB, 5,000 files, 2,000 writes | **None (BYOG only)** | Q1263 |
  | Devices | 5 | 5 | With auto-response pings, the account's free DO requests hold about 890 online devices, shared with the site Worker |
  | Control-plane API | 60 calls a minute per device | 60 | — |
  | The ceiling (D1730) | $50 | **$0** (Q1291) | — |

  The tight quotas apply while the try-out is at $0. With the higher quotas, the try-out holds **200** users; with these, **800**. Either way, "the try-out is full" is the honest message past that point, and BYOG is offered (§7.2).

## 10. A costed estimate of LC2

All figures are estimates, at list prices read on 2026-10-23. "Full" means every user at the full quota. "Typical" means 10 % of it, the planning assumption for a try-out, where most accounts are idle. Starter's free quota covers only the first 5 instances. Every instance past those is billed from its first RU ($0.10 per 1M RUs, $0.20 per GiB-month).

| | 100 users | 1,000 users | 10,000 users |
|---|---|---|---|
| **TiDB Starter cells** (25 MiB and 250k RUs each) | 1 free cell: **$0** | 5 cells: 4 free plus 1 paid. Full: about **$6**; typical: under $1 | 50 cells, 46 of them paid, so a card and spending limits. Full: about **$225** in RUs plus about **$44** in storage; typical: about **$27** |
| **TiDB Starter cells** (100 MiB and 1M RUs each, §15.3) | 2 free cells: **$0** | 20 cells, 16 of them paid. Full: about **$96**; typical: about **$10** | 200 cells, **over the 100-instance cap** (an Instance Capacity Plan through Sales). Full: about **$1,200** |
| **Control DB** | Free instance | Free instance | Free instance; RUs may pass its free 50M (estimate) |
| **Workers and Durable Objects** | Free: about 200 devices (2 a user, estimate) is about 22,000 DO requests a day: **$0** | About 2,000 devices is about 224,000 DO requests a day, **over Free**. Workers Paid: $5, plus about $1 in DO requests: about **$6** | About 20,000 devices. Workers Paid: $5, plus DO requests (about $10), DO duration (about $9) and Worker requests (about $3): about **$27** |
| **WorkOS AuthKit** | $0 | $0 | $0 (free to 1M MAU) |
| **Resend** | $0 | $0, but at 60 invitations a day, inviting 1,000 people takes about 17 days | Past 100 emails a day: magic codes alone exceed it. Resend's paid plan, about $20 a month (verify), is an owner decision (Q1290) |
| **Filesystem** | $0 (off) | $0 (off) | $0 (off) |
| **Total, typical** | **$0** | **$0 to about $10** plus **$6** for Workers Paid once users average 2 online devices | **About $50–300 a month** |
| **Against D1730's $0 ceiling** | Fits | **Doesn't fit at 2 devices a user.** Free's DO requests hold about 890 online devices (about 450 users), and the cells hold 800 accounts at 25 MiB | **Doesn't fit.** Needs a budget, a card on the TiDB organization, and probably Workers Paid |

**Reading it:**
- At $0, the try-out tops out at about **800 accounts** (the cells), and earlier if users average more than one online device: Free's DO requests hold about 890 online devices with auto-response pings (Task 3), shared with the site Worker. That is about 450 users at 2 devices each.
- Each step past that is the owner's decision, in order:
  1. a card on the TiDB organization, with spending limits per cell;
  2. Workers Paid ($5 a month), on Q1292's trigger;
  3. Resend's paid plan.

## 11. Changes design 78 needs (for the owner)

None changes the architecture. Each is a correction found by measurement.

1. **§14.6 and D1721, metering:**
   - Starter gives **no per-tenant bytes and no per-tenant RUs**. The Meter computes storage from `TABLE_ROWS` times a sampled row size.
   - It can attribute RUs only for SQL the control plane runs itself (`tidb_last_query_info`). Devices' own SQL can't be metered per tenant, so the cell's hard cap is the only RU guard.
2. **§15.1, fairness:** `MAX_USER_CONNECTIONS` isn't enforced on Starter. The per-tenant cap becomes a watcher that counts `CLUSTER_PROCESSLIST` sessions and kills the extras.
3. **§15.4, the kill switch:** `ACCOUNT LOCK` doesn't end live sessions. The kill switch is lock, then a kill sweep.
4. **§15.1, §15.3 and Q1265, cell size and quotas:** a free cell holds about 50 tenants at §15.3's quotas, or 200 at the quotas in §9. At $0 that is a ceiling of 200 or 800 accounts.
5. **§9.4, §14.5 and D1713, handoff:**
   - Layers don't isolate Git-workspace content, so a run is a layer over a plain tree, with Git history carried by a WIP ref.
   - Scoped tokens can't fork layers or use Git-workspace APIs, so the fork is done by the control plane or by the user's own session.
   - Drop the 5-minute timed drain; drain at handoff and approval, and only when the mount is dirty.
   - A failed upload behind `close()` returning 0 has to be surfaced from the drain counters.
6. **§15.2 and D1718, the cohort:** confirmed off. Even with a card, no per-file-system write counter exists, so D1718's write budget needs Loams' own counting.
7. **§25.2, Q1278 and D1738, email** (owner ruling, recorded as R-LC2a-1):
   - The sender is `cloud@loams.dev`.
   - "One sending domain, one reputation" doesn't hold: Brevo and Cloudflare Email Routing share loams.dev, and `mail.loams.dev` is Brevo's.
   - The DMARC plan waits on alignment for all three senders.
   - Turn Resend's open and click tracking off.
8. **§25.9, bounces:** WorkOS gets no delivery results from a custom provider. Resend's webhook is the only bounce source, for all mail.
9. **§25.4, the policy:**
   - The allow-list is the four `authentication_method` values. `Impersonation`, `CrossAppAuth`, `ExternalAuth` and `MigratedSession` are refused too.
   - The registration action matches `context.invitation.id`.
   - Add "impersonation off" to the checklist.
10. **§25.5 and §25.9, expiry:** there is no `invitation.expired` event, so expiry comes only from the hourly sweep. Resend never extends an invitation.
11. **§25.6, privacy:** WorkOS's event history keeps a deleted user's email for up to 90 days. Say so in the notice.
12. **D1753, lists:** the HubSpot free-mail source has no licence. Counsel decides, or we build our own free-mail list.
13. **§17, OpenFGA:** its MySQL datastore works on Starter, but it needs a host near the instance, so the default (Postgres beside Zitadel) stands.
14. **§9.2, §17 and §25.13, the device link:**
    - pings use `setWebSocketAutoResponse`, since plain hibernation bills every ping, and no hibernation uses up the daily GB-s with one user;
    - presence comes from last-seen times, never from "socket open";
    - the per-user DO is created with a `locationHint` near the control DB, and the writes go through it: 19–25 ms, against 270–290 ms from a distant edge colo;
    - every write carries an idempotency key or a fence, because aborted HTTP-driver writes commit;
    - §25.13's "several hundred online devices" becomes about 890 with auto-response pings, shared with the site Worker.

    Also, `connect()` to Starter's MySQL port works on Free without Hyperdrive, an input to design 79's Q1304.

## 12. What was created and deleted

| Provider | Created | Deleted | Left |
|---|---|---|---|
| TiDB Cloud (no-card org) | Starter `lc0-cells` (Task 1, labelled `lc0`); Starter `lc0-worker` (Task 3); file system `lc0-fs` (Task 2); 1 scoped Filesystem token; the `ti` SQL users for each instance | All of them | **Nothing** (`list-db-clusters` and `list-file-systems` empty). Billing API: $0 |
| WorkOS **staging** | 1 test user (`lc0-test-1@example.net`); 1 invitation (`lc0-resend-probe@example.com`) | The user was deleted. The invitation was revoked (WorkOS can't delete invitations). No email was sent | 1 revoked invitation; events in WorkOS's 90-day history |
| Resend | Nothing (read-only calls) | — | — |
| Cloudflare (Workers only) | Worker `loams-cloud-lc0` on `workers.dev`, with its Durable Object classes and secrets | The Worker, its DO namespaces and its secrets (`wrangler delete`). `teardown_leaves_nothing` passed, and a later `GET …/workers/scripts` shows no `lc0` script | **Nothing.** No other script, route, domain, DNS record, KV namespace, R2 bucket or setting was touched |
| Local | `ti` 0.2.10 and `ti-drive9` in `~/.ti/bin`; podman images | Spike credentials, containers, the OpenFGA container, mounts, locators and the spike CA | `~/.ti/bin` (the tools). The OpenFGA image may be removed |

**The Cloudflare API calls** (method and path, from Task 3's log; the token is never logged):
- **Every write targeted `loams-cloud-lc0`:** 24 POST, 5 PATCH, 3 PUT and 1 DELETE, made by `wrangler deploy`, `wrangler secret` and `wrangler delete` on `…/workers/scripts/loams-cloud-lc0/*`, `…/workers/services/loams-cloud-lc0` and `…/workers/workers/loams-cloud-lc0`.
- **Everything else was a read:**
  - `GET /user/tokens/verify` and `GET /accounts/:acct/tokens/verify`;
  - `GET …/workers/subdomain` (×2);
  - `GET …/workers/scripts` (×3, including the teardown check and the final check above);
  - `GET …/workers/durable_objects/namespaces`;
  - `GET …/storage/kv/namespaces` (wrangler's delete check);
  - `POST /graphql`, for analytics reads and schema introspection only.

The owner corrected the token's scope during the run: it has full access to Workers, not the Workers Scripts: Edit asked for. The spike stayed on its own Worker throughout.

## 13. LC2a: can it start, and what it needs

**Yes, LC2a can start.** Its prerequisite, a go from LC0 Task 3, holds (§4), and the WorkOS and Cloudflare (verify) items are answered (§8, R-LC2a-1 to R-LC2a-11). LC2a's Worker should take three things from Task 3: auto-response pings, a DO near the control DB, and idempotency keys everywhere. It should take two from Task 0: Resend as the single bounce source, and the four-method allow-list. Items 1–5 below block the parts of LC2a that send email or receive WorkOS callbacks. Tasks 1, 3 and 4 can start before them.

**What LC2a needs from the owner before code:**
1. **A sending-only Resend key** limited to loams.dev, in `~/resend.env`. The current key has full access. Keep the full-access key only in WorkOS's custom provider.
2. **Resend settings:** open and click tracking **off**; a decision on the sending region (`ap-northeast-1` now; `us-east-1` means re-adding the domain).
3. **Which WorkOS environment Resend is connected to.** If it is staging, staging mail counts against the 100 a day.
4. **WorkOS staging:**
   - redirect URIs for Miniflare and the staging Worker (or permission for an agent to add them by API);
   - Sign-up off;
   - only Google, GitHub, Microsoft and Magic Auth on;
   - impersonation off.
5. **The webhook and actions secrets** (`WORKOS_WEBHOOK_SECRET`, `WORKOS_ACTIONS_SECRET`). WorkOS shows them only when the endpoints are registered, which needs a deployed staging Worker URL. So LC2a Tasks 6 and 8 deploy a staging Worker first, the owner registers the endpoints, and then pastes the secrets into `~/workos-staging.env`.
6. **Turnstile:** a test widget's site key and secret in `~/turnstile-staging.env` (Task 3).
7. **A GitHub App token** for the age check, read-only public user data (Task 7).
8. **An owner-controlled address** for the single real send in the end-to-end run (Task 11). Every other test uses `example.com`, which WorkOS drops.
9. **DMARC:** where `rua` reports should go, and whether Brevo still sends as loams.dev.
10. **Counsel (Q1276):** WorkOS ToS §2(a) and §5; the Resend ToS; the HubSpot list; PingCAP's CSA §3.2(e) if Zero comes back.
11. **Task 6's laptop run** and **Task 4's Zero claim**: owner runs; see §5 and §7. Neither blocks LC2a.
12. **Revoke the LC0 credentials** when LC0 is closed: the TiDB org API key (R-LC0-2), the Cloudflare token (its scope is wider than asked: full Workers access), and the full-access Resend key from `~/resend.env`.
