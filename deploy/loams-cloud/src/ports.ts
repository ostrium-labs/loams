// The control plane's outside world, as ports. The Worker wires the real ones
// (src/worker.ts, later); tests wire the doubles in test/harness/fakes.ts.

/** Cloudflare Turnstile's Siteverify (§25.7). Tokens are single use. */
export interface Turnstile {
  verify(token: string, remoteIp: string, idempotencyKey: string): Promise<boolean>;
}

/**
 * Fixed-window counters (§25.7). In the Worker, a Durable Object, because the
 * rate-limiting binding counts per location (R-LC2a-9) and the limits here are
 * global. `hit` counts one and says whether the caller is still within `limit`.
 */
export interface RateLimiter {
  hit(key: string, limit: number, windowMs: number): Promise<{ ok: boolean; retryAfterMs: number }>;
}

export type Template = "confirm" | "already_listed" | "removal" | "still_interested";

export interface OutgoingMail {
  template: Template;
  to: string;
  /** Links whose secrets travel in the URL fragment (D1740). */
  confirmUrl?: string;
  removeUrl: string;
}

/**
 * The Worker's own email: Resend over fetch, through the shared daily pacer
 * (LC2a Task 2, which waits on the owner's sending-only Resend key, R-LC2a-2).
 * Until then the Worker has no mailer, and tests use a recording fake.
 */
export interface Mailer {
  send(mail: OutgoingMail): Promise<void>;
}

export interface Clock {
  now(): number;
}

export const systemClock: Clock = { now: () => Date.now() };

/** Structured logs. Every line goes through redact() first (log.ts). */
export interface Logger {
  log(event: string, fields?: Record<string, unknown>): void;
}

/** Work that runs after the response, so response timing doesn't depend on address state (§25.7). */
export interface WaitUntil {
  waitUntil(p: Promise<unknown>): void;
}
