// The TypeScript half of the corpus (DX0 Task 4): a worker for workflows.ts
// and a driver per scenario, with the fixed workflow ID corpus-ts-<scenario>.
// The Go generator exports the histories afterwards (`corpus-go export`), so
// both languages' files come from one exporter.
//
//   node dist/generate.js <address> <namespace> <out.json> [scenario,...]
import { Client, Connection, WorkflowFailedError, WorkflowUpdateFailedError } from '@temporalio/client';
import { NativeConnection, Worker } from '@temporalio/worker';
import { writeFileSync } from 'node:fs';
import * as activities from './activities';
import * as wf from './workflows';

export const SCENARIOS: Array<[string, 'DX1' | 'DX2' | 'DX3' | 'DX4']> = [
  ['hello', 'DX1'],
  ['two_activities_sequential', 'DX1'],
  ['activity_parallel_fanout_10', 'DX1'],
  ['signal_then_complete', 'DX1'],
  ['query_while_running', 'DX1'],
  ['cancel_requested', 'DX1'],
  ['terminated', 'DX1'],
  ['workflow_failed_application_error', 'DX1'],
  ['activity_heartbeat_details', 'DX1'],
  ['timer_short', 'DX2'],
  ['timer_cancelled', 'DX2'],
  ['activity_retry_backoff_3_attempts', 'DX2'],
  ['activity_start_to_close_timeout', 'DX2'],
  ['activity_schedule_to_start_timeout', 'DX2'],
  ['activity_heartbeat_timeout', 'DX2'],
  ['signal_with_start', 'DX2'],
  ['child_workflow', 'DX2'],
  ['child_workflow_cancelled', 'DX2'],
  ['continue_as_new_3', 'DX2'],
  ['id_reuse_allow_duplicate_failed_only', 'DX2'],
  ['id_conflict_fail', 'DX2'],
  ['local_activity', 'DX2'],
  ['side_effect_and_version_marker', 'DX2'],
  ['search_attributes_upsert', 'DX3'],
  ['memo', 'DX3'],
  ['large_payload_1mb', 'DX3'],
  ['update_accepted_completed', 'DX4'],
  ['update_rejected', 'DX4'],
];

const queue = 'corpus-ts';

interface Observation {
  scenario: string;
  workflow_id: string;
  result?: string;
  expected?: string[];
}

async function waitStarted(client: Client, id: string): Promise<void> {
  for (;;) {
    const d = await client.workflow.getHandle(id).describe();
    if ((d.historyLength ?? 0) >= 4 && d.raw.pendingWorkflowTask == null) return;
    await new Promise((r) => setTimeout(r, 200));
  }
}

