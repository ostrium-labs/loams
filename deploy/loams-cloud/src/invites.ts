// Invitations through WorkOS (design 78 §25.1, §25.9, §25.10; D1737, D1747;
// plan LC2a Task 5).
//
// POST /admin/invites queues up to 50 confirmed entries under an
// Idempotency-Key. The queue (drain) creates one application-wide WorkOS
// invitation per entry, with expires_in_days (default 14) and no organisation,
// paced by the shared daily email budget (at most 60 invitations a day). WorkOS
// sends the email. We keep only workos_invitation_id.
//
// Exactly one WorkOS invitation per entry, even across retries: an entry is
// claimed with a guarded update on `attempts`, and a retried create first
// adopts the address's pending WorkOS invitation, so a create whose response
// was lost (LC0: aborted writes commit, and WorkOS's are no different) is never
// repeated. At most one open invite per entry holds through invites.open_key.

import type { AdminPrincipal } from "./admin.ts";
import { parseLimit } from "./admin.ts";
import type { RouteContext } from "./app.ts";
import { decodeCursor, encodeCursor, type Audit } from "./audit.ts";
import type { EmailBudget } from "./budget.ts";
import { newId, sha256Hex } from "./crypto.ts";
import { ApiError, invalid, json, readJson } from "./http.ts";
import type { Clock, Logger } from "./ports.ts";
import type { Sql } from "./sql.ts";
import type { Waitlist } from "./waitlist.ts";
import { WorkOsError, type Invitation, type WorkOsInvitations } from "./workos.ts";

export const MAX_BATCH = 50;
export const DEFAULT_EXPIRY_DAYS = 14;
export const MAX_RESENDS = 3;
export const MAX_ATTEMPTS = 5;

export type InviteState = "queued" | "pending" | "accepted" | "expired" | "revoked" | "failed";
const OPEN: readonly InviteState[] = ["queued", "pending", "accepted"];

export interface InviteRow {
  id: string;
  waitlist_id: string;
  email_hmac: string;
  workos_invitation_id: string | null;
  state: InviteState;
  expires_in_days: number;
  expires_ms: number | null;
  resend_count: number;
  attempts: number;
  accepted_user_id: string | null;
  created_by: string;
  batch_key: string;
  created_ms: number;
}

const COLS = "id, waitlist_id, email_hmac, workos_invitation_id, state, expires_in_days, expires_ms, resend_count, attempts, accepted_user_id, created_by, batch_key, created_ms";

function rowOf(r: Record<string, unknown>): InviteRow {
  const num = (v: unknown) => (v == null ? null : Number(v));
  return {
    id: String(r.id),
    waitlist_id: String(r.waitlist_id),
    email_hmac: String(r.email_hmac),
    workos_invitation_id: r.workos_invitation_id == null ? null : String(r.workos_invitation_id),
    state: String(r.state) as InviteState,
    expires_in_days: Number(r.expires_in_days),
    expires_ms: num(r.expires_ms),
    resend_count: Number(r.resend_count),
    attempts: Number(r.attempts),
    accepted_user_id: r.accepted_user_id == null ? null : String(r.accepted_user_id),
    created_by: String(r.created_by),
    batch_key: String(r.batch_key),
    created_ms: Number(r.created_ms),
  };
}

export interface InvitesDeps {
  sql: Sql;
  workos: WorkOsInvitations;
  waitlist: Waitlist;
  budget: EmailBudget;
  audit: Audit;
  clock: Clock;
  logger: Logger;
}

export type SkipReason = "not_found" | "not_confirmed" | "already_invited" | "suppressed";

export class Invites {
  readonly d: InvitesDeps;
  constructor(deps: InvitesDeps) {
    this.d = deps;
  }

  async byId(id: string): Promise<InviteRow | null> {
    const r = await this.d.sql.execute(`SELECT ${COLS} FROM invites WHERE id = ?`, [id]);
    return r.rows[0] ? rowOf(r.rows[0]) : null;
  }

