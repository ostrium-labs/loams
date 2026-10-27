// Azure ports: items on Cosmos DB for NoSQL and queue on Queue Storage, through
// their REST APIs with the app's user-assigned managed identity (Cosmos DB keys
// and storage shared keys are off in the module). No retries (CB1 plan §4).
import { Throttled } from "../app.mjs";

export function identityToken(resource, environ = process.env, fetchImpl = fetch) {
  let token = null;
  let expires = 0;
  let pending = null;
  return async function get() {
    if (token && Date.now() < expires - 60_000) return token;
    pending ??= (async () => {
      const url = new URL(environ.IDENTITY_ENDPOINT);
      url.searchParams.set("resource", resource);
      url.searchParams.set("api-version", "2019-08-01");
      url.searchParams.set("client_id", environ.LOAMS_IDENTITY_CLIENT_ID);
      const res = await fetchImpl(url, { headers: { "X-IDENTITY-HEADER": environ.IDENTITY_HEADER } });
      if (!res.ok) throw new Error(`identity token for ${resource}: ${res.status}`);
      const body = await res.json();
      token = body.access_token;
      expires = Number(body.expires_on) * 1000;
    })().finally(() => {
      pending = null;
    });
    await pending;
    return token;
  };
}

// Cosmos DB ids may not contain / \ ? #.
const id = (sk) => encodeURIComponent(sk);

async function check(res, what) {
  if (res.ok) return res;
  const text = await res.text();
  if (res.status === 429) throw new Throttled(`${what}: ${res.status}`);
  throw new Error(`${what}: ${res.status} ${text.slice(0, 200)}`);
}

export function azureEnv(environ = process.env, fetchImpl = fetch) {
  const endpoint = environ.LOAMS_COSMOS_ENDPOINT.replace(/:443\/?$/, "").replace(/\/$/, "");
  const coll = `dbs/${environ.LOAMS_COSMOS_DATABASE}/colls/${environ.LOAMS_ITEMS}`;
  const cosmosToken = identityToken("https://cosmos.azure.com", environ, fetchImpl);
  const storageToken = identityToken("https://storage.azure.com", environ, fetchImpl);
  const queueUrl = `${environ.LOAMS_QUEUE_ENDPOINT.replace(/\/$/, "")}/${environ.LOAMS_QUEUE}/messages`;

  const headers = async (pk, extra = {}) => ({
    Authorization: encodeURIComponent(`type=aad&ver=1.0&sig=${await cosmosToken()}`),
    "x-ms-version": "2018-12-31",
    "x-ms-date": new Date().toUTCString(),
    "x-ms-documentdb-partitionkey": JSON.stringify([pk]),
    ...extra,
  });

  return {
    target: "azure",
    items: {
      async get({ pk, sk }) {
        const res = await fetchImpl(`${endpoint}/${coll}/docs/${id(sk)}`, { headers: await headers(pk) });
        if (res.status === 404) return null;
        await check(res, "cosmos get");
        const body = await res.json();
        return { key: { pk, sk }, value: JSON.parse(body.v), version: body._etag };
      },
      async put({ pk, sk }, value) {
        const res = await fetchImpl(`${endpoint}/${coll}/docs`, {
          method: "POST",
          headers: await headers(pk, { "content-type": "application/json", "x-ms-documentdb-is-upsert": "True" }),
          body: JSON.stringify({ id: id(sk), pk, sk, v: JSON.stringify(value) }),
        });
        await check(res, "cosmos put");
        return { version: res.headers.get("etag") };
      },
      async list(pk, { after, limit = 100 } = {}) {
        const extra = {
          "content-type": "application/query+json",
          "x-ms-documentdb-isquery": "True",
          "x-ms-max-item-count": String(limit),
        };
        if (after) extra["x-ms-continuation"] = after;
        const res = await fetchImpl(`${endpoint}/${coll}/docs`, {
          method: "POST",
          headers: await headers(pk, extra),
          body: JSON.stringify({ query: "SELECT * FROM c WHERE c.pk = @pk ORDER BY c.id", parameters: [{ name: "@pk", value: pk }] }),
        });
        await check(res, "cosmos query");
        const body = await res.json();
        const items = body.Documents.map((d) => ({ key: { pk, sk: d.sk }, value: JSON.parse(d.v), version: d._etag }));
        return { items, next: res.headers.get("x-ms-continuation") || undefined };
      },
    },
    queue: {
      // The Functions queue trigger expects base64 message text (its default).
      async send(_queue, event) {
        const text = Buffer.from(JSON.stringify(event)).toString("base64");
        const res = await fetchImpl(queueUrl, {
          method: "POST",
          headers: {
            Authorization: `Bearer ${await storageToken()}`,
            "x-ms-version": "2023-11-03",
            "x-ms-date": new Date().toUTCString(),
            "content-type": "application/xml",
          },
          body: `<QueueMessage><MessageText>${text}</MessageText></QueueMessage>`,
        });
        await check(res, "queue send");
      },
    },
  };
}
