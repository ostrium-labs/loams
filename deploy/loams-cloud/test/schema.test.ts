// LC2a Task 1: the control-DB schema (design 78 §25.3, D1739).
import { test, describe, after } from "node:test";
import assert from "node:assert/strict";
import { migrate, splitStatements } from "../src/schema.ts";
import { freshDatabase, loadMigrations, skipNoTidb } from "./harness/tidb.ts";

const TABLES = [
  "schema_migrations", "waitlist", "waitlist_tokens", "waitlist_suppressed", "invites", "accounts",
  "workos_events_cursor", "admin_mfa_factors", "admin_step_ups", "idempotency_keys", "email_budget", "audit",
];

describe("schema text", () => {
  const migrations = loadMigrations();

  test("migrations are numbered from 1 without gaps", () => {
    assert.deepEqual(migrations.map((m) => m.version), migrations.map((_, i) => i + 1));
  });

  test("schema_stays_inside_starter_features", () => {
    // TiDB X Starter: no TTL attribute, no partitioning (plan LC2a Task 1). No
    // foreign keys or CHECK constraints either: Starter doesn't enforce them by default.
    for (const m of migrations) {
      const code = splitStatements(m.text).join("\n").toUpperCase();
      for (const banned of ["TTL", "PARTITION", "FOREIGN KEY", "REFERENCES", "CHECK (", "CHECK(", "AUTO_INCREMENT"]) {
        assert.ok(!code.includes(banned), `${m.name} uses ${banned}`);
      }
    }
  });

  test("every statement is idempotent", () => {
    for (const m of migrations) {
      for (const s of splitStatements(m.text)) {
        assert.match(s, /^(CREATE TABLE IF NOT EXISTS|INSERT IGNORE|CREATE INDEX IF NOT EXISTS|ALTER TABLE \S+ ADD COLUMN IF NOT EXISTS)/, s.slice(0, 60));
      }
    }
  });

  test("splitStatements drops comments and blank statements", () => {
    assert.deepEqual(splitStatements("-- a; b\nCREATE TABLE x (a INT); -- c;\n\n;SELECT 1"), ["CREATE TABLE x (a INT)", "SELECT 1"]);
  });

  test("every table is created", () => {
    const created = migrations.flatMap((m) => [...m.text.matchAll(/CREATE TABLE IF NOT EXISTS (\w+)/g)].map((x) => x[1]));
    assert.deepEqual([...created].sort(), [...TABLES].sort());
  });
});

