// The CB1 bench app: design 71's portable contract (D1391) with only what the
// benchmark measures. `fetch` and `queue` take an env whose `items` and `queue`
// ports are the cloud's adapter (src/adapters/*), so the same code runs on every
// target. Not @loams/app (AB0); a fixture with its shape.

const loadedAt = Date.now();
const instance = globalThis.crypto.randomUUID();
let served = 0;

export class Throttled extends Error {
  constructor(message) {
    super(message);
    this.name = "Throttled";
  }
}

const PK = "bench";
const KEY = /^[A-Za-z0-9_.-]{1,128}$/;
const json = (body, status = 200) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

function quantile(sorted, q) {
  if (sorted.length === 0) return null;
  const i = Math.min(sorted.length - 1, Math.max(0, Math.ceil(q * sorted.length) - 1));
  return sorted[i];
}

async function route(request, env, ctx) {
  const url = new URL(request.url);
  const path = url.pathname;
  const method = request.method;

  if (path === "/ping" && method === "GET") {
    return json({ ok: true, target: env.target, instance, first: served === 1, uptime_ms: Date.now() - loadedAt });
  }

  const item = path.match(/^\/items\/([^/]+)$/);
  if (item) {
    const sk = decodeURIComponent(item[1]);
    if (!KEY.test(sk)) return json({ error: "bad key" }, 400);
    if (method === "GET") {
      const got = await env.items.get({ pk: PK, sk }, { consistent: true });
      return got ? json({ key: sk, version: got.version, value: got.value }) : json({ error: "not found" }, 404);
    }
    if (method === "PUT") {
      const text = await request.text();
      if (text.length > 256 * 1024) return json({ error: "value too large" }, 413);
      const { version } = await env.items.put({ pk: PK, sk }, { body: text });
      return json({ key: sk, version });
    }
    return json({ error: "method not allowed" }, 405);
  }

  if (path === "/enqueue" && method === "POST") {
    const run = url.searchParams.get("run") ?? "default";
    if (!KEY.test(run)) return json({ error: "bad run" }, 400);
    const event = {
      specversion: "1.0",
      id: globalThis.crypto.randomUUID(),
      source: "loams-bench",
      type: "dev.loams.bench.enqueued",
      time: new Date().toISOString(),
      data: { run, sent_ms: Date.now(), pad: "x".repeat(64) },
    };
    await env.queue.send("bench", event, { origin: url.origin });
    return json({ id: event.id }, 202);
  }

  if (path === "/queue/stats" && method === "GET") {
    const run = url.searchParams.get("run") ?? "default";
    if (!KEY.test(run)) return json({ error: "bad run" }, 400);
    const lags = [];
    let after;
    do {
      const page = await env.items.list(`q#${run}`, { after, limit: 100 });
      for (const it of page.items) lags.push(Number(JSON.parse(it.value.body).lag_ms));
      after = page.next;
    } while (after);
    lags.sort((a, b) => a - b);
    return json({
      run,
      received: lags.length,
      lag_ms: { p50: quantile(lags, 0.5), p95: quantile(lags, 0.95), p99: quantile(lags, 0.99), max: lags.at(-1) ?? null },
    });
  }

  return json({ error: "not found" }, 404);
}

export default {
  async fetch(request, env, ctx) {
    served += 1;
    try {
      return await route(request, env, ctx);
    } catch (err) {
      if (err instanceof Throttled || err?.name === "Throttled") return json({ error: "throttled", detail: String(err.message) }, 429);
      console.error("bench app error", err);
      return json({ error: "internal", detail: String(err?.message ?? err) }, 500);
    }
  },

  // Each message is a CloudEvent from /enqueue; the consumer records its delivery
  // lag as an item, so /queue/stats can report it. Throttles retry the message.
  async queue(batch, env, ctx) {
    for (const msg of batch.messages) {
      const event = msg.body;
      try {
        const lag = Date.now() - Number(event.data.sent_ms);
        await env.items.put({ pk: `q#${event.data.run}`, sk: event.id }, { body: JSON.stringify({ lag_ms: lag }) });
        msg.ack();
      } catch (err) {
        console.error("bench queue error", err);
        msg.retry();
      }
    }
  },
};
