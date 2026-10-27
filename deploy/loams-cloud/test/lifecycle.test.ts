// LC2a Task 9: lifecycle, deletion and retention (design 78 §25.6, §25.9; D1731).
import { test, describe, before, after } from "node:test";
import assert from "node:assert/strict";
import { Admin, parseAdmins } from "../src/admin.ts";
import { EmailBudget } from "../src/budget.ts";
import { emailHmac, encrypt, newId } from "../src/crypto.ts";
import { Invites } from "../src/invites.ts";
import { DAY, Lifecycle, RETENTION } from "../src/lifecycle.ts";
import type { WorkOsUsers } from "../src/workos.ts";
import { WorkOsError } from "../src/workos.ts";
import { skipNoTidb } from "./harness/tidb.ts";
import { world, fragment, type World } from "./harness/fakes.ts";
import { FakeInvitations, FakeMfa, OSTRIUM_ORG, TokenIssuer } from "./harness/workos.ts";

class FakeUsers implements WorkOsUsers {
  readonly users = new Set<string>();
  readonly deleted: string[] = [];
  failNext = false;
  async deleteUser(id: string): Promise<void> {
    if (this.failNext) {
      this.failNext = false;
      throw new WorkOsError(503, "WorkOS DELETE /user_management/users/{id}: 503");
    }
    this.deleted.push(id);
    this.users.delete(id);
  }
}

interface Rig {
  w: World;
  iss: TokenIssuer;
  users: FakeUsers;
  wos: FakeInvitations;
  invites: Invites;
  life: Lifecycle;
  seq: number;
}

async function rig(): Promise<Rig> {
  const w = await world();
  const iss = await TokenIssuer.create();
  const users = new FakeUsers();
  const wos = new FakeInvitations(() => w.clock.now());
  const verifier = iss.verifier(() => w.clock.now());
  const invites = new Invites({ sql: w.sql, workos: wos, waitlist: w.waitlist, budget: new EmailBudget(w.sql), audit: w.audit, clock: w.clock, logger: w.log.logger });
  const life = new Lifecycle({ sql: w.sql, waitlist: w.waitlist, workos: users, audit: w.audit, clock: w.clock, logger: w.log.logger, verifier });
  const admin = new Admin({
    sql: w.sql, verifier, mfa: new FakeMfa(), waitlist: w.waitlist, audit: w.audit, limiter: { hit: async () => ({ ok: true, retryAfterMs: 0 }) }, clock: w.clock, logger: w.log.logger,
    config: { ostriumOrgId: OSTRIUM_ORG, admins: parseAdmins(`[[admin]]\nworkos_user_id = "user_01ADMIN000000000000000"\nname = "Owner"\n`), totpIssuer: "Loams Cloud" },
  });
  w.deps.routes = [life.route(), admin.routes([invites.routes()])];
  return { w, iss, users, wos, invites, life, seq: 0 };
}

async function entry(r: Rig, confirm = true) {
  const email = `life${++r.seq}@example.com`;
  await r.w.waitlist.join({ email });
  if (confirm) assert.ok(await r.w.waitlist.confirm(fragment(r.w.mailer.to(email).at(-1)!.confirmUrl)));
  return { id: (await r.w.waitlist.byHmac(await emailHmac(r.w.keys, email)))!.id, email };
}

/** An account as Task 7's callback will create it (seeded here, since the callback waits on WorkOS). */
async function account(r: Rig, email: string, signin?: string) {
  const id = newId("acct");
  const user = `user_01${String(++r.seq).padStart(20, "0")}`;
  const now = r.w.clock.now();
  await r.w.sql.execute(
    "INSERT INTO accounts (id, workos_user_id, contact_email_enc, contact_email_hmac, signin_email_hmac, invite_id, status, created_ms, updated_ms) VALUES (?, ?, ?, ?, ?, ?, 'active', ?, ?)",
    [id, user, await encrypt(r.w.keys, email), await emailHmac(r.w.keys, email), signin ? await emailHmac(r.w.keys, signin) : null, newId("inv"), now, now],
  );
  r.users.users.add(user);
  return { id, user };
}

