// LC2a Task 5: invitations through WorkOS (design 78 §25.1, §25.9, §25.10).
import { test, describe, before, after } from "node:test";
import assert from "node:assert/strict";
import { Admin, parseAdmins } from "../src/admin.ts";
import { EmailBudget } from "../src/budget.ts";
import { emailHmac } from "../src/crypto.ts";
import { Invites } from "../src/invites.ts";
import { HttpWorkOs } from "../src/workos.ts";
import { skipNoTidb } from "./harness/tidb.ts";
import { world, fragment, type World } from "./harness/fakes.ts";
import { FakeInvitations, FakeMfa, OSTRIUM_ORG, TokenIssuer } from "./harness/workos.ts";

const ADMIN = "user_01ADMIN000000000000000";

interface Rig {
  w: World;
  wos: FakeInvitations;
  invites: Invites;
  budget: EmailBudget;
  token: () => Promise<string>;
  seq: number;
}

async function rig(limits: Partial<{ total: number; invite: number }> = {}): Promise<Rig> {
  const w = await world();
  const iss = await TokenIssuer.create();
  const wos = new FakeInvitations(() => w.clock.now());
  const budget = new EmailBudget(w.sql, limits);
  const invites = new Invites({ sql: w.sql, workos: wos, waitlist: w.waitlist, budget, audit: w.audit, clock: w.clock, logger: w.log.logger });
  w.waitlist.d.revokeInvites = (id, actor) => invites.revokeAllFor(id, actor);
  const admin = new Admin({
    sql: w.sql, verifier: iss.verifier(() => w.clock.now()), mfa: new FakeMfa(), waitlist: w.waitlist, audit: w.audit, limiter: { hit: async () => ({ ok: true, retryAfterMs: 0 }) }, clock: w.clock, logger: w.log.logger,
    config: { ostriumOrgId: OSTRIUM_ORG, admins: parseAdmins(`[[admin]]\nworkos_user_id = "${ADMIN}"\nname = "Owner"\n`), totpIssuer: "Loams Cloud" },
  });
  w.deps.routes = [admin.routes([invites.routes()])];
  const token = () => iss.adminToken(w.clock.now());
  const t = await token();
  await w.call("POST", "/admin/mfa/enroll", undefined, { authorization: `Bearer ${t}` });
  const v = await w.call("POST", "/admin/mfa/verify", { code: "246810" }, { authorization: `Bearer ${t}` });
  assert.equal(v.status, 200);
  return { w, wos, invites, budget, token, seq: 0 };
}

/** A waitlist entry, confirmed unless `confirm` is false. */
async function entry(r: Rig, confirm = true): Promise<{ id: string; email: string }> {
  const email = `person${++r.seq}@example.com`;
  await r.w.waitlist.join({ email });
  if (confirm) assert.ok(await r.w.waitlist.confirm(fragment(r.w.mailer.to(email).at(-1)!.confirmUrl)));
  const row = await r.w.waitlist.byHmac(await emailHmac(r.w.keys, email));
  return { id: row!.id, email };
}

async function admin(r: Rig, method: string, path: string, body?: unknown, headers: Record<string, string> = {}) {
  const res = await r.w.call(method, path, body, { authorization: `Bearer ${await r.token()}`, ...headers });
  if (res.status !== 403 || res.body?.error?.code !== "STEP_UP_REQUIRED") return res;
  // The tests move the clock by days; step up again and retry.
  assert.equal((await r.w.call("POST", "/admin/mfa/verify", { code: "246810" }, { authorization: `Bearer ${await r.token()}` })).status, 200);
  return r.w.call(method, path, body, { authorization: `Bearer ${await r.token()}`, ...headers });
}

const invite = (r: Rig, ids: string[], key: string, extra: Record<string, unknown> = {}) =>
  admin(r, "POST", "/admin/invites", { waitlist_ids: ids, ...extra }, { "idempotency-key": key });

