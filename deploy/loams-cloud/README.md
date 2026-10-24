# loams-cloud: the Loams Cloud control plane

The Worker behind the free, invite-only Loams Cloud try-out (design [§78 §25](../../docs/design/78-loams-cloud-on-tidb-x.md#25-invite-only-access-workos-authkit-and-the-sign-in-policy-d1736d1757-owner-amendments-of-2026-10-23), plan [LC1, LC2a](../../docs/plans/2026-10-23-lc1-loams-cloud.md)). It runs on Workers Free with SQLite-backed Durable Objects, and keeps its records in a TiDB Cloud Starter instance, the control DB (D1717, D1739).

**Status: preview, not deployed.** Nothing here is live. The code is built and tested against local doubles (an in-memory store, a throwaway TiDB, fake WorkOS, Turnstile and mail). Wiring to the real WorkOS staging environment, Resend and Turnstile waits for the owner's setup steps in the plan's "Owner steps and credentials".

## Layout

| Path | What |
|---|---|
| `schema/0001_waitlist.sql` | The control-DB tables of §25.3 (LC2a Task 1) |
| `src/sql.ts` | The control-DB port, and the write rule: every write is one statement safe to repeat (LC0 Task 3 found that aborted writes commit) |
| `src/schema.ts` | The migrator |
| `test/` | `node --test` suites; `test/harness/tidb.ts` reaches TiDB through the mysql client |

There are no npm dependencies. The code uses only Web APIs that both Workers and Node 22 provide, so the suites run on plain Node with type stripping.

## Tests

```sh
npm test                 # everything that needs no database
scripts/test-tidb.sh     # the same, plus the control-DB suites on a throwaway TiDB v8.5.8 (unistore)
```

`scripts/test-tidb.sh` starts `tidb-server` on unistore (one process, no PD or TiKV) from tiup's `tidb:v8.5.8`, or from `TIDB_SERVER`, and removes its data directory afterwards.