describe("schema on TiDB", { skip: skipNoTidb }, () => {
  const { sql, dispose } = freshDatabase();
  after(dispose);
  const now = Date.now();

  test("schema_applies_cleanly", async () => {
    const migrations = loadMigrations();
    assert.deepEqual(await migrate(sql, migrations), migrations.map((m) => m.version));
    // A second run applies nothing, and re-running every statement is harmless.
    assert.deepEqual(await migrate(sql, migrations), []);
    for (const m of migrations) for (const s of splitStatements(m.text)) await sql.execute(s);
    const r = await sql.execute("SELECT table_name AS t FROM information_schema.tables WHERE table_schema = DATABASE()");
    assert.deepEqual(r.rows.map((x) => String(x.t)).sort(), [...TABLES].sort());
    const v = await sql.execute("SELECT version FROM schema_migrations ORDER BY version");
    assert.deepEqual(v.rows.map((x) => Number(x.version)), migrations.map((m) => m.version));
  });

  test("email_hmac_is_unique", async () => {
    const ins = "INSERT INTO waitlist (id, email_hmac, email_enc, status, created_ms, updated_ms) VALUES (?, ?, ?, 'pending', ?, ?)";
    const h = "a".repeat(64);
    await sql.execute(ins, ["wl_1", h, "enc1", now, now]);
    await assert.rejects(sql.execute(ins, ["wl_2", h, "enc2", now, now]), /Duplicate entry/);
    // The retry-safe form: the same insert with INSERT IGNORE affects nothing.
    const r = await sql.execute(ins.replace("INSERT INTO", "INSERT IGNORE INTO"), ["wl_3", h, "enc3", now, now]);
    assert.equal(r.rowsAffected, 0);
  });

  test("workos_user_id_is_unique", async () => {
    const ins = "INSERT INTO accounts (id, workos_user_id, contact_email_enc, contact_email_hmac, invite_id, status, created_ms, updated_ms) VALUES (?, ?, 'e', ?, ?, 'active', ?, ?)";
    await sql.execute(ins, ["acct_1", "user_01", "b".repeat(64), "inv_1", now, now]);
    await assert.rejects(sql.execute(ins, ["acct_2", "user_01", "c".repeat(64), "inv_2", now, now]), /Duplicate entry/);
    // One account per invitation, too.
    await assert.rejects(sql.execute(ins, ["acct_3", "user_02", "d".repeat(64), "inv_1", now, now]), /Duplicate entry/);
  });

  test("workos_invitation_id_is_unique_and_batch_entries_are_idempotent", async () => {
    const ins = "INSERT IGNORE INTO invites (id, waitlist_id, email_hmac, workos_invitation_id, state, expires_in_days, created_by, batch_key, created_ms, updated_ms) VALUES (?, ?, ?, ?, 'queued', 14, 'user_admin', ?, ?, ?)";
    assert.equal((await sql.execute(ins, ["inv_a", "wl_1", "a".repeat(64), null, "k1", now, now])).rowsAffected, 1);
    // A retried batch with the same Idempotency-Key and entry inserts nothing new.
    assert.equal((await sql.execute(ins, ["inv_b", "wl_1", "a".repeat(64), null, "k1", now, now])).rowsAffected, 0);
    // NULL workos ids don't collide; real ones do.
    assert.equal((await sql.execute(ins, ["inv_c", "wl_2", "e".repeat(64), null, "k1", now, now])).rowsAffected, 1);
    await sql.execute("UPDATE invites SET workos_invitation_id = 'invitation_1' WHERE id = 'inv_a'");
    await assert.rejects(sql.execute("UPDATE invites SET workos_invitation_id = 'invitation_1' WHERE id = 'inv_c'"), /Duplicate entry/);
  });

  test("one_open_invite_per_entry", async () => {
    const ins = "INSERT IGNORE INTO invites (id, waitlist_id, email_hmac, state, open_key, expires_in_days, created_by, batch_key, created_ms, updated_ms) VALUES (?, ?, ?, ?, ?, 14, 'user_admin', ?, ?, ?)";
    assert.equal((await sql.execute(ins, ["inv_o1", "wl_9", "f".repeat(64), "queued", "wl_9", "k2", now, now])).rowsAffected, 1);
    // A second open invite for the same entry, from another batch, is refused.
    assert.equal((await sql.execute(ins, ["inv_o2", "wl_9", "f".repeat(64), "queued", "wl_9", "k3", now, now])).rowsAffected, 0);
    // Once the first is closed, a new one may open; closed ones don't collide.
    await sql.execute("UPDATE invites SET state = 'revoked', open_key = NULL WHERE id = 'inv_o1'");
    assert.equal((await sql.execute(ins, ["inv_o3", "wl_9", "f".repeat(64), "queued", "wl_9", "k4", now, now])).rowsAffected, 1);
    assert.equal((await sql.execute(ins, ["inv_o4", "wl_9", "f".repeat(64), "revoked", null, "k5", now, now])).rowsAffected, 1);
  });

  test("no_plaintext_email_column_outside_enc", async () => {
    const r = await sql.execute(
      "SELECT table_name AS t, column_name AS c, data_type AS d FROM information_schema.columns WHERE table_schema = DATABASE()",
    );
    const cols = r.rows.map((x) => ({ t: String(x.t), c: String(x.c).toLowerCase(), d: String(x.d).toLowerCase() }));
    // Any column that could hold an address: its name says email, or contact.
    const emailish = cols.filter((x) => /email|mail|contact|address/.test(x.c));
    assert.ok(emailish.length >= 4, "the address columns are present");
    for (const x of emailish) {
      assert.match(x.c, /_(enc|hmac)$/, `${x.t}.${x.c} must be an _enc or _hmac column`);
      if (x.c.endsWith("_hmac")) assert.equal(x.d, "char", `${x.t}.${x.c} holds a hex HMAC`);
    }
    // And nothing else holds free text an address could hide in, except the
    // optional answers, which the API refuses when they contain an address.
    const text = cols.filter((x) => ["varchar", "text", "mediumtext", "json"].includes(x.d) && !/_(enc|hmac|id|ms)$|^id$/.test(x.c));
    const allowed = new Set([
      "schema_migrations.name", "waitlist.status", "waitlist.build", "waitlist.source", "waitlist.source_other",
      "waitlist_tokens.kind", "waitlist_suppressed.reason", "invites.state", "invites.open_key", "invites.created_by", "invites.batch_key", "email_budget.day", "email_budget.kind",
      "accounts.status", "admin_step_ups.sid", "workos_events_cursor.feed", "workos_events_cursor.after", "idempotency_keys.scope",
      "idempotency_keys.idem_key", "idempotency_keys.response_json", "audit.type", "audit.actor", "audit.subject", "audit.detail",
    ]);
    for (const x of text) assert.ok(allowed.has(`${x.t}.${x.c}`), `unexpected free-text column ${x.t}.${x.c}`);
  });
});
