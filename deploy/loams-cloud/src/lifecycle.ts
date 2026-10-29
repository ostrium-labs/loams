// Account lifecycle, deletion and retention (design 78 §25.6, §25.9; D1731,
// D1744, D1747, D1757; plan LC2a Task 9).
//
// DELETE /me schedules deletion; the deletion job, run at once and again from
// the cron, deletes the WorkOS user, keeps only a suppression HMAC of the
// contact address, and deletes our account row. Every step is safe to repeat.
//
// The retention jobs carry out §25.6's table. Each statement deletes or clears
// at most BATCH rows, so one cron invocation stays short; a backlog drains over
// the following runs.

import { isPersonSession } from "./admin.ts";
import type { RouteContext } from "./app.ts";
import type { Audit } from "./audit.ts";
import { ApiError, json } from "./http.ts";
import type { JwtVerifier } from "./jwt.ts";
import type { Clock, Logger } from "./ports.ts";
import type { Sql } from "./sql.ts";
import type { Waitlist } from "./waitlist.ts";
import type { WorkOsUsers } from "./workos.ts";

export const DAY = 86_400_000;
export const RETENTION = {
  unconfirmed: 7 * DAY,
  confirmedUntilReminder: 365 * DAY,
  afterReminder: 30 * DAY,
  afterInviteExpired: 30 * DAY,
  suppression: 730 * DAY,
  audit: 365 * DAY,
  spentTokens: 1 * DAY,
  idempotencyKeys: 7 * DAY,
  /** D1731: deletion within 7 days of the request. */
  deletion: 7 * DAY,
} as const;
export const BATCH = 200;

export interface LifecycleDeps {
  sql: Sql;
  waitlist: Waitlist;
  workos: WorkOsUsers;
  audit: Audit;
  clock: Clock;
  logger: Logger;
  verifier: JwtVerifier;
}

export interface RetentionReport {
  unconfirmed: number;
  reminded: number;
  unreconfirmed: number;
  inviteExpired: number;
  ipPrefixes: number;
  tokens: number;
  suppressions: number;
  audit: number;
  idempotencyKeys: number;
  stepUps: number;
}

export class Lifecycle {
  readonly d: LifecycleDeps;
  constructor(deps: LifecycleDeps) {
    this.d = deps;
  }

  /** The signed-in account behind an AuthKit access token (*User* auth, §25.10). */
  async account(req: Request): Promise<{ id: string; workos_user_id: string; status: string }> {
    const m = (req.headers.get("authorization") ?? "").match(/^Bearer\s+([A-Za-z0-9._-]+)$/);
    if (!m) throw new ApiError(401, "UNAUTHENTICATED", "sign in first");
    let sub: string;
    try {
      const c = await this.d.verifier.verify(m[1]);
      if (!isPersonSession(c)) throw new Error("not a person session");
      sub = c.sub;
    } catch {
      throw new ApiError(401, "UNAUTHENTICATED", "sign in first");
    }
    const r = await this.d.sql.execute("SELECT id, workos_user_id, status FROM accounts WHERE workos_user_id = ?", [sub]);
    const a = r.rows[0];
    if (!a) throw new ApiError(401, "UNAUTHENTICATED", "no Loams Cloud account for this sign-in");
    return { id: String(a.id), workos_user_id: String(a.workos_user_id), status: String(a.status) };
  }

  /** DELETE /me: schedule, then try at once. Returns the deadline. */
  async requestDeletion(accountId: string): Promise<number> {
    const now = this.d.clock.now();
    await this.d.sql.execute(
      "UPDATE accounts SET status = 'deleting', deletion_due_ms = ?, updated_ms = ? WHERE id = ? AND status IN ('active', 'suspended')",
      [now + RETENTION.deletion, now, accountId],
    );
    const r = await this.d.sql.execute("SELECT deletion_due_ms FROM accounts WHERE id = ?", [accountId]);
    const by = Number(r.rows[0]?.deletion_due_ms ?? now + RETENTION.deletion);
    await this.d.audit.write({ type: "account.deletion_scheduled", actor: accountId, subject: accountId, detail: { by: new Date(by).toISOString() } });
    return by;
  }

  /**
   * Deletes accounts marked `deleting`: the WorkOS user, then our row, keeping
   * only the contact address's HMAC as a suppression (§25.3). There is no tenant
   * data to export or delete yet in LC2a; the per-user Durable Object and the
   * cell data join this job in LC2, with the export first (D1757).
   */
  async runDeletions(max = 50): Promise<number> {
    const { sql, clock } = this.d;
    const r = await sql.execute(
      `SELECT id, workos_user_id, contact_email_hmac, signin_email_hmac FROM accounts WHERE status = 'deleting' ORDER BY deletion_due_ms LIMIT ${max}`,
    );
    let done = 0;
    for (const a of r.rows) {
      const id = String(a.id);
      try {
        await this.d.workos.deleteUser(String(a.workos_user_id));
      } catch (e) {
        this.d.logger.log("account.workos_delete_failed", { account_id: id, error: String(e) });
        continue; // the next run retries, well inside the 7 days
      }
      for (const h of [a.contact_email_hmac, a.signin_email_hmac]) {
        if (h) await sql.execute("INSERT IGNORE INTO waitlist_suppressed (email_hmac, reason, created_ms) VALUES (?, 'deleted', ?)", [String(h), clock.now()]);
      }
      const del = await sql.execute("DELETE FROM accounts WHERE id = ? AND status = 'deleting'", [id]);
      if (del.rowsAffected === 1) {
        await this.d.audit.write({ type: "account.deleted", actor: "system", subject: id, detail: { workos_user_deleted: true } });
        done++;
      }
    }
    return done;
  }

