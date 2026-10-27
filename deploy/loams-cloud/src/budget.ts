// The shared daily email budget (design 78 §25.2, D1738, Q1288; R-LC2a-4).
// Resend Free sends 100 emails a day across WorkOS's mail (invitations, magic
// codes) and the Worker's own (confirm, already listed, removal). At most 60 of
// them are invitations. The counts live in the control DB's email_budget table.
//
// WorkOS's magic codes and verification mails go through Resend too, and we
// can't count them, so the total is a ceiling on what we cause, not on what
// Resend sends. TOTAL_PER_DAY can be set lower to keep a margin for them.

import type { Sql } from "./sql.ts";

export const TOTAL_PER_DAY = 100;
export const INVITES_PER_DAY = 60;

export type BudgetKind = "invite" | "waitlist";

export function utcDay(ms: number): string {
  return new Date(ms).toISOString().slice(0, 10);
}

export class EmailBudget {
  private readonly sql: Sql;
  private readonly limits: { total: number; invite: number; waitlist: number };
  constructor(sql: Sql, limits: Partial<{ total: number; invite: number; waitlist: number }> = {}) {
    this.sql = sql;
    const total = limits.total ?? TOTAL_PER_DAY;
    this.limits = { total, invite: limits.invite ?? INVITES_PER_DAY, waitlist: limits.waitlist ?? total };
  }

  async used(nowMs: number): Promise<{ total: number; invite: number; waitlist: number }> {
    const r = await this.sql.execute("SELECT kind, n FROM email_budget WHERE day = ?", [utcDay(nowMs)]);
    const out = { total: 0, invite: 0, waitlist: 0 };
    for (const row of r.rows) {
      const k = String(row.kind) as keyof typeof out;
      if (k in out) out[k] = Number(row.n);
    }
    return out;
  }

  /** How many more of `kind` may go out today. */
  async remaining(kind: BudgetKind, nowMs: number): Promise<number> {
    const u = await this.used(nowMs);
    return Math.max(0, Math.min(this.limits.total - u.total, this.limits[kind] - u[kind]));
  }

  /**
   * Reserves one send. Two guarded increments: the total, then the kind. If the
   * kind is full after the total was taken, the total is given back. A retried
   * increment over-counts, which only ever sends less.
   */
  async take(kind: BudgetKind, nowMs: number): Promise<boolean> {
    const day = utcDay(nowMs);
    await this.sql.execute(
      "INSERT IGNORE INTO email_budget (day, kind, n, updated_ms) VALUES (?, 'total', 0, ?), (?, ?, 0, ?)",
      [day, nowMs, day, kind, nowMs],
    );
    const t = await this.sql.execute(
      "UPDATE email_budget SET n = n + 1, updated_ms = ? WHERE day = ? AND kind = 'total' AND n < ?",
      [nowMs, day, this.limits.total],
    );
    if (t.rowsAffected !== 1) return false;
    const k = await this.sql.execute(
      "UPDATE email_budget SET n = n + 1, updated_ms = ? WHERE day = ? AND kind = ? AND n < ?",
      [nowMs, day, kind, this.limits[kind]],
    );
    if (k.rowsAffected === 1) return true;
    await this.sql.execute("UPDATE email_budget SET n = n - 1, updated_ms = ? WHERE day = ? AND kind = 'total' AND n > 0", [nowMs, day]);
    return false;
  }
}
