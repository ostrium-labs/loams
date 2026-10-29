// LC2a Task 4: admin authorisation (design 78 §25.8, D1746).
import { test, describe, before, after } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { Admin, parseAdmins, STEP_UP_MS } from "../src/admin.ts";
import { JwtVerifier } from "../src/jwt.ts";
import { skipNoTidb, schemaDir } from "./harness/tidb.ts";
import { world, fragment, type World } from "./harness/fakes.ts";
import { FakeMfa, ISSUER, OSTRIUM_ORG, TokenIssuer } from "./harness/workos.ts";

const ADMIN = "user_01ADMIN000000000000000";

describe("admins.toml", () => {
  test("the shipped allow-list parses and starts empty", () => {
    assert.deepEqual(parseAdmins(readFileSync(join(schemaDir, "..", "config", "admins.toml"), "utf8")), []);
  });
  test("entries need a WorkOS user id and a name, once each", () => {
    assert.deepEqual(parseAdmins(`[[admin]]\nworkos_user_id = "${ADMIN}"\nname = "Owner"\n`), [{ workos_user_id: ADMIN, name: "Owner" }]);
    assert.throws(() => parseAdmins(`[[admin]]\nworkos_user_id = "someone@example.com"\nname = "x"\n`), /bad workos_user_id/);
    assert.throws(() => parseAdmins(`[[admin]]\nworkos_user_id = "${ADMIN}"\n`), /needs a name/);
    assert.throws(() => parseAdmins(`[[admin]]\nworkos_user_id = "${ADMIN}"\nname = "a"\n[[admin]]\nworkos_user_id = "${ADMIN}"\nname = "b"\n`), /twice/);
  });
});