describe("invites", { skip: skipNoTidb }, () => {
  let r: Rig;
  before(async () => { r = await rig(); });
  after(() => r.w.dispose());

  test("batch_skips_unconfirmed_and_suppressed", async () => {
    const a = await entry(r);
    const b = await entry(r);
    const pending = await entry(r, false);
    const bounced = await entry(r);
    await r.w.sql.execute("INSERT INTO waitlist_suppressed (email_hmac, reason, created_ms) VALUES (?, 'hard_bounce', ?)", [await emailHmac(r.w.keys, bounced.email), r.w.clock.now()]);
    const first = await invite(r, [a.id], "batch-skip-1");
    assert.equal(first.status, 202);
    const res = await invite(r, [a.id, b.id, pending.id, bounced.id, "wl_AAAAAAAAAAAAAAAAAAAAAA"], "batch-skip-2");
    assert.equal(res.status, 202);
    assert.deepEqual(res.body.invites.map((x: { waitlist_id: string }) => x.waitlist_id), [b.id]);
    assert.ok(res.body.invites.every((x: { state: string; id: string }) => x.state === "queued" && /^inv_/.test(x.id)));
    assert.deepEqual(res.body.skipped, [
      { waitlist_id: a.id, reason: "already_invited" },
      { waitlist_id: pending.id, reason: "not_confirmed" },
      { waitlist_id: bounced.id, reason: "suppressed" },
      { waitlist_id: "wl_AAAAAAAAAAAAAAAAAAAAAA", reason: "not_found" },
    ]);
    assert.equal(r.wos.createdFor(pending.email), 0);
    assert.equal(r.wos.createdFor(bounced.email), 0);
    // Bad batches: empty, too many, not ids, unknown fields, a bad expiry, no key.
    const many = Array.from({ length: 51 }, (_, i) => `wl_${String(i).padStart(22, "A")}`);
    for (const body of [{ waitlist_ids: [] }, { waitlist_ids: many }, { waitlist_ids: ["someone@example.com"] }, { waitlist_ids: [a.id], role: "x" }, { waitlist_ids: [a.id], expires_in_days: 31 }]) {
      assert.equal((await admin(r, "POST", "/admin/invites", body, { "idempotency-key": `bad-${Math.random().toString(36).slice(2)}` })).status, 400);
    }
    assert.equal((await admin(r, "POST", "/admin/invites", { waitlist_ids: [a.id] })).status, 400);
  });

  test("invite_calls_workos_once_per_entry", async () => {
    const es = [await entry(r), await entry(r), await entry(r), await entry(r)];
    const ids = es.map((e) => e.id);
    // The first create's response is lost after WorkOS made it; the second fails outright.
    r.wos.loseNextCreateResponse = true;
    const res = await invite(r, ids, "once-per-entry", { expires_in_days: 7 });
    assert.equal(res.status, 202);
    assert.equal(res.body.invites.length, 4);
    // The same request again, with the same key: the same answer, and no new creates.
    const again = await invite(r, ids, "once-per-entry", { expires_in_days: 7 });
    assert.deepEqual(again.body.invites, res.body.invites);
    // A different batch under the same key is refused.
    const reused = await invite(r, ids.slice(0, 2), "once-per-entry");
    assert.equal(reused.status, 409);
    assert.equal(reused.body.error.code, "IDEMPOTENCY_KEY_REUSED");
    // The lost one is still queued; the next drain adopts WorkOS's pending invitation instead of creating another.
    r.wos.failNextCreate = 1;
    await Promise.all([r.invites.drain(), r.invites.drain()]);
    await r.invites.drain();
    for (const e of es) assert.equal(r.wos.createdFor(e.email), 1, `${e.email}: exactly one WorkOS invitation`);
    const list = await admin(r, "GET", "/admin/invites?state=pending&limit=200");
    const mine = list.body.items.filter((i: { waitlist_id: string }) => ids.includes(i.waitlist_id));
    assert.equal(mine.length, 4);
    for (const i of mine) {
      assert.match(i.workos_invitation_id, /^invitation_/);
      assert.equal(Date.parse(i.expires_at), r.wos.invitations.get(i.workos_invitation_id)!.expiresAt);
      assert.equal(i.resend_count, 0);
    }
    // The waitlist rows are now invited, and the audit has created and sent for each.
    for (const e of es) assert.equal((await r.w.waitlist.byId(e.id))?.status, "invited");
    const sent = await r.w.sql.execute("SELECT COUNT(*) AS n FROM audit WHERE type = 'dev.loams.cloud.invite.sent' AND subject IN (?, ?, ?, ?)", mine.map((i: { id: string }) => i.id));
    assert.equal(Number(sent.rows[0].n), 4);
    // And no invitation token or accept URL is stored anywhere.
    const cols = await r.w.sql.execute("SELECT column_name AS c FROM information_schema.columns WHERE table_schema = DATABASE() AND table_name = 'invites'");
    assert.ok(!cols.rows.some((c) => /token|url/i.test(String(c.c))));
  });

  test("a create that keeps failing ends as failed after five attempts, and reopens nothing", async () => {
    const e = await entry(r);
    r.wos.failNextCreate = 5;
    const res = await invite(r, [e.id], "keeps-failing");
    const id = res.body.invites[0].id;
    for (let i = 0; i < 6; i++) await r.invites.drain();
    const inv = await r.invites.byId(id);
    assert.equal(inv?.state, "failed");
    assert.equal(inv?.attempts, 5);
    assert.equal(r.wos.createdFor(e.email), 0);
    assert.match(r.w.log.text(), /invites\.create_failed/);
    assert.equal((await r.w.waitlist.byId(e.id))?.status, "confirmed", "the entry can be invited again");
  });

  test("resend_limit_enforced", async () => {
    const e = await entry(r);
    const id = (await invite(r, [e.id], "resend-limit")).body.invites[0].id;
    const before = (await r.invites.byId(id))!;
    for (let i = 1; i <= 3; i++) {
      const res = await admin(r, "POST", `/admin/invites/${id}/resend`, {});
      assert.equal(res.status, 200);
      assert.equal(res.body.resend_count, i);
      assert.equal(res.body.state, "pending");
      assert.equal(Date.parse(res.body.expires_at), before.expires_ms, "a resend never extends (R-LC2a-5.6)");
    }
    const fourth = await admin(r, "POST", `/admin/invites/${id}/resend`, {});
    assert.equal(fourth.status, 409);
    assert.equal(fourth.body.error.code, "RESEND_LIMIT");
    assert.equal(r.wos.invitations.get(before.workos_invitation_id!)!.resent, 3);
    // extend: revoke and send a new one for the full term from now; it counts as a resend.
    const e2 = await entry(r);
    const id2 = (await invite(r, [e2.id], "resend-extend")).body.invites[0].id;
    const old = (await r.invites.byId(id2))!;
    r.w.clock.advance(3 * 86_400_000);
    const ext = await admin(r, "POST", `/admin/invites/${id2}/resend`, { extend: true });
    assert.equal(ext.status, 200);
    const now = (await r.invites.byId(id2))!;
    assert.notEqual(now.workos_invitation_id, old.workos_invitation_id);
    assert.equal(r.wos.invitations.get(old.workos_invitation_id!)!.state, "revoked");
    assert.equal(now.expires_ms, r.w.clock.now() + 14 * 86_400_000);
    assert.equal(now.resend_count, 1);
    assert.equal(r.wos.createdFor(e2.email), 2);
    // Only pending invites resend.
    assert.equal((await admin(r, "POST", `/admin/invites/${id2}/resend`, { extend: "yes" })).status, 400);
    await admin(r, "POST", `/admin/invites/${id2}/revoke`);
    const dead = await admin(r, "POST", `/admin/invites/${id2}/resend`, {});
    assert.equal(dead.body.error.code, "INVITE_NOT_RESENDABLE");
    assert.equal((await admin(r, "POST", "/admin/invites/inv_AAAAAAAAAAAAAAAAAAAAAA/resend", {})).status, 404);
  });

  test("revoke_mirrors_workos_state", async () => {
    // Pending: revoked at WorkOS and here, and the entry goes back to confirmed.
    const a = await entry(r);
    const ia = (await invite(r, [a.id], "revoke-a")).body.invites[0].id;
    const wa = (await r.invites.byId(ia))!.workos_invitation_id!;
    const rev = await admin(r, "POST", `/admin/invites/${ia}/revoke`);
    assert.equal(rev.status, 200);
    assert.equal(rev.body.state, "revoked");
    assert.equal(r.wos.invitations.get(wa)!.state, "revoked");
    assert.equal((await r.w.waitlist.byId(a.id))?.status, "confirmed");
    assert.equal((await admin(r, "POST", `/admin/invites/${ia}/revoke`)).body.error.code, "INVITE_NOT_REVOCABLE");
    // Accepted at WorkOS first: the revoke is refused and our row mirrors the acceptance.
    const b = await entry(r);
    const ib = (await invite(r, [b.id], "revoke-b")).body.invites[0].id;
    const wb = (await r.invites.byId(ib))!.workos_invitation_id!;
    r.wos.accept(wb, "user_01NEWPERSON000000000000");
    const refused = await admin(r, "POST", `/admin/invites/${ib}/revoke`);
    assert.equal(refused.status, 409);
    assert.equal(refused.body.error.code, "INVITE_NOT_REVOCABLE");
    const ab = (await r.invites.byId(ib))!;
    assert.equal(ab.state, "accepted");
    assert.equal(ab.accepted_user_id, "user_01NEWPERSON000000000000");
    // Expired at WorkOS (there is no invitation.expired event, R-LC2a-5.6): mirrored as expired.
    const c = await entry(r);
    const ic = (await invite(r, [c.id], "revoke-c", { expires_in_days: 1 })).body.invites[0].id;
    r.w.clock.advance(86_400_000 + 1);
    assert.equal((await admin(r, "POST", `/admin/invites/${ic}/revoke`)).body.error.code, "INVITE_NOT_REVOCABLE");
    assert.equal((await r.invites.byId(ic))!.state, "expired");
    // Still queued: revoked here without calling WorkOS.
    const d = await entry(r);
    const budgetFull = new Invites({ ...r.invites.d, budget: new EmailBudget(r.w.sql, { invite: 0 }) });
    const id = (await budgetFull.enqueue(ADMIN, "queued-only-1", { waitlist_ids: [d.id] }).catch(() => null))?.invites[0]?.id;
    assert.equal(id, undefined, "a zero budget refuses the batch");
    const q = await r.invites.enqueue(ADMIN, "queued-only-2", { waitlist_ids: [d.id] });
    const calls = r.wos.calls.length;
    const rq = await admin(r, "POST", `/admin/invites/${q.invites[0].id}/revoke`);
    assert.equal(rq.body.state, "revoked");
    assert.equal(r.wos.calls.length, calls);
    await r.invites.drain();
    assert.equal(r.wos.createdFor(d.email), 0);
  });

  test("removing a waitlist entry revokes its invitation at WorkOS", async () => {
    const e = await entry(r);
    const id = (await invite(r, [e.id], "remove-revokes")).body.invites[0].id;
    const wid = (await r.invites.byId(id))!.workos_invitation_id!;
    assert.equal((await admin(r, "DELETE", `/admin/waitlist/${e.id}`)).status, 204);
    assert.equal(r.wos.invitations.get(wid)!.state, "revoked");
    assert.equal((await r.invites.byId(id))!.state, "revoked");
  });

  test("the invites list pages and filters by state", async () => {
    const all: string[] = [];
    let cursor: string | null = "";
    while (cursor !== null) {
      const res = await admin(r, "GET", `/admin/invites?limit=4${cursor ? `&cursor=${cursor}` : ""}`);
      assert.equal(res.status, 200);
      all.push(...res.body.items.map((i: { id: string }) => i.id));
      cursor = res.body.next_cursor;
    }
    assert.equal(new Set(all).size, all.length);
    const total = await r.w.sql.execute("SELECT COUNT(*) AS n FROM invites");
    assert.equal(all.length, Number(total.rows[0].n));
    const revoked = await admin(r, "GET", "/admin/invites?state=revoked");
    assert.ok(revoked.body.items.length >= 3 && revoked.body.items.every((i: { state: string }) => i.state === "revoked"));
    assert.equal((await admin(r, "GET", "/admin/invites?state=lost")).status, 400);
  });
});

