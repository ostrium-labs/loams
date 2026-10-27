// LC0 Task 3 spike: the Loams Cloud control plane on a Worker (design 78 §9.2, §17, §25.13).
// Not product code. One Durable Object per user holds presence and leases with WebSockets;
// durable records go to a free TiDB Starter instance through @tidbcloud/serverless.
//
// Three Durable Object classes, so analytics separate them by namespace:
//   UserHib    hibernating WebSockets (acceptWebSocket + webSocketMessage); every ping wakes it
//   UserAuto   hibernating WebSockets + setWebSocketAutoResponse("ping" -> "pong"); pings never wake it
//   UserNoHib  ordinary WebSockets (ws.accept + event listeners); the object stays in memory
//
// Every route needs `Authorization: Bearer <LC0_BEARER>` (a Worker secret).

import { DurableObject } from "cloudflare:workers";
import { connect, DatabaseError, type FullResult } from "@tidbcloud/serverless";
import { tcpProbe } from "./tcp.ts";

export interface Env {
  DATABASE_URL: string;
  LC0_BEARER: string;
  USER_HIB: DurableObjectNamespace<UserHib>;
  USER_AUTO: DurableObjectNamespace<UserAuto>;
  USER_NOHIB: DurableObjectNamespace<UserNoHib>;
  RL: RateLimit; // probe: does the rate-limiting binding deploy and work on Workers Free?
}

const PING_MS = 30_000;
const LEASE_MS = 60_000; // renewed by ping (§9.2)
const ASLEEP_AFTER_MS = 2 * PING_MS + 5_000; // two missed pings, 5 s grace
const OFFLINE_AFTER_MS = 24 * 3600_000;
const DB_NOW = "CAST(UNIX_TIMESTAMP(NOW(3)) * 1000 AS SIGNED)";

function db(env: Env, opts: { url?: string; timeoutMs?: number; abortAfterMs?: number } = {}) {
  const timeoutMs = opts.timeoutMs ?? 10_000;
  return connect({
    url: opts.url ?? env.DATABASE_URL,
    fullResult: true,
    // The driver has no timeout and no retries of its own; bound every request.
    fetch: (input: string, init?: any) =>
      fetch(input, { ...init, signal: AbortSignal.timeout(opts.abortAfterMs ?? timeoutMs) }) as any,
  });
}

async function exec(env: Env, sql: string, args: unknown[] = []): Promise<{ r: FullResult; ms: number }> {
  const t0 = Date.now(); // Date.now advances across I/O in Workers, so this times the round trip.
  const r = (await db(env).execute(sql, args)) as FullResult;
  return { r, ms: Date.now() - t0 };
}

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
}

