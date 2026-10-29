// LC2a Task 3: the public waitlist endpoints (design 78 §25.7, §25.10).
import { test, describe, before, after } from "node:test";
import assert from "node:assert/strict";
import { emailHmac } from "../src/crypto.ts";
import { skipNoTidb } from "./harness/tidb.ts";
import { world, fragment, type World } from "./harness/fakes.ts";

let nextIp = 0;
/** Joins from a fresh /24 unless an address is given, so tests don't trip the prefix limit by accident. */
function join(w: World, email: string, extra: Record<string, unknown> = {}, ip = `10.${(++nextIp >> 8) & 255}.${nextIp & 255}.1`) {
  return w.call("POST", "/waitlist", { email, turnstile_token: w.turnstile.issue(), ...extra }, { "cf-connecting-ip": ip });
}

async function auditTypes(w: World, subject?: string): Promise<string[]> {
  const r = await w.sql.execute(`SELECT type FROM audit ${subject ? "WHERE subject = ?" : ""} ORDER BY time_ms, id`, subject ? [subject] : []);
  return r.rows.map((x) => String(x.type).replace("dev.loams.cloud.", ""));
}

describe("waitlist", { skip: skipNoTidb }, () => {
  let w: World;
  before(async () => { w = await world(); });
  after(() => w.dispose());

  test("join sends a confirm link, and confirm makes the entry count", async () => {
    const r = await join(w, "Ada@Example.com", { build: "a notes app", source: "github" });
    assert.equal(r.status, 202);
    assert.equal(r.body.status, "check_your_email");
    assert.match(r.body.request_id, /^req_/);
    assert.equal(r.headers.get("cache-control"), "no-store");
    assert.equal(r.headers.get("referrer-policy"), "no-referrer");
    const mails = w.mailer.to("ada@example.com");
    assert.equal(mails.length, 1);
    assert.equal(mails[0].template, "confirm");
    assert.match(mails[0].confirmUrl!, /^https:\/\/loams\.dev\/cloud\/confirm#wlc_/);
    assert.match(mails[0].removeUrl, /^https:\/\/loams\.dev\/cloud\/leave#wlr_/);
    const c = await w.call("POST", "/waitlist/confirm", { token: fragment(mails[0].confirmUrl) });
    assert.equal(c.status, 200);
    assert.equal(c.body.status, "confirmed");
    const row = await w.waitlist.byHmac(await emailHmac(w.keys, "ada@example.com"));
    assert.equal(row?.status, "confirmed");
    assert.equal(row?.build, "a notes app");
    assert.equal(await w.waitlist.decryptEmail(row!), "ada@example.com");
    assert.deepEqual((await auditTypes(w, row!.id)).sort(), ["waitlist.confirm_sent", "waitlist.confirmed", "waitlist.joined"]);
  });

  test("waitlist_responses_do_not_enumerate", async () => {
    // Every address state: new, pending, confirmed, suppressed, disposable, no MX, bad Turnstile aside.
    await join(w, "pending@example.org");
    await join(w, "confirmed@example.org");
    await w.call("POST", "/waitlist/confirm", { token: fragment(w.mailer.to("confirmed@example.org")[0].confirmUrl) });
    await join(w, "gone@example.org");
    await w.call("DELETE", "/waitlist", { token: fragment(w.mailer.to("gone@example.org")[0].removeUrl) });
    const cases = ["brand-new@example.org", "pending@example.org", "confirmed@example.org", "gone@example.org", "x@mailinator.com", "x@no-mx.example"];
    const seen = new Set<string>();
    for (const email of cases) {
      const r = await join(w, email);
      const { request_id, ...rest } = r.body;
      assert.match(request_id, /^req_/);
      seen.add(JSON.stringify({ status: r.status, body: rest, headers: [...r.headers.entries()].filter(([k]) => k !== "x-request-id") }));
    }
    assert.equal(seen.size, 1, "the same status, body and headers for every state");
    // Removal requests by address answer the same way too.
    const rm = new Set<string>();
    for (const email of ["confirmed@example.org", "never@example.org", "gone@example.org"]) {
      const r = await w.call("DELETE", "/waitlist", { email, turnstile_token: w.turnstile.issue() });
      const { request_id: _, ...rest } = r.body;
      rm.add(JSON.stringify({ status: r.status, body: rest }));
    }
    assert.equal(rm.size, 1);
    // Only the inbox owner learns anything: nothing at all goes to the suppressed, disposable or no-MX addresses.
    assert.equal(w.mailer.to("gone@example.org").length, 1, "only the original confirm");
    assert.equal(w.mailer.to("x@mailinator.com").length, 0);
    assert.equal(w.mailer.to("x@no-mx.example").length, 0);
    assert.equal(w.mailer.to("never@example.org").length, 0);
    // A confirmed address gets "already on the list", at most once a day.
    const confirmedMails = w.mailer.to("confirmed@example.org").map((m) => m.template);
    assert.deepEqual(confirmedMails, ["confirm", "already_listed", "removal"]);
    await join(w, "confirmed@example.org");
    assert.equal(w.mailer.to("confirmed@example.org").filter((m) => m.template === "already_listed").length, 1);
    w.clock.advance(24 * 3600_000 + 1);
    await join(w, "confirmed@example.org");
    assert.equal(w.mailer.to("confirmed@example.org").filter((m) => m.template === "already_listed").length, 2);
  });

  test("confirm_is_single_use", async () => {
    await join(w, "once@example.net");
    const token = fragment(w.mailer.to("once@example.net")[0].confirmUrl);
    assert.equal((await w.call("POST", "/waitlist/confirm", { token })).status, 200);
    const again = await w.call("POST", "/waitlist/confirm", { token });
    assert.equal(again.status, 410);
    assert.equal(again.body.error.code, "TOKEN_UNUSABLE");
    // Unknown, malformed and expired tokens get the same answer.
    assert.equal((await w.call("POST", "/waitlist/confirm", { token: "wlc_" + "A".repeat(43) })).body.error.code, "TOKEN_UNUSABLE");
    assert.equal((await w.call("POST", "/waitlist/confirm", { token: "nonsense" })).body.error.code, "TOKEN_UNUSABLE");
    await join(w, "late@example.net");
    w.clock.advance(48 * 3600_000 + 1);
    assert.equal((await w.call("POST", "/waitlist/confirm", { token: fragment(w.mailer.to("late@example.net")[0].confirmUrl) })).status, 410);
    // A second confirm email invalidates the first (D1740).
    await join(w, "twice@example.net");
    await join(w, "twice@example.net");
    const [first, second] = w.mailer.to("twice@example.net");
    assert.equal((await w.call("POST", "/waitlist/confirm", { token: fragment(first.confirmUrl) })).status, 410);
    assert.equal((await w.call("POST", "/waitlist/confirm", { token: fragment(second.confirmUrl) })).status, 200);
  });

  test("turnstile_token_reused_is_refused", async () => {
    const token = w.turnstile.issue();
    const a = await w.call("POST", "/waitlist", { email: "ts@example.com", turnstile_token: token }, { "cf-connecting-ip": "198.51.100.1" });
    assert.equal(a.status, 202);
    const b = await w.call("POST", "/waitlist", { email: "ts2@example.com", turnstile_token: token }, { "cf-connecting-ip": "198.51.100.1" });
    assert.equal(b.status, 403);
    assert.equal(b.body.error.code, "TURNSTILE_FAILED");
    assert.equal(w.mailer.to("ts2@example.com").length, 0);
    const c = await w.call("POST", "/waitlist", { email: "ts3@example.com" }, { "cf-connecting-ip": "198.51.100.1" });
    assert.equal(c.status, 400);
    const d = await w.call("DELETE", "/waitlist", { email: "ts@example.com", turnstile_token: token }, { "cf-connecting-ip": "198.51.100.1" });
    assert.equal(d.status, 403);
  });

  test("waitlist_accepts_free_mail_but_not_disposable", async () => {
    for (const email of ["someone@gmail.com", "someone@outlook.com", "someone@icloud.com", "someone@proton.me"]) {
      await join(w, email, {}, "192.0.2.10");
      assert.equal(w.mailer.to(email).length, 1, `${email} is accepted on the waitlist (D1753)`);
    }
    w.clock.advance(10 * 60_000);
    for (const email of ["someone@mailinator.com", "someone@sub.mailinator.com", "someone@10minutemail.com"]) {
      const r = await join(w, email, {}, "192.0.2.11");
      assert.equal(r.status, 202);
      assert.equal(w.mailer.to(email).length, 0, `${email} is disposable`);
      const h = await emailHmac(w.keys, email);
      assert.equal(await w.waitlist.byHmac(h), null);
      const ev = await w.sql.execute("SELECT detail FROM audit WHERE subject = ? AND type = 'dev.loams.cloud.waitlist.rejected'", [h]);
      assert.match(String(ev.rows[0]?.detail), /disposable/);
    }
  });

  test("rate_limits_apply_per_prefix_and_address", async () => {
    w.clock.advance(24 * 3600_000);
    // Five joins per prefix per 10 minutes; the sixth from the same /24 is refused.
    for (let i = 0; i < 5; i++) assert.equal((await join(w, `rl${i}@example.com`, {}, `100.64.1.${i + 1}`)).status, 202);
    const sixth = await join(w, "rl5@example.com", {}, "100.64.1.200");
    assert.equal(sixth.status, 429);
    assert.equal(sixth.body.error.code, "RATE_LIMITED");
    assert.ok(Number(sixth.headers.get("retry-after")) > 0);
    // Another prefix is unaffected.
    assert.equal((await join(w, "rl6@example.com", {}, "100.64.2.1")).status, 202);
    // IPv6 is limited per /48.
    for (let i = 0; i < 5; i++) await join(w, `v6-${i}@example.com`, {}, `2001:db8:aa:${i}::1`);
    assert.equal((await join(w, "v6-x@example.com", {}, "2001:db8:aa:ffff::9")).status, 429);
    // Twenty a day per prefix.
    let refused = 0;
    for (let i = 0; i < 6; i++) {
      w.clock.advance(10 * 60_000);
      for (let j = 0; j < 5; j++) if ((await join(w, `day${i}-${j}@example.com`, {}, "100.64.3.1")).status === 429) refused++;
    }
    assert.equal(refused, 10, "30 attempts, 20 allowed");
    // Per address: at most 3 confirmation emails in 24 hours, from any prefix.
    const ips = ["100.65.0.1", "100.66.0.1", "100.67.0.1", "100.68.0.1", "100.69.0.1"];
    for (const ip of ips) await join(w, "per-address@example.com", {}, ip);
    assert.equal(w.mailer.to("per-address@example.com").length, 3);
    w.clock.advance(24 * 3600_000 + 1);
    await join(w, "per-address@example.com", {}, "100.70.0.1");
    assert.equal(w.mailer.to("per-address@example.com").length, 4);
    // Token endpoints: 30 a minute per prefix.
    let tokenRefused = 0;
    for (let i = 0; i < 32; i++) {
      const r = await w.call("POST", "/waitlist/confirm", { token: "wlc_" + "B".repeat(43) }, { "cf-connecting-ip": "100.71.0.1" });
      if (r.status === 429) tokenRefused++;
    }
    assert.equal(tokenRefused, 2);
  });

  test("removal by token deletes the entry and keeps only the suppression HMAC", async () => {
    w.clock.advance(24 * 3600_000);
    await join(w, "leaver@example.com", { build: "something" }, "100.72.0.1");
    const mail = w.mailer.to("leaver@example.com")[0];
    const h = await emailHmac(w.keys, "leaver@example.com");
    const row = await w.waitlist.byHmac(h);
    const r = await w.call("DELETE", "/waitlist", { token: fragment(mail.removeUrl) }, { "cf-connecting-ip": "100.72.0.1" });
    assert.equal(r.status, 204);
    assert.equal(await w.waitlist.byHmac(h), null);
    assert.equal((await w.sql.execute("SELECT COUNT(*) AS n FROM waitlist_tokens WHERE waitlist_id = ?", [row!.id])).rows[0].n, "0");
    assert.equal((await w.sql.execute("SELECT reason FROM waitlist_suppressed WHERE email_hmac = ?", [h])).rows[0].reason, "removed");
    // The token is spent, and the address can't come back.
    assert.equal((await w.call("DELETE", "/waitlist", { token: fragment(mail.removeUrl) }, { "cf-connecting-ip": "100.72.0.1" })).status, 410);
    await join(w, "leaver@example.com", {}, "100.72.0.2");
    assert.equal(w.mailer.to("leaver@example.com").length, 1);
    // The confirm link of a removed entry is dead too.
    assert.equal((await w.call("POST", "/waitlist/confirm", { token: fragment(mail.confirmUrl) }, { "cf-connecting-ip": "100.72.0.3" })).status, 410);
  });

  test("removal by address mails a removal link only when listed", async () => {
    w.clock.advance(24 * 3600_000);
    await join(w, "listed@example.com", {}, "100.73.0.1");
    const r = await w.call("DELETE", "/waitlist", { email: "listed@example.com", turnstile_token: w.turnstile.issue() }, { "cf-connecting-ip": "100.73.0.2" });
    assert.equal(r.status, 202);
    const removal = w.mailer.to("listed@example.com").filter((m) => m.template === "removal");
    assert.equal(removal.length, 1);
    assert.equal(removal[0].confirmUrl, undefined);
    assert.equal((await w.call("DELETE", "/waitlist", { token: fragment(removal[0].removeUrl) }, { "cf-connecting-ip": "100.73.0.3" })).status, 204);
  });

  test("bad requests are 400 and never depend on the address's state", async () => {
    w.clock.advance(24 * 3600_000);
    const ip = { "cf-connecting-ip": "100.74.0.1" };
    assert.equal((await w.call("POST", "/waitlist", "not json", ip)).status, 400);
    assert.equal((await w.call("POST", "/waitlist", { email: "no-at-sign", turnstile_token: "t" }, ip)).status, 400);
    assert.equal((await w.call("POST", "/waitlist", { email: "a@example.com", turnstile_token: "t", build: "x".repeat(501) }, ip)).status, 400);
    assert.equal((await w.call("POST", "/waitlist", { email: "a@example.com", turnstile_token: "t", source: "tv" }, ip)).status, 400);
    assert.equal((await w.call("POST", "/waitlist", { email: "a@example.com", turnstile_token: "t", build: "mail me at bob@example.com" }, ip)).status, 400);
    const big = await w.call("POST", "/waitlist", { email: "a@example.com", turnstile_token: "t", pad: "x".repeat(9000) }, ip);
    assert.equal(big.status, 413);
    assert.equal((await w.call("GET", "/nope")).status, 404);
  });

  test("a failed send is logged, and the next join sends again", async () => {
    w.clock.advance(24 * 3600_000);
    w.mailer.failNext = true;
    await join(w, "flaky@example.com", {}, "100.75.0.1");
    assert.equal(w.mailer.to("flaky@example.com").length, 0);
    assert.match(w.log.text(), /waitlist\.send_failed/);
    await join(w, "flaky@example.com", {}, "100.75.0.2");
    assert.equal(w.mailer.to("flaky@example.com").length, 1);
  });

  test("tokens_are_redacted_from_logs", async () => {
    // Everything this suite did went through the capturing logger: no token,
    // no address, ever. The audit table holds no address either.
    const text = w.log.text();
    assert.ok(w.log.lines.length > 50, "the suite logged its requests");
    for (const m of w.mailer.sent) {
      assert.ok(!text.includes(m.to), "no address in the logs");
      assert.ok(!text.includes(fragment(m.removeUrl)), "no removal token in the logs");
      if (m.confirmUrl) assert.ok(!text.includes(fragment(m.confirmUrl)), "no confirm token in the logs");
    }
    assert.doesNotMatch(text, /wl[cr]_[A-Za-z0-9_-]{20,}/);
    assert.doesNotMatch(text, /ts_ok_\d+/, "no Turnstile token");
    const audit = await w.sql.execute("SELECT actor, subject, detail FROM audit");
    assert.ok(audit.rows.length > 20);
    assert.doesNotMatch(JSON.stringify(audit.rows), /@/);
    // And the database holds no plaintext address anywhere.
    const dump = JSON.stringify((await w.sql.execute("SELECT * FROM waitlist")).rows) + JSON.stringify((await w.sql.execute("SELECT * FROM waitlist_tokens")).rows);
    for (const m of w.mailer.sent) assert.ok(!dump.includes(m.to));
  });
});