  private async openFor(waitlistId: string): Promise<InviteRow | null> {
    const r = await this.d.sql.execute(`SELECT ${COLS} FROM invites WHERE open_key = ?`, [waitlistId]);
    return r.rows[0] ? rowOf(r.rows[0]) : null;
  }

  private async queuedCount(): Promise<number> {
    const r = await this.d.sql.execute("SELECT COUNT(*) AS n FROM invites WHERE state = 'queued'");
    return Number(r.rows[0]?.n ?? 0);
  }

  /** POST /admin/invites. Returns the response body. Safe to repeat with the same key. */
  async enqueue(admin: string, idemKey: string, body: Record<string, unknown>): Promise<{ invites: { id: string; waitlist_id: string; state: "queued" }[]; skipped: { waitlist_id: string; reason: SkipReason }[] }> {
    const { sql, clock } = this.d;
    const ids = body.waitlist_ids;
    if (!Array.isArray(ids) || ids.length < 1 || ids.length > MAX_BATCH || !ids.every((x) => typeof x === "string" && /^wl_[A-Za-z0-9_-]{22}$/.test(x))) {
      throw invalid(`waitlist_ids is 1 to ${MAX_BATCH} waitlist ids`);
    }
    const unique = [...new Set(ids as string[])];
    const days = body.expires_in_days ?? DEFAULT_EXPIRY_DAYS;
    if (!Number.isInteger(days) || (days as number) < 1 || (days as number) > 30) throw invalid("expires_in_days is 1 to 30");
    const extra = Object.keys(body).filter((k) => k !== "waitlist_ids" && k !== "expires_in_days");
    if (extra.length) throw invalid(`unknown field ${extra[0]}`);

    // The batch key is the admin's Idempotency-Key, scoped to the admin.
    const scope = `admin:${admin}:invites`;
    const requestHash = await sha256Hex(JSON.stringify({ ids: unique, days }));
    const now = clock.now();
    await sql.execute(
      "INSERT IGNORE INTO idempotency_keys (scope, idem_key, request_hash, created_ms) VALUES (?, ?, ?, ?)",
      [scope, idemKey, requestHash, now],
    );
    const k = await sql.execute("SELECT request_hash, response_json FROM idempotency_keys WHERE scope = ? AND idem_key = ?", [scope, idemKey]);
    if (String(k.rows[0]?.request_hash) !== requestHash) throw new ApiError(409, "IDEMPOTENCY_KEY_REUSED", "this Idempotency-Key was used for a different batch");
    if (k.rows[0]?.response_json) return JSON.parse(String(k.rows[0].response_json));

    const batchKey = (await sha256Hex(scope + "\u0000" + idemKey)).slice(0, 32);
    const already = new Map<string, InviteRow>();
    for (const r of (await sql.execute(`SELECT ${COLS} FROM invites WHERE batch_key = ?`, [batchKey])).rows) {
      const row = rowOf(r);
      already.set(row.waitlist_id, row);
    }

    const eligible: { id: string; email_hmac: string }[] = [];
    const skipped: { waitlist_id: string; reason: SkipReason }[] = [];
    for (const id of unique) {
      if (already.has(id)) continue;
      const w = await this.d.waitlist.byId(id);
      if (!w) skipped.push({ waitlist_id: id, reason: "not_found" });
      else if (await this.d.waitlist.isSuppressed(w.email_hmac)) skipped.push({ waitlist_id: id, reason: "suppressed" });
      else if (w.status === "pending") skipped.push({ waitlist_id: id, reason: "not_confirmed" });
      else if (w.status === "invited" || (await this.openFor(id))) skipped.push({ waitlist_id: id, reason: "already_invited" });
      else eligible.push({ id, email_hmac: w.email_hmac });
    }

    // The daily cap: today's remaining invitations, less those already queued.
    if (eligible.length) {
      const room = (await this.d.budget.remaining("invite", now)) - (await this.queuedCount());
      if (eligible.length > room) {
        throw new ApiError(409, "DAILY_SEND_CAP", `today's invitation budget has room for ${Math.max(0, room)} more; try a smaller batch or tomorrow`);
      }
    }

    for (const e of eligible) {
      const inviteId = newId("inv");
      const ins = await sql.execute(
        `INSERT IGNORE INTO invites (id, waitlist_id, email_hmac, state, open_key, expires_in_days, created_by, batch_key, created_ms, updated_ms)
         VALUES (?, ?, ?, 'queued', ?, ?, ?, ?, ?, ?)`,
        [inviteId, e.id, e.email_hmac, e.id, days, admin, batchKey, now, now],
      );
      if (ins.rowsAffected === 1) {
        await sql.execute("UPDATE waitlist SET status = 'invited', updated_ms = ? WHERE id = ? AND status = 'confirmed'", [now, e.id]);
        await this.d.audit.write({ type: "invite.created", actor: admin, subject: inviteId, detail: { waitlist_id: e.id } });
        continue;
      }
      // Lost the race to another batch, or our own earlier attempt landed.
      const open = await this.openFor(e.id);
      if (open && open.batch_key === batchKey) already.set(e.id, open);
      else skipped.push({ waitlist_id: e.id, reason: "already_invited" });
    }
    for (const r of (await sql.execute(`SELECT ${COLS} FROM invites WHERE batch_key = ?`, [batchKey])).rows) {
      const row = rowOf(r);
      already.set(row.waitlist_id, row);
    }

    const out = {
      invites: unique.filter((id) => already.has(id)).map((id) => ({ id: already.get(id)!.id, waitlist_id: id, state: "queued" as const })),
      skipped,
    };
    await sql.execute("UPDATE idempotency_keys SET response_json = ? WHERE scope = ? AND idem_key = ? AND response_json IS NULL", [JSON.stringify(out), scope, idemKey]);
    return out;
  }

