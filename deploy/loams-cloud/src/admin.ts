// Admin authorisation and the admin read endpoints (design 78 §25.8, §25.10;
// D1746; plan LC2a Task 4).
//
// Every /admin/* call needs all three, checked in this order:
//   1. a valid AuthKit access token for a person's session, with org_id equal to
//      the Ostrium WorkOS organisation and role cloud_admin;
//   2. the user in config/admins.toml, the allow-list a reviewed PR changes;
//   3. a TOTP step-up through WorkOS's MFA API within the last 15 minutes,
//      recorded against the token's sid.
// /admin/mfa/enroll and /admin/mfa/verify need 1 and 2 only.
//
// Agents and service accounts can never be cloud_admin (D1746, like meter_admin,
// D1603): a token without a person's session (no sid, a non-user subject, an
// M2M client) or with an `act` claim (delegation) is refused before the role is
// read, and deploy/openfga's platform#cloud_admin admits users only.

import type { Audit } from "./audit.ts";
import { decodeCursor, encodeCursor } from "./audit.ts";
import { ApiError, invalid, json, readJson, reqString } from "./http.ts";
import type { JwtVerifier, AccessClaims } from "./jwt.ts";
import type { Clock, Logger, RateLimiter } from "./ports.ts";
import type { Sql } from "./sql.ts";
import { parseToml } from "./toml.ts";
import type { RouteContext } from "./app.ts";
import type { Waitlist } from "./waitlist.ts";
import type { WorkOsMfa } from "./workos.ts";

export const STEP_UP_MS = 15 * 60_000;
export const ADMIN_PER_MIN = 60;
export const ROLE = "cloud_admin";

export interface AdminEntry {
  workos_user_id: string;
  name: string;
}

/** config/admins.toml: `[[admin]]` tables with `workos_user_id` and `name`. */
export function parseAdmins(text: string): AdminEntry[] {
  const t = parseToml(text, "admins.toml");
  const list = (t.admin ?? []) as unknown;
  if (!Array.isArray(list)) throw new Error("admins.toml: [[admin]] tables expected");
  const out: AdminEntry[] = [];
  for (const a of list as Record<string, unknown>[]) {
    const id = a.workos_user_id;
    const name = a.name;
    if (typeof id !== "string" || !/^user_[A-Za-z0-9]{10,40}$/.test(id)) throw new Error(`admins.toml: bad workos_user_id ${String(id)}`);
    if (typeof name !== "string" || !name) throw new Error(`admins.toml: ${id} needs a name`);
    if (out.some((x) => x.workos_user_id === id)) throw new Error(`admins.toml: ${id} listed twice`);
    out.push({ workos_user_id: id, name });
  }
  return out;
}

export interface AdminConfig {
  /** The Ostrium WorkOS organisation's id (org_...). */
  ostriumOrgId: string;
  admins: readonly AdminEntry[];
  /** The TOTP issuer shown in authenticator apps. */
  totpIssuer: string;
}

export interface AdminDeps {
  sql: Sql;
  verifier: JwtVerifier;
  mfa: WorkOsMfa;
  config: AdminConfig;
  waitlist: Waitlist;
  audit: Audit;
  limiter: RateLimiter;
  clock: Clock;
  logger: Logger;
}

export interface AdminPrincipal {
  userId: string;
  sid: string;
  claims: AccessClaims;
}

const unauthenticated = () => new ApiError(401, "UNAUTHENTICATED", "sign in as an Ostrium admin", { "www-authenticate": 'Bearer realm="loams-cloud-admin"' });
const denied = () => new ApiError(403, "PERMISSION_DENIED", "this account can't administer Loams Cloud");

/** Whether the token is a person's own session: not an agent, a service account or a delegation. */
export function isPersonSession(c: AccessClaims): boolean {
  return typeof c.sid === "string" && c.sid.length > 0 && /^user_/.test(c.sub) && c.act === undefined && c.may_act === undefined;
}

export class Admin {
  readonly d: AdminDeps;
  constructor(deps: AdminDeps) {
    this.d = deps;
  }

  /** Checks 1 and 2. Throws 401 or 403. */
  async principal(req: Request): Promise<AdminPrincipal> {
    const h = req.headers.get("authorization") ?? "";
    const m = h.match(/^Bearer\s+([A-Za-z0-9._-]+)$/);
    if (!m) throw unauthenticated();
    let claims: AccessClaims;
    try {
      claims = await this.d.verifier.verify(m[1]);
    } catch (e) {
      this.d.logger.log("admin.token_rejected", { reason: String((e as Error).message) });
      throw unauthenticated();
    }
    if (!isPersonSession(claims)) {
      this.d.logger.log("admin.denied", { reason: "not_a_person_session", sub: claims.sub });
      throw denied();
    }
    const roles = new Set([claims.role, ...(Array.isArray(claims.roles) ? claims.roles : [])]);
    if (claims.org_id !== this.d.config.ostriumOrgId || !roles.has(ROLE)) {
      this.d.logger.log("admin.denied", { reason: "role", sub: claims.sub });
      throw denied();
    }
    if (!this.d.config.admins.some((a) => a.workos_user_id === claims.sub)) {
      this.d.logger.log("admin.denied", { reason: "not_allow_listed", sub: claims.sub });
      throw denied();
    }
    const rl = await this.d.limiter.hit(`admin:${claims.sub}`, ADMIN_PER_MIN, 60_000);
    if (!rl.ok) throw new ApiError(429, "RATE_LIMITED", "too many admin calls", { "retry-after": String(Math.ceil(rl.retryAfterMs / 1000)) });
    return { userId: claims.sub, sid: claims.sid!, claims };
  }

