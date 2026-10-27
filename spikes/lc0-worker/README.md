# LC0 Task 3 — the control plane on a Worker (spike)

Spike for [LC1 plan](../../docs/plans/2026-10-23-lc1-loams-cloud.md) Task 3 and design 78 §9.2, §17 and §25.13. **Not product code.** It is `"private": true`, it is outside the pnpm workspace, and it never runs in CI, because it needs the owner's Cloudflare and TiDB Cloud accounts. LC1's first task deletes it or keeps it as a fixture.

## What it is

- **`src/worker.ts`**: a Worker named `loams-cloud-lc0`, served on `workers.dev` only. Every route requires `Authorization: Bearer <LC0_BEARER>`, a Worker secret. It runs:
  - **TiDB routes** through `@tidbcloud/serverless` 0.3.0 (2026-03-17), against a free Starter instance named `lc0-worker`:
    - `/bench`: single-statement latency;
    - `/intents`: idempotency keys;
    - `/db-lease/*`: leases decided by TiDB's conditional single statements;
    - `/dberr`: the HTTP driver's behaviour on errors;
    - `/tcp`: an optional `connect()` probe of the MySQL endpoint.
  - **One Durable Object per user** (SQLite-backed, the only kind on Workers Free), in three classes so analytics separate them:

    | Class | WebSockets | A ping… |
    |---|---|---|
    | `UserHib` | hibernating (`ctx.acceptWebSocket`, `webSocketMessage`) | wakes the object |
    | `UserAuto` | hibernating, plus `setWebSocketAutoResponse("ping" → "pong")` | is answered by the runtime without waking the object; presence and lease renewal read `getWebSocketAutoResponseTimestamp` |
    | `UserNoHib` | ordinary (`ws.accept()` and listeners) | is handled in memory; the object never leaves memory |

  - **How the object works:**
    - It keeps presence (online / asleep / offline, §9.2) in each socket's attachment, not in storage. A ping therefore writes no rows.
    - It decides leases synchronously in its own SQLite, with no `await` between the read and the write.
    - It records each new lease epoch in TiDB with a fenced upsert (`GREATEST(epoch)`).
    - A lease lasts 60 s from its holder's last ping.
- **`driver/`**: the local driver (Node 26, TypeScript run by type stripping).
  - `drive.ts`: 20 simulated devices for one variant.
  - `bench.ts`: latency.
  - `errors.ts`: idempotency and transient errors.
  - `analytics.ts`: Cloudflare GraphQL analytics.
- **`test/control-plane.test.ts`**:
  - `lease_is_exclusive_under_races`;
  - `device_reconnects_after_sleep`;
  - `teardown_leaves_nothing`, which runs only with `LC0_TEARDOWN=1`.

## How to run

```sh
npm ci                                       # spikes/ is outside the pnpm workspace
LC0_SCRATCH=<private 0700 dir> ./setup.sh    # Starter instance, schema, deploy, secrets
export LC0_URL=https://loams-cloud-lc0.<subdomain>.workers.dev LC0_BEARER_FILE=<dir>/bearer
export NODE_OPTIONS=--dns-result-order=ipv4first   # see "Surprises" (IPv6 on this network)
node driver/bench.ts
node driver/errors.ts
node driver/drive.ts --variant hib  --user m2-hib   --minutes 60 --out results/m2-hib.json
node driver/drive.ts --variant hib  --user m2-hibal --minutes 60 --align --out results/m2-hibal.json
node driver/drive.ts --variant auto --user m2-auto  --minutes 60 --out results/m2-auto.json
node driver/drive.ts --variant nohib --user m2-nohib --minutes 60 --out results/m2-nohib.json
node --test test/
bash -c 'set -a; . ~/cloudflare-lc0.env; set +a; node driver/analytics.ts <from> <to> results/analytics.json'
LC0_CLUSTER_ID=<id> ./teardown.sh            # deletes the Worker and the instance, then teardown_leaves_nothing
```

Credentials are sourced only inside the command that needs them. The SQL connection string goes into the Worker through `wrangler secret put` from stdin. Nothing in `results/` holds a secret.

## Results (2026-10-23)

Raw files are in `results/`:
- `bench.json`, `errors.json`, `tcp.json`;
- `m2-*.json`: the driver's counts for each variant;
- `analytics-run.json`: Cloudflare GraphQL for the measurement window;
- `tests-live.txt`.

The account is on **Workers Free**. The Starter instance is in `aws-us-east-1`. The driver ran on a home connection in India.

### Single-statement latency (`@tidbcloud/serverless` 0.3.0)

Each figure is 120 samples, in ms, given as p50 / p95.

