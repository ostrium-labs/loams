// Shared helpers for the local driver and the tests (Node 26, type stripping).
// LC0_URL is the workers.dev URL; LC0_BEARER_FILE names a 0600 file holding the bearer secret.
import { readFileSync } from "node:fs";

export const BASE = process.env.LC0_URL ?? "";
const BEARER = process.env.LC0_BEARER_FILE ? readFileSync(process.env.LC0_BEARER_FILE, "utf8").trim() : "";
if (!BASE || !BEARER) throw new Error("set LC0_URL and LC0_BEARER_FILE");

export const counters = { http: 0, ws_upgrades: 0, ws_sent: 0, ws_received: 0, retries: 0 };

export function authHeaders(extra: Record<string, string> = {}) {
  return { authorization: `Bearer ${BEARER}`, ...extra };
}

export async function call(method: string, path: string, body?: unknown, opts: { timeoutMs?: number; bearer?: string } = {}) {
  counters.http++;
  const res = await fetch(BASE + path, {
    method,
    headers: { ...authHeaders({ "content-type": "application/json" }), ...(opts.bearer !== undefined ? { authorization: opts.bearer } : {}) },
    body: body === undefined ? undefined : JSON.stringify(body),
    signal: AbortSignal.timeout(opts.timeoutMs ?? 30_000),
  });
  const text = await res.text();
  let data: any = text;
  try {
    data = JSON.parse(text);
  } catch {}
  return { status: res.status, data };
}

export function sleep(ms: number) {
  return new Promise((r) => setTimeout(r, ms));
}

export function pct(xs: number[], p: number) {
  if (!xs.length) return NaN;
  const s = [...xs].sort((a, b) => a - b);
  return s[Math.min(s.length - 1, Math.ceil((p / 100) * s.length) - 1)];
}

// Retries with the same idempotency key on network errors, timeouts and 5xx; exponential backoff
// with jitter. Never retries 4xx (bad auth, conflicting payload).
export async function withRetries<T>(fn: (attempt: number) => Promise<{ status: number; data: T }>, max = 5) {
  let last: any;
  for (let attempt = 0; attempt < max; attempt++) {
    try {
      const r = await fn(attempt);
      if (r.status < 500) return { ...r, attempts: attempt + 1 };
      last = r;
    } catch (e) {
      last = { status: 0, data: String((e as Error).name) };
    }
    counters.retries++;
    await sleep(Math.min(4000, 100 * 2 ** attempt) * (0.5 + Math.random()));
  }
  return { ...last, attempts: max };
}

// A simulated device: one WebSocket to its user's Durable Object.
export class Device {
  ws: WebSocket | null = null;
  asleep = false;
  pending = new Map<string, (m: any) => void>();
  wakes = new Set<string>();
  pongs = 0;
  onPong?: (m: any) => void;
  variant: string;
  user: string;
  id: string;
  constructor(variant: string, user: string, id: string) {
    this.variant = variant;
    this.user = user;
    this.id = id;
  }

  hint = process.env.LC0_HINT ?? "enam";
  url() {
    return BASE.replace(/^http/, "ws") + `/u/${this.variant}/${this.user}/ws?device=${this.id}&hint=${this.hint}`;
  }

  connect(): Promise<void> {
    counters.ws_upgrades++;
    return new Promise((resolve, reject) => {
      const ws = new (WebSocket as any)(this.url(), { headers: authHeaders() }) as WebSocket;
      this.ws = ws;
      ws.onopen = () => resolve();
      ws.onerror = (e) => reject(new Error("ws error " + String((e as any).message ?? "")));
      ws.onmessage = (ev) => {
        if (this.asleep) return; // a sleeping laptop processes nothing
        counters.ws_received++;
        const d = String(ev.data);
        if (d === "pong") {
          this.pongs++;
          this.onPong?.({ t: "pong" });
          return;
        }
        const m = JSON.parse(d);
        if (m.wake) this.wakes.add(m.wake);
        if (m.t === "pong") {
          this.pongs++;
          this.onPong?.(m);
        }
        if (m.id && this.pending.has(m.id)) {
          this.pending.get(m.id)!(m);
          this.pending.delete(m.id);
        }
      };
    });
  }

  // Real devices retry a failed upgrade; about 1 in 400 upgrades failed in this spike.
  async connectWithRetry(max = 5) {
    for (let i = 0; ; i++) {
      try {
        return await this.connect();
      } catch (e) {
        counters.retries++;
        if (i + 1 >= max) throw e;
        await sleep(200 * 2 ** i);
      }
    }
  }

  send(m: unknown) {
    counters.ws_sent++;
    this.ws!.send(typeof m === "string" ? m : JSON.stringify(m));
  }

  // A half-dead TCP path (seen here on IPv6: the socket stays OPEN while every frame is
  // retransmitted into the void) is only detectable by a missing pong. No pong in 10 s: the
  // device drops the socket and reconnects.
  deadSockets = 0;
  onDead?: () => void;
  ping(seq: number) {
    if (this.asleep || !this.ws || this.ws.readyState !== 1) return;
    const before = this.pongs;
    const ws = this.ws;
    this.send(this.variant === "auto" ? "ping" : { t: "ping", seq });
    setTimeout(() => {
      if (this.pongs === before && this.ws === ws && !this.asleep) this.kill();
    }, 10_000);
  }

  kill() {
    this.deadSockets++;
    const ws = this.ws;
    this.ws = null;
    if (ws) {
      ws.onmessage = ws.onclose = ws.onerror = null;
      try {
        ws.close();
      } catch {}
    }
    this.onDead?.();
  }

  request(m: Record<string, unknown>, timeoutMs = 15_000): Promise<any> {
    const id = Math.random().toString(36).slice(2);
    return new Promise((resolve, reject) => {
      const t = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error("timeout"));
      }, timeoutMs);
      this.pending.set(id, (r) => {
        clearTimeout(t);
        resolve(r);
      });
      this.send({ ...m, id });
    });
  }

  claim(run: string) {
    return this.request({ t: "claim", run, user: this.user });
  }

  close() {
    try {
      this.ws?.close();
    } catch {}
  }
}