  /**
   * Sends queued invitations while the budget allows. Run after POST
   * /admin/invites and from the cron sweep. Returns how many were sent.
   */
  async drain(max = MAX_BATCH): Promise<number> {
    const { sql, clock } = this.d;
    const r = await sql.execute(`SELECT ${COLS} FROM invites WHERE state = 'queued' ORDER BY created_ms, id LIMIT ${max}`);
    let sent = 0;
    for (const raw of r.rows) {
      const inv = rowOf(raw);
      // Claim it: one drain per invite at a time.
      const claim = await sql.execute(
        "UPDATE invites SET attempts = attempts + 1, updated_ms = ? WHERE id = ? AND state = 'queued' AND attempts = ?",
        [clock.now(), inv.id, inv.attempts],
      );
      if (claim.rowsAffected !== 1) continue;
      const w = await this.d.waitlist.byId(inv.waitlist_id);
      if (!w) {
        await this.close(inv.id, "revoked", ["queued"]);
        continue;
      }
      const email = await this.d.waitlist.decryptEmail(w);
      let invitation: Invitation | null = null;
      try {
        // Adopt a pending invitation an earlier attempt created before taking budget for a new one.
        if (inv.attempts > 0) invitation = await this.d.workos.findPendingInvitation(email);
        if (!invitation) {
          if (!(await this.d.budget.take("invite", clock.now()))) {
            // Out of budget: give the claim back and stop until tomorrow.
            await sql.execute("UPDATE invites SET attempts = attempts - 1 WHERE id = ? AND state = 'queued' AND attempts = ?", [inv.id, inv.attempts + 1]);
            this.d.logger.log("invites.budget_exhausted", { invite_id: inv.id });
            break;
          }
          invitation = await this.d.workos.createInvitation(email, inv.expires_in_days);
        }
      } catch (e) {
        this.d.logger.log("invites.create_failed", { invite_id: inv.id, attempt: inv.attempts + 1, error: String(e) });
        if (inv.attempts + 1 >= MAX_ATTEMPTS) {
          if (await this.close(inv.id, "failed", ["queued"])) {
            await this.d.audit.write({ type: "invite.failed", actor: "system", subject: inv.id, detail: { attempts: inv.attempts + 1 } });
          }
        }
        continue;
      }
      const u = await sql.execute(
        "UPDATE invites SET workos_invitation_id = ?, state = 'pending', expires_ms = ?, updated_ms = ? WHERE id = ? AND state = 'queued'",
        [invitation.id, invitation.expiresAt, clock.now(), inv.id],
      );
      if (u.rowsAffected === 1) {
        sent++;
        await this.d.audit.write({ type: "invite.sent", actor: "system", subject: inv.id, detail: { workos_invitation_id: invitation.id } });
      } else {
        // Revoked while we were creating it: revoke it at WorkOS too.
        await this.d.workos.revokeInvitation(invitation.id).catch(() => undefined);
      }
    }
    return sent;
  }

