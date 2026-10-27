// @qdrant/js-client-rest smoke against a running Loams Qdrant gateway
// (plan MA1 Task 25, design §66). One function per check; each check makes
// fresh collections named `smoke_js_<check>_<uuid>` and drops them after.
// The checks cover every client call `@mastra/qdrant` 1.1.4 makes.
//
//   node smoke.mjs --version <1.18|1.19> [--url http://127.0.0.1:6333]
//
// With LOAMS_QDRANT_API_KEY set, every client sends it as `api-key`. The
// last line is `qdrant-js <version>: <n> checks passed`; any failure exits 1.

import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { parseArgs } from 'node:util';

const { values: args } = parseArgs({
  options: {
    version: { type: 'string' },
    url: { type: 'string', default: 'http://127.0.0.1:6333' },
  },
});
const version = args.version;
if (version !== '1.18' && version !== '1.19') {
  console.error('usage: node smoke.mjs --version <1.18|1.19> [--url <url>]');
  process.exit(2);
}
const { QdrantClient } = await import(`qdrant-js-${version}`);
// The package's `exports` leave out package.json, so read it from disk.
const pkg = JSON.parse(
  readFileSync(new URL(`node_modules/qdrant-js-${version}/package.json`, import.meta.url), 'utf8'),
);
const apiKey = process.env.LOAMS_QDRANT_API_KEY || undefined;

/** A client like `@mastra/qdrant` builds one: `new QdrantClient(params)`. */
function client(extra = {}) {
  return new QdrantClient({ url: args.url, apiKey, ...extra });
}

const CHECKS = [];
function check(fn) {
  CHECKS.push(fn);
}

/** A per-check context: a client and the collections it made. */
class Ctx {
  constructor(name) {
    this.check = name;
    this.client = client({ checkCompatibility: false });
    this.made = [];
  }

  name(suffix = '') {
    const n = `smoke_js_${this.check}${suffix}_${randomUUID().replaceAll('-', '')}`;
    this.made.push(n);
    return n;
  }

  /** A collection of one unnamed 3-dimensional Cosine vector. */
  async collection(suffix = '') {
    const name = this.name(suffix);
    assert.equal(
      await this.client.createCollection(name, { vectors: { size: 3, distance: 'Cosine' } }),
      true,
    );
    return name;
  }

  async close() {
    for (const n of this.made) {
      await this.client.deleteCollection(n).catch(() => {});
    }
  }
}

const ids = (points) => points.map((p) => p.id).sort((a, b) => String(a).localeCompare(String(b)));

// ----- the checks -----

check(async function server_version() {
  // The client checks the server version in its constructor and warns
  // when it is incompatible; it must not warn.
  const warnings = [];
  const warn = console.warn;
  console.warn = (...m) => warnings.push(m.join(' '));
  try {
    const c = client();
    const info = await c.versionInfo();
    await new Promise((r) => setTimeout(r, 200));
    assert.match(info.version, /^1\.\d+\.\d+$/);
    const [major, minor] = info.version.split('.').map(Number);
    const [cMajor, cMinor] = pkg.version.split('.').map(Number);
    assert.equal(major, cMajor);
    assert.ok(Math.abs(minor - cMinor) <= 1, `${pkg.version} vs ${info.version}`);
  } finally {
    console.warn = warn;
  }
  const compat = warnings.filter((w) => /incompatible|server version/i.test(w));
  assert.deepEqual(compat, []);
});

