// The Worker's wiring (src/entry.ts), with Durable Objects and the HTTP driver doubled.
import { test, describe, after } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { randomBytes } from "../src/crypto.ts";
import { build, controlDbHandler, CRONS, DoRateLimiter, DoSql, fetchHandler, missingSettings, rateLimiterHandler, scheduled, type Bundled, type DoNamespace, type DoState, type Env } from "../src/entry.ts";
import { migrate } from "../src/schema.ts";
import { freshDatabase, loadMigrations, schemaDir, skipNoTidb } from "./harness/tidb.ts";
import { WaitBag } from "./harness/fakes.ts";

const root = join(schemaDir, "..");
const files: Bundled = {
  admins: readFileSync(join(root, "config", "admins.toml"), "utf8"),
  disposable: readFileSync(join(root, "policy", "disposable_email_blocklist.conf"), "utf8"),
  overrides: readFileSync(join(root, "policy", "email-domains.toml"), "utf8"),
  migrations: loadMigrations(),
};
const key = () => btoa(String.fromCharCode(...randomBytes(32)));

/** A namespace whose objects are made by `make`, one per name, like idFromName. */
function namespace(make: (name: string) => (req: Request) => Promise<Response>): DoNamespace & { hints: (string | undefined)[] } {
  const objects = new Map<string, (req: Request) => Promise<Response>>();
  const hints: (string | undefined)[] = [];
  return {
    hints,
    idFromName: (name: string) => name,
    get(id: unknown, opts?: { locationHint?: string }) {
      hints.push(opts?.locationHint);
      const name = String(id);
      if (!objects.has(name)) objects.set(name, make(name));
      return { fetch: (input: string, init?: RequestInit) => objects.get(name)!(new Request(input, init)) };
    },
  };
}

function memoryState(): DoState {
  const m = new Map<string, unknown>();
  let chain = Promise.resolve();
  return {
    storage: {
      async get<T>(k: string) { return m.get(k) as T | undefined; },
      async put<T>(k: string, v: T) { m.set(k, v); },
      async delete(k: string) { return m.delete(k); },
    },
    blockConcurrencyWhile<T>(f: () => Promise<T>): Promise<T> {
      const p = chain.then(f);
      chain = p.then(() => undefined, () => undefined);
      return p;
    },
  };
}

describe("settings", () => {
  test("every secret and id is required, and the Worker answers 503 without them", async () => {
    assert.deepEqual(missingSettings({}).sort(), ["DATABASE_URL", "LOAMS_CLOUD_AES_KEY", "LOAMS_CLOUD_HMAC_KEY", "LOAMS_CLOUD_OSTRIUM_ORG", "TURNSTILE_SECRET", "WORKOS_API_KEY", "WORKOS_CLIENT_ID"]);
    const res = await fetchHandler(() => build({} as Env, files))(new Request("https://api.loams.dev/cloud/v1/waitlist", { method: "POST" }), new WaitBag());
    assert.equal(res.status, 503);
    assert.equal((await res.json()).error.code, "UNAVAILABLE");
  });
});

describe("the rate limiter object", () => {
  test("counts a fixed window, globally, and resets in the next", async () => {
    let t = 1_000_000;
    const ns = namespace(() => rateLimiterHandler(memoryState(), () => t));
    const l = new DoRateLimiter(ns);
    const results = await Promise.all(Array.from({ length: 7 }, () => l.hit("join:ip", 5, 60_000)));
    assert.equal(results.filter((r) => r.ok).length, 5, "concurrent hits are serialised");
    assert.ok(results.every((r) => r.retryAfterMs > 0));
    assert.equal((await l.hit("other", 5, 60_000)).ok, true, "keys are separate objects");
    t += 60_000;
    assert.equal((await l.hit("join:ip", 5, 60_000)).ok, true);
  });
});