describe("JWT verification", () => {
  let iss: TokenIssuer;
  before(async () => { iss = await TokenIssuer.create(); });
  const now = Date.UTC(2026, 10, 1);

  test("a good token verifies; tampering, expiry, issuer and algorithm are refused", async () => {
    const v = iss.verifier(() => now);
    const t = await iss.adminToken(now);
    assert.equal((await v.verify(t)).sub, ADMIN);
    const [h, p, s] = t.split(".");
    const forged = btoa(JSON.stringify({ ...JSON.parse(atob(p.replace(/-/g, "+").replace(/_/g, "/"))), role: "cloud_admin", sub: "user_01EVIL0000000000000000" })).replace(/=+$/, "").replace(/\+/g, "-").replace(/\//g, "_");
    await assert.rejects(v.verify(`${h}.${forged}.${s}`), /bad signature/);
    await assert.rejects(iss.verifier(() => now + 10 * 60_000).verify(t), /expired/);
    await assert.rejects(v.verify(await iss.adminToken(now, { iss: "https://evil.test" })), /issuer/);
    await assert.rejects(v.verify(await iss.sign({ iss: ISSUER, sub: ADMIN, exp: now / 1000 + 60 }, { alg: "HS256" })), /algorithm/);
    await assert.rejects(v.verify(await iss.sign({ iss: ISSUER, sub: ADMIN, exp: now / 1000 + 60 }, { alg: "none" })), /algorithm/);
    await assert.rejects(v.verify("a.b"), /malformed/);
    const other = await TokenIssuer.create("key_2");
    await assert.rejects(v.verify(await other.adminToken(now)), /unknown signing key/);
  });

  test("a rotated key is picked up by refetching the JWKS, at most once a minute", async () => {
    const k2 = await TokenIssuer.create("key_2");
    let keys = [iss.jwk];
    let fetches = 0;
    let t = now;
    const v = new JwtVerifier(async () => { fetches++; return { keys }; }, { issuer: ISSUER, now: () => t });
    await v.verify(await iss.adminToken(now));
    keys = [iss.jwk, k2.jwk];
    await assert.rejects(v.verify(await k2.adminToken(now)), /unknown signing key/, "inside the refetch interval");
    t += 61_000;
    assert.equal((await v.verify(await k2.adminToken(now, { exp: now / 1000 + 3600 }))).sub, ADMIN);
    assert.equal(fetches, 2);
  });
});

describe("admin endpoints", { skip: skipNoTidb }, () => {
  let w: World;
  let iss: TokenIssuer;
  let mfa: FakeMfa;
  let admin: Admin;
  const auth = (t: string) => ({ authorization: `Bearer ${t}` });

  before(async () => {
    w = await world();
    iss = await TokenIssuer.create();
    mfa = new FakeMfa();
    admin = new Admin({
      sql: w.sql, verifier: iss.verifier(() => w.clock.now()), mfa, waitlist: w.waitlist, audit: w.audit, limiter: w.limiter, clock: w.clock, logger: w.log.logger,
      config: { ostriumOrgId: OSTRIUM_ORG, admins: parseAdmins(`[[admin]]\nworkos_user_id = "${ADMIN}"\nname = "Owner"\n`), totpIssuer: "Loams Cloud" },
    });
    w.deps.routes = [admin.routes()];
  });
  after(() => w.dispose());

  async function stepUp(token: string) {
    const r = await w.call("POST", "/admin/mfa/verify", { code: "246810" }, auth(token));
    assert.equal(r.status, 200, JSON.stringify(r.body));
    return r;
  }

  test("admin_requires_role_allowlist_and_step_up", async () => {
    const now = w.clock.now();
    // No token, or a bad one: 401.
    assert.equal((await w.call("GET", "/admin/waitlist")).status, 401);
    assert.equal((await w.call("GET", "/admin/waitlist", undefined, auth("x.y.z"))).status, 401);
    assert.equal((await w.call("GET", "/admin/waitlist", undefined, { authorization: "Basic abc" })).status, 401);
    // The wrong org, no role, another role, or not on the allow-list: 403 PERMISSION_DENIED, for every admin path.
    const refusals = [
      await iss.adminToken(now, { org_id: "org_01SOMEONEELSE00000000000" }),
      await iss.adminToken(now, { role: "member", roles: ["member"] }),
      await iss.adminToken(now, { role: undefined, roles: undefined }),
      await iss.adminToken(now, { sub: "user_01NOTLISTED0000000000", sid: "session_01OTHER00000000000" }),
    ];
    for (const t of refusals) {
      for (const [m, p] of [["GET", "/admin/waitlist"], ["GET", "/admin/audit"], ["DELETE", "/admin/waitlist/wl_AAAAAAAAAAAAAAAAAAAAAA"], ["POST", "/admin/mfa/enroll"], ["POST", "/admin/mfa/verify"]]) {
        const r = await w.call(m, p, m === "POST" ? { code: "246810" } : undefined, auth(t));
        assert.equal(r.status, 403, `${m} ${p}`);
        assert.equal(r.body.error.code, "PERMISSION_DENIED");
      }
    }
    // Role and allow-list but no step-up: 403 STEP_UP_REQUIRED.
    const good = await iss.adminToken(now);
    const r = await w.call("GET", "/admin/waitlist", undefined, auth(good));
    assert.equal(r.status, 403);
    assert.equal(r.body.error.code, "STEP_UP_REQUIRED");
    // Verify before enrolling: refused.
    assert.equal((await w.call("POST", "/admin/mfa/verify", { code: "246810" }, auth(good))).body.error.code, "NOT_ENROLLED");
    // Enrol once; a second enrolment is 409.
    const e = await w.call("POST", "/admin/mfa/enroll", undefined, auth(good));
    assert.equal(e.status, 200);
    assert.match(e.body.factor_id, /^auth_factor_/);
    assert.ok(e.body.qr_code && e.body.secret);
    assert.equal((await w.call("POST", "/admin/mfa/enroll", undefined, auth(good))).body.error.code, "ALREADY_ENROLLED");
    // A wrong code: 422; then the right one opens the admin API.
    const bad = await w.call("POST", "/admin/mfa/verify", { code: "111111" }, auth(good));
    assert.equal(bad.status, 422);
    assert.equal(bad.body.error.code, "CODE_INVALID");
    assert.equal((await w.call("POST", "/admin/mfa/verify", { code: "12" }, auth(good))).status, 422);
    const v = await stepUp(good);
    assert.equal(Date.parse(v.body.step_up_until), now + STEP_UP_MS);
    assert.equal((await w.call("GET", "/admin/waitlist", undefined, auth(good))).status, 200);
    assert.equal((await w.call("GET", "/admin/audit", undefined, auth(good))).status, 200);
    // The step-up belongs to the session: another session of the same admin needs its own.
    const otherSession = await iss.adminToken(now, { sid: "session_01SECOND000000000000" });
    assert.equal((await w.call("GET", "/admin/waitlist", undefined, auth(otherSession))).body.error.code, "STEP_UP_REQUIRED");
    // And a step-up can't be borrowed by another user who presents the same sid.
    const borrowed = await iss.adminToken(now, { sub: "user_01NOTLISTED0000000000" });
    assert.equal((await w.call("GET", "/admin/waitlist", undefined, auth(borrowed))).status, 403);
  });

  test("step_up_expires_after_15_minutes", async () => {
    const t0 = w.clock.now();
    const sid = "session_01EXPIRY000000000000";
    await stepUp(await iss.adminToken(t0, { sid }));
    w.clock.advance(STEP_UP_MS - 1000);
    assert.equal((await w.call("GET", "/admin/audit", undefined, auth(await iss.adminToken(w.clock.now(), { sid })))).status, 200);
    w.clock.advance(1000);
    const r = await w.call("GET", "/admin/audit", undefined, auth(await iss.adminToken(w.clock.now(), { sid })));
    assert.equal(r.status, 403);
    assert.equal(r.body.error.code, "STEP_UP_REQUIRED");
    // A fresh verification extends it again.
    await stepUp(await iss.adminToken(w.clock.now(), { sid }));
    assert.equal((await w.call("GET", "/admin/audit", undefined, auth(await iss.adminToken(w.clock.now(), { sid })))).status, 200);
  });

  test("agents_cannot_be_cloud_admin", async () => {
    // Every shape a non-person token can take, each with the right org, role and an
    // allow-listed subject where the shape allows it: refused before the role counts.
    const sid = "session_01AGENTS000000000000";
    await stepUp(await iss.adminToken(w.clock.now(), { sid }));
    const now = w.clock.now();
    const shapes = [
      await iss.adminToken(now, { sid: undefined }), // no session: an M2M or API token
      await iss.adminToken(now, { sub: "client_01M2M0000000000000000", sid }), // an M2M application
      await iss.adminToken(now, { sub: "agent_01AGENT000000000000000", sid }), // an agent principal
      await iss.adminToken(now, { sub: "service_account_01SA00000000000", sid }),
      await iss.adminToken(now, { sid, act: { sub: "agent_01AGENT000000000000000" } }), // an agent acting for the admin
      await iss.adminToken(now, { sid, may_act: { sub: "agent_01AGENT000000000000000" } }),
    ];
    for (const t of shapes) {
      const r = await w.call("GET", "/admin/audit", undefined, auth(t));
      assert.equal(r.status, 403);
      assert.equal(r.body.error.code, "PERMISSION_DENIED");
    }
    // The authorizer model admits users only (deploy/openfga/core.fga).
    const model = readFileSync(join(schemaDir, "..", "..", "openfga", "core.fga"), "utf8");
    assert.match(model, /define cloud_admin: \[user\]\n/);
  });

  test("admin_list_pages_and_filters", async () => {
    w.clock.advance(24 * 3600_000);
    const sid = "session_01LISTING000000000000";
    const token = () => iss.adminToken(w.clock.now(), { sid });
    await stepUp(await token());
    const emails: string[] = [];
    for (let i = 0; i < 7; i++) {
      w.clock.advance(1000);
      const email = `list${i}@example.com`;
      emails.push(email);
      await w.call("POST", "/waitlist", { email, turnstile_token: w.turnstile.issue(), source: i % 2 ? "github" : "friend" }, { "cf-connecting-ip": `100.80.${i}.1` });
      if (i < 4) await w.call("POST", "/waitlist/confirm", { token: fragment(w.mailer.to(email)[0].confirmUrl) }, { "cf-connecting-ip": `100.81.${i}.1` });
    }
    const since = new Date(w.clock.now() - 10_000).toISOString();
    // Pages of 3, newest first, with the address decrypted for the admin.
    const seen: string[] = [];
    let cursor: string | null = "";
    let pages = 0;
    while (cursor !== null) {
      const r = await w.call("GET", `/admin/waitlist?since=${since}&limit=3${cursor ? `&cursor=${cursor}` : ""}`, undefined, auth(await token()));
      assert.equal(r.status, 200);
      assert.ok(r.body.items.length <= 3);
      seen.push(...r.body.items.map((x: { email: string }) => x.email));
      cursor = r.body.next_cursor;
      pages++;
    }
    assert.equal(pages, 3);
    assert.deepEqual(seen, [...emails].reverse());
    // Filters: status and source.
    const confirmed = await w.call("GET", `/admin/waitlist?since=${since}&status=confirmed`, undefined, auth(await token()));
    assert.deepEqual(confirmed.body.items.map((x: { email: string }) => x.email).sort(), emails.slice(0, 4).sort());
    assert.ok(confirmed.body.items.every((x: { confirmed_at: string | null; invite: unknown }) => x.confirmed_at && x.invite === null));
    const github = await w.call("GET", `/admin/waitlist?since=${since}&source=github`, undefined, auth(await token()));
    assert.deepEqual(github.body.items.map((x: { email: string }) => x.email).sort(), ["list1@example.com", "list3@example.com", "list5@example.com"]);
    // Bad parameters are 400.
    for (const q of ["status=nope", "limit=0", "limit=201", "since=yesterday", "cursor=!!!"]) {
      assert.equal((await w.call("GET", `/admin/waitlist?${q}`, undefined, auth(await token()))).status, 400, q);
    }
    // Delete one: 204, then 404; the address is suppressed and the audit names the admin.
    const victim = confirmed.body.items[0];
    assert.equal((await w.call("DELETE", `/admin/waitlist/${victim.id}`, undefined, auth(await token()))).status, 204);
    assert.equal((await w.call("DELETE", `/admin/waitlist/${victim.id}`, undefined, auth(await token()))).status, 404);
    const a = await w.call("GET", `/admin/audit?subject=${victim.id}&type=waitlist.deleted`, undefined, auth(await token()));
    assert.equal(a.body.items.length, 1);
    assert.equal(a.body.items[0].data.actor, ADMIN);
    assert.equal(a.body.items[0].specversion, "1.0");
    // The audit pages too, and every read of the list is itself audited.
    const page1 = await w.call("GET", "/admin/audit?limit=2&type=admin.waitlist_read", undefined, auth(await token()));
    assert.equal(page1.body.items.length, 2);
    const page2 = await w.call("GET", `/admin/audit?limit=2&type=admin.waitlist_read&cursor=${page1.body.next_cursor}`, undefined, auth(await token()));
    assert.ok(page2.body.items.length >= 1);
    assert.notEqual(page1.body.items[0].id, page2.body.items[0].id);
  });

  test("admin calls are rate limited per admin", async () => {
    w.clock.advance(3600_000);
    const sid = "session_01RATE0000000000000000";
    await stepUp(await iss.adminToken(w.clock.now(), { sid }));
    const t = await iss.adminToken(w.clock.now(), { sid });
    let limited = 0;
    for (let i = 0; i < 61; i++) if ((await w.call("GET", "/admin/audit?limit=1", undefined, auth(t))).status === 429) limited++;
    assert.equal(limited, 2, "60 a minute, counting the step-up call");
  });

  test("admin tokens never reach the logs", () => {
    assert.doesNotMatch(w.log.text(), /eyJ[A-Za-z0-9_-]{20,}/);
  });
});