| From | `SELECT 1` | `INSERT` | upsert (`ON DUPLICATE KEY`) | `UPDATE` by PK | `SELECT` by PK |
|---|---|---|---|---|---|
| The Worker, colo MAA (Chennai) | 286 / 300 | 284 / 292 | 274 / 288 | 274 / 286 | 269 / 281 |
| A Durable Object created with `locationHint: "enam"` | 19 / 24 | 24 / 27 | 25 / 27 | 25 / 29 | 21 / 25 |

**First request:**
- From the Worker, the first request took up to 1,560 ms (TLS to the HTTP gateway).
- From the Durable Object, it took 111 ms.

**Later the same day,** the Worker ran in CDG: `SELECT 1` was 92 / 394 ms.

**Conclusion:** writes belong in the per-user Durable Object, placed near the TiDB region, and not in the edge Worker.

### Idempotency keys under retries (`driver/errors.ts`)

**The setup:**
- 60 keys, 15 in each of 4 modes. Every retry reused the same key, with exponential backoff and jitter.
- The write is one statement, `INSERT … ON DUPLICATE KEY UPDATE idem_key = idem_key`. `rowsAffected` is 1 for a new row and 0 for a duplicate. A stored payload hash turns a reused key with a different payload into a `409`.

| Mode | Attempts | Result |
|---|---|---|
| clean | 15 | 15 × `201` |
| lost response (the Worker writes, then returns `503`) | 30 | each retry `200 created:false` |
| client aborts mid-request (50–350 ms) | 30 | each retry `200 created:false`: **the Worker finished the write after the client went away, 15 of 15** |
| two concurrent sends of the same key | 30 | exactly one `201`; the other `200` |

**Totals:**
- **60 keys gave 60 rows in TiDB.** Duplicate suppression is correct.
- A reused key with a different payload returned `409`.

### `@tidbcloud/serverless` on errors (from the Worker, `/dberr`)

The driver has **no timeout and no retries of its own**. The spike passes a `fetch` wrapper with `AbortSignal.timeout`.

| Case | What the caller sees | Time |
|---|---|---|
| Wrong password | `DatabaseError`, status 401, code 49900002, "Error 1045 (28000): Access denied…" | 280 ms |
| Wrong host | `DatabaseError`, status 530, message `<none>` | 199 ms |
| Timeout (500 ms on `SELECT SLEEP(3)`) | `TimeoutError` from the injected `fetch` | 500 ms |
| No timeout, `SELECT SLEEP(3)` | succeeds | 3,272 ms |
| Syntax error | `DatabaseError`, status 400 | 269 ms |
| Write aborted after 5, 20, 40, 60, 80, 100, 150 or 250 ms | `TimeoutError` | **committed anyway: 8 of 8** |

**So a timeout never means "not written".** Every write needs an idempotency key or a fenced condition.

**The device side:**
- A missing or wrong bearer gets `401`, and the device doesn't retry.
- A client timeout raises `TimeoutError`, and the device retries.
- An unresolvable host raises `ENOTFOUND`.

### Twenty devices per user, 60 minutes per variant (pings every 30 s)

**The run:** 04:05:38–05:05:39 UTC, all four variants at once, with separate users. Each user had 20 devices, and device `d00` held one run lease throughout.

**The DO columns** come from Cloudflare GraphQL analytics, by object id. **The device columns** come from the driver.

| Per hour, 20 devices | `UserHib` (random phase) | `UserHib`, pings aligned to :00/:30 | `UserAuto` (auto-response pings) | `UserNoHib` |
|---|---|---|---|---|
| Pings sent / pongs received | 2,390 / 2,378 | 2,396 / 2,389 | 2,390 / 2,381 | 2,389 / 2,382 |
| DO invocations: `hibernation` (WebSocket events) | 2,414 | 2,446 | **41** | — |
| DO invocations: `http` (upgrades, `/state`) | 53 | 53 | 52 | 57 |
| `inboundWebsocketMsgCount` | 0 (counted as invocations instead) | 0 | 0 | 2,403 |
| DO wakes (constructor runs) | **1** (never evicted) | 129 | 24 | 1 |
| `activeTime` | 14.4 s | 15.2 s | 12.7 s | **3,720 s** |
| `duration` | 1.84 GB-s | 1.94 GB-s | 1.62 GB-s | **476.2 GB-s** |
| DO `cpuTime` | 607 ms | 498 ms | 61 ms | 449 ms |
| SQLite rows written (spike bookkeeping: wake counter, last-seen on close) | 86 | 211 | 105 | 91 |
| WebSocket upgrades (Worker requests) | 47 | 48 | 48 | 51 |
| Reconnects / dead sockets found by pong timeout / failed upgrades | 25 / 8 / 2 | 25 / 5 / 3 | 24 / 5 / 4 | 29 / 5 / 2 |

