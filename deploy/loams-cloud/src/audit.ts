// Audit events (D1748): CloudEvents under dev.loams.cloud.*, stored in the
// control DB's `audit` table, with ids and HMACs as subjects and never a
// plaintext address. Each event has a Worker-minted id, so a retried insert
// (LC0: aborted writes commit) is INSERT IGNORE and lands once.

import { newId, EMAIL_IN_TEXT } from "./crypto.ts";
import type { Sql } from "./sql.ts";

export type AuditType =
  | "waitlist.joined" | "waitlist.confirmed" | "waitlist.rejected" | "waitlist.removed" | "waitlist.deleted"
  | "waitlist.confirm_sent" | "waitlist.already_listed_sent" | "waitlist.removal_sent" | "waitlist.reminder_sent" | "waitlist.purged"
  | "invite.created" | "invite.sent" | "invite.resent" | "invite.revoked" | "invite.expired" | "invite.failed"
  | "invite.redeemed" | "invite.redeem_refused"
  | "account.created" | "account.suspended" | "account.deletion_scheduled" | "account.deleted"
  | "admin.mfa_enrolled" | "admin.step_up" | "admin.denied" | "admin.waitlist_read"
  | "email.bounced";

export interface AuditEvent {
  type: AuditType;
  actor: string;
  subject: string;
  detail?: Record<string, unknown>;
}

export interface CloudEvent {
  specversion: "1.0";
  id: string;
  source: "loams-cloud";
  type: string;
  subject: string;
  time: string;
  data: { actor: string } & Record<string, unknown>;
}

export class Audit {
  private readonly sql: Sql;
  private readonly now: () => number;
  constructor(sql: Sql, now: () => number) {
    this.sql = sql;
    this.now = now;
  }

  async write(e: AuditEvent, id = newId("evt")): Promise<string> {
    const detail = e.detail ? JSON.stringify(e.detail) : null;
    // The table's own guard against a caller slipping an address in (D1748).
    if (EMAIL_IN_TEXT.test(e.subject) || (detail && EMAIL_IN_TEXT.test(detail)) || EMAIL_IN_TEXT.test(e.actor)) {
      throw new Error(`audit ${e.type}: refusing a plaintext address`);
    }
    await this.sql.execute(
      "INSERT IGNORE INTO audit (id, time_ms, type, actor, subject, detail) VALUES (?, ?, ?, ?, ?, ?)",
      [id, this.now(), `dev.loams.cloud.${e.type}`, e.actor, e.subject, detail],
    );
    return id;
  }

  /** Newest first; `cursor` is the last item's `time_ms:id`. */
  async list(q: { subject?: string; type?: string; since?: number; cursor?: string; limit: number }): Promise<{ items: CloudEvent[]; next_cursor: string | null }> {
    const where: string[] = [];
    const args: unknown[] = [];
    if (q.subject) { where.push("subject = ?"); args.push(q.subject); }
    if (q.type) { where.push("type = ?"); args.push(q.type.startsWith("dev.loams.cloud.") ? q.type : `dev.loams.cloud.${q.type}`); }
    if (q.since !== undefined) { where.push("time_ms >= ?"); args.push(q.since); }
    if (q.cursor) {
      const [t, id] = decodeCursor(q.cursor);
      where.push("(time_ms < ? OR (time_ms = ? AND id < ?))");
      args.push(t, t, id);
    }
    const r = await this.sql.execute(
      `SELECT id, time_ms, type, actor, subject, detail FROM audit ${where.length ? "WHERE " + where.join(" AND ") : ""} ORDER BY time_ms DESC, id DESC LIMIT ${q.limit + 1}`,
      args,
    );
    const rows = r.rows.slice(0, q.limit);
    const items = rows.map((row): CloudEvent => {
      const detail = row.detail == null ? {} : typeof row.detail === "string" ? JSON.parse(row.detail) : (row.detail as Record<string, unknown>);
      return {
        specversion: "1.0",
        id: String(row.id),
        source: "loams-cloud",
        type: String(row.type),
        subject: String(row.subject),
        time: new Date(Number(row.time_ms)).toISOString(),
        data: { actor: String(row.actor), ...detail },
      };
    });
    const last = rows.at(-1);
    return { items, next_cursor: r.rows.length > q.limit && last ? encodeCursor(Number(last.time_ms), String(last.id)) : null };
  }
}

export function encodeCursor(t: number, id: string): string {
  return btoa(`${t}:${id}`).replace(/=+$/, "");
}

export function decodeCursor(c: string): [number, string] {
  let s: string;
  try {
    s = atob(c);
  } catch {
    throw new Error("bad cursor");
  }
  const i = s.indexOf(":");
  const t = Number(s.slice(0, i));
  if (i < 0 || !Number.isFinite(t)) throw new Error("bad cursor");
  return [t, s.slice(i + 1)];
}
