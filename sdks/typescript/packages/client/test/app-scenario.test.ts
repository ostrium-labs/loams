// SDKP Parts B and C for TypeScript: the app scenario through the public facade.
//
// `typescript_app_collections`, `typescript_app_search`, `typescript_app_graph`,
// and (Part C) `typescript_app_sql` and `typescript_app_streams` replay
// `sdks/fixtures/recorded/app/` against the fixture server (`helpers/app-replay.ts` says how), so each passes only when
// the facade sends exactly the recorded requests and reads the recorded answers
// the way they mean. A live `loams dev` cannot replay a recording (its tokens
// and ids are its own), and replaying against one would create the `app`
// namespace a `--drift` re-record needs to be fresh, so the replay runs against
// the fixture server only.

import { afterAll, beforeAll, describe, expect, it } from 'vite-plus/test';
import { Loams } from '../src/loams.js';
import { replayAppFixture } from './helpers/app-replay.js';
import { startFixtureServer, type FixtureServer } from './helpers/server.js';

let server: FixtureServer;

beforeAll(async () => {
  server = await startFixtureServer({ forceFixtures: true });
});

afterAll(async () => {
  await server?.stop();
});

describe('typescript_app_collections', () => {
  it('namespaces, collections, documents, tokens, paging and the two refusals', async () => {
    await expect(replayAppFixture(server.endpoint, 'app_collections')).resolves.toBeUndefined();
  });
});

describe('typescript_app_search', () => {
  it('text, vector and hybrid search, the next page, and the refused weighted sum', async () => {
    await expect(replayAppFixture(server.endpoint, 'app_search')).resolves.toBeUndefined();
  });
});

describe('typescript_app_graph', () => {
  it('a build without the graph feature refuses every graph call, typed', async () => {
    await expect(replayAppFixture(server.endpoint, 'app_graph')).resolves.toBeUndefined();
  });
});

describe('typescript_app_sql', () => {
  it('a parameterised read, a capped read, Arrow IPC and the two refusals', async () => {
    await expect(replayAppFixture(server.endpoint, 'app_sql')).resolves.toBeUndefined();
  });

  it('sql.records sends the $1 values and reads the rows as records', async () => {
    const loams = new Loams({
      endpoint: server.endpoint,
      namespace: 'app',
      maxRetries: 0,
      transportOptions: { useBinaryFormat: false },
    });
    const records = await loams.sql.records(
      'SELECT tags, count(*) AS c FROM notes WHERE n > $1 GROUP BY tags ORDER BY tags',
      [1],
      { headers: { 'loams-fixture-name': 'app_sql', 'loams-fixture-step': '0' } },
    );
    expect(records).toEqual([
      { tags: 'policy', c: 2 },
      { tags: 'shipping', c: 1 },
    ]);
  });
});

describe('typescript_app_streams', () => {
  it('create, list, describe, a keyed produce and its duplicate, fetch, CloudEvents, not_found', async () => {
    await expect(replayAppFixture(server.endpoint, 'app_streams')).resolves.toBeUndefined();
  });

  it('streams.subscribe reads the recorded page and knows where to resume', async () => {
    const loams = new Loams({
      endpoint: server.endpoint,
      namespace: 'app',
      maxRetries: 0,
      transportOptions: { useBinaryFormat: false },
    });
    const subscription = loams.streams.subscribe(
      { stream: 'note-events', maxRecords: 10 },
      { headers: { 'loams-fixture-name': 'app_streams', 'loams-fixture-step': '5' } },
    );
    const offsets: bigint[] = [];
    for await (const record of subscription) {
      offsets.push(BigInt(record.offset));
      if (offsets.length === 2) break;
    }
    expect(offsets).toEqual([0n, 1n]);
    expect(subscription.offset).toBe(2n);
  });
});