**For the Worker as a whole** in the window (all variants, plus checks): 184 requests, CPU p50 1.1–1.5 ms and p99 3.4 ms, no CPU-limit errors. Free's limit is 10 ms.

#### Extrapolated, per device

These are the hour above × 24 (or × 720 for a month), divided by 20 devices. Steady state counts reconnects only, not the first connect.

| Per device | Hibernation, ping handled | Hibernation, aligned | **Auto-response** | No hibernation |
|---|---|---|---|---|
| DO requests a day, actual (analytics) | 2,960 | 3,000 | **112** (includes this driver's `/state` calls) | 68 HTTP + 2,884 messages |
| DO requests a day, billed at 20:1 (Paid rule) | 208 | 210 | 112 | 212 |
| DO requests a month, actual | 88,800 | 90,000 | **3,350** | 88,600 |
| DO duration a day | 2.2 GB-s | 2.3 GB-s | 1.9 GB-s | **11,428 GB-s per user**, whatever the device count |
| DO CPU a day | 0.73 s | 0.60 s | 0.07 s | 0.54 s |
| Worker requests a day (upgrades after reconnects) | ≈ 34 | ≈ 34 | ≈ 34 | ≈ 37 |
| Worker requests a month | ≈ 1,000 | ≈ 1,000 | ≈ 1,000 | ≈ 1,100 |

#### Against the Workers Free limits

The limits are those of the account, shared with the loams.dev site Worker.

| Limit | Plain hibernation | Auto-response | No hibernation |
|---|---|---|---|
| DO 100,000 requests a day | **≈ 33 devices** if Free counts messages 1:1 (the docs don't say; see below), or ≈ 480 at 20:1 | **≈ 890 devices**, whatever the ratio | ≈ 33 |
| DO 13,000 GB-s a day | ≈ 5,900 devices | ≈ 6,700 devices | **1 user** (88 % of the cap) |
| Workers 100,000 requests a day | ≈ 2,900 devices | ≈ 2,900 devices | — |
| 10 ms CPU per request | holds (p99 3.4 ms) | holds | holds |
| DO 100,000 rows written a day | holds only because a ping writes **no** rows; presence lives in socket attachments. One row per ping would cap the account at ≈ 34 devices | same | same |

**Twenty devices fit Free in every variant except no-hibernation.** With plain hibernation, though, one 20-device user takes ≈ 59 % of the account's daily DO requests if messages count 1:1.

### Tests (live, against the deployed Worker; `results/tests-live.txt`)

- **`lease_is_exclusive_under_races`: pass.**
  - 10 rounds × 20 devices claiming the same run through the DO.
  - 10 rounds × 20 concurrent HTTP claims decided by TiDB's single statements (`INSERT IGNORE`, then a conditional `UPDATE`).
  - A 20-way race after expiry: one winner, and the epoch moves to 2.
  - Every round had exactly one winner, and TiDB's row and event log agreed with it.
  - One earlier run failed on a WebSocket upgrade error, not on exclusivity. Upgrades now retry, as devices do.
- **`device_reconnects_after_sleep`: pass.**
  - A 36–40 s sleep keeps the lease; another device can't take it.
  - An 80 s sleep shows the device as `asleep`, and another device may take its lease (epoch 2). The sleeper reconnects, finds the run taken and stops.
  - A sleeper whose run nobody took reconnects and takes it back with the same epoch.
  - Every sleeper is `online` again after reconnecting.
- **`teardown_leaves_nothing`:** see "Teardown" below.

### Optional: TiDB's MySQL endpoint over `connect()` from the Worker (`results/tcp.json`)

This works on Free, with no Hyperdrive. `src/tcp.ts` is a minimal client: greeting, then `SSLRequest`, then `startTls()`, then `mysql_native_password` after an auth switch.
- **Server:** `8.0.11-TiDB-v8.5.3-serverless`.
- **Connection setup** (TCP + STARTTLS + auth): 413–588 ms.
- **`SELECT 1` on the open connection,** from CDG: 61–88 ms. The HTTP driver from the same colo: p50 92 ms.

There is no pooling across invocations without Hyperdrive. Every new invocation pays the setup again.

### Cloudflare (verify) items

Read 2026-10-23.

| Item | Answer | Source |
|---|---|---|
| Durable Objects free limits | 100,000 requests a day; 13,000 GB-s a day; 5M rows read and 100,000 rows written a day; 5 GB total. Reset at 00:00 UTC. "If you exceed any one of the free tier limits, further operations of that type will fail with an error." SQLite-backed classes only; 100 classes on Free | https://developers.cloudflare.com/durable-objects/platform/pricing/ , https://developers.cloudflare.com/durable-objects/platform/limits/ |
| Does 20:1 apply to the free daily DO limit? | **Not stated.** The footnote is on the Paid column: "For compute requests billing-only, a 20:1 ratio is applied to incoming WebSocket messages… The 20:1 ratio does not affect Durable Object metrics and analytics." The analytics here count every message as a request. Plan for 1:1 on Free. Auto-response pings make the question moot | https://developers.cloudflare.com/durable-objects/platform/pricing/ (footnote 2) |
| Protocol pings and auto-responses | "no charge … for incoming WebSocket protocol pings"; auto-responses "will not incur additional wall-clock time, and so they will not be charged". **Measured:** 2,390 auto-responded pings in an hour produced **0** invocations and woke nothing | same page, footnotes 2 and 3; https://developers.cloudflare.com/durable-objects/api/state/ |
| Duration with hibernation | "Durable Objects that are idle and eligible for hibernation are not billed for duration … even before the runtime has hibernated them". **Measured:** `UserHib` with random-phase pings was never evicted (1 wake in an hour), yet it was billed 1.84 GB-s | same page |
| Cron Triggers on Free | **5 per account** (Paid 250) | https://developers.cloudflare.com/workers/platform/limits/ |
| Rate-limiting binding on Free | The docs say nothing about plans. **Measured:** it deploys and enforces on this Free account; a 5-per-60 s limit allowed 6 of 8 calls. It is approximate and per-colo; periods are 10 or 60 s only | https://developers.cloudflare.com/workers/runtime-apis/bindings/rate-limit/ |
| Turnstile Siteverify | "Each token can only be validated once"; valid 300 s; a reused or expired token returns `timeout-or-duplicate`; `idempotency_key` (a UUID) makes a Siteverify call safe to retry | https://developers.cloudflare.com/turnstile/get-started/server-side-validation/ |
| Other Free limits that matter | 50 subrequests per request (each HTTP-driver statement is one); 10 ms CPU per request | https://developers.cloudflare.com/workers/platform/limits/ |

### Surprises

1. **Hibernation alone doesn't save requests.**
   - Every ping is a billed DO request. Only `setWebSocketAutoResponse` avoids that: it cut requests ×26 and CPU ×10.
   - Presence and lease renewal then come from `getWebSocketAutoResponseTimestamp`. The object wakes only for real messages.
   - Aligning pings to the wall clock lets the object be evicted (129 wakes), but it saves nothing: duration is billed only while handlers run.
2. **No hibernation means one user per account.** `accept()` bills the whole connected time: 11,428 GB-s a day for one object, against a 13,000 GB-s cap.
3. **Location dominates latency.**
   - A Worker near the user is 270–290 ms per statement to `aws-us-east-1` from India. A DO pinned near the region (`locationHint`) is 19–25 ms.
   - The DO is placed where it is first created, unless a hint is given.
4. **Half-dead WebSockets.**
   - On this machine's IPv6 path, ~75 % of sockets went silently dead within a minute. Each stayed `OPEN` on the client while the kernel retransmitted (`ss` showed Send-Q stuck), and the DO kept the server half.
   - Only a pong timeout detects this. Clients must reconnect when a pong is missing (here, after 10 s).
   - On IPv4, there were still ~1.25 reconnects per device per hour, and 2–4 failed upgrades per 20 devices per hour. Upgrades need retry.
   - The DO accumulates zombie sockets (25 for 20 devices), so presence must use the last-seen time, never "socket open".
   - The first measurement run was discarded for this reason (`--dns-result-order=ipv4first` for the clean run).
5. **Aborted writes commit.**
   - The Worker keeps running after the client disconnects.
   - The HTTP driver's aborted fetches committed 8 of 8 times.
   - Idempotency keys aren't optional.
6. **`connect()` to TiDB's MySQL port works without Hyperdrive.** It is an input to Q1304. The HTTP driver remains simpler: no setup cost per invocation.

### What was created and deleted

**Cloudflare:**
- **Created:** the Worker `loams-cloud-lc0` on `workers.dev`, with no routes, no custom domains and preview URLs off. With it:
  - three SQLite-backed Durable Object classes (migration `v1`);
  - two secrets;
  - one rate-limit binding.
- **Deleted:** `wrangler delete --name loams-cloud-lc0` removed the script and its Durable Object namespaces.
- **Not touched:** no other script, and no DNS, KV, R2 or account settings.

**TiDB Cloud:**
- **Created:** one free Starter instance, `lc0-worker`, in `aws-us-east-1`, with no spending-limit flag, and `ti`'s three SQL users.
- **Deleted:** the instance (state `DELETED`) and `~/.ti/db_users/<cluster-id>/`.

**`teardown_leaves_nothing`: pass** (`results/tests-teardown.txt`). It checked through the APIs that:
- the script is gone;
- no Durable Object namespace names it;
- no Starter instance is named `lc0-worker`;
- the local SQL users are gone.
