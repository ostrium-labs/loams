// Google Cloud ports: items on Firestore (the (default) database) and queue on
// Cloud Tasks, both through their REST APIs with the metadata server's token, so
// the Cloud Run binary has no gRPC dependency. No retries (CB1 plan §4).
//
// Layout: <collection>/<pk>/rows/<sk>, so `list` reads one partition in key order.
import { Throttled } from "../app.mjs";

const METADATA = "http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token";

export function tokenSource(fetchImpl = fetch) {
  let token = null;
  let expires = 0;
  let pending = null;
  return async function get() {
    if (token && Date.now() < expires - 60_000) return token;
    pending ??= (async () => {
      const res = await fetchImpl(METADATA, { headers: { "Metadata-Flavor": "Google" } });
      if (!res.ok) throw new Error(`metadata token: ${res.status}`);
      const body = await res.json();
      token = body.access_token;
      expires = Date.now() + body.expires_in * 1000;
    })().finally(() => {
      pending = null;
    });
    await pending;
    return token;
  };
}

const enc = (s) => encodeURIComponent(s).replace(/\./g, "%2E");
const dec = (s) => decodeURIComponent(s);

async function check(res, what) {
  if (res.ok) return res;
  const text = await res.text();
  if (res.status === 429 || /RESOURCE_EXHAUSTED/.test(text)) throw new Throttled(`${what}: ${res.status}`);
  throw new Error(`${what}: ${res.status} ${text.slice(0, 200)}`);
}

export function gcpEnv(environ = process.env, fetchImpl = fetch) {
  const project = environ.LOAMS_PROJECT;
  const collection = environ.LOAMS_ITEMS;
  const queue = environ.LOAMS_QUEUE;
  const target = environ.LOAMS_TASKS_TARGET ?? "/_loams/queue";
  const token = tokenSource(fetchImpl);
  const base = `https://firestore.googleapis.com/v1/projects/${project}/databases/(default)/documents`;
  const doc = (pk, sk) => `${base}/${collection}/${enc(pk)}/rows/${enc(sk)}`;
  const auth = async () => ({ Authorization: `Bearer ${await token()}` });

  return {
    target: "gcp",
    items: {
      async get({ pk, sk }) {
        const res = await fetchImpl(doc(pk, sk), { headers: await auth() });
        if (res.status === 404) return null;
        await check(res, "firestore get");
        const body = await res.json();
        return { key: { pk, sk }, value: JSON.parse(body.fields.v.stringValue), version: body.updateTime };
      },
      async put({ pk, sk }, value) {
        const res = await fetchImpl(doc(pk, sk), {
          method: "PATCH",
          headers: { ...(await auth()), "content-type": "application/json" },
          body: JSON.stringify({ fields: { v: { stringValue: JSON.stringify(value) } } }),
        });
        await check(res, "firestore put");
        const body = await res.json();
        return { version: body.updateTime };
      },
      async list(pk, { after, limit = 100 } = {}) {
        const url = new URL(`${base}/${collection}/${enc(pk)}/rows`);
        url.searchParams.set("pageSize", String(limit));
        url.searchParams.set("orderBy", "__name__");
        if (after) url.searchParams.set("pageToken", after);
        const res = await fetchImpl(url, { headers: await auth() });
        await check(res, "firestore list");
        const body = await res.json();
        const items = (body.documents ?? []).map((d) => ({
          key: { pk, sk: dec(d.name.split("/").at(-1)) },
          value: JSON.parse(d.fields.v.stringValue),
          version: d.updateTime,
        }));
        return { items, next: body.nextPageToken || undefined };
      },
    },
    queue: {
      // The queue's own http_target sets the host and OIDC token; the task names
      // the app's own origin so the request is also correct without the override.
      async send(_queue, event, { origin } = {}) {
        const res = await fetchImpl(`https://cloudtasks.googleapis.com/v2/${queue}/tasks`, {
          method: "POST",
          headers: { ...(await auth()), "content-type": "application/json" },
          body: JSON.stringify({
            task: {
              httpRequest: {
                httpMethod: "POST",
                url: `${origin}${target}`,
                headers: { "content-type": "application/json" },
                body: Buffer.from(JSON.stringify(event)).toString("base64"),
              },
            },
          }),
        });
        await check(res, "cloud tasks create");
      },
    },
  };
}
