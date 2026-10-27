// The Worker's wiring (D1727; LC0 Task 3's findings), kept free of Cloudflare
// and npm imports so Node's tests can run it: src/worker.ts passes in the
// Durable Object base and @tidbcloud/serverless's connect.
//
// - Control-DB writes go through one Durable Object, ControlDb, created with a
//   location hint near the Starter region (lc0-report §4: ~20 ms from there,
//   ~280 ms from the edge). Account-scoped writes move to the per-user DO in LC2.
// - Rate limits are counted in RateLimiter Durable Objects: the rate-limiting
//   binding counts per location (R-LC2a-9), and §25.7's limits are global.
// - Crons: the hourly invite drain, expiry sweep and deletions; retention daily.

import { Admin, parseAdmins } from "./admin.ts";
import { handle, type AppDeps } from "./app.ts";
import { Audit } from "./audit.ts";
import { EmailBudget } from "./budget.ts";
import { importKeys } from "./crypto.ts";
import { DohDns, loadDomainLists } from "./domains.ts";
import { Invites } from "./invites.ts";
import { httpJwks, JwtVerifier } from "./jwt.ts";
import { Lifecycle } from "./lifecycle.ts";
import { migrate } from "./schema.ts";
import { consoleLogger } from "./log.ts";
import { systemClock, type Logger, type Mailer, type RateLimiter, type WaitUntil } from "./ports.ts";
import type { Sql, SqlResult } from "./sql.ts";
import { SiteverifyTurnstile } from "./turnstile.ts";
import { Waitlist } from "./waitlist.ts";
import { HttpWorkOs } from "./workos.ts";

// ---- The few Cloudflare shapes used here (no @cloudflare/workers-types). ----

export interface DoStub {
  fetch(input: string, init?: RequestInit): Promise<Response>;
}
export interface DoNamespace {
  idFromName(name: string): unknown;
  get(id: unknown, opts?: { locationHint?: string }): DoStub;
}
export interface DoStorage {
  get<T>(key: string): Promise<T | undefined>;
  put<T>(key: string, value: T): Promise<void>;
  delete(key: string): Promise<boolean>;
  setAlarm?(t: number): Promise<void>;
}
export interface DoState {
  storage: DoStorage;
  blockConcurrencyWhile<T>(f: () => Promise<T>): Promise<T>;
}

export interface Env {
  CONTROL_DB: DoNamespace;
  LIMITER: DoNamespace;
  /** Secrets (wrangler secret put): never in wrangler.toml. */
  DATABASE_URL: string;
  LOAMS_CLOUD_HMAC_KEY: string;
  LOAMS_CLOUD_AES_KEY: string;
  TURNSTILE_SECRET: string;
  WORKOS_API_KEY: string;
  /** Vars. */
  WORKOS_CLIENT_ID: string;
  WORKOS_ISSUER?: string;
  LOAMS_CLOUD_OSTRIUM_ORG: string;
  SITE_URL?: string;
  TURNSTILE_HOSTNAME?: string;
  CONTROL_DB_LOCATION?: string;
}

export interface Bundled {
  admins: string;
  disposable: string;
  overrides: string;
  migrations: { version: number; name: string; text: string }[];
}

/** @tidbcloud/serverless's connect, as the Worker uses it. */
export type Connect = (opts: { url: string; fullResult: true; fetch?: typeof fetch }) => {
  execute(sql: string, params?: unknown[]): Promise<unknown>;
};

const REQUIRED: (keyof Env)[] = ["DATABASE_URL", "LOAMS_CLOUD_HMAC_KEY", "LOAMS_CLOUD_AES_KEY", "TURNSTILE_SECRET", "WORKOS_API_KEY", "WORKOS_CLIENT_ID", "LOAMS_CLOUD_OSTRIUM_ORG"];

