// corpus_replays_ts (DX0 Task 4): every TypeScript history in the corpus
// replays with the pinned SDK's replayer and zero nondeterminism errors.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { Worker } from '@temporalio/worker';

const dir = process.env.CORPUS_HISTORIES ?? join(__dirname, '..', '..', '..', 'histories', 'ts');

test('corpus_replays_ts', async () => {
  const files = readdirSync(dir).filter((f) => f.endsWith('.json')).sort();
  assert.ok(files.length > 0, `no histories in ${dir}`);
  const failures: string[] = [];
  for (const f of files) {
    // The workflow ID the generator used: child IDs derive from it.
    const workflowId = `corpus-ts-${f.replace(/(\.run\d+)?\.json$/, '')}`;
    // runReplayHistory reads the protojson itself (with the SDK's own proto
    // classes), which runReplayHistories does not.
    const history: unknown = JSON.parse(readFileSync(join(dir, f), 'utf8'));
    try {
      await Worker.runReplayHistory({ workflowsPath: require.resolve('./workflows') }, history, workflowId);
    } catch (err) {
      failures.push(`${f}: ${String(err)}`);
    }
  }
  assert.deepEqual(failures, []);
});
