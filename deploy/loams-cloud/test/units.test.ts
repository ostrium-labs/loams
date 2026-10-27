// Unit tests that need no database: addresses and keys, the TOML subset, the
// domain lists, log redaction, and the Turnstile and DNS clients (with a fake fetch).
import { test, describe } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { join } from "node:path";
import { decrypt, emailHmac, encrypt, hmacHex, ipPrefix, newId, newToken, normaliseEmail, testKeys, timingSafeEqual } from "../src/crypto.ts";
import { parseToml } from "../src/toml.ts";
import { DohDns, emailCodeDomainRefusal, isDisposable, loadDomainLists } from "../src/domains.ts";
import { redact, redactFields } from "../src/log.ts";
import { SiteverifyTurnstile } from "../src/turnstile.ts";
import { loadLists, policyDir } from "./harness/fakes.ts";

describe("crypto", () => {
  test("normaliseEmail lower-cases and refuses malformed addresses", () => {
    assert.equal(normaliseEmail("  Ada.Lovelace+cloud@Example.COM "), "ada.lovelace+cloud@example.com");
    for (const bad of ["", "a", "@example.com", "a@", "a@b", "a..b@example.com", ".a@example.com", "a@-example.com", "a b@example.com", `${"x".repeat(65)}@example.com`]) {
      assert.equal(normaliseEmail(bad), null, bad);
    }
  });

  test("HMACs are stable per key and separated by purpose", async () => {
    const k = await testKeys();
    assert.equal(await emailHmac(k, "a@example.com"), await emailHmac(k, "a@example.com"));
    assert.notEqual(await emailHmac(k, "a@example.com"), await hmacHex(k, "ip", "a@example.com"));
    assert.notEqual(await emailHmac(k, "a@example.com"), await emailHmac(await testKeys(), "a@example.com"));
    assert.match(await emailHmac(k, "a@example.com"), /^[0-9a-f]{64}$/);
  });

  test("AES-GCM round-trips, with a fresh IV each time, and other keys can't read it", async () => {
    const k = await testKeys();
    const a = await encrypt(k, "a@example.com");
    assert.notEqual(a, await encrypt(k, "a@example.com"));
    assert.equal(await decrypt(k, a), "a@example.com");
    await assert.rejects(decrypt(await testKeys(), a));
    assert.ok(!a.includes("example"));
  });

  test("ids and tokens have their prefixes and lengths", () => {
    assert.match(newId("wl"), /^wl_[A-Za-z0-9_-]{22}$/);
    assert.match(newToken("wlc"), /^wlc_[A-Za-z0-9_-]{43}$/);
    assert.notEqual(newToken("wlr"), newToken("wlr"));
    assert.ok(timingSafeEqual("abc", "abc"));
    assert.ok(!timingSafeEqual("abc", "abd"));
    assert.ok(!timingSafeEqual("abc", "abcd"));
  });

  test("ipPrefix is /24 for IPv4 and /48 for IPv6", () => {
    assert.equal(ipPrefix("203.0.113.77"), "203.0.113.0/24");
    assert.equal(ipPrefix("2001:db8:aa:1::1"), "2001:db8:aa::/48");
    assert.equal(ipPrefix("2001:0db8:00aa:ffff:1:2:3:4"), "2001:db8:aa::/48");
    assert.equal(ipPrefix("::1"), "0:0:0::/48");
    assert.equal(ipPrefix("unknown"), "unknown");
  });
});

describe("toml subset", () => {
  test("parses strings, numbers, booleans, arrays and tables", () => {
    const t = parseToml(`# c\nname = "x # not a comment"\nn = 3\nb = true\nlist = [\n  "a", # one\n  "b",\n]\n[t]\nk = "v"\n[[admin]]\nid = "1"\n[[admin]]\nid = "2"\n`);
    assert.deepEqual(t, { name: "x # not a comment", n: 3, b: true, list: ["a", "b"], t: { k: "v" }, admin: [{ id: "1" }, { id: "2" }] });
  });

  test("refuses what it doesn't understand", () => {
    assert.throws(() => parseToml("a = 1.5"));
    assert.throws(() => parseToml("a = [1, 2]"));
    assert.throws(() => parseToml("a.b = 1"));
    assert.throws(() => parseToml('a = "x"\na = "y"'));
  });
});