  /** §25.6's retention table. */
  async runRetention(): Promise<RetentionReport> {
    const { sql, clock, waitlist } = this.d;
    const now = clock.now();
    const rep: RetentionReport = { unconfirmed: 0, reminded: 0, unreconfirmed: 0, inviteExpired: 0, ipPrefixes: 0, tokens: 0, suppressions: 0, audit: 0, idempotencyKeys: 0, stepUps: 0 };

    // Unconfirmed entries: 7 days.
    for (const row of (await sql.execute(`SELECT id FROM waitlist WHERE status = 'pending' AND created_ms <= ? LIMIT ${BATCH}`, [now - RETENTION.unconfirmed])).rows) {
      await waitlist.purge(String(row.id), "unconfirmed_7_days");
      rep.unconfirmed++;
    }
    // Confirmed, never invited, 12 months: "still interested?", then 30 days to answer.
    const stale = await sql.execute(
      `SELECT id, email_hmac, email_enc, status, build, source, source_other, created_ms, confirmed_ms FROM waitlist w
       WHERE status = 'confirmed' AND reminder_sent_ms IS NULL AND confirmed_ms <= ?
         AND NOT EXISTS (SELECT 1 FROM invites i WHERE i.waitlist_id = w.id) LIMIT ${BATCH}`,
      [now - RETENTION.confirmedUntilReminder],
    );
    for (const row of stale.rows) {
      const w = await waitlist.byId(String(row.id));
      if (w) {
        await waitlist.remind(w);
        rep.reminded++;
      }
    }
    for (const row of (await sql.execute(`SELECT id FROM waitlist WHERE status = 'confirmed' AND reminder_sent_ms <= ? LIMIT ${BATCH}`, [now - RETENTION.afterReminder])).rows) {
      await waitlist.purge(String(row.id), "no_answer_to_reminder");
      rep.unreconfirmed++;
    }
    // Invited but expired: 30 days after the last invite closed, unless an admin invited again.
    const expired = await sql.execute(
      `SELECT w.id FROM waitlist w WHERE w.status = 'confirmed'
         AND EXISTS (SELECT 1 FROM invites i WHERE i.waitlist_id = w.id AND i.state = 'expired')
         AND NOT EXISTS (SELECT 1 FROM invites i WHERE i.waitlist_id = w.id AND (i.open_key IS NOT NULL OR i.updated_ms > ?))
       LIMIT ${BATCH}`,
      [now - RETENTION.afterInviteExpired],
    );
    for (const row of expired.rows) {
      await waitlist.purge(String(row.id), "invite_expired_30_days");
      rep.inviteExpired++;
    }
    // IP-prefix HMACs: 7 days.
    rep.ipPrefixes = (await sql.execute(
      `UPDATE waitlist SET ip_prefix_hmac = NULL, ip_prefix_expires_ms = NULL WHERE ip_prefix_expires_ms <= ? LIMIT ${BATCH}`,
      [now],
    )).rowsAffected;
    // Spent or expired tokens, a day after.
    rep.tokens = (await sql.execute(`DELETE FROM waitlist_tokens WHERE expires_ms <= ? OR used_ms <= ? LIMIT ${BATCH}`, [now - RETENTION.spentTokens, now - RETENTION.spentTokens])).rowsAffected;
    // Suppression HMACs: 24 months.
    rep.suppressions = (await sql.execute(`DELETE FROM waitlist_suppressed WHERE created_ms <= ? LIMIT ${BATCH}`, [now - RETENTION.suppression])).rowsAffected;
    // Audit events: 12 months.
    rep.audit = (await sql.execute(`DELETE FROM audit WHERE time_ms <= ? LIMIT ${BATCH}`, [now - RETENTION.audit])).rowsAffected;
    rep.idempotencyKeys = (await sql.execute(`DELETE FROM idempotency_keys WHERE created_ms <= ? LIMIT ${BATCH}`, [now - RETENTION.idempotencyKeys])).rowsAffected;
    rep.stepUps = (await sql.execute(`DELETE FROM admin_step_ups WHERE step_up_until_ms <= ? LIMIT ${BATCH}`, [now])).rowsAffected;
    this.d.logger.log("retention", { ...rep });
    return rep;
  }

  /** DELETE /me. */
  route() {
    return async (rc: RouteContext): Promise<Response | null> => {
      if (rc.path !== "/me" || rc.req.method !== "DELETE") return null;
      const a = await this.account(rc.req);
      const by = await this.requestDeletion(a.id);
      rc.ctx.waitUntil(this.runDeletions().catch((e) => this.d.logger.log("account.deletion_failed", { error: String(e) })));
      return json(rc.requestId, 202, { status: "deletion_scheduled", by: new Date(by).toISOString() });
    };
  }
}