describe("lifecycle", { skip: skipNoTidb }, () => {
  let r: Rig;
  before(async () => { r = await rig(); });
  after(() => r.w.dispose());

  test("deletion_leaves_only_hmac_and_deletes_workos_user", async () => {
    const a = await account(r, "leaving@example.com", "leaving.gh@example.org");
    const token = await r.iss.sign({ iss: "https://api.workos.com/user_management/client_TEST0000000000", sub: a.user, sid: "session_01ME0000000000000000", iat: r.w.clock.now() / 1000, exp: r.w.clock.now() / 1000 + 300 });
    // WorkOS is down for the first attempt: deletion stays scheduled and the next run finishes it.
    r.users.failNext = true;
    const res = await r.w.call("DELETE", "/me", undefined, { authorization: `Bearer ${token}` });
    assert.equal(res.status, 202);
    assert.equal(res.body.status, "deletion_scheduled");
    assert.equal(Date.parse(res.body.by), r.w.clock.now() + RETENTION.deletion);
    assert.equal((await r.w.sql.execute("SELECT status FROM accounts WHERE id = ?", [a.id])).rows[0].status, "deleting");
    assert.equal(await r.life.runDeletions(), 1);
    assert.deepEqual(r.users.deleted, [a.user], "the WorkOS user is gone");
    assert.equal((await r.w.sql.execute("SELECT COUNT(*) AS n FROM accounts WHERE id = ?", [a.id])).rows[0].n, "0");
    // Only the HMACs remain, as suppressions; nothing anywhere decrypts to the address.
    const sup = await r.w.sql.execute("SELECT email_hmac FROM waitlist_suppressed WHERE reason = 'deleted'");
    const hmacs = sup.rows.map((x) => String(x.email_hmac));
    assert.ok(hmacs.includes(await emailHmac(r.w.keys, "leaving@example.com")));
    assert.ok(hmacs.includes(await emailHmac(r.w.keys, "leaving.gh@example.org")));
    for (const t of ["waitlist", "invites", "accounts", "audit", "waitlist_tokens"]) {
      const dump = JSON.stringify((await r.w.sql.execute(`SELECT * FROM ${t}`)).rows);
      assert.ok(!dump.includes("leaving"), `${t} holds no address`);
    }
    // A repeated request finds nothing, and the old token no longer has an account.
    assert.equal((await r.w.call("DELETE", "/me", undefined, { authorization: `Bearer ${token}` })).status, 401);
    assert.equal((await r.w.call("DELETE", "/me")).status, 401);
    // The address can't rejoin the waitlist either.
    await r.w.waitlist.join({ email: "leaving@example.com" });
    assert.equal(r.w.mailer.to("leaving@example.com").length, 0);
    const types = (await r.w.sql.execute("SELECT type FROM audit WHERE subject = ? ORDER BY time_ms", [a.id])).rows.map((x) => String(x.type));
    assert.deepEqual(types.sort(), ["dev.loams.cloud.account.deleted", "dev.loams.cloud.account.deletion_scheduled"]);
  });

  test("an agent or M2M token can't delete an account", async () => {
    const a = await account(r, "kept@example.com");
    const now = r.w.clock.now() / 1000;
    for (const claims of [{ sub: a.user }, { sub: a.user, sid: "session_01X0000000000000000", act: { sub: "agent_1" } }]) {
      const t = await r.iss.sign({ iss: "https://api.workos.com/user_management/client_TEST0000000000", iat: now, exp: now + 300, ...claims });
      assert.equal((await r.w.call("DELETE", "/me", undefined, { authorization: `Bearer ${t}` })).status, 401);
    }
    assert.equal((await r.w.sql.execute("SELECT status FROM accounts WHERE id = ?", [a.id])).rows[0].status, "active");
  });

  test("retention_jobs_delete_on_schedule", async () => {
    const t0 = r.w.clock.now();
    const pending = await entry(r, false);
    const confirmed = await entry(r);
    const remindedButAnswers = await entry(r);
    await r.w.sql.execute("INSERT INTO waitlist_suppressed (email_hmac, reason, created_ms) VALUES (?, 'removed', ?)", ["e".repeat(64), t0]);
    await r.w.audit.write({ type: "waitlist.rejected", actor: "public", subject: "f".repeat(64) });

    // Day 6: nothing is due.
    r.w.clock.advance(6 * DAY);
    let rep = await r.life.runRetention();
    assert.equal(rep.unconfirmed, 0);
    assert.ok(await r.w.waitlist.byId(pending.id));
    // Day 8: the unconfirmed entry is gone (not suppressed: it may join again), its IP prefix too.
    r.w.clock.advance(2 * DAY);
    rep = await r.life.runRetention();
    assert.ok(rep.unconfirmed >= 1);
    assert.equal(await r.w.waitlist.byId(pending.id), null);
    assert.equal(await r.w.waitlist.isSuppressed(await emailHmac(r.w.keys, pending.email)), false);
    const stale = await r.w.sql.execute("SELECT COUNT(*) AS n FROM waitlist_tokens WHERE expires_ms <= ? OR used_ms <= ?", [r.w.clock.now() - DAY, r.w.clock.now() - DAY]);
    assert.equal(stale.rows[0].n, "0", "spent and expired tokens go");
    // 12 months after confirming: a "still interested?" email with a confirm link, once.
    r.w.clock.advance(RETENTION.confirmedUntilReminder);
    rep = await r.life.runRetention();
    assert.ok(rep.reminded >= 2);
    const reminder = r.w.mailer.to(remindedButAnswers.email).at(-1)!;
    assert.equal(reminder.template, "still_interested");
    assert.match(reminder.confirmUrl!, /#wlc_/);
    assert.equal((await r.life.runRetention()).reminded, 0, "not twice");
    // One person answers; the other doesn't.
    assert.ok(await r.w.waitlist.confirm(fragment(reminder.confirmUrl)));
    r.w.clock.advance(RETENTION.afterReminder);
    rep = await r.life.runRetention();
    assert.ok(rep.unreconfirmed >= 1);
    assert.equal(await r.w.waitlist.byId(confirmed.id), null);
    assert.ok(await r.w.waitlist.byId(remindedButAnswers.id), "renewed for another 12 months");
    // 24 months: the suppression HMAC goes; 12 months: the audit event goes.
    assert.equal((await r.w.sql.execute("SELECT COUNT(*) AS n FROM audit WHERE subject = ?", ["f".repeat(64)])).rows[0].n, "0");
    r.w.clock.advance(RETENTION.suppression);
    rep = await r.life.runRetention();
    assert.ok(rep.suppressions >= 1);
    assert.equal(await r.w.waitlist.isSuppressed("e".repeat(64)), false);
  });

  test("expired_invitations_are_mirrored", async () => {
    const e = await entry(r);
    const admin = "user_01ADMIN000000000000000";
    const q = await r.invites.enqueue(admin, "sweep-expiry", { waitlist_ids: [e.id], expires_in_days: 2 });
    await r.invites.drain();
    const id = q.invites[0].id;
    assert.equal((await r.invites.byId(id))!.state, "pending");
    // Before expiry the sweep reads nothing from WorkOS.
    const calls = r.wos.calls.length;
    assert.equal(await r.invites.sweep(), 0);
    assert.equal(r.wos.calls.length, calls);
    // After: mirrored as expired, the entry is confirmed again, and the audit says so.
    r.w.clock.advance(2 * DAY + 1);
    assert.equal(await r.invites.sweep(), 1);
    assert.equal((await r.invites.byId(id))!.state, "expired");
    assert.equal((await r.w.waitlist.byId(e.id))!.status, "confirmed");
    assert.equal((await r.w.sql.execute("SELECT COUNT(*) AS n FROM audit WHERE subject = ? AND type = 'dev.loams.cloud.invite.expired'", [id])).rows[0].n, "1");
    // An acceptance WorkOS saw first is mirrored too, not expired.
    const e2 = await entry(r);
    const q2 = await r.invites.enqueue(admin, "sweep-accepted", { waitlist_ids: [e2.id], expires_in_days: 1 });
    await r.invites.drain();
    const inv2 = (await r.invites.byId(q2.invites[0].id))!;
    r.wos.accept(inv2.workos_invitation_id!, "user_01ACCEPTED00000000000");
    r.w.clock.advance(DAY + 1);
    await r.invites.sweep();
    assert.equal((await r.invites.byId(inv2.id))!.state, "accepted");
    // 30 days after the invite expired with no new invite, the entry goes.
    r.w.clock.advance(RETENTION.afterInviteExpired);
    const rep = await r.life.runRetention();
    assert.ok(rep.inviteExpired >= 1);
    assert.equal(await r.w.waitlist.byId(e.id), null);
  });
});
