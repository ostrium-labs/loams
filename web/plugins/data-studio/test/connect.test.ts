// @vitest-environment node
// API2 Task 10: the console reads the data plane over Connect. No `/v1/namespaces`
// REST path is left under web/ except the routes whose Connect service does not
// exist yet, each named here with the task that removes it (API2 ruling 10.3).

import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vite-plus/test';

const WEB = fileURLToPath(new URL('../../..', import.meta.url));
const SKIP = new Set(['node_modules', 'dist', 'gen', 'test', 'tests', '.vite']);
const SOURCE = /\.(ts|tsx|js|jsx|mjs)$/;

/** Files that may still name a `/v1/namespaces` route, and why. */
const PENDING: Record<string, string> = {
  // Data Studio's SQL tab: POST /v1/namespaces/{ns}/sql until loams.sql.v1 (API2 Task 1).
  'plugins/data-studio/src/client.ts': 'loams.sql.v1, API2 Task 1',
  // Streams and links: until loams.stream.v1 (API2 Task 2).
  'plugins/streams/src/client.ts': 'loams.stream.v1, API2 Task 2',
  'plugins/overview/src/load.ts': 'loams.stream.v1, API2 Task 2',
};

function* files(dir: string): Generator<string> {
  for (const name of readdirSync(dir)) {
    if (SKIP.has(name) || name.startsWith('.')) continue;
    const path = join(dir, name);
    if (statSync(path).isDirectory()) yield* files(path);
    else if (SOURCE.test(name)) yield path;
  }
}

describe('API2 Task 10', () => {
  it('data_studio_uses_connect', () => {
    const named = [...files(WEB)]
      .filter((f) => readFileSync(f, 'utf8').includes('/v1/namespaces'))
      .map((f) => relative(WEB, f))
      .sort();
    expect(named).toEqual(Object.keys(PENDING).sort());
    // Data Studio's one REST call is SQL; collections, documents and search are Connect.
    const client = readFileSync(join(WEB, 'plugins/data-studio/src/client.ts'), 'utf8');
    expect(client.match(/\/v1\/namespaces[^`'"]*/g)).toEqual([
      // oxlint-disable-next-line no-template-curly-in-string -- the source text of the template
      '/v1/namespaces/${encodeURIComponent(ns)}/sql',
    ]);
    for (const service of [
      'collection.CollectionService',
      'documents.DocumentService',
      'query.QueryService',
    ]) {
      expect(client).toContain(service);
    }
  });
});
