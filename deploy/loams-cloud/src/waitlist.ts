// The waitlist with double opt-in (design 78 §25.1, §25.3, §25.6, §25.7;
// D1737, D1739, D1740, D1744, D1745, D1753; plan LC2a Task 3).
//
// Everything that depends on an address's state runs after the response (the
// HTTP layer hands it to waitUntil), so the requester learns nothing from the
// status, the body or the timing. Only the inbox owner learns anything.
//
// Every write is one retry-safe statement (src/sql.ts): inserts carry minted ids
// or natural keys with INSERT IGNORE, and updates are guarded by the state they
// move from.

import type { Audit } from "./audit.ts";
import { decrypt, domainOf, emailHmac, encrypt, newId, newToken, sha256Hex, type Keys } from "./crypto.ts";
import { isDisposable, type DomainLists, type Dns } from "./domains.ts";
import type { Clock, Logger, Mailer, Template } from "./ports.ts";
import type { Sql } from "./sql.ts";

export const CONFIRM_TTL_MS = 48 * 3600_000; // Q1281
export const REMOVE_TTL_MS = 90 * 24 * 3600_000; // Q1281
export const DAY_MS = 24 * 3600_000;
/** Confirmation (and removal) emails per address per 24 hours (§25.7). */
export const PER_ADDRESS_SENDS = 3;
export const IP_PREFIX_TTL_MS = 7 * DAY_MS; // §25.6

export const SOURCES = ["search", "github", "social", "friend", "event", "other"] as const;
export type Source = (typeof SOURCES)[number];

export type Status = "pending" | "confirmed" | "invited";

export interface WaitlistRow {
  id: string;
  email_hmac: string;
  email_enc: string;
  status: Status;
  build: string | null;
  source: string | null;
  source_other: string | null;
  created_ms: number;
  confirmed_ms: number | null;
}

export interface JoinInput {
  email: string; // normalised
  build?: string;
  source?: Source;
  sourceOther?: string;
  ipPrefixHmac?: string;
}

export interface WaitlistDeps {
  sql: Sql;
  keys: Keys;
  lists: DomainLists;
  dns: Dns;
  mailer: Mailer;
  audit: Audit;
  clock: Clock;
  logger: Logger;
  /** Where the email links point: https://loams.dev */
  siteUrl: string;
  /** Revokes an entry's open invitations before it is removed (LC2a Task 5 wires WorkOS). */
  revokeInvites?: (waitlistId: string, actor: string) => Promise<void>;
}

export type RemoveReason = "removed" | "deleted" | "complaint" | "hard_bounce";

function rowOf(r: Record<string, unknown>): WaitlistRow {
  return {
    id: String(r.id),
    email_hmac: String(r.email_hmac),
    email_enc: String(r.email_enc),
    status: String(r.status) as Status,
    build: r.build == null ? null : String(r.build),
    source: r.source == null ? null : String(r.source),
    source_other: r.source_other == null ? null : String(r.source_other),
    created_ms: Number(r.created_ms),
    confirmed_ms: r.confirmed_ms == null ? null : Number(r.confirmed_ms),
  };
}

const COLS = "id, email_hmac, email_enc, status, build, source, source_other, created_ms, confirmed_ms";

export class Waitlist {
  readonly d: WaitlistDeps;
  constructor(deps: WaitlistDeps) {
    this.d = deps;
  }

  async byHmac(hmac: string): Promise<WaitlistRow | null> {
    const r = await this.d.sql.execute(`SELECT ${COLS} FROM waitlist WHERE email_hmac = ?`, [hmac]);
    return r.rows[0] ? rowOf(r.rows[0]) : null;
  }

  async byId(id: string): Promise<WaitlistRow | null> {
    const r = await this.d.sql.execute(`SELECT ${COLS} FROM waitlist WHERE id = ?`, [id]);
    return r.rows[0] ? rowOf(r.rows[0]) : null;
  }

  async isSuppressed(hmac: string): Promise<boolean> {
    const r = await this.d.sql.execute("SELECT 1 AS x FROM waitlist_suppressed WHERE email_hmac = ?", [hmac]);
    return r.rows.length > 0;
  }