  /** Check 3. Throws 403 STEP_UP_REQUIRED. */
  async requireStepUp(p: AdminPrincipal): Promise<void> {
    const r = await this.d.sql.execute("SELECT workos_user_id, step_up_until_ms FROM admin_step_ups WHERE sid = ?", [p.sid]);
    const row = r.rows[0];
    if (!row || String(row.workos_user_id) !== p.userId || Number(row.step_up_until_ms) <= this.d.clock.now()) {
      throw new ApiError(403, "STEP_UP_REQUIRED", "verify a TOTP code first (POST /admin/mfa/verify)");
    }
  }

  async factorOf(userId: string): Promise<string | null> {
    const r = await this.d.sql.execute("SELECT factor_id FROM admin_mfa_factors WHERE workos_user_id = ?", [userId]);
    return r.rows[0] ? String(r.rows[0].factor_id) : null;
  }

  async enroll(p: AdminPrincipal): Promise<{ factor_id: string; qr_code: string; secret: string; uri: string }> {
    if (await this.factorOf(p.userId)) throw new ApiError(409, "ALREADY_ENROLLED", "this admin already has a TOTP factor");
    const e = await this.d.mfa.enrollTotp(p.userId, this.d.config.totpIssuer, p.userId);
    // INSERT IGNORE: two racing enrolments keep the first factor; the loser is told so.
    const ins = await this.d.sql.execute("INSERT IGNORE INTO admin_mfa_factors (workos_user_id, factor_id, created_ms) VALUES (?, ?, ?)", [p.userId, e.factorId, this.d.clock.now()]);
    if (ins.rowsAffected !== 1 && (await this.factorOf(p.userId)) !== e.factorId) throw new ApiError(409, "ALREADY_ENROLLED", "this admin already has a TOTP factor");
    await this.d.audit.write({ type: "admin.mfa_enrolled", actor: p.userId, subject: p.userId, detail: { factor_id: e.factorId } });
    return { factor_id: e.factorId, qr_code: e.qrCode, secret: e.secret, uri: e.uri };
  }

  async verify(p: AdminPrincipal, code: string): Promise<number> {
    if (!/^\d{6}$/.test(code)) throw new ApiError(422, "CODE_INVALID", "the code is six digits");
    const factor = await this.factorOf(p.userId);
    if (!factor) throw new ApiError(409, "NOT_ENROLLED", "enrol a TOTP factor first (POST /admin/mfa/enroll)");
    const rl = await this.d.limiter.hit(`mfa:${p.userId}`, 5, 5 * 60_000);
    if (!rl.ok) throw new ApiError(429, "RATE_LIMITED", "too many codes; wait a few minutes", { "retry-after": String(Math.ceil(rl.retryAfterMs / 1000)) });
    const challenge = await this.d.mfa.challenge(factor);
    if (!(await this.d.mfa.verify(challenge, code))) {
      await this.d.audit.write({ type: "admin.denied", actor: p.userId, subject: p.userId, detail: { reason: "code_invalid" } });
      throw new ApiError(422, "CODE_INVALID", "that code is not valid");
    }
    const now = this.d.clock.now();
    const until = now + STEP_UP_MS;
    await this.d.sql.execute(
      `INSERT INTO admin_step_ups (sid, workos_user_id, step_up_until_ms, updated_ms) VALUES (?, ?, ?, ?)
       ON DUPLICATE KEY UPDATE workos_user_id = VALUES(workos_user_id), step_up_until_ms = VALUES(step_up_until_ms), updated_ms = VALUES(updated_ms)`,
      [p.sid, p.userId, until, now],
    );
    await this.d.audit.write({ type: "admin.step_up", actor: p.userId, subject: p.userId, detail: { sid: p.sid } });
    return until;
  }