function scrub(s: string): string {
  return s
    .replace(/mysql:\/\/[^\s"']*/g, "mysql://***")
    .replace(/'[^']*'@'[^']*'/g, "'<user>'@'<host>'")
    .replace(/(password|pwd|secret|token)[^\s,;]*/gi, "$1=***")
    .slice(0, 300);
}

function errInfo(e: unknown) {
  if (e instanceof DatabaseError) return { kind: "DatabaseError", status: e.status, code: e.details?.code ?? null, message: scrub(e.message) };
  if (e instanceof Error) return { kind: e.name, message: scrub(e.message) };
  return { kind: typeof e, message: scrub(String(e)) };
}

async function sha256(s: string): Promise<string> {
  const d = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(s));
  return [...new Uint8Array(d)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

function timingSafeEq(a: string, b: string): boolean {
  if (a.length !== b.length) return false;
  let x = 0;
  for (let i = 0; i < a.length; i++) x |= a.charCodeAt(i) ^ b.charCodeAt(i);
  return x === 0;
}

// ---------- TiDB-only routes (no Durable Object) ----------

// Single-statement lease claim on TiDB: INSERT IGNORE wins a fresh run; otherwise a conditional
// UPDATE wins an expired lease or renews one's own. Both are atomic single statements.
async function dbLeaseClaim(env: Env, user: string, run: string, device: string, ttl: number) {
  const ins = await exec(
    env,
    `INSERT IGNORE INTO lc0.leases (user_id, run_id, holder, epoch, expires_ms, updated_ms) VALUES (?, ?, ?, 1, ${DB_NOW} + ?, ${DB_NOW})`,
    [user, run, device, ttl],
  );
  let won = (ins.r.rowsAffected ?? 0) === 1;
  let ms = ins.ms;
  if (!won) {
    const up = await exec(
      env,
      `UPDATE lc0.leases SET epoch = IF(holder = ?, epoch, epoch + 1), holder = ?, expires_ms = ${DB_NOW} + ?, updated_ms = ${DB_NOW}
       WHERE user_id = ? AND run_id = ? AND (holder = ? OR expires_ms < ${DB_NOW})`,
      [device, device, ttl, user, run, device],
    );
    won = (up.r.rowsAffected ?? 0) === 1;
    ms += up.ms;
  }
  const cur = await exec(env, "SELECT holder, epoch, expires_ms FROM lc0.leases WHERE user_id = ? AND run_id = ?", [user, run]);
  const row = (cur.r.rows?.[0] ?? {}) as any;
  return { won, holder: row.holder, epoch: Number(row.epoch), write_ms: ms };
}

// Idempotent create of a run intent: one statement; rowsAffected says new (1) or duplicate (0).
async function createIntent(env: Env, body: any, failAfterWrite: boolean) {
  const payload = JSON.stringify({ user: body.user, run: body.run, target: body.target });
  const hash = await sha256(payload);
  const w = await exec(
    env,
    `INSERT INTO lc0.run_intents (idem_key, user_id, run_id, target_device, payload_hash, created_ms) VALUES (?, ?, ?, ?, ?, ${DB_NOW})
     ON DUPLICATE KEY UPDATE idem_key = idem_key`,
    [body.idem, body.user, body.run, body.target, hash],
  );
  if (failAfterWrite) return json({ error: "simulated failure after the write" }, 503);
  const created = (w.r.rowsAffected ?? 0) === 1;
  const cur = await exec(env, "SELECT payload_hash, run_id, created_ms FROM lc0.run_intents WHERE idem_key = ?", [body.idem]);
  const row = (cur.r.rows?.[0] ?? {}) as any;
  if (row.payload_hash !== hash) return json({ error: "idempotency key reused with a different payload" }, 409);
  return json({ created, run: row.run_id, created_ms: Number(row.created_ms), write_ms: w.ms }, created ? 201 : 200);
}

async function bench(env: Env, kind: string, n: number) {
  const samples: number[] = [];
  const errors: unknown[] = [];
  const tag = crypto.randomUUID().slice(0, 8);
  for (let i = 0; i < n; i++) {
    try {
      let ms: number;
      if (kind === "insert") ms = (await exec(env, `INSERT INTO lc0.bench (k, v, at_ms) VALUES (?, ?, ${DB_NOW})`, [`${tag}-${i}`, "x".repeat(64)])).ms;
      else if (kind === "upsert")
        ms = (await exec(env, `INSERT INTO lc0.devices (user_id, device_id, name, state, last_seen_ms, updated_ms) VALUES (?, ?, ?, 'online', ${DB_NOW}, ${DB_NOW})
          ON DUPLICATE KEY UPDATE state = VALUES(state), last_seen_ms = VALUES(last_seen_ms), updated_ms = VALUES(updated_ms)`, ["bench", `d${i % 20}`, "bench device"])).ms;
      else if (kind === "update") ms = (await exec(env, `UPDATE lc0.devices SET last_seen_ms = ${DB_NOW} WHERE user_id = 'bench' AND device_id = ?`, [`d${i % 20}`])).ms;
      else if (kind === "select") ms = (await exec(env, "SELECT * FROM lc0.devices WHERE user_id = 'bench' AND device_id = ?", [`d${i % 20}`])).ms;
      else if (kind === "noop") ms = (await exec(env, "SELECT 1")).ms;
      else throw new Error("unknown bench kind");
      samples.push(ms);
    } catch (e) {
      errors.push(errInfo(e));
    }
  }
  return { kind, n, samples, errors, colo: null as string | null };
}

// Driver behaviour on transient errors, seen from the Worker.
async function dbErr(env: Env, kind: string) {
  const t0 = Date.now();
  const out: any = { kind };
  try {
    if (kind === "badcreds") {
      const u = new URL(env.DATABASE_URL);
      u.password = "wrong-" + crypto.randomUUID();
      await db(env, { url: u.toString() }).execute("SELECT 1");
    } else if (kind === "badhost") {
      const u = new URL(env.DATABASE_URL);
      u.hostname = "gateway99.nowhere.prod.aws.tidbcloud.com";
      await db(env, { url: u.toString() }).execute("SELECT 1");
    } else if (kind === "timeout") {
      await db(env, { timeoutMs: 500 }).execute("SELECT SLEEP(3)");
    } else if (kind === "slow") {
      await db(env, { timeoutMs: 30_000 }).execute("SELECT SLEEP(3)");
    } else if (kind === "syntax") {
      await db(env).execute("SELEC 1");
    } else if (kind === "abort-write") {
      // Abort the HTTP request mid-flight, then look for the row: an aborted write may still commit.
      const k = "abort-" + crypto.randomUUID();
      const results: any[] = [];
      for (const ms of [5, 20, 40, 60, 80, 100, 150, 250]) {
        const key = `${k}-${ms}`;
        let err: any = null;
        try {
          await db(env, { abortAfterMs: ms }).execute(`INSERT INTO lc0.bench (k, v, at_ms) VALUES (?, 'abort', ${DB_NOW})`, [key]);
        } catch (e) {
          err = errInfo(e);
        }
        await new Promise((r) => setTimeout(r, 1500));
        const found = await exec(env, "SELECT COUNT(*) AS c FROM lc0.bench WHERE k = ?", [key]);
        results.push({ abort_after_ms: ms, client_error: err?.kind ?? null, committed: Number((found.r.rows?.[0] as any)?.c) === 1 });
      }
      out.results = results;
      out.ok = true;
      out.ms = Date.now() - t0;
      return out;
    } else throw new Error("unknown kind");
    out.ok = true;
  } catch (e) {
    out.ok = false;
    out.error = errInfo(e);
  }
  out.ms = Date.now() - t0;
  return out;
}

// ---------- the per-user Durable Object ----------

type Attachment = { device: string; lastSeen: number; connectedAt: number };

abstract class UserBase extends DurableObject<Env> {
  readonly wakeId = crypto.randomUUID().slice(0, 8);
  readonly wakeAt = Date.now();
  msgs = 0;
  wakes: number;

  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    const sql = ctx.storage.sql;
    sql.exec(`CREATE TABLE IF NOT EXISTS leases (run TEXT PRIMARY KEY, holder TEXT NOT NULL, epoch INTEGER NOT NULL, claimed_ms INTEGER NOT NULL)`);
    sql.exec(`CREATE TABLE IF NOT EXISTS seen (device TEXT PRIMARY KEY, last_seen_ms INTEGER NOT NULL)`);
    sql.exec(`CREATE TABLE IF NOT EXISTS stats (k TEXT PRIMARY KEY, v INTEGER NOT NULL)`);
    // One row write per wake: the count of constructor runs is the count of wakes from hibernation.
    sql.exec(`INSERT INTO stats (k, v) VALUES ('wakes', 1) ON CONFLICT(k) DO UPDATE SET v = v + 1`);
    this.wakes = Number(sql.exec(`SELECT v FROM stats WHERE k = 'wakes'`).one().v);
  }

  abstract sockets(device?: string): WebSocket[];
  abstract acceptSocket(ws: WebSocket, a: Attachment): void;
  abstract attachment(ws: WebSocket): Attachment | null;
  abstract setAttachment(ws: WebSocket, a: Attachment): void;
  autoResponseAt(_ws: WebSocket): number {
    return 0;
  }

  lastSeen(device: string): number {
    let t = 0;
    for (const ws of this.sockets(device)) {
      const a = this.attachment(ws);
      if (a) t = Math.max(t, a.lastSeen);
      t = Math.max(t, this.autoResponseAt(ws));
    }
    const row = this.ctx.storage.sql.exec(`SELECT last_seen_ms FROM seen WHERE device = ?`, device).toArray()[0];
    if (row) t = Math.max(t, Number(row.last_seen_ms));
    return t;
  }

  presence(device: string, now = Date.now()) {
    const seen = this.lastSeen(device);
    const open = this.sockets(device).length > 0;
    const age = now - seen;
    const state = age < ASLEEP_AFTER_MS && open ? "online" : age < OFFLINE_AFTER_MS ? "asleep" : "offline";
    return { device, state, last_seen_age_ms: age, socket_open: open };
  }

  leaseValid(run: { holder: string; claimed_ms: number }, now = Date.now()) {
    return Math.max(run.claimed_ms, this.lastSeen(run.holder)) + LEASE_MS > now;
  }

  // The DO decides the lease synchronously in its own SQLite (single-threaded: no interleaving
  // between the read and the write), then records the new epoch in TiDB with a fenced upsert.
  async claim(user: string, run: string, device: string) {
    const now = Date.now();
    const sql = this.ctx.storage.sql;
    const cur = sql.exec(`SELECT holder, epoch, claimed_ms FROM leases WHERE run = ?`, run).toArray()[0] as any;
    let epoch: number;
    if (!cur) epoch = 1;
    else if (cur.holder === device) epoch = Number(cur.epoch);
    else if (!this.leaseValid({ holder: cur.holder, claimed_ms: Number(cur.claimed_ms) }, now)) epoch = Number(cur.epoch) + 1;
    else return { ok: false, holder: cur.holder as string, epoch: Number(cur.epoch) };
    sql.exec(
      `INSERT INTO leases (run, holder, epoch, claimed_ms) VALUES (?, ?, ?, ?) ON CONFLICT(run) DO UPDATE SET holder = excluded.holder, epoch = excluded.epoch, claimed_ms = excluded.claimed_ms`,
      run, device, epoch, now,
    );
    if (cur && cur.holder === device && Number(cur.epoch) === epoch) return { ok: true, holder: device, epoch, renewed: true };
    try {
      // Fenced: a stale epoch never overwrites a newer one (holder is assigned before epoch).
      const t0 = Date.now();
      await db(this.env).execute(
        `INSERT INTO lc0.leases (user_id, run_id, holder, epoch, expires_ms, updated_ms) VALUES (?, ?, ?, ?, ${DB_NOW} + ${LEASE_MS}, ${DB_NOW})
         ON DUPLICATE KEY UPDATE holder = IF(VALUES(epoch) > epoch, VALUES(holder), holder), epoch = GREATEST(epoch, VALUES(epoch)), updated_ms = VALUES(updated_ms)`,
        [user, run, device, epoch],
      );
      await db(this.env).execute(
        `INSERT IGNORE INTO lc0.lease_events (user_id, run_id, epoch, holder, at_ms) VALUES (?, ?, ?, ?, ${DB_NOW})`,
        [user, run, epoch, device],
      );
      return { ok: true, holder: device, epoch, db_ms: Date.now() - t0 };
    } catch (e) {
      // Undo only if nothing changed since: the device retries with the same request.
      sql.exec(`DELETE FROM leases WHERE run = ? AND holder = ? AND epoch = ?`, run, device, epoch);
      if (cur) sql.exec(`INSERT OR IGNORE INTO leases (run, holder, epoch, claimed_ms) VALUES (?, ?, ?, ?)`, run, cur.holder, cur.epoch, cur.claimed_ms);
      return { ok: false, error: errInfo(e) };
    }
  }

  release(run: string, device: string) {
    const r = this.ctx.storage.sql.exec(`DELETE FROM leases WHERE run = ? AND holder = ?`, run, device);
    return { ok: r.rowsWritten > 0 };
  }

  async onMessage(ws: WebSocket, data: string | ArrayBuffer) {
    this.msgs++;
    const a = this.attachment(ws);
    if (!a) return;
    const now = Date.now();
    a.lastSeen = now;
    this.setAttachment(ws, a); // not storage: survives hibernation, no rows written
    if (typeof data !== "string") return;
    if (data === "ping") {
      ws.send("pong");
      return;
    }
    let m: any;
    try {
      m = JSON.parse(data);
    } catch {
      return;
    }
    const base = { wake: this.wakeId, wakes: this.wakes, n: this.msgs, now };
    if (m.t === "ping") ws.send(JSON.stringify({ t: "pong", seq: m.seq, ...base }));
    else if (m.t === "claim") ws.send(JSON.stringify({ t: "claim", id: m.id, run: m.run, ...(await this.claim(m.user, m.run, a.device)), ...base }));
    else if (m.t === "release") ws.send(JSON.stringify({ t: "release", id: m.id, run: m.run, ...this.release(m.run, a.device), ...base }));
  }

  onClose(ws: WebSocket) {
    const a = this.attachment(ws);
    if (a) this.ctx.storage.sql.exec(
      `INSERT INTO seen (device, last_seen_ms) VALUES (?, ?) ON CONFLICT(device) DO UPDATE SET last_seen_ms = MAX(last_seen_ms, excluded.last_seen_ms)`,
      a.device, Math.max(a.lastSeen, this.autoResponseAt(ws)),
    );
  }

  async fetch(req: Request): Promise<Response> {
    const url = new URL(req.url);
    const now = Date.now();
    if (url.pathname.endsWith("/ws")) {
      if (req.headers.get("upgrade") !== "websocket") return json({ error: "expected a websocket upgrade" }, 426);
      const device = url.searchParams.get("device") ?? "";
      if (!/^[A-Za-z0-9_-]{1,64}$/.test(device)) return json({ error: "bad device" }, 400);
      // A reconnect replaces the device's older socket (it may be half-dead after sleep).
      for (const old of this.sockets(device)) {
        this.onClose(old);
        try {
          old.close(4000, "replaced by a newer connection");
        } catch {}
      }
      const pair = new WebSocketPair();
      this.acceptSocket(pair[1], { device, lastSeen: now, connectedAt: now });
      return new Response(null, { status: 101, webSocket: pair[0] });
    }
    if (url.pathname.endsWith("/state")) {
      const devices = new Set<string>();
      for (const ws of this.sockets()) {
        const a = this.attachment(ws);
        if (a) devices.add(a.device);
      }
      for (const r of this.ctx.storage.sql.exec(`SELECT device FROM seen`)) devices.add(String(r.device));
      const leases = this.ctx.storage.sql.exec(`SELECT run, holder, epoch, claimed_ms FROM leases`).toArray().map((r: any) => ({
        run: r.run, holder: r.holder, epoch: Number(r.epoch), valid: this.leaseValid({ holder: r.holder, claimed_ms: Number(r.claimed_ms) }, now),
      }));
      return json({
        id: this.ctx.id.toString(), wake: this.wakeId, wakes: this.wakes, msgs_this_wake: this.msgs, awake_for_ms: now - this.wakeAt,
        sockets: this.sockets().length, presence: [...devices].sort().map((d) => this.presence(d, now)), leases,
      });
    }
    if (url.pathname.endsWith("/bench")) {
      // The same single-statement writes, issued from inside the Durable Object.
      const r = await bench(this.env, url.searchParams.get("kind") ?? "upsert", Math.min(Number(url.searchParams.get("n") ?? 20), 45));
      r.colo = (req as any).cf?.colo ?? null;
      return json(r);
    }
    if (url.pathname.endsWith("/claim") && req.method === "POST") {
      const b = (await req.json()) as any;
      return json(await this.claim(b.user, b.run, b.device));
    }
    return json({ error: "not found" }, 404);
  }
}

export class UserHib extends UserBase {
  sockets(device?: string) {
    return this.ctx.getWebSockets(device);
  }
  acceptSocket(ws: WebSocket, a: Attachment) {
    this.ctx.acceptWebSocket(ws, [a.device]);
    ws.serializeAttachment(a);
  }
  attachment(ws: WebSocket) {
    return (ws.deserializeAttachment() as Attachment | null) ?? null;
  }
  setAttachment(ws: WebSocket, a: Attachment) {
    ws.serializeAttachment(a);
  }
  async webSocketMessage(ws: WebSocket, data: string | ArrayBuffer) {
    await this.onMessage(ws, data);
  }
  async webSocketClose(ws: WebSocket, code: number, reason: string) {
    this.onClose(ws);
    try {
      ws.close(code === 1005 ? 1000 : code, reason);
    } catch {}
  }
  async webSocketError(ws: WebSocket) {
    this.onClose(ws);
  }
}

// Pings ("ping" text frames) are answered by the runtime without waking the object; presence and
// lease renewal read the last auto-response time instead.
export class UserAuto extends UserHib {
  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    ctx.setWebSocketAutoResponse(new WebSocketRequestResponsePair("ping", "pong"));
  }
  override autoResponseAt(ws: WebSocket) {
    return this.ctx.getWebSocketAutoResponseTimestamp(ws)?.getTime() ?? 0;
  }
}

export class UserNoHib extends UserBase {
  private live = new Map<WebSocket, Attachment>();
  sockets(device?: string) {
    return [...this.live.entries()].filter(([, a]) => !device || a.device === device).map(([ws]) => ws);
  }
  acceptSocket(ws: WebSocket, a: Attachment) {
    ws.accept();
    this.live.set(ws, a);
    ws.addEventListener("message", (ev) => void this.onMessage(ws, ev.data as string | ArrayBuffer));
    const done = () => {
      this.onClose(ws);
      this.live.delete(ws);
    };
    ws.addEventListener("close", done);
    ws.addEventListener("error", done);
  }
  attachment(ws: WebSocket) {
    return this.live.get(ws) ?? null;
  }
  setAttachment(ws: WebSocket, a: Attachment) {
    this.live.set(ws, a);
  }
}

// ---------- the Worker ----------

function ns(env: Env, variant: string): DurableObjectNamespace<UserBase> | null {
  if (variant === "hib") return env.USER_HIB as any;
  if (variant === "auto") return env.USER_AUTO as any;
  if (variant === "nohib") return env.USER_NOHIB as any;
  return null;
}

export default {
  async fetch(req: Request, env: Env): Promise<Response> {
    const auth = req.headers.get("authorization") ?? "";
    if (!env.LC0_BEARER || !timingSafeEq(auth, `Bearer ${env.LC0_BEARER}`)) return json({ error: "unauthorized" }, 401);
    const url = new URL(req.url);
    const p = url.pathname.split("/").filter(Boolean);
    const colo = (req as any).cf?.colo ?? null;
    try {
      if (p[0] === "health") return json({ ok: true, colo });
      if (p[0] === "tcp") {
        try {
          return json(await tcpProbe(env.DATABASE_URL));
        } catch (e) {
          return json({ ok: false, error: errInfo(e) });
        }
      }
      if (p[0] === "rl") {
        const r = [];
        for (let i = 0; i < 8; i++) r.push((await env.RL.limit({ key: "probe" })).success);
        return json({ results: r });
      }
      if (p[0] === "bench") {
        const r = await bench(env, url.searchParams.get("kind") ?? "upsert", Math.min(Number(url.searchParams.get("n") ?? 20), 45));
        r.colo = colo;
        return json(r);
      }
      if (p[0] === "dberr") return json(await dbErr(env, url.searchParams.get("kind") ?? ""));
      if (p[0] === "intents" && req.method === "POST") return await createIntent(env, await req.json(), url.searchParams.get("fail_after_write") === "1");
      if (p[0] === "intents" && req.method === "GET") {
        const user = url.searchParams.get("user") ?? "";
        const r = await exec(env, "SELECT COUNT(*) AS c, COUNT(DISTINCT run_id) AS runs FROM lc0.run_intents WHERE user_id = ?", [user]);
        return json(r.r.rows?.[0]);
      }
      if (p[0] === "db-lease" && p[1] === "claim" && req.method === "POST") {
        const b = (await req.json()) as any;
        return json(await dbLeaseClaim(env, b.user, b.run, b.device, Number(b.ttl ?? LEASE_MS)));
      }
      if (p[0] === "db-lease" && p[1] === "get") {
        const r = await exec(env, "SELECT holder, epoch FROM lc0.leases WHERE user_id = ? AND run_id = ?", [url.searchParams.get("user"), url.searchParams.get("run")]);
        const e = await exec(env, "SELECT epoch, holder FROM lc0.lease_events WHERE user_id = ? AND run_id = ? ORDER BY epoch", [url.searchParams.get("user"), url.searchParams.get("run")]);
        return json({ lease: r.r.rows?.[0] ?? null, events: e.r.rows ?? [] });
      }
      // /u/<variant>/<user>/(ws|state|claim)
      if (p[0] === "u" && p.length >= 4) {
        const n = ns(env, p[1]);
        if (!n || !/^[A-Za-z0-9_-]{1,64}$/.test(p[2])) return json({ error: "bad variant or user" }, 400);
        // locationHint applies only when the object is first created (e.g. "enam", near aws-us-east-1).
        const hint = url.searchParams.get("hint") as DurableObjectLocationHint | null;
        return n.get(n.idFromName(p[2]), hint ? { locationHint: hint } : undefined).fetch(req);
      }
      return json({ error: "not found" }, 404);
    } catch (e) {
      return json({ error: errInfo(e) }, 500);
    }
  },
};
