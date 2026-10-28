// Local server for tests and the harness's dry run: the app on node:http with
// the in-memory ports; /enqueue delivers to `queue` after a short delay, as a
// cloud queue would.
import { createServer } from "node:http";
import app from "../app.mjs";
import { memoryEnv } from "../adapters/memory.mjs";

export function serve({ port = 0, throttleEvery = 0, deliverMs = 20 } = {}) {
  const env = memoryEnv({
    throttleEvery,
    onSend: async (_q, event) => {
      setTimeout(() => {
        app.queue({ queue: "bench", messages: [{ id: event.id, body: event, ack() {}, retry() {} }] }, env, {});
      }, deliverMs);
    },
  });
  const server = createServer(async (req, res) => {
    const chunks = [];
    for await (const c of req) chunks.push(c);
    const body = chunks.length && !["GET", "HEAD"].includes(req.method) ? Buffer.concat(chunks) : undefined;
    const request = new Request(`http://${req.headers.host}${req.url}`, { method: req.method, headers: req.headers, body });
    const out = await app.fetch(request, env, { waitUntil() {}, passThroughOnException() {} });
    res.writeHead(out.status, Object.fromEntries(out.headers.entries()));
    res.end(Buffer.from(await out.arrayBuffer()));
  });
  return new Promise((resolve) => server.listen(port, "127.0.0.1", () => resolve({ server, env, port: server.address().port })));
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const port = Number(process.env.PORT ?? 8787);
  const { port: bound } = await serve({ port, throttleEvery: Number(process.env.THROTTLE_EVERY ?? 0) });
  console.log(`bench app on http://127.0.0.1:${bound}`);
}