describe("domain lists", () => {
  const lists = loadLists();

  test("the vendored lists load, at their pins", () => {
    assert.ok(lists.disposable.size > 9000, "the disposable list");
    assert.ok(lists.disposable.has("mailinator.com"));
    const pins = readFileSync(join(policyDir, "PINS.md"), "utf8");
    assert.match(pins, /8c05972cf08375d540d50b96b15b7db360fb0a29/);
    const sha = createHash("sha256").update(readFileSync(join(policyDir, "disposable_email_blocklist.conf"))).digest("hex");
    assert.ok(pins.includes(sha), "the vendored file is the pinned one, unmodified");
  });

  test("disposable matches the domain and its parents; free mail is not disposable", () => {
    assert.ok(isDisposable(lists, "mailinator.com"));
    assert.ok(isDisposable(lists, "eu.mailinator.com"));
    for (const d of ["gmail.com", "outlook.com", "example.com", "ostriumlabs.org"]) assert.ok(!isDisposable(lists, d), d);
  });

  test("allow beats the lists; block beats absence", () => {
    const l = loadDomainLists({ disposable: "throwaway.test\n", overrides: 'block = ["corp-blocked.test", "gmx.*"]\nallow = ["throwaway.test"]\n' });
    assert.ok(!isDisposable(l, "throwaway.test"));
    assert.equal(emailCodeDomainRefusal(l, "throwaway.test"), null);
    assert.equal(emailCodeDomainRefusal(l, "corp-blocked.test"), "blocked");
    assert.equal(emailCodeDomainRefusal(l, "gmx.de"), "blocked");
    assert.equal(emailCodeDomainRefusal(l, "gmx.co.uk"), "blocked");
    assert.equal(emailCodeDomainRefusal(l, "gmxsomething.de"), null);
    assert.throws(() => loadDomainLists({ disposable: "", overrides: 'block = ["a.test"]\nallow = ["a.test"]\n' }), /both/);
  });

  test("the owner's named families are blocked for email codes", () => {
    for (const d of ["gmail.com", "googlemail.com", "outlook.com", "hotmail.com", "live.com", "yahoo.co.uk", "yahoo.com", "icloud.com", "me.com", "proton.me", "protonmail.com", "aol.com", "gmx.de", "mail.ru", "yandex.ru", "qq.com", "163.com", "privaterelay.appleid.com"]) {
      assert.equal(emailCodeDomainRefusal(lists, d), "blocked", d);
    }
    assert.equal(emailCodeDomainRefusal(lists, "mailinator.com"), "disposable");
    assert.equal(emailCodeDomainRefusal(lists, "ostriumlabs.org"), null);
  });
});

describe("log redaction", () => {
  test("tokens, addresses, bearer values and fragments are removed", () => {
    const t = "wlc_" + "A".repeat(43);
    const line = redact(`confirm ${t} for ada@example.com via https://loams.dev/cloud/confirm#${t} Authorization: Bearer eyJabc.def token=xyz`);
    assert.ok(!line.includes("A".repeat(20)));
    assert.ok(!line.includes("ada@example.com"));
    assert.ok(!line.includes("eyJabc"));
    assert.ok(!line.includes("xyz"));
    const f = redactFields({ email: "a@b.co", to: "c@d.co", nested: { note: "see e@f.co" }, n: 3 });
    assert.deepEqual(f, { email: "[redacted]", to: "[redacted]", nested: { note: "see [email]" }, n: 3 });
  });
});

describe("Turnstile Siteverify client", () => {
  function fakeFetch(responses: (Response | Error)[]) {
    const calls: FormData[] = [];
    const f = (async (_url: string, init: RequestInit) => {
      calls.push(init.body as FormData);
      const r = responses.shift();
      if (r instanceof Error) throw r;
      return r!;
    }) as unknown as typeof fetch;
    return { f, calls };
  }
  const ok = (body: unknown) => new Response(JSON.stringify(body), { status: 200 });

  test("passes the secret, token, IP and idempotency key; checks the hostname", async () => {
    const { f, calls } = fakeFetch([ok({ success: true, hostname: "loams.dev" }), ok({ success: true, hostname: "evil.test" })]);
    const t = new SiteverifyTurnstile("sekret", { fetch: f, expectedHostname: "loams.dev" });
    assert.equal(await t.verify("tok", "203.0.113.1", "req_1"), true);
    assert.equal(calls[0].get("secret"), "sekret");
    assert.equal(calls[0].get("response"), "tok");
    assert.equal(calls[0].get("remoteip"), "203.0.113.1");
    assert.equal(calls[0].get("idempotency_key"), "req_1");
    assert.equal(await t.verify("tok2", "203.0.113.1", "req_2"), false);
  });

  test("a duplicate is refused; a 5xx is retried once with the same key; errors fail closed", async () => {
    const { f, calls } = fakeFetch([
      ok({ success: false, "error-codes": ["timeout-or-duplicate"] }),
      new Response("", { status: 502 }), ok({ success: true }),
      new Error("network"), new Error("network"),
    ]);
    const t = new SiteverifyTurnstile("s", { fetch: f });
    assert.equal(await t.verify("used", "", "k1"), false);
    assert.equal(await t.verify("fresh", "", "k2"), true);
    assert.equal(calls[1].get("idempotency_key"), calls[2].get("idempotency_key"));
    assert.equal(await t.verify("x", "", "k3"), false);
    assert.equal(await t.verify("", "", "k4"), false);
  });
});

describe("DNS over HTTPS", () => {
  const doh = (body: unknown, status = 200) => (async () => new Response(JSON.stringify(body), { status })) as unknown as typeof fetch;

  test("MX present, absent, null MX and NXDOMAIN", async () => {
    assert.equal(await new DohDns({ fetch: doh({ Status: 0, Answer: [{ type: 15, data: "10 mx.example.com." }] }) }).hasMx("example.com"), true);
    assert.equal(await new DohDns({ fetch: doh({ Status: 0 }) }).hasMx("example.com"), false);
    assert.equal(await new DohDns({ fetch: doh({ Status: 0, Answer: [{ type: 15, data: "0 ." }] }) }).hasMx("example.com"), false);
    assert.equal(await new DohDns({ fetch: doh({ Status: 3 }) }).hasMx("example.com"), false);
  });

  test("resolver failures follow failOpen", async () => {
    assert.equal(await new DohDns({ fetch: doh({}, 500) }).hasMx("example.com"), false);
    assert.equal(await new DohDns({ fetch: doh({}, 500), failOpen: true }).hasMx("example.com"), true);
    assert.equal(await new DohDns({ fetch: doh({ Status: 2 }), failOpen: true }).hasMx("example.com"), true);
  });
});
