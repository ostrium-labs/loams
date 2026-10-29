# loams-cloud: the Loams Cloud control plane

The Worker behind the free, invite-only Loams Cloud try-out (design [§78 §25](../../docs/design/78-loams-cloud-on-tidb-x.md#25-invite-only-access-workos-authkit-and-the-sign-in-policy-d1736d1757-owner-amendments-of-2026-10-23), plan [LC1, LC2a](../../docs/plans/2026-10-23-lc1-loams-cloud.md)). It runs on Workers Free with SQLite-backed Durable Objects, and keeps its records in a TiDB Cloud Starter instance, the control DB (D1717, D1739).

**Status: preview, not deployed.** Nothing here is live. The code is built and tested against local doubles (an in-memory store, a throwaway TiDB, fake WorkOS, Turnstile and mail). Wiring to the real WorkOS staging environment, Resend and Turnstile waits for the owner's setup steps in the plan's "Owner steps and credentials".

## Layout

| Path | What |
|---|---|
| `schema/0001_waitlist.sql` | The control-DB tables of §25.3 (LC2a Task 1) |
| `src/sql.ts` | The control-DB port, and the write rule: every write is one statement safe to repeat (LC0 Task 3 found that aborted writes commit) |
| `src/schema.ts` | The migrator |
| `src/worker.ts`, `src/entry.ts`, `wrangler.toml` | The Worker: the ControlDb Durable Object (control-DB statements, with a location hint near Starter), RateLimiter objects, the crons. Not deployed |
| `src/app.ts` | The `/cloud/v1` router and the public waitlist endpoints (LC2a Task 3) |
| `src/waitlist.ts` | Join, confirm and remove, with double opt-in and enumeration-safe answers |
| `src/crypto.ts` | Address normalisation, HMAC and AES-GCM storage, ids and tokens |
| `src/domains.ts`, `policy/` | The domain lists (pins in `policy/PINS.md`) and the DNS-over-HTTPS MX check |
| `src/turnstile.ts` | Turnstile Siteverify, single use, with an idempotency key |
| `src/log.ts` | Log redaction: no token, address or bearer value is ever written |
| `src/admin.ts`, `config/admins.toml` | Admin authorisation: the Ostrium org's `cloud_admin` role, the allow-list, and a TOTP step-up within 15 minutes (LC2a Task 4) |
| `src/jwt.ts` | AuthKit access tokens, RS256 against WorkOS's JWKS |
| `src/workos.ts` | The WorkOS REST client: MFA and invitations |
| `src/invites.ts` | Admin invites: the queue, WorkOS create, resend, extend and revoke (LC2a Task 5) |
| `src/lifecycle.ts` | `DELETE /me`, the deletion job, the retention jobs of §25.6 (LC2a Task 9) |
| `src/budget.ts` | The shared daily email budget: 100 a day, at most 60 invitations (D1738) |
| `src/ports.ts` | The outside world as interfaces: Turnstile, rate limits, mail, the clock, logs |
| `test/` | `node --test` suites; `test/harness/tidb.ts` reaches TiDB through the mysql client |

Everything but `src/worker.ts` uses only Web APIs that both Workers and Node 22 provide, so the suites run on plain Node with type stripping and need no `npm install`. `src/worker.ts` is the only file that imports `cloudflare:workers` and `@tidbcloud/serverless` (0.3.0, LC0's pin); `src/entry.ts` holds the wiring it calls, and is tested.

## Tests

```sh
npm test                 # everything that needs no database
scripts/test-tidb.sh     # the same, plus the control-DB suites on a throwaway TiDB v8.5.8 (unistore)
```

npm ci --ignore-scripts && npm run check:worker   # src/worker.ts typechecks, and wrangler bundles it (--dry-run)
```

`check:worker` deploys nothing and needs no Cloudflare credential.

```sh
# Deploying (not yet: the owner's steps in the plan come first):
#   wrangler secret put DATABASE_URL   (and the other secrets listed in wrangler.toml)
#   wrangler deploy
```

`scripts/test-tidb.sh` starts `tidb-server` on unistore (one process, no PD or TiKV) from tiup's `tidb:v8.5.8`, or from `TIDB_SERVER`, and removes its data directory afterwards.