  /** Closes an invite from one of `from`; reopens the waitlist entry for a revoke. */
  private async close(id: string, state: "revoked" | "expired" | "failed", from: readonly InviteState[]): Promise<boolean> {
    const { sql, clock } = this.d;
    const now = clock.now();
    const u = await sql.execute(
      `UPDATE invites SET state = ?, open_key = NULL, updated_ms = ? WHERE id = ? AND state IN (${from.map(() => "?").join(", ")})`,
      [state, now, id, ...from],
    );
    if (u.rowsAffected !== 1) return false;
    const inv = await this.byId(id);
    if (inv) await sql.execute("UPDATE waitlist SET status = 'confirmed', updated_ms = ? WHERE id = ? AND status = 'invited'", [now, inv.waitlist_id]);
    return true;
  }

  /** Mirrors WorkOS's view of an invitation into our row. */
  private async mirror(inv: InviteRow, w: Invitation): Promise<InviteRow> {
    const { sql, clock } = this.d;
    if (w.state === "pending") return inv;
    if (w.state === "accepted") {
      await sql.execute(
        "UPDATE invites SET state = 'accepted', accepted_user_id = ?, updated_ms = ? WHERE id = ? AND state IN ('queued', 'pending')",
        [w.acceptedUserId, clock.now(), inv.id],
      );
    } else {
      await this.close(inv.id, w.state, ["queued", "pending"]);
      if (w.state === "expired") await this.d.audit.write({ type: "invite.expired", actor: "system", subject: inv.id });
    }
    return (await this.byId(inv.id))!;
  }

  /**
   * The hourly sweep (LC2a Task 9): WorkOS emits no invitation.expired event
   * (R-LC2a-5.6), so pending invites past their expiry are read back from WorkOS
   * and mirrored. Returns how many changed state.
   */
  async sweep(max = 200): Promise<number> {
    const r = await this.d.sql.execute(
      `SELECT ${COLS} FROM invites WHERE state = 'pending' AND expires_ms <= ? ORDER BY expires_ms LIMIT ${max}`,
      [this.d.clock.now()],
    );
    let changed = 0;
    for (const raw of r.rows) {
      const inv = rowOf(raw);
      if (!inv.workos_invitation_id) continue;
      try {
        const after = await this.mirror(inv, await this.d.workos.getInvitation(inv.workos_invitation_id));
        if (after.state !== inv.state) changed++;
      } catch (e) {
        this.d.logger.log("invites.sweep_failed", { invite_id: inv.id, error: String(e) });
      }
    }
    return changed;
  }

