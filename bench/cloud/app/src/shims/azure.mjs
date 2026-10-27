// Azure Functions entry shim (Node, programming model v4): one HTTP trigger for
// every route (host.json sets routePrefix to ""), and the storage queue trigger
// to `queue` (a thrown error retries the message; after maxDequeueCount it moves
// to <queue>-poison).
import { app as functions } from "@azure/functions";
import app from "../app.mjs";
import { azureEnv } from "../adapters/azure.mjs";

const env = azureEnv();

function context() {
  const pending = [];
  return { ctx: { waitUntil: (p) => pending.push(p), passThroughOnException() {} }, settle: () => Promise.allSettled(pending) };
}

functions.http("fetch", {
  route: "{*path}",
  methods: ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"],
  authLevel: "anonymous",
  handler: async (req) => {
    const { ctx, settle } = context();
    try {
      const hasBody = !["GET", "HEAD"].includes(req.method);
      const request = new Request(req.url, {
        method: req.method,
        headers: Object.fromEntries(req.headers.entries()),
        body: hasBody ? Buffer.from(await req.arrayBuffer()) : undefined,
      });
      const res = await app.fetch(request, env, ctx);
      return { status: res.status, headers: Object.fromEntries(res.headers.entries()), body: await res.text() };
    } finally {
      await settle();
    }
  },
});

functions.storageQueue("queue", {
  queueName: process.env.LOAMS_QUEUE || "unused",
  connection: "LoamsQueue",
  handler: async (message, invocation) => {
    const { ctx, settle } = context();
    let ok = true;
    const msg = {
      id: invocation.triggerMetadata?.id,
      body: typeof message === "string" ? JSON.parse(message) : message,
      attempts: Number(invocation.triggerMetadata?.dequeueCount ?? 1),
      ack() {},
      retry() {
        ok = false;
      },
    };
    try {
      await app.queue({ queue: "bench", messages: [msg] }, env, ctx);
    } finally {
      await settle();
    }
    if (!ok) throw new Error("retry");
  },
});