check(async function collections(ctx) {
  const name = await ctx.collection();
  const named = ctx.name('_named');
  assert.equal(
    await ctx.client.createCollection(named, {
      vectors: { text: { size: 3, distance: 'Dot' }, img: { size: 2, distance: 'Euclid' } },
    }),
    true,
  );
  const listed = (await ctx.client.getCollections()).collections.map((c) => c.name);
  assert.ok(listed.includes(name) && listed.includes(named), listed.join(','));
  const info = await ctx.client.getCollection(name);
  assert.deepEqual(
    [info.config.params.vectors.size, info.config.params.vectors.distance],
    [3, 'Cosine'],
  );
  assert.equal(info.points_count, 0);
  const vectors = (await ctx.client.getCollection(named)).config.params.vectors;
  assert.deepEqual(
    [vectors.text.size, vectors.text.distance, vectors.img.size, vectors.img.distance],
    [3, 'Dot', 2, 'Euclid'],
  );
  assert.deepEqual(await ctx.client.collectionExists(name), { exists: true });
  assert.equal(await ctx.client.deleteCollection(named), true);
  assert.deepEqual(await ctx.client.collectionExists(named), { exists: false });
  // Mastra's createIndex treats a 409 as "exists": the status must be 409.
  await assert.rejects(
    ctx.client.createCollection(name, { vectors: { size: 3, distance: 'Cosine' } }),
    (err) => err.status === 409,
  );
});

check(async function upsert_and_query(ctx) {
  const name = await ctx.collection();
  const uuid = randomUUID();
  await ctx.client.upsert(name, {
    wait: true,
    points: [
      { id: uuid, vector: [1, 0, 0], payload: { kind: 'a', n: 1 } },
      { id: 7, vector: [0, 1, 0], payload: { kind: 'b', n: 2 } },
      { id: 8, vector: [0.9, 0.1, 0], payload: { kind: 'a', n: 3 } },
    ],
  });
  assert.equal((await ctx.client.getCollection(name)).points_count, 3);
  const all = await ctx.client.query(name, {
    query: [1, 0, 0],
    limit: 10,
    with_payload: true,
    with_vector: true,
  });
  assert.equal(all.points[0].id, uuid);
  assert.deepEqual(all.points[0].payload, { kind: 'a', n: 1 });
  assert.equal(all.points[0].vector.length, 3);
  const filtered = await ctx.client.query(name, {
    query: [1, 0, 0],
    filter: { must: [{ key: 'kind', match: { value: 'a' } }, { key: 'n', range: { gt: 1 } }] },
    limit: 10,
    with_payload: true,
    with_vector: false,
  });
  assert.deepEqual(ids(filtered.points), [8]);
  assert.equal(filtered.points[0].vector, undefined);
  // `using` names a vector of a named-vector collection.
  const named = ctx.name('_named');
  await ctx.client.createCollection(named, {
    vectors: { text: { size: 3, distance: 'Dot' }, img: { size: 2, distance: 'Dot' } },
  });
  await ctx.client.upsert(named, {
    wait: true,
    points: [
      { id: 1, vector: { text: [1, 0, 0], img: [0, 1] } },
      { id: 2, vector: { text: [0, 1, 0], img: [1, 0] } },
    ],
  });
  const byImg = await ctx.client.query(named, { query: [1, 0], using: 'img', limit: 1 });
  assert.equal(byImg.points[0].id, 2);
});

check(async function scroll_and_payload(ctx) {
  const name = await ctx.collection();
  await ctx.client.upsert(name, {
    wait: true,
    points: [1, 2, 3, 4].map((id) => ({ id, vector: [1, id, 0], payload: { g: id % 2 } })),
  });
  const page = await ctx.client.scroll(name, {
    filter: { must: [{ key: 'g', match: { value: 0 } }] },
    limit: 10,
    with_payload: true,
    with_vector: false,
  });
  assert.deepEqual(ids(page.points), [2, 4]);
  assert.equal(page.next_page_offset ?? null, null);
  // By ids, then by filter.
  await ctx.client.setPayload(name, { payload: { tag: 'one' }, points: [1], wait: true });
  await ctx.client.setPayload(name, {
    payload: { tag: 'even' },
    filter: { must: [{ key: 'g', match: { value: 0 } }] },
    wait: true,
  });
  const tagged = await ctx.client.scroll(name, { limit: 10, with_payload: true });
  const tags = Object.fromEntries(tagged.points.map((p) => [p.id, p.payload.tag ?? null]));
  assert.deepEqual(tags, { 1: 'one', 2: 'even', 3: null, 4: 'even' });
});

