// Idempotency keys under retries, and behaviour on transient errors. Writes results/errors.json.
import { writeFileSync } from "node:fs";
import { BASE, authHeaders, call, counters, withRetries } from "./lib.ts";

const user = `t-idem-${Math.random().toString(36).slice(2, 8)}`;
const out: any = { user };

// The device side's retrying POST: same idempotency key on every attempt.
async function postIntent(body: any, inject: (attempt: number) => { failAfterWrite?: boolean; abortMs?: number }) {
  return withRetries(async (attempt) => {
    const i = inject(attempt);
    counters.http++;
    const res = await fetch(BASE + "/intents" + (i.failAfterWrite ? "?fail_after_write=1" : ""), {
      method: "POST",
      headers: authHeaders({ "content-type": "application/json" }),
      body: JSON.stringify(body),
      signal: AbortSignal.timeout(i.abortMs ?? 30_000), // abortMs: the network dies mid-request
    });
    return { status: res.status, data: await res.json() };
  });
}

const N = 60;
const modes = ["clean", "lost-response", "abort-mid-request", "concurrent-dup"];
const results: any[] = [];
for (let k = 0; k < N; k++) {
  const mode = modes[k % modes.length];
  const body = { idem: `${user}-${k}`, user, run: `run-${k}`, target: "homelab" };
  if (mode === "concurrent-dup") {
    const [a, b] = await Promise.all([postIntent(body, () => ({})), postIntent(body, () => ({}))]);
    results.push({ mode, statuses: [a.status, b.status], created: [a.data.created, b.data.created], attempts: a.attempts + b.attempts });
    continue;
  }
  const abortMs = 50 + Math.floor(Math.random() * 300);
  const r = await postIntent(body, (attempt) =>
    attempt > 0 ? {} : mode === "lost-response" ? { failAfterWrite: true } : mode === "abort-mid-request" ? { abortMs } : {},
  );
  results.push({ mode, status: r.status, created: r.data?.created, attempts: r.attempts, abortMs: mode === "abort-mid-request" ? abortMs : undefined });
}
await new Promise((r) => setTimeout(r, 3000));
const count = await call("GET", `/intents?user=${user}`);
const conflict = await call("POST", "/intents", { idem: `${user}-0`, user, run: "a-different-run", target: "laptop" });
out.idempotency = {
  keys: N,
  rows_in_tidb: Number(count.data.c),
  distinct_runs: Number(count.data.runs),
  duplicate_suppression_correct: Number(count.data.c) === N,
  by_mode: Object.fromEntries(modes.map((m) => {
    const rs = results.filter((r) => r.mode === m);
    return [m, { n: rs.length, attempts: rs.reduce((s, r) => s + r.attempts, 0), first_reply_said_created: rs.filter((r) => r.created === true || (Array.isArray(r.created) && r.created.includes(true))).length, samples: rs.slice(0, 3) }];
  })),
  same_key_different_payload: conflict.status,
};
console.log(JSON.stringify(out.idempotency, null, 1));

// Transient errors seen by the device side.
out.device_side = {
  bad_bearer: (await call("GET", "/health", undefined, { bearer: "Bearer wrong" })).status,
  no_bearer: (await fetch(BASE + "/health")).status,
  timeout_100ms: await call("GET", "/bench?kind=noop&n=1", undefined, { timeoutMs: 100 }).then((r) => r.status, (e) => (e as Error).name),
  unreachable_host: await fetch("https://loams-cloud-lc0-nonexistent.invalid/health", { signal: AbortSignal.timeout(5000) }).then((r) => r.status, (e) => `${(e as Error).name}: ${(e as any).cause?.code ?? ""}`),
};
console.log(JSON.stringify(out.device_side));

// Errors from @tidbcloud/serverless inside the Worker.
out.worker_side = {};
for (const kind of ["badcreds", "badhost", "timeout", "slow", "syntax", "abort-write"]) {
  out.worker_side[kind] = (await call("GET", `/dberr?kind=${kind}`, undefined, { timeoutMs: 120_000 })).data;
  console.log(kind, JSON.stringify(out.worker_side[kind]));
}
out.counters = counters;
writeFileSync("results/errors.json", JSON.stringify(out, null, 2));