  /** POST /waitlist, after the response. */
  async join(input: JoinInput): Promise<void> {
    const { sql, audit, clock } = this.d;
    const now = clock.now();
    const hmac = await emailHmac(this.d.keys, input.email);
    if (await this.isSuppressed(hmac)) {
      await audit.write({ type: "waitlist.rejected", actor: "public", subject: hmac, detail: { reason: "suppressed" } });
      return;
    }
    const domain = domainOf(input.email);
    if (isDisposable(this.d.lists, domain)) {
      await audit.write({ type: "waitlist.rejected", actor: "public", subject: hmac, detail: { reason: "disposable" } });
      return;
    }
    if (!(await this.d.dns.hasMx(domain))) {
      await audit.write({ type: "waitlist.rejected", actor: "public", subject: hmac, detail: { reason: "no_mx" } });
      return;
    }

    let row = await this.byHmac(hmac);
    if (!row) {
      const id = newId("wl");
      const enc = await encrypt(this.d.keys, input.email);
      const ins = await sql.execute(
        `INSERT IGNORE INTO waitlist (id, email_hmac, email_enc, status, build, source, source_other, created_ms, updated_ms, ip_prefix_hmac, ip_prefix_expires_ms)
         VALUES (?, ?, ?, 'pending', ?, ?, ?, ?, ?, ?, ?)`,
        [id, hmac, enc, input.build ?? null, input.source ?? null, input.sourceOther ?? null, now, now,
          input.ipPrefixHmac ?? null, input.ipPrefixHmac ? now + IP_PREFIX_TTL_MS : null],
      );
      row = await this.byHmac(hmac);
      if (!row) return; // removed in between
      if (ins.rowsAffected === 1) await audit.write({ type: "waitlist.joined", actor: "public", subject: row.id, detail: { source: input.source ?? null } });
    }

    if (row.status === "pending") {
      await this.sendWithTokens(row, input.email, "confirm");
      return;
    }
    // Confirmed or invited: at most one "you're already on the list" a day (Q1287).
    const upd = await sql.execute(
      "UPDATE waitlist SET already_listed_ms = ?, updated_ms = ? WHERE id = ? AND (already_listed_ms IS NULL OR already_listed_ms <= ?)",
      [now, now, row.id, now - DAY_MS],
    );
    if (upd.rowsAffected === 1) await this.sendWithTokens(row, input.email, "already_listed");
  }

  /** Sends one waitlist email with a fresh removal link (and a confirm link for `confirm`). */
  private async sendWithTokens(row: WaitlistRow, email: string, template: Template): Promise<void> {
    const { sql, clock } = this.d;
    const now = clock.now();
    const withConfirm = template === "confirm" || template === "still_interested";
    if (template !== "already_listed") {
      // The per-address limit, counted from the tokens themselves.
      const kind = withConfirm ? "confirm" : "remove";
      const c = await sql.execute(
        "SELECT COUNT(*) AS n FROM waitlist_tokens WHERE waitlist_id = ? AND kind = ? AND created_ms > ?",
        [row.id, kind, now - DAY_MS],
      );
      if (Number(c.rows[0]?.n ?? 0) >= PER_ADDRESS_SENDS) {
        this.d.logger.log("waitlist.per_address_limit", { waitlist_id: row.id, template });
        return;
      }
    }
    let confirmUrl: string | undefined;
    if (withConfirm) {
      // A second confirm email invalidates the first (D1740).
      await sql.execute(
        "UPDATE waitlist_tokens SET expires_ms = ? WHERE waitlist_id = ? AND kind = 'confirm' AND used_ms IS NULL AND expires_ms > ?",
        [now, row.id, now],
      );
      const t = newToken("wlc");
      await this.insertToken(t, row.id, "confirm", now + CONFIRM_TTL_MS);
      confirmUrl = `${this.d.siteUrl}/cloud/confirm#${t}`;
    }
    const r = newToken("wlr");
    await this.insertToken(r, row.id, "remove", now + REMOVE_TTL_MS);
    const removeUrl = `${this.d.siteUrl}/cloud/leave#${r}`;
    try {
      await this.d.mailer.send({ template, to: email, confirmUrl, removeUrl });
      const type = ({ confirm: "waitlist.confirm_sent", already_listed: "waitlist.already_listed_sent", removal: "waitlist.removal_sent", still_interested: "waitlist.reminder_sent" } as const)[template];
      await this.d.audit.write({ type, actor: "system", subject: row.id });
    } catch (e) {
      this.d.logger.log("waitlist.send_failed", { waitlist_id: row.id, template, error: String(e) });
    }
  }

  private async insertToken(token: string, waitlistId: string, kind: "confirm" | "remove", expiresMs: number): Promise<void> {
    await this.d.sql.execute(
      "INSERT IGNORE INTO waitlist_tokens (token_hash, waitlist_id, kind, created_ms, expires_ms) VALUES (?, ?, ?, ?, ?)",
      [await sha256Hex(token), waitlistId, kind, this.d.clock.now(), expiresMs],
    );
  }

  /** Spends a token once. Returns its waitlist id, or null when it is unusable (expired, used or unknown). */
  private async spend(token: string, kind: "confirm" | "remove"): Promise<string | null> {
    const prefix = kind === "confirm" ? "wlc_" : "wlr_";
    if (!token.startsWith(prefix) || !/^wl[cr]_[A-Za-z0-9_-]{43}$/.test(token)) return null;
    const now = this.d.clock.now();
    const h = await sha256Hex(token);
    const u = await this.d.sql.execute(
      "UPDATE waitlist_tokens SET used_ms = ? WHERE token_hash = ? AND kind = ? AND used_ms IS NULL AND expires_ms > ?",
      [now, h, kind, now],
    );
    if (u.rowsAffected !== 1) return null;
    const r = await this.d.sql.execute("SELECT waitlist_id FROM waitlist_tokens WHERE token_hash = ?", [h]);
    return r.rows[0] ? String(r.rows[0].waitlist_id) : null;
  }

