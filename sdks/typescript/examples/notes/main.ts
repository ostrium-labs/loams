// The notes app (SDKP Part D): the app scenario of `sdks/fixtures/app-scenario.md`
// as a program, through the public `@loams/client` API.
//
//   LOAMS_ENDPOINT=http://127.0.0.1:8080 LOAMS_NAMESPACE=app npx tsx sdks/typescript/examples/notes/main.ts
//
// It is type-checked with the client (`pnpm --filter @loams/client typecheck`),
// so it cannot drift from the API; running it needs a `loams dev`. In an app,
// import from '@loams/client'; here the import is the workspace source.

import {
  AlreadyExistsError,
  Filter,
  Loams,
  Schema,
  Search,
  Sort,
  rrf,
  type DocumentInput,
} from '../../packages/client/src/index.js';

const notes: DocumentInput[] = [
  { id: 1, source: { title: 'Refund policy', body: 'refund policy for every order', tags: 'policy', n: 1 }, vectors: { embedding: [1, 0, 0, 0] } },
  { id: 2, source: { title: 'Shipping times', body: 'shipping takes three days', tags: 'shipping', n: 2 }, vectors: { embedding: [0.9, 0.1, 0, 0] } },
  { id: 3, source: { title: 'Refund window', body: 'the refund window is thirty days', tags: 'policy', n: 3 }, vectors: { embedding: [0, 0, 1, 0] } },
  { id: 4, source: { title: 'Returns', body: 'returns need a receipt before a refund', tags: 'policy', n: 4 }, vectors: { embedding: [0.5, 0.5, 0, 0] } },
];

export async function main(): Promise<void> {
  // LOAMS_ENDPOINT, LOAMS_API_KEY (or LOAMS_TOKEN) and LOAMS_NAMESPACE; reads
  // see this process's own writes.
  const loams = Loams.fromEnv({ sessionConsistency: true });

  try {
    await loams.namespaces.create({ namespace: loams.namespace ?? 'app' });
  } catch (error) {
    if (!(error instanceof AlreadyExistsError)) throw error;
  }
  try {
    await loams.collections.create({
      name: 'notes',
      schema: new Schema().text('title').text('body').keyword('tags').i64('n').vector('embedding', 4).toJson(),
    });
  } catch (error) {
    if (!(error instanceof AlreadyExistsError)) throw error;
  }

  const handle = loams.collection('notes');
  await handle.upsert(notes);

  // Hybrid: text and vector, fused with RRF, filtered, highlighted, projected.
  const hits = await handle.search(
    handle
      .query()
      .text('refund', { fields: ['body'] })
      .vector('embedding', [1, 0, 0, 0], { k: 10 })
      .fuse(rrf(60))
      .filter(Filter.eq('tags', 'policy'))
      .highlight('body')
      .select({ include: ['title'] })
      .limit(3),
  );
  for (const hit of hits.hits) console.log(hit.pk, hit.score, hit.source);

  // Every policy note, page by page (the sort ends in the primary key).
  for await (const hit of loams.search.queryAll(
    Search.on('notes').text('refund', { fields: ['body'] }).filter(Filter.eq('tags', 'policy')).sort(Sort.score(), Sort.pk()).limit(2),
  )) {
    console.log('page hit', hit);
  }

  const policy = await handle.count({ where: Filter.eq('tags', 'policy') });
  console.log('policy notes:', policy.count);

  // SQL over the same collection, with a `$1` parameter, as records.
  const perTag = await loams.sql.records(
    'SELECT tags, count(*) AS c FROM notes WHERE n > $1 GROUP BY tags ORDER BY tags',
    [1],
  );
  for (const row of perTag) console.log(row.tags, row.c);

  // A stream of note events: produce with a producer key, then read it back.
  await loams.streams.create({ name: 'note-events', partitions: 1 });
  const encode = (value: unknown) => new TextEncoder().encode(JSON.stringify(value));
  await loams.streams.produce({
    stream: 'note-events',
    records: [{ value: encode({ note: 1, op: 'created' }) }],
    producerId: 'notes-example',
    producerSeq: 1n,
  });
  const events = loams.streams.subscribe({ stream: 'note-events' });
  for await (const event of events) {
    console.log('event at', event.offset);
    break;
  }

  // A bulk load: chunked, one idempotency key per chunk, the tokens merged.
  const loaded = await handle.writeBatch(
    Array.from({ length: 100 }, (_, i) => ({ id: 1000 + i, source: { title: `note ${i}`, tags: 'bulk', n: i } })),
    { chunkRows: 25 },
  );
  console.log(`wrote ${loaded.rows} notes in ${loaded.chunks} chunks; token ${loaded.token}`);
}

if (import.meta.url === `file://${process.argv[1]}`) {
  await main();
}
