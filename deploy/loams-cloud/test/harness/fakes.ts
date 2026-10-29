// Local doubles for everything outside the Worker: Turnstile, rate limits,
// mail, DNS, the clock and the log sink. None of them talks to a network.

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { Audit } from "../../src/audit.ts";
import { testKeys, type Keys } from "../../src/crypto.ts";
import { loadDomainLists, type DomainLists, type Dns } from "../../src/domains.ts";
import { redactingLogger } from "../../src/log.ts";
import type { Clock, Logger, Mailer, OutgoingMail, RateLimiter, Turnstile, WaitUntil } from "../../src/ports.ts";
import { migrate } from "../../src/schema.ts";
import { Waitlist } from "../../src/waitlist.ts";
import { handle, type AppDeps } from "../../src/app.ts";
import { freshDatabase, loadMigrations, schemaDir, type TidbCli } from "./tidb.ts";

export const policyDir = join(schemaDir, "..", "policy");

export function loadLists(): DomainLists {
  return loadDomainLists({
    disposable: readFileSync(join(policyDir, "disposable_email_blocklist.conf"), "utf8"),
    overrides: readFileSync(join(policyDir, "email-domains.toml"), "utf8"),
  });
}

/** Siteverify's single-use rule: a token passes once. Tokens starting "bad" never pass. */
export class FakeTurnstile implements Turnstile {
  private n = 0;
  readonly spent = new Set<string>();
  calls = 0;
  issue(): string {
    return `ts_ok_${++this.n}`;
  }
  async verify(token: string): Promise<boolean> {
    this.calls++;
    if (token.startsWith("bad") || this.spent.has(token)) return false;
    this.spent.add(token);
    return true;
  }
}

export class MemoryLimiter implements RateLimiter {
  private readonly windows = new Map<string, { start: number; n: number }>();
  readonly clock: Clock;
  constructor(clock: Clock) {
    this.clock = clock;
  }
  async hit(key: string, limit: number, windowMs: number) {
    const now = this.clock.now();
    const start = Math.floor(now / windowMs) * windowMs;
    const w = this.windows.get(key);
    const cur = w && w.start === start ? w : { start, n: 0 };
    cur.n++;
    this.windows.set(key, cur);
    return { ok: cur.n <= limit, retryAfterMs: start + windowMs - now };
  }
}

export class RecordingMailer implements Mailer {
  readonly sent: OutgoingMail[] = [];
  failNext = false;
  async send(mail: OutgoingMail): Promise<void> {
    if (this.failNext) {
      this.failNext = false;
      throw new Error("mail provider unavailable");
    }
    this.sent.push(mail);
  }
  to(addr: string): OutgoingMail[] {
    return this.sent.filter((m) => m.to === addr);
  }
}

export class FakeDns implements Dns {
  readonly noMx = new Set<string>(["no-mx.example"]);
  async hasMx(domain: string): Promise<boolean> {
    return !this.noMx.has(domain);
  }
}

export class TestClock implements Clock {
  t: number;
  constructor(t = Date.UTC(2026, 10, 1, 12, 0, 0)) {
    this.t = t;
  }
  now(): number {
    return this.t;
  }
  advance(ms: number): void {
    this.t += ms;
  }
}

export class CaptureLog {
  readonly lines: string[] = [];
  readonly logger: Logger = redactingLogger((l) => this.lines.push(l));
  text(): string {
    return this.lines.join("\n");
  }
}

export class WaitBag implements WaitUntil {
  private readonly pending: Promise<unknown>[] = [];
  waitUntil(p: Promise<unknown>): void {
    this.pending.push(p);
  }
  async drain(): Promise<void> {
    while (this.pending.length) await this.pending.shift();
  }
}

export interface World {
  sql: TidbCli;
  keys: Keys;
  clock: TestClock;
  turnstile: FakeTurnstile;
  limiter: MemoryLimiter;
  mailer: RecordingMailer;
  dns: FakeDns;
  log: CaptureLog;
  audit: Audit;
  waitlist: Waitlist;
  deps: AppDeps;
  bag: WaitBag;
  dispose: () => void;
  /** Sends a request and runs its after-response work. */
  call(method: string, path: string, body?: unknown, headers?: Record<string, string>): Promise<{ status: number; body: any; headers: Headers }>;
}

export async function world(overrides: Partial<AppDeps> = {}): Promise<World> {
  const { sql, dispose } = freshDatabase();
  await migrate(sql, loadMigrations());
  const keys = await testKeys();
  const clock = new TestClock();
  const turnstile = new FakeTurnstile();
  const limiter = new MemoryLimiter(clock);
  const mailer = new RecordingMailer();
  const dns = new FakeDns();
  const log = new CaptureLog();
  const audit = new Audit(sql, () => clock.now());
  const waitlist = new Waitlist({ sql, keys, lists: loadLists(), dns, mailer, audit, clock, logger: log.logger, siteUrl: "https://loams.dev" });
  const deps: AppDeps = { keys, waitlist, turnstile, limiter, clock, logger: log.logger, ...overrides };
  const bag = new WaitBag();
  const w: World = {
    sql, keys, clock, turnstile, limiter, mailer, dns, log, audit, waitlist, deps, bag, dispose,
    async call(method, path, body, headers = {}) {
      const init: RequestInit = { method, headers: { "content-type": "application/json", "cf-connecting-ip": "203.0.113.7", ...headers } };
      if (body !== undefined) init.body = typeof body === "string" ? body : JSON.stringify(body);
      const res = await handle(new Request(`https://api.loams.dev/cloud/v1${path}`, init), w.deps, bag);
      await bag.drain();
      const text = await res.text();
      return { status: res.status, body: text ? JSON.parse(text) : null, headers: res.headers };
    },
  };
  return w;
}

/** The token in an email link's fragment. */
export function fragment(url: string | undefined): string {
  if (!url) throw new Error("no link");
  return url.slice(url.indexOf("#") + 1);
}