  /** POST /waitlist/confirm. True when confirmed; false means 410 TOKEN_UNUSABLE. */
  async confirm(token: string): Promise<boolean> {
    const id = await this.spend(token, "confirm");
    if (!id) return false;
    const now = this.d.clock.now();
    const u = await this.d.sql.execute(
      "UPDATE waitlist SET status = 'confirmed', confirmed_ms = ?, updated_ms = ? WHERE id = ? AND status = 'pending'",
      [now, now, id],
    );
    if (u.rowsAffected === 1) {
      await this.d.audit.write({ type: "waitlist.confirmed", actor: "public", subject: id });
      return true;
    }
    // A "still interested?" link: the entry stays, for another 12 months (§25.6).
    const renew = await this.d.sql.execute(
      "UPDATE waitlist SET confirmed_ms = ?, reminder_sent_ms = NULL, updated_ms = ? WHERE id = ? AND status = 'confirmed' AND reminder_sent_ms IS NOT NULL",
      [now, now, id],
    );
    if (renew.rowsAffected === 1) await this.d.audit.write({ type: "waitlist.confirmed", actor: "public", subject: id, detail: { renewed: true } });
    return (await this.byId(id)) !== null;
  }

  /** The 12-month "still interested?" email, with a fresh confirm link (§25.6). */
  async remind(row: WaitlistRow): Promise<void> {
    const now = this.d.clock.now();
    const u = await this.d.sql.execute(
      "UPDATE waitlist SET reminder_sent_ms = ?, updated_ms = ? WHERE id = ? AND status = 'confirmed' AND reminder_sent_ms IS NULL",
      [now, now, row.id],
    );
    if (u.rowsAffected === 1) await this.sendWithTokens(row, await this.decryptEmail(row), "still_interested");
  }

  /**
   * Deletes an entry by retention, without suppressing its address: the person
   * may join again. Tokens first, then the row; both safe to repeat.
   */
  async purge(id: string, reason: string): Promise<void> {
    await this.d.sql.execute("DELETE FROM waitlist_tokens WHERE waitlist_id = ?", [id]);
    const d = await this.d.sql.execute("DELETE FROM waitlist WHERE id = ?", [id]);
    if (d.rowsAffected === 1) await this.d.audit.write({ type: "waitlist.purged", actor: "system", subject: id, detail: { reason } });
  }

  /** DELETE /waitlist with a removal token. False means 410 TOKEN_UNUSABLE. */
  async removeByToken(token: string): Promise<boolean> {
    const id = await this.spend(token, "remove");
    if (!id) return false;
    await this.remove(id, "removed", "public");
    return true;
  }

  /** DELETE /waitlist with an address, after the response: mail a removal link if the address is listed. */
  async requestRemoval(email: string): Promise<void> {
    const hmac = await emailHmac(this.d.keys, email);
    if (await this.isSuppressed(hmac)) return;
    const row = await this.byHmac(hmac);
    if (!row) return;
    await this.sendWithTokens(row, email, "removal");
  }

  /**
   * Removes an entry: suppress the HMAC first, then revoke open invitations, then
   * delete the tokens and the row. Each step is safe to repeat, so a retry after
   * a lost response finishes the job. Only the suppression HMAC remains (§25.3).
   */
  async remove(id: string, reason: RemoveReason, actor: string): Promise<boolean> {
    const { sql, clock } = this.d;
    const row = await this.byId(id);
    if (!row) return false;
    await sql.execute("INSERT IGNORE INTO waitlist_suppressed (email_hmac, reason, created_ms) VALUES (?, ?, ?)", [row.email_hmac, reason, clock.now()]);
    if (this.d.revokeInvites) await this.d.revokeInvites(id, actor);
    else await sql.execute("UPDATE invites SET state = 'revoked', updated_ms = ? WHERE waitlist_id = ? AND state = 'queued'", [clock.now(), id]);
    await sql.execute("DELETE FROM waitlist_tokens WHERE waitlist_id = ?", [id]);
    await sql.execute("DELETE FROM waitlist WHERE id = ?", [id]);
    await this.d.audit.write({ type: reason === "deleted" ? "waitlist.deleted" : "waitlist.removed", actor, subject: id, detail: { reason } });
    return true;
  }

  /** The address, for sending and the admin list only. */
  decryptEmail(row: Pick<WaitlistRow, "email_enc">): Promise<string> {
    return decrypt(this.d.keys, row.email_enc);
  }
}