/** Names the missing settings; the Worker refuses to serve without them. */
export function missingSettings(env: Partial<Env>): string[] {
  return REQUIRED.filter((k) => typeof env[k] !== "string" || !(env[k] as string));
}

// ---- The control DB through its Durable Object. ----

/** The Sql port on the Worker side: every statement goes to the ControlDb DO. */
export class DoSql implements Sql {
  private readonly stub: DoStub;
  constructor(ns: DoNamespace, locationHint = "enam") {
    this.stub = ns.get(ns.idFromName("control"), { locationHint });
  }
  async execute(sql: string, params: readonly unknown[] = []): Promise<SqlResult> {
    const r = await this.stub.fetch("https://control-db/execute", { method: "POST", body: JSON.stringify({ sql, params }) });
    const body = (await r.json()) as SqlResult | { error: string };
    if (!r.ok || "error" in body) throw new Error((body as { error: string }).error ?? `control DB ${r.status}`);
    return body as SqlResult;
  }
}

/** The ControlDb Durable Object's request handler. */
export function controlDbHandler(env: Pick<Env, "DATABASE_URL">, connect: Connect) {
  const conn = connect({
    url: env.DATABASE_URL,
    fullResult: true,
    // The driver has no timeout of its own (LC0 Task 3): bound every request.
    fetch: ((input: RequestInfo | URL, init?: RequestInit) => fetch(input, { ...init, signal: AbortSignal.timeout(10_000) })) as typeof fetch,
  });
  return async (req: Request): Promise<Response> => {
    if (req.method !== "POST") return new Response(JSON.stringify({ error: "POST only" }), { status: 405 });
    const { sql, params } = (await req.json()) as { sql: string; params?: unknown[] };
    try {
      const r = (await conn.execute(sql, params ?? [])) as { rows?: Record<string, unknown>[]; rowsAffected?: number | null };
      return Response.json({ rows: r.rows ?? [], rowsAffected: r.rowsAffected ?? 0 });
    } catch (e) {
      // The message only: the driver's errors can carry the URL (lc0 spike's scrub()).
      const msg = String((e as Error).message ?? e).replace(/mysql:\/\/[^\s"']*/g, "mysql://***").slice(0, 500);
      return Response.json({ error: msg }, { status: 500 });
    }
  };
}

// ---- Rate limits in Durable Objects. ----

export class DoRateLimiter implements RateLimiter {
  private readonly ns: DoNamespace;
  constructor(ns: DoNamespace) {
    this.ns = ns;
  }
  async hit(key: string, limit: number, windowMs: number) {
    const stub = this.ns.get(this.ns.idFromName(key));
    const r = await stub.fetch("https://limiter/hit", { method: "POST", body: JSON.stringify({ limit, windowMs }) });
    return (await r.json()) as { ok: boolean; retryAfterMs: number };
  }
}

/** The RateLimiter Durable Object's handler: one fixed window per object. */
export function rateLimiterHandler(state: DoState, now: () => number = Date.now) {
  return async (req: Request): Promise<Response> => {
    const { limit, windowMs } = (await req.json()) as { limit: number; windowMs: number };
    const out = await state.blockConcurrencyWhile(async () => {
      const t = now();
      const start = Math.floor(t / windowMs) * windowMs;
      const w = (await state.storage.get<{ start: number; n: number }>("w")) ?? { start, n: 0 };
      const cur = w.start === start ? w : { start, n: 0 };
      cur.n++;
      await state.storage.put("w", cur);
      if (state.storage.setAlarm) await state.storage.setAlarm(start + windowMs * 2); // clean up when idle
      return { ok: cur.n <= limit, retryAfterMs: start + windowMs - t };
    });
    return Response.json(out);
  };
}

/** Waitlist mail until LC2a Task 2 lands (the owner's sending-only Resend key, R-LC2a-2). */
export const notConfiguredMailer: Mailer = {
  async send() {
    throw new Error("mail is not configured yet (LC2a Task 2)");
  },
};

// ---- The app. ----

export interface Built {
  deps: AppDeps;
  invites: Invites;
  lifecycle: Lifecycle;
}

export async function build(env: Env, files: Bundled, opts: { mailer?: Mailer; logger?: Logger; sql?: Sql; limiter?: RateLimiter } = {}): Promise<Built> {
  const missing = missingSettings(env);
  if (missing.length) throw new Error(`loams-cloud: missing settings: ${missing.join(", ")}`);
  const logger = opts.logger ?? consoleLogger;
  const clock = systemClock;
  const sql = opts.sql ?? new DoSql(env.CONTROL_DB, env.CONTROL_DB_LOCATION ?? "enam");
  const keys = await importKeys(env.LOAMS_CLOUD_HMAC_KEY, env.LOAMS_CLOUD_AES_KEY);
  // Once per isolate: a SELECT when the schema is current, the missing migrations otherwise.
  await migrate(sql, files.migrations);
  const audit = new Audit(sql, () => clock.now());
  const workos = new HttpWorkOs(env.WORKOS_API_KEY);
  const waitlist = new Waitlist({
    sql, keys, audit, clock, logger,
    lists: loadDomainLists({ disposable: files.disposable, overrides: files.overrides }),
    dns: new DohDns({ failOpen: true }),
    mailer: opts.mailer ?? notConfiguredMailer,
    siteUrl: env.SITE_URL ?? "https://loams.dev",
  });
  const limiter = opts.limiter ?? new DoRateLimiter(env.LIMITER);
  const verifier = new JwtVerifier(httpJwks(`https://api.workos.com/sso/jwks/${env.WORKOS_CLIENT_ID}`), {
    issuer: env.WORKOS_ISSUER ?? `https://api.workos.com/user_management/${env.WORKOS_CLIENT_ID}`,
  });
  const invites = new Invites({ sql, workos, waitlist, budget: new EmailBudget(sql), audit, clock, logger });
  waitlist.d.revokeInvites = (id, actor) => invites.revokeAllFor(id, actor);
  const admin = new Admin({
    sql, verifier, mfa: workos, waitlist, audit, limiter, clock, logger,
    config: { ostriumOrgId: env.LOAMS_CLOUD_OSTRIUM_ORG, admins: parseAdmins(files.admins), totpIssuer: "Loams Cloud" },
  });
  const lifecycle = new Lifecycle({ sql, waitlist, workos, audit, clock, logger, verifier });
  const deps: AppDeps = {
    keys, waitlist, limiter, clock, logger,
    turnstile: new SiteverifyTurnstile(env.TURNSTILE_SECRET, { expectedHostname: env.TURNSTILE_HOSTNAME }),
    routes: [lifecycle.route(), admin.routes([invites.routes()])],
  };
  return { deps, invites, lifecycle };
}

export const CRONS = {
  hourly: "7 * * * *",
  daily: "17 3 * * *",
} as const;

/** The scheduled handler's work, by cron expression. */
export async function scheduled(cron: string, b: Built): Promise<Record<string, unknown>> {
  if (cron === CRONS.hourly) {
    const sent = await b.invites.drain();
    const swept = await b.invites.sweep();
    const deleted = await b.lifecycle.runDeletions();
    return { sent, swept, deleted };
  }
  if (cron === CRONS.daily) return { ...(await b.lifecycle.runRetention()) };
  throw new Error(`unknown cron ${cron}`);
}

/** The Worker's fetch: only /cloud/v1 is served. */
export function fetchHandler(get: () => Promise<Built>) {
  return async (req: Request, ctx: WaitUntil): Promise<Response> => {
    let b: Built;
    try {
      b = await get();
    } catch (e) {
      consoleLogger.log("startup.failed", { error: String(e) });
      return Response.json({ error: { code: "UNAVAILABLE", message: "Loams Cloud is not configured" } }, { status: 503 });
    }
    return handle(req, b.deps, ctx);
  };
}
