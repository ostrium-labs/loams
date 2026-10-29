// The Loams cases' app for DBOS Transact TypeScript (DX0 Task 5); the same
// commands as ../py/app.py: setup, start <wf> (CRASH_AT kills it when that
// step starts), resume <wf>, count <wf>, enqueue <prefix> <n>, serve <s>,
// queued <prefix>, set-event <wf> <v> [s], get-event <wf> <s>.
// Environment: DBOS_URL, DBOS_LISTEN (1 or 0), DBOS_EXECUTOR.
import { DBOS } from '@dbos-inc/dbos-sdk';
import pg from 'pg';

const URL = process.env.DBOS_URL;
const CRASH_AT = Number(process.env.CRASH_AT ?? '0');
const [cmd, ...args] = process.argv.slice(2);

async function sql(q, params = []) {
  const c = new pg.Client({ connectionString: URL });
  await c.connect();
  try {
    return (await c.query(q, params)).rows;
  } finally {
    await c.end();
  }
}

DBOS.setConfig({
  name: 'loams-crash-ts',
  systemDatabaseUrl: URL,
  useListenNotify: (process.env.DBOS_LISTEN ?? '1') === '1',
  executorID: process.env.DBOS_EXECUTOR ?? 'local',
});
// Queues are registered after launch (DBOS.registerQueue, v5); the SDK is
// CommonJS, so only its documented named exports are imported (T5.R8).
let queue;

async function step(wf, n) {
  return DBOS.runStep(
    async () => {
      if (n === CRASH_AT) process.kill(process.pid, 'SIGKILL');
      await sql('INSERT INTO loams_steps (wf, step) VALUES ($1, $2)', [wf, n]);
    },
    { name: `step${n}` },
  );
}
const five = DBOS.registerWorkflow(
  async (wf) => {
    for (let n = 1; n <= 5; n++) await step(wf, n);
    return 'done';
  },
  { name: 'five' },
);
const one = DBOS.registerWorkflow(
  async (wf) => {
    await step(wf, 1);
    return wf;
  },
  { name: 'one' },
);
const setter = DBOS.registerWorkflow(
  async (v) => {
    await DBOS.setEvent('k', v);
    await DBOS.recv('done', 60);
    return v;
  },
  { name: 'setter' },
);

async function main() {
  if (cmd === 'setup') {
    await sql('CREATE TABLE IF NOT EXISTS loams_steps (wf text NOT NULL, step int NOT NULL)');
    return;
  }
  if (cmd === 'count') {
    const rows = await sql('SELECT step, count(*)::int AS n FROM loams_steps WHERE wf = $1 GROUP BY step ORDER BY step', [args[0]]);
    console.log(JSON.stringify(Object.fromEntries(rows.map((r) => [String(r.step), r.n]))));
    return;
  }
  if (cmd === 'queued') {
    const rows = await sql('SELECT wf, count(*)::int AS n FROM loams_steps WHERE wf LIKE $1 GROUP BY wf', [`${args[0]}%`]);
    console.log(JSON.stringify({ workflows: rows.length, max_runs: Math.max(0, ...rows.map((r) => r.n)) }));
    return;
  }
  await DBOS.launch();
  queue = await DBOS.registerQueue('loams', { concurrency: 10 });
  if (cmd === 'start') {
    const h = await DBOS.startWorkflow(five, { workflowID: args[0] })(args[0]);
    console.log(await h.getResult());
  } else if (cmd === 'resume') {
    console.log(await DBOS.retrieveWorkflow(args[0]).getResult());
  } else if (cmd === 'enqueue') {
    for (let i = 0; i < Number(args[1]); i++) {
      const wf = `${args[0]}-${String(i).padStart(5, '0')}`;
      await DBOS.startWorkflow(one, { workflowID: wf, queueName: queue.name })(wf);
    }
    console.log('enqueued');
  } else if (cmd === 'serve') {
    await new Promise((r) => setTimeout(r, Number(args[0]) * 1000));
  } else if (cmd === 'set-event') {
    const h = await DBOS.startWorkflow(setter, { workflowID: args[0] })(args[1]);
    console.log(JSON.stringify({ set_at: Date.now() / 1000 }));
    await new Promise((r) => setTimeout(r, Number(args[2] ?? '5') * 1000));
    await DBOS.send(args[0], 'ok', 'done');
    await h.getResult();
  } else if (cmd === 'get-event') {
    const v = await DBOS.getEvent(args[0], 'k', Number(args[1]));
    console.log(JSON.stringify({ value: v, got_at: Date.now() / 1000 }));
  } else {
    throw new Error(`unknown command ${cmd}`);
  }
  await DBOS.shutdown();
}

main().then(
  () => process.exit(0),
  (err) => {
    console.error(err);
    process.exit(1);
  },
);
