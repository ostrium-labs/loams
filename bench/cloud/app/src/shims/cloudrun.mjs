// Cloud Run entry shim (design 79 D1791: a Bun binary compiled ahead of time).
// Requests go to `fetch`; Cloud Tasks pushes to /_loams/queue go to `queue` (2xx
// acks, 5xx retries). Request-based billing gives CPU only while a request is in
// flight, so `ctx.waitUntil` promises are awaited before responding (§5.1).
import app from "../app.mjs";
import { gcpEnv } from "../adapters/gcp.mjs";

const env = gcpEnv();
const queuePath = process.env.LOAMS_TASKS_TARGET ?? "/_loams/queue";

async function handle(request) {
  const pending = [];
  const ctx = { waitUntil: (p) => pending.push(p), passThroughOnException() {} };
  try {
    const url = new URL(request.url);
    if (url.pathname === queuePath && request.method === "POST") {
      // A fixture check, not authentication: an HTTP-target header can be forged.
      // A real app (AB1) verifies the task's OIDC token or keeps the service private.
      if (!request.headers.get("x-cloudtasks-queuename")) return new Response("forbidden", { status: 403 });
      let ok = true;
      const msg = {
        id: request.headers.get("x-cloudtasks-taskname"),
        body: await request.json(),
        attempts: Number(request.headers.get("x-cloudtasks-taskretrycount") ?? 0) + 1,
        ack() {},
        retry() {
          ok = false;
        },
      };
      await app.queue({ queue: "bench", messages: [msg] }, env, ctx);
      return new Response(ok ? "ok" : "retry", { status: ok ? 200 : 500 });
    }
    if (url.pathname === "/_loams/scheduled" && request.method === "POST" && app.scheduled) {
      await app.scheduled({ scheduledTime: Date.now() }, env, ctx);
      return new Response("ok");
    }
    return await app.fetch(request, env, ctx);
  } finally {
    await Promise.allSettled(pending);
  }
}

Bun.serve({ port: Number(process.env.PORT ?? 8080), hostname: "0.0.0.0", fetch: handle });

// Cloud Run sends SIGTERM before stopping an instance; Bun as PID 1 has no default handler.
process.on("SIGTERM", () => process.exit(0));