check(async function update_vectors(ctx) {
  const name = await ctx.collection();
  await ctx.client.upsert(name, { wait: true, points: [{ id: 1, vector: [1, 0, 0] }] });
  await ctx.client.updateVectors(name, { points: [{ id: 1, vector: [0, 0, 1] }], wait: true });
  const hit = await ctx.client.query(name, { query: [0, 0, 1], limit: 1, with_vector: true });
  assert.equal(hit.points[0].id, 1);
  assert.ok(hit.points[0].score > 0.99, hit.points[0].score);
});

check(async function delete_points(ctx) {
  const name = await ctx.collection();
  await ctx.client.upsert(name, {
    wait: true,
    points: [1, 2, 3, 4].map((id) => ({ id, vector: [1, id, 0], payload: { g: id % 2 } })),
  });
  await ctx.client.delete(name, { points: [1], wait: true });
  await ctx.client.delete(name, {
    filter: { must: [{ key: 'g', match: { value: 0 } }] },
    wait: true,
  });
  const left = await ctx.client.scroll(name, { limit: 10 });
  assert.deepEqual(ids(left.points), [3]);
});

check(async function payload_indexes(ctx) {
  const name = await ctx.collection();
  // Every type `@mastra/qdrant` sends except `geo` (#898).
  const types = ['keyword', 'integer', 'float', 'text', 'bool', 'datetime', 'uuid'];
  for (const type of types) {
    const result = await ctx.client.createPayloadIndex(name, {
      field_name: `f_${type}`,
      field_schema: type,
      wait: true,
    });
    assert.equal(result.status, 'completed');
  }
  const schema = (await ctx.client.getCollection(name)).payload_schema;
  assert.deepEqual(
    Object.keys(schema).sort(),
    types.map((t) => `f_${t}`).sort(),
  );
  assert.equal(schema.f_keyword.data_type, 'keyword');
  // deletePayloadIndex retires the index (MA1 Task 23, D1210).
  const deleted = await ctx.client.deletePayloadIndex(name, 'f_keyword', { wait: true });
  assert.equal(deleted.status, 'completed');
  assert.equal((await ctx.client.getCollection(name)).payload_schema.f_keyword, undefined);
  const missing = await ctx.client.deletePayloadIndex(name, 'nonExistentField', { wait: true });
  assert.equal(missing.status, 'completed');
});

check(async function namespace_header(ctx) {
  // `@mastra/qdrant` passes `headers` through to the client (MA-R0.7).
  const ns = `smoke_js_${randomUUID().replaceAll('-', '').slice(0, 12)}`;
  const scoped = client({ checkCompatibility: false, headers: { 'Loams-Namespace': ns } });
  const name = `smoke_js_namespace_${randomUUID().replaceAll('-', '')}`;
  try {
    await scoped.createCollection(name, { vectors: { size: 3, distance: 'Cosine' } });
    assert.deepEqual(await scoped.collectionExists(name), { exists: true });
    assert.deepEqual(await ctx.client.collectionExists(name), { exists: false });
  } finally {
    await scoped.deleteCollection(name).catch(() => {});
  }
});

// ----- the runner -----

let passed = 0;
let failed = 0;
for (const fn of CHECKS) {
  const ctx = new Ctx(fn.name);
  try {
    await fn(ctx);
    console.log(`ok   ${fn.name}`);
    passed += 1;
  } catch (err) {
    console.log(`FAIL ${fn.name}: ${err?.stack ?? err}`);
    if (err?.data) console.log(JSON.stringify(err.data));
    failed += 1;
  } finally {
    await ctx.close();
  }
}
if (failed > 0) {
  console.log(`qdrant-js ${pkg.version}: ${failed} of ${passed + failed} checks failed`);
  process.exit(1);
}
console.log(`qdrant-js ${pkg.version}: ${passed} checks passed`);
