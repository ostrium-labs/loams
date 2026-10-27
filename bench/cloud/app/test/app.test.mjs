import { test } from "node:test";
import assert from "node:assert/strict";
import { serve } from "../src/shims/node.mjs";
import { fromResponse, toRequest } from "../src/shims/lambda-http.mjs";
import { gcpEnv } from "../src/adapters/gcp.mjs";
import { azureEnv } from "../src/adapters/azure.mjs";

async function withServer(opts, fn) {
  const { server, port } = await serve(opts);
  try {
    await fn(`http://127.0.0.1:${port}`);
  } finally {
    server.close();
  }
}

test("ping reports the instance's first request once", () =>
  withServer({}, async (base) => {
    const a = await (await fetch(`${base}/ping`)).json();
    const b = await (await fetch(`${base}/ping`)).json();
    assert.equal(a.ok, true);
    assert.equal(a.instance, b.instance);
    assert.equal(b.first, false);
    assert.equal(typeof a.uptime_ms, "number");
  }));

test("items round trip, 404 for a missing key, 400 for a bad key", () =>
  withServer({}, async (base) => {
    const put = await fetch(`${base}/items/k001`, { method: "PUT", body: "x".repeat(1024) });
    assert.equal(put.status, 200);
    const got = await (await fetch(`${base}/items/k001`)).json();
    assert.equal(got.value.body.length, 1024);
    assert.equal((await fetch(`${base}/items/k999`)).status, 404);
    assert.equal((await fetch(`${base}/items/a%2Fb`)).status, 400);
  }));

test("a store throttle is a 429, not a 500", () =>
  withServer({ throttleEvery: 1 }, async (base) => {
    const res = await fetch(`${base}/items/k001`);
    assert.equal(res.status, 429);
    assert.equal((await res.json()).error, "throttled");
  }));

test("enqueue is delivered and its lag is reported", () =>
  withServer({}, async (base) => {
    for (let i = 0; i < 5; i++) assert.equal((await fetch(`${base}/enqueue?run=r1`, { method: "POST" })).status, 202);
    await new Promise((r) => setTimeout(r, 150));
    const stats = await (await fetch(`${base}/queue/stats?run=r1`)).json();
    assert.equal(stats.received, 5);
    assert.ok(stats.lag_ms.p50 >= 0 && stats.lag_ms.max >= stats.lag_ms.p50);
  }));

test("lambda: Function URL event to Request and back", async () => {
  const req = toRequest({
    rawPath: "/items/k1",
    rawQueryString: "a=1",
    headers: { "content-type": "text/plain" },
    body: Buffer.from("hello").toString("base64"),
    isBase64Encoded: true,
    requestContext: { domainName: "abc.lambda-url.us-east-1.on.aws", http: { method: "PUT" } },
  });
  assert.equal(req.url, "https://abc.lambda-url.us-east-1.on.aws/items/k1?a=1");
  assert.equal(await req.text(), "hello");
  const out = await fromResponse(new Response("{}", { status: 201, headers: { "content-type": "application/json" } }));
  assert.deepEqual([out.statusCode, out.headers["content-type"], out.body], [201, "application/json", "{}"]);
});

function fakeFetch(routes) {
  const calls = [];
  const f = async (url, init = {}) => {
    const u = String(url);
    calls.push({ url: u, init });
    for (const [re, handler] of routes) if (re.test(u)) return handler(u, init);
    throw new Error(`unexpected ${u}`);
  };
  f.calls = calls;
  return f;
}

test("gcp: Firestore document paths, Cloud Tasks body, 429 is Throttled", async () => {
  let throttle = false;
  const f = fakeFetch([
    [/metadata\.google\.internal/, () => Response.json({ access_token: "t", expires_in: 3600 })],
    [/firestore\.googleapis\.com/, (u, init) =>
      throttle ? new Response("RESOURCE_EXHAUSTED", { status: 429 })
      : init.method === "PATCH" ? Response.json({ updateTime: "2026-10-27T00:00:00Z" })
      : Response.json({ fields: { v: { stringValue: JSON.stringify({ body: "b" }) } }, updateTime: "u" })],
    [/cloudtasks\.googleapis\.com/, () => Response.json({ name: "task" })],
  ]);
  const env = gcpEnv({ LOAMS_PROJECT: "p", LOAMS_ITEMS: "loams-t-items", LOAMS_QUEUE: "projects/p/locations/us-central1/queues/q" }, f);
  await env.items.put({ pk: "bench", sk: "k.1" }, { body: "b" });
  assert.match(f.calls.at(-1).url, /documents\/loams-t-items\/bench\/rows\/k%2E1$/);
  assert.equal((await env.items.get({ pk: "bench", sk: "k.1" })).value.body, "b");
  await env.queue.send("bench", { id: "e1" }, { origin: "https://svc.run.app" });
  const task = JSON.parse(f.calls.at(-1).init.body).task.httpRequest;
  assert.equal(task.url, "https://svc.run.app/_loams/queue");
  assert.equal(JSON.parse(Buffer.from(task.body, "base64")).id, "e1");
  throttle = true;
  await assert.rejects(env.items.get({ pk: "bench", sk: "k.1" }), { name: "Throttled" });
});

test("azure: Cosmos DB AAD header and partition key, base64 queue message", async () => {
  const f = fakeFetch([
    [/identity/, () => Response.json({ access_token: "tok", expires_on: String(Math.floor(Date.now() / 1000) + 3600) })],
    [/documents\.azure\.com/, (u, init) => (init.method === "POST" ? new Response("{}", { status: 201, headers: { etag: "e" } }) : new Response("", { status: 429 }))],
    [/queue\.core\.windows\.net/, () => new Response("", { status: 201 })],
  ]);
  const env = azureEnv({
    IDENTITY_ENDPOINT: "http://localhost:4141/identity",
    IDENTITY_HEADER: "h",
    LOAMS_IDENTITY_CLIENT_ID: "c",
    LOAMS_COSMOS_ENDPOINT: "https://acct.documents.azure.com:443/",
    LOAMS_COSMOS_DATABASE: "loams",
    LOAMS_ITEMS: "loams-t-items",
    LOAMS_QUEUE_ENDPOINT: "https://acct.queue.core.windows.net/",
    LOAMS_QUEUE: "loams-t-queue",
  }, f);
  assert.deepEqual(await env.items.put({ pk: "bench", sk: "k1" }, { body: "b" }), { version: "e" });
  const put = f.calls.at(-1);
  assert.equal(put.url, "https://acct.documents.azure.com/dbs/loams/colls/loams-t-items/docs");
  assert.equal(put.init.headers.Authorization, encodeURIComponent("type=aad&ver=1.0&sig=tok"));
  assert.equal(put.init.headers["x-ms-documentdb-partitionkey"], '["bench"]');
  await assert.rejects(env.items.get({ pk: "bench", sk: "k1" }), { name: "Throttled" });
  await env.queue.send("bench", { id: "e1" });
  const text = f.calls.at(-1).init.body.match(/<MessageText>(.*)<\/MessageText>/)[1];
  assert.equal(JSON.parse(Buffer.from(text, "base64")).id, "e1");
});