describe("the daily budget", { skip: skipNoTidb }, () => {
  let r: Rig;
  before(async () => { r = await rig({ total: 8, invite: 5 }); });
  after(() => r.w.dispose());

  test("daily_budget_blocks_batch", async () => {
    const es = [];
    for (let i = 0; i < 7; i++) es.push(await entry(r));
    // Joining sent 7 waitlist emails; the batch shares the total with them only through the pacer
    // (LC2a Task 2), so here the invitation cap (5) binds first.
    const big = await invite(r, es.slice(0, 6).map((e) => e.id), "budget-big");
    assert.equal(big.status, 409);
    assert.equal(big.body.error.code, "DAILY_SEND_CAP");
    const ok = await invite(r, es.slice(0, 5).map((e) => e.id), "budget-ok");
    assert.equal(ok.status, 202);
    assert.equal((await r.budget.used(r.w.clock.now())).invite, 5);
    // Full for today: one more is refused, and so is a resend.
    assert.equal((await invite(r, [es[5].id], "budget-full")).body.error.code, "DAILY_SEND_CAP");
    const resend = await admin(r, "POST", `/admin/invites/${ok.body.invites[0].id}/resend`, {});
    assert.equal(resend.body.error.code, "DAILY_SEND_CAP");
    assert.equal((await r.invites.byId(ok.body.invites[0].id))!.resend_count, 0, "a refused resend isn't counted");
    // Tomorrow there is room again.
    r.w.clock.advance(86_400_000);
    assert.equal((await invite(r, [es[5].id], "budget-tomorrow")).status, 202);
  });

  test("pacer_shares_daily_budget_with_invitations", async () => {
    // The total is shared: waitlist mail taken through the same budget leaves less room for invitations.
    r.w.clock.advance(86_400_000);
    const now = r.w.clock.now();
    for (let i = 0; i < 5; i++) assert.ok(await r.budget.take("waitlist", now));
    assert.equal(await r.budget.remaining("invite", now), 3, "8 total, 5 used by waitlist mail");
    for (let i = 0; i < 3; i++) assert.ok(await r.budget.take("invite", now));
    assert.equal(await r.budget.take("invite", now), false);
    assert.equal(await r.budget.take("waitlist", now), false);
    assert.deepEqual(await r.budget.used(now), { total: 8, invite: 3, waitlist: 5 });
  });

  test("queued invites wait for tomorrow's budget", async () => {
    r.w.clock.advance(86_400_000);
    const e = await entry(r);
    await r.invites.enqueue(ADMIN, "queued-wait", { waitlist_ids: [e.id] });
    // Spend today's invite budget behind the queue's back.
    const now = r.w.clock.now();
    while (await r.budget.take("invite", now));
    assert.equal(await r.invites.drain(), 0);
    assert.equal(r.wos.createdFor(e.email), 0);
    r.w.clock.advance(86_400_000);
    assert.equal(await r.invites.drain(), 1);
    assert.equal(r.wos.createdFor(e.email), 1);
  });
});