  /** POST /admin/invites/{id}/revoke. */
  async revoke(id: string, admin: string): Promise<InviteRow> {
    const inv = await this.byId(id);
    if (!inv) throw new ApiError(404, "NOT_FOUND", "no such invite");
    if (inv.state === "queued") {
      if (await this.close(id, "revoked", ["queued"])) {
        await this.d.audit.write({ type: "invite.revoked", actor: admin, subject: id });
        return (await this.byId(id))!;
      }
      return this.revoke(id, admin); // it was sent meanwhile
    }
    if (inv.state !== "pending" || !inv.workos_invitation_id) throw new ApiError(409, "INVITE_NOT_REVOCABLE", `the invite is ${inv.state}`);
    let w: Invitation;
    try {
      w = await this.d.workos.revokeInvitation(inv.workos_invitation_id);
    } catch (e) {
      // Already revoked, accepted or expired at WorkOS: read its state and mirror it.
      if (!(e instanceof WorkOsError) || e.status >= 500) throw e;
      w = await this.d.workos.getInvitation(inv.workos_invitation_id);
    }
    const after = await this.mirror(inv, w);
    if (after.state === "revoked") {
      await this.d.audit.write({ type: "invite.revoked", actor: admin, subject: id, detail: { workos_invitation_id: inv.workos_invitation_id } });
      return after;
    }
    throw new ApiError(409, "INVITE_NOT_REVOCABLE", `the invite is ${after.state}`);
  }

  /** Revokes every open invite of a waitlist entry (its removal, or a bounce). */
  async revokeAllFor(waitlistId: string, actor: string): Promise<void> {
    const r = await this.d.sql.execute(`SELECT ${COLS} FROM invites WHERE waitlist_id = ? AND state IN ('queued', 'pending')`, [waitlistId]);
    for (const row of r.rows) {
      try {
        await this.revoke(String(row.id), actor);
      } catch (e) {
        if (!(e instanceof ApiError)) throw e;
      }
    }
  }

  /** POST /admin/invites/{id}/resend, with `extend` revoking and sending a new invitation (D1747). */
  async resend(id: string, admin: string, extend: boolean): Promise<InviteRow> {
    const { sql, clock } = this.d;
    let inv = await this.byId(id);
    if (!inv) throw new ApiError(404, "NOT_FOUND", "no such invite");
    if (inv.state === "pending" && inv.workos_invitation_id) {
      // WorkOS may have moved on (accepted, or expired with no event, R-LC2a-5.6): check first.
      inv = await this.mirror(inv, await this.d.workos.getInvitation(inv.workos_invitation_id));
    }
    if (inv.state !== "pending" || !inv.workos_invitation_id) throw new ApiError(409, "INVITE_NOT_RESENDABLE", `the invite is ${inv.state}`);
    if (inv.resend_count >= MAX_RESENDS) throw new ApiError(409, "RESEND_LIMIT", `an invite is resent at most ${MAX_RESENDS} times`);
    // Reserve the resend first: the guarded count makes a retried call a no-op instead of a fourth send.
    const bump = await sql.execute(
      "UPDATE invites SET resend_count = resend_count + 1, updated_ms = ? WHERE id = ? AND state = 'pending' AND resend_count = ?",
      [clock.now(), id, inv.resend_count],
    );
    if (bump.rowsAffected !== 1) throw new ApiError(409, "INVITE_NOT_RESENDABLE", "the invite changed; reload and try again");
    if (!(await this.d.budget.take("invite", clock.now()))) {
      await sql.execute("UPDATE invites SET resend_count = resend_count - 1 WHERE id = ? AND resend_count = ?", [id, inv.resend_count + 1]);
      throw new ApiError(409, "DAILY_SEND_CAP", "today's invitation budget is spent; try tomorrow");
    }
    const old = inv.workos_invitation_id;
    if (!extend) {
      const w = await this.d.workos.resendInvitation(old);
      await sql.execute("UPDATE invites SET expires_ms = ?, updated_ms = ? WHERE id = ?", [w.expiresAt, clock.now(), id]);
      await this.d.audit.write({ type: "invite.resent", actor: admin, subject: id, detail: { workos_invitation_id: old, extend: false } });
      return (await this.byId(id))!;
    }
    // Extend: revoke, then create a new invitation for the full term from now.
    const w = await this.d.waitlist.byId(inv.waitlist_id);
    if (!w) throw new ApiError(409, "INVITE_NOT_RESENDABLE", "the waitlist entry is gone");
    const email = await this.d.waitlist.decryptEmail(w);
    await this.d.workos.revokeInvitation(old).catch((e) => {
      if (!(e instanceof WorkOsError) || e.status >= 500) throw e;
    });
    const fresh = (await this.d.workos.findPendingInvitation(email)) ?? (await this.d.workos.createInvitation(email, inv.expires_in_days));
    await sql.execute(
      "UPDATE invites SET workos_invitation_id = ?, expires_ms = ?, updated_ms = ? WHERE id = ? AND workos_invitation_id = ?",
      [fresh.id, fresh.expiresAt, clock.now(), id, old],
    );
    await this.d.audit.write({ type: "invite.resent", actor: admin, subject: id, detail: { workos_invitation_id: fresh.id, replaces: old, extend: true } });
    return (await this.byId(id))!;
  }