  async listWaitlist(q: URLSearchParams): Promise<{ items: unknown[]; next_cursor: string | null }> {
    const limit = parseLimit(q.get("limit"), 50, 200);
    const where: string[] = [];
    const args: unknown[] = [];
    const status = q.get("status");
    if (status) {
      if (!["pending", "confirmed", "invited"].includes(status)) throw invalid("status is pending, confirmed or invited");
      where.push("status = ?");
      args.push(status);
    }
    const source = q.get("source");
    if (source) { where.push("source = ?"); args.push(source); }
    const since = parseSince(q.get("since"));
    if (since !== undefined) { where.push("created_ms >= ?"); args.push(since); }
    const cursor = q.get("cursor");
    if (cursor) {
      const [t, id] = cursorOrThrow(cursor);
      where.push("(created_ms < ? OR (created_ms = ? AND id < ?))");
      args.push(t, t, id);
    }
    const r = await this.d.sql.execute(
      `SELECT id, email_enc, status, build, source, source_other, created_ms, confirmed_ms FROM waitlist
       ${where.length ? "WHERE " + where.join(" AND ") : ""} ORDER BY created_ms DESC, id DESC LIMIT ${limit + 1}`,
      args,
    );
    const rows = r.rows.slice(0, limit);
    const invites = new Map<string, { id: string; state: string; expires_at: string | null }>();
    if (rows.length) {
      const ids = rows.map((x) => String(x.id));
      const inv = await this.d.sql.execute(
        `SELECT id, waitlist_id, state, expires_ms FROM invites WHERE waitlist_id IN (${ids.map(() => "?").join(", ")}) ORDER BY created_ms`,
        ids,
      );
      for (const i of inv.rows) {
        invites.set(String(i.waitlist_id), { id: String(i.id), state: String(i.state), expires_at: i.expires_ms == null ? null : new Date(Number(i.expires_ms)).toISOString() });
      }
    }
    const items = [];
    for (const x of rows) {
      items.push({
        id: String(x.id),
        email: await this.d.waitlist.decryptEmail({ email_enc: String(x.email_enc) }),
        status: String(x.status),
        created_at: new Date(Number(x.created_ms)).toISOString(),
        confirmed_at: x.confirmed_ms == null ? null : new Date(Number(x.confirmed_ms)).toISOString(),
        build: x.build ?? null,
        source: x.source ?? null,
        source_other: x.source_other ?? null,
        invite: invites.get(String(x.id)) ?? null,
      });
    }
    const last = rows.at(-1);
    return { items, next_cursor: r.rows.length > limit && last ? encodeCursor(Number(last.created_ms), String(last.id)) : null };
  }

  /** The admin routes. Later tasks add their own through `extra`. */
  routes(extra: ((p: AdminPrincipal, rc: RouteContext) => Promise<Response | null>)[] = []) {
    return async (rc: RouteContext): Promise<Response | null> => {
      const { req, path, url, requestId } = rc;
      if (!path.startsWith("/admin/")) return null;
      const p = await this.principal(req);

      if (path === "/admin/mfa/enroll" && req.method === "POST") return json(requestId, 200, await this.enroll(p));
      if (path === "/admin/mfa/verify" && req.method === "POST") {
        const body = await readJson(req);
        const until = await this.verify(p, reqString(body, "code", 16));
        return json(requestId, 200, { step_up_until: new Date(until).toISOString() });
      }

      await this.requireStepUp(p);

      if (path === "/admin/waitlist" && req.method === "GET") {
        const out = await this.listWaitlist(url.searchParams);
        await this.d.audit.write({ type: "admin.waitlist_read", actor: p.userId, subject: "waitlist", detail: { count: out.items.length } });
        return json(requestId, 200, out);
      }
      const del = path.match(/^\/admin\/waitlist\/(wl_[A-Za-z0-9_-]{22})$/);
      if (del && req.method === "DELETE") {
        if (!(await this.d.waitlist.remove(del[1], "deleted", p.userId))) throw new ApiError(404, "NOT_FOUND", "no such waitlist entry");
        return json(requestId, 204, null);
      }
      if (path === "/admin/audit" && req.method === "GET") {
        const q = url.searchParams;
        try {
          return json(requestId, 200, await this.d.audit.list({
            subject: q.get("subject") ?? undefined,
            type: q.get("type") ?? undefined,
            since: parseSince(q.get("since")),
            cursor: q.get("cursor") ?? undefined,
            limit: parseLimit(q.get("limit"), 50, 200),
          }));
        } catch (e) {
          if (e instanceof Error && e.message === "bad cursor") throw invalid("bad cursor");
          throw e;
        }
      }
      for (const r of extra) {
        const res = await r(p, rc);
        if (res) return res;
      }
      throw new ApiError(404, "NOT_FOUND", "no such endpoint");
    };
  }
}

export function parseLimit(v: string | null, dflt: number, max: number): number {
  if (v === null || v === "") return dflt;
  const n = Number(v);
  if (!Number.isInteger(n) || n < 1 || n > max) throw invalid(`limit is 1 to ${max}`);
  return n;
}

export function parseSince(v: string | null): number | undefined {
  if (v === null || v === "") return undefined;
  const t = /^\d+$/.test(v) ? Number(v) : Date.parse(v);
  if (!Number.isFinite(t)) throw invalid("since is an ISO date or epoch milliseconds");
  return t;
}

function cursorOrThrow(c: string): [number, string] {
  try {
    return decodeCursor(c);
  } catch {
    throw invalid("bad cursor");
  }
}