describe("the WorkOS invitations client", () => {
  test("creates an application-wide invitation with expires_in_days, and drops the token", async () => {
    const calls: { url: string; init: RequestInit }[] = [];
    const f = (async (url: string, init: RequestInit) => {
      calls.push({ url, init });
      return new Response(JSON.stringify({ object: "invitation", id: "invitation_01X", email: "a@example.com", state: "pending", expires_at: "2026-11-15T00:00:00.000Z", token: "SECRET", accept_invitation_url: "https://x/SECRET", accepted_user_id: null }), { status: 201 });
    }) as unknown as typeof fetch;
    const c = new HttpWorkOs("sk_test_abc", { fetch: f });
    const inv = await c.createInvitation("a@example.com", 14);
    assert.deepEqual(inv, { id: "invitation_01X", state: "pending", expiresAt: Date.parse("2026-11-15T00:00:00.000Z"), acceptedUserId: null });
    assert.ok(!JSON.stringify(inv).includes("SECRET"));
    assert.equal(calls[0].url, "https://api.workos.com/user_management/invitations");
    assert.deepEqual(JSON.parse(String(calls[0].init.body)), { email: "a@example.com", expires_in_days: 14 });
    assert.equal((calls[0].init.headers as Record<string, string>).authorization, "Bearer sk_test_abc");
  });

  test("errors carry the status and code, and never the key", async () => {
    const f = (async () => new Response(JSON.stringify({ code: "invite_not_pending", message: "no" }), { status: 400 })) as unknown as typeof fetch;
    const c = new HttpWorkOs("sk_test_abc", { fetch: f });
    await assert.rejects(c.revokeInvitation("invitation_01ABCDEFGHIJKL"), (e: Error & { status?: number; code?: string }) => {
      assert.equal(e.status, 400);
      assert.equal(e.code, "invite_not_pending");
      assert.ok(!e.message.includes("sk_test"));
      assert.ok(!e.message.includes("invitation_01ABCDEFGHIJKL"), "ids are elided from messages");
      return true;
    });
  });
});