  async list(q: URLSearchParams): Promise<{ items: unknown[]; next_cursor: string | null }> {
    const limit = parseLimit(q.get("limit"), 50, 200);
    const where: string[] = [];
    const args: unknown[] = [];
    const state = q.get("state");
    if (state) {
      if (![...OPEN, "expired", "revoked", "failed"].includes(state as InviteState)) throw invalid("unknown state");
      where.push("state = ?");
      args.push(state);
    }
    const cursor = q.get("cursor");
    if (cursor) {
      let t: number, cid: string;
      try {
        [t, cid] = decodeCursor(cursor);
      } catch {
        throw invalid("bad cursor");
      }
      where.push("(created_ms < ? OR (created_ms = ? AND id < ?))");
      args.push(t, t, cid);
    }
    const r = await this.d.sql.execute(
      `SELECT ${COLS} FROM invites ${where.length ? "WHERE " + where.join(" AND ") : ""} ORDER BY created_ms DESC, id DESC LIMIT ${limit + 1}`,
      args,
    );
    const rows = r.rows.slice(0, limit).map(rowOf);
    const last = rows.at(-1);
    return {
      items: rows.map(present),
      next_cursor: r.rows.length > limit && last ? encodeCursor(last.created_ms, last.id) : null,
    };
  }

  /** The admin routes for invites, for Admin.routes(). */
  routes() {
    return async (p: AdminPrincipal, rc: RouteContext): Promise<Response | null> => {
      const { req, path, url, requestId } = rc;
      if (path === "/admin/invites" && req.method === "POST") {
        const key = req.headers.get("idempotency-key") ?? "";
        if (!/^[A-Za-z0-9_-]{8,64}$/.test(key)) throw invalid("an Idempotency-Key header of 8 to 64 characters [A-Za-z0-9_-] is required");
        const out = await this.enqueue(p.userId, key, await readJson(req));
        rc.ctx.waitUntil(this.drain().catch((e) => this.d.logger.log("invites.drain_failed", { error: String(e) })));
        return json(requestId, 202, out);
      }
      if (path === "/admin/invites" && req.method === "GET") return json(requestId, 200, await this.list(url.searchParams));
      const m = path.match(/^\/admin\/invites\/(inv_[A-Za-z0-9_-]{22})\/(revoke|resend)$/);
      if (m && req.method === "POST") {
        if (m[2] === "revoke") {
          const inv = await this.revoke(m[1], p.userId);
          return json(requestId, 200, { id: inv.id, state: inv.state });
        }
        const body = await readJson(req);
        if (body.extend !== undefined && typeof body.extend !== "boolean") throw invalid("extend is a boolean");
        const inv = await this.resend(m[1], p.userId, body.extend === true);
        return json(requestId, 200, { id: inv.id, state: inv.state, expires_at: inv.expires_ms == null ? null : new Date(inv.expires_ms).toISOString(), resend_count: inv.resend_count });
      }
      return null;
    };
  }
}

export function present(i: InviteRow) {
  return {
    id: i.id,
    waitlist_id: i.waitlist_id,
    workos_invitation_id: i.workos_invitation_id,
    state: i.state,
    expires_at: i.expires_ms == null ? null : new Date(i.expires_ms).toISOString(),
    resend_count: i.resend_count,
    created_by: i.created_by,
    created_at: new Date(i.created_ms).toISOString(),
  };
}
