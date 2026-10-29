// Cloudflare Turnstile Siteverify (§25.7; R-LC2a-9: tokens are single use, a
// replay returns `timeout-or-duplicate`, tokens live 300 s, and
// `idempotency_key` makes a retried verification safe).

import type { Turnstile } from "./ports.ts";

export const SITEVERIFY = "https://challenges.cloudflare.com/turnstile/v0/siteverify";

export class SiteverifyTurnstile implements Turnstile {
  private readonly secret: string;
  private readonly fetchFn: typeof fetch;
  private readonly expectedHostname?: string;
  constructor(secret: string, opts: { fetch?: typeof fetch; expectedHostname?: string } = {}) {
    this.secret = secret;
    this.fetchFn = opts.fetch ?? fetch;
    this.expectedHostname = opts.expectedHostname;
  }

  async verify(token: string, remoteIp: string, idempotencyKey: string): Promise<boolean> {
    if (!token || token.length > 2048) return false;
    const form = new FormData();
    form.set("secret", this.secret);
    form.set("response", token);
    if (remoteIp && remoteIp !== "unknown") form.set("remoteip", remoteIp);
    form.set("idempotency_key", idempotencyKey);
    // One retry, with the same idempotency key, on a network error or a 5xx.
    for (let attempt = 0; attempt < 2; attempt++) {
      try {
        const r = await this.fetchFn(SITEVERIFY, { method: "POST", body: form, signal: AbortSignal.timeout(5000) });
        if (r.status >= 500) continue;
        if (!r.ok) return false;
        const body = (await r.json()) as { success: boolean; hostname?: string };
        if (!body.success) return false;
        if (this.expectedHostname && body.hostname !== this.expectedHostname) return false;
        return true;
      } catch {
        // retry once
      }
    }
    return false; // fail closed
  }
}