describe("the control DB object", { skip: skipNoTidb }, () => {
  const db = freshDatabase();
  after(db.dispose);
  // @tidbcloud/serverless's connect, played by the mysql-client double.
  const connect = () => ({ execute: (sql: string, params?: unknown[]) => db.sql.execute(sql, params ?? []) });
  const ns = namespace(() => controlDbHandler({ DATABASE_URL: "mysql://u:secret@host/db" }, connect));

  test("statements go through one object, placed by its location hint", async () => {
    const sql = new DoSql(ns, "enam");
    await migrate(sql, loadMigrations());
    const ins = await sql.execute("INSERT IGNORE INTO waitlist_suppressed (email_hmac, reason, created_ms) VALUES (?, 'removed', ?)", ["a".repeat(64), 1]);
    assert.equal(ins.rowsAffected, 1);
    const r = await sql.execute("SELECT reason FROM waitlist_suppressed WHERE email_hmac = ?", ["a".repeat(64)]);
    assert.deepEqual(r.rows, [{ reason: "removed" }]);
    assert.ok(ns.hints.every((h) => h === "enam"));
  });

  test("errors come back without the connection string", async () => {
    const leaky = namespace(() => controlDbHandler({ DATABASE_URL: "x" }, () => ({ execute: async () => { throw new Error("connect mysql://user:hunter2@gateway.tidbcloud.com:4000/db failed"); } })));
    await assert.rejects(new DoSql(leaky).execute("SELECT 1"), (e: Error) => !e.message.includes("hunter2") && e.message.includes("mysql://***"));
  });

  test("the built app serves the waitlist and runs both crons", async () => {
    const env = {
      CONTROL_DB: ns, LIMITER: namespace(() => rateLimiterHandler(memoryState())),
      DATABASE_URL: "mysql://u:secret@host/db", LOAMS_CLOUD_HMAC_KEY: key(), LOAMS_CLOUD_AES_KEY: key(),
      TURNSTILE_SECRET: "test-secret", WORKOS_API_KEY: "sk_test_x", WORKOS_CLIENT_ID: "client_TEST", LOAMS_CLOUD_OSTRIUM_ORG: "org_TEST",
    } as Env;
    const lines: string[] = [];
    // The real clients are wired, so fetch is replaced first: nothing leaves the machine.
    // Siteverify refuses the token, so the answer is 403, never a 5xx.
    const realFetch = globalThis.fetch;
    const urls: string[] = [];
    globalThis.fetch = (async (u: RequestInfo | URL) => {
      urls.push(String(u));
      return new Response(JSON.stringify({ success: false }), { status: 200 });
    }) as typeof fetch;
    try {
      const b = await build(env, files, { logger: { log: (e, f) => lines.push(JSON.stringify({ e, ...f })) } });
      const bag = new WaitBag();
      const res = await fetchHandler(async () => b)(new Request("https://api.loams.dev/cloud/v1/waitlist", { method: "POST", body: JSON.stringify({ email: "a@example.com", turnstile_token: "tok" }), headers: { "cf-connecting-ip": "203.0.113.9" } }), bag);
      assert.equal(res.status, 403);
      await bag.drain();
      // Admin routes are mounted and closed.
      assert.equal((await fetchHandler(async () => b)(new Request("https://api.loams.dev/cloud/v1/admin/waitlist"), bag)).status, 401);
      assert.equal((await fetchHandler(async () => b)(new Request("https://api.loams.dev/cloud/v1/me", { method: "DELETE" }), bag)).status, 401);
      // The crons run against the empty control DB.
      assert.deepEqual(await scheduled(CRONS.hourly, b), { sent: 0, swept: 0, deleted: 0 });
      const daily = await scheduled(CRONS.daily, b);
      assert.equal(daily.unconfirmed, 0);
      await assert.rejects(scheduled("* * * * *", b), /unknown cron/);
      assert.deepEqual(urls, ["https://challenges.cloudflare.com/turnstile/v0/siteverify"]);
    } finally {
      globalThis.fetch = realFetch;
    }
    assert.ok(!lines.join("\n").includes("a@example.com"));
  });

  test("the crons in wrangler.toml are the ones the handler knows", () => {
    const toml = readFileSync(join(root, "wrangler.toml"), "utf8");
    const m = toml.match(/crons = \[([^\]]*)\]/);
    assert.deepEqual(m![1].split(",").map((s) => s.trim().replace(/"/g, "")), [CRONS.hourly, CRONS.daily]);
    assert.match(toml, /workers_dev = false/, "no public URL until the owner chooses one");
    assert.doesNotMatch(toml, /^\s*route/m);
  });
});