async function drive(client: Client, name: string, id: string): Promise<Observation> {
  const o: Observation = { scenario: name, workflow_id: id, expected: [] };
  const start = (args: unknown[] = [], extra: Record<string, unknown> = {}) =>
    client.workflow.start(name, { taskQueue: queue, workflowId: id, args, ...extra });
  const settle = async (p: Promise<unknown>) => {
    o.result = JSON.stringify(await p);
  };
  switch (name) {
    case 'hello':
      return settle((await start(['corpus'])).result()).then(() => o);
    case 'signal_then_complete': {
      const h = await start();
      await waitStarted(client, id);
      await h.signal(wf.goSignal, 's');
      await settle(h.result());
      return o;
    }
    case 'signal_with_start': {
      const h = await client.workflow.signalWithStart(name, {
        taskQueue: queue, workflowId: id, signal: wf.goSignal, signalArgs: ['s'],
      });
      await settle(h.result());
      return o;
    }
    case 'query_while_running': {
      const h = await start();
      await waitStarted(client, id);
      const state = await h.query(wf.stateQuery);
      if (state !== 'waiting') throw new Error(`query answered ${state}`);
      o.expected!.push('query state=waiting');
      await h.signal(wf.finishSignal);
      await settle(h.result());
      return o;
    }
    case 'cancel_requested':
    case 'terminated': {
      const h = await start();
      await waitStarted(client, id);
      if (name === 'cancel_requested') await h.cancel();
      else await h.terminate('corpus terminates it');
      try {
        await h.result();
      } catch (err) {
        o.expected!.push(String(err));
        return o;
      }
      throw new Error(`the workflow completed instead of closing as ${name}`);
    }
    case 'workflow_failed_application_error': {
      const h = await start();
      try {
        await h.result();
      } catch (err) {
        if (err instanceof WorkflowFailedError) {
          o.expected!.push(String(err.cause));
          return o;
        }
        throw err;
      }
      throw new Error('want an application error');
    }
    case 'continue_as_new_3':
      await settle((await start([0])).result());
      return o;
    case 'id_reuse_allow_duplicate_failed_only': {
      const policy = { workflowIdReusePolicy: 'ALLOW_DUPLICATE_FAILED_ONLY' };
      const first = await start([true], policy);
      await first.result().then(
        () => {
          throw new Error('the first run should fail');
        },
        () => undefined,
      );
      const second = await start([false], policy);
      o.expected!.push('the first run failed; the second started with the same ID');
      await settle(second.result());
      return o;
    }
    case 'id_conflict_fail': {
      const policy = { workflowIdConflictPolicy: 'FAIL' };
      const h = await start([], policy);
      try {
        await start([], policy);
        throw new Error('the second start should be refused');
      } catch (err) {
        if (!String(err).includes('already started') && !(err as Error).name?.includes('AlreadyStarted')) throw err;
        o.expected!.push('the second start was refused: WorkflowExecutionAlreadyStarted');
      }
      await h.signal(wf.finishSignal);
      await settle(h.result());
      return o;
    }
    case 'memo':
      await settle((await start([], { memo: { initial: 'set at start' } })).result());
      return o;
    case 'update_accepted_completed':
    case 'update_rejected': {
      const h = await start();
      await waitStarted(client, id);
      const by = name === 'update_rejected' ? -1 : 2;
      try {
        const total = await h.executeUpdate(wf.incrementUpdate, { args: [by], updateId: `${id}-update` });
        o.expected!.push(`update returned ${total}`);
        if (by < 0) throw new Error('the validator should reject a negative increment');
      } catch (err) {
        if (!(by < 0 && err instanceof WorkflowUpdateFailedError)) throw err;
        o.expected!.push(`update rejected: ${String(err.cause ?? err)}`);
      }
      await h.signal(wf.finishSignal);
      await settle(h.result());
      return o;
    }
    default:
      await settle((await start()).result());
      return o;
  }
}

async function main(): Promise<void> {
  const [address, namespace, out, only] = process.argv.slice(2);
  if (!address || !namespace || !out) throw new Error('usage: generate.js <address> <namespace> <out.json> [scenarios]');
  const want = only ? new Set(only.split(',')) : undefined;
  // TEMPORAL_API_KEY: a bearer token on every call, in plain text (the Loams
  // gateway's API keys; its compose profile has no TLS). Unset, nothing changes.
  const key = process.env.TEMPORAL_API_KEY;
  const metadata = key ? { authorization: `Bearer ${key}` } : undefined;
  const connection = await NativeConnection.connect({ address, metadata });
  const worker = await Worker.create({
    connection, namespace, taskQueue: queue, activities,
    workflowsPath: require.resolve('./workflows'),
  });
  const running = worker.run();
  const client = new Client({ connection: await Connection.connect({ address, metadata }), namespace });
  const observations: Observation[] = [];
  try {
    for (const [name] of SCENARIOS) {
      if (want && !want.has(name)) continue;
      const o = await drive(client, name, `corpus-ts-${name}`);
      console.log(`corpus: ${name}: ${o.result ?? ''}`);
      observations.push(o);
    }
  } finally {
    worker.shutdown();
    await running;
  }
  writeFileSync(out, JSON.stringify({ namespace, language: 'ts', scenarios: observations }, null, 2) + '\n');
}

if (require.main === module) {
  main().then(
    () => process.exit(0),
    (err) => {
      console.error(err);
      process.exit(1);
    },
  );
}
