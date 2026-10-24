// The corpus's TypeScript workflows (DX0 Task 4): the Go scenarios except
// the Go-only ones (workflow_task_failure_then_fix, schedule_triggered,
// reset_to_first_workflow_task). Each is exported under its scenario name,
// so the generator and the replayer agree on it. A change here changes the
// histories: regenerate the corpus in the same PR.
import {
  ActivityFailure,
  ApplicationFailure,
  CancellationScope,
  condition,
  continueAsNew,
  defineQuery,
  defineSignal,
  defineUpdate,
  isCancellation,
  patched,
  proxyActivities,
  proxyLocalActivities,
  setHandler,
  sleep,
  startChild,
  executeChild,
  TimeoutFailure,
  upsertMemo,
  upsertSearchAttributes,
  workflowInfo,
} from '@temporalio/workflow';
import type * as activities from './activities';

export const goSignal = defineSignal<[string]>('go');
export const finishSignal = defineSignal('finish');
export const stateQuery = defineQuery<string>('state');
export const incrementUpdate = defineUpdate<number, [number]>('increment');

const once = { startToCloseTimeout: '30s', retry: { maximumAttempts: 1 } } as const;
const { greet, heartbeat_thrice, large } = proxyActivities<typeof activities>(once);

function timeoutKind(err: unknown): string {
  if (err instanceof ActivityFailure && err.cause instanceof TimeoutFailure) {
    return `TIMEOUT_TYPE_${err.cause.timeoutType === undefined ? 'UNSPECIFIED' : String(err.cause.timeoutType)}`;
  }
  return `not a timeout: ${String(err)}`;
}

async function waitFinish(): Promise<void> {
  let done = false;
  setHandler(finishSignal, () => {
    done = true;
  });
  await condition(() => done);
}

// --- DX1 ----------------------------------------------------------------------

export async function hello(name: string): Promise<string> {
  return greet(name);
}

export async function two_activities_sequential(): Promise<string> {
  const a = await greet('one');
  const b = await greet('two');
  return `${a}, ${b}`;
}

export async function activity_parallel_fanout_10(): Promise<string[]> {
  return Promise.all(Array.from({ length: 10 }, (_, i) => greet(`n${i}`)));
}

export async function signal_then_complete(): Promise<string> {
  let v: string | undefined;
  setHandler(goSignal, (s) => {
    v = s;
  });
  await condition(() => v !== undefined);
  return `got ${v}`;
}

export async function query_while_running(): Promise<string> {
  let state = 'waiting';
  setHandler(stateQuery, () => state);
  await waitFinish();
  state = 'done';
  return state;
}

export async function cancel_requested(): Promise<void> {
  await sleep('1h');
}

export async function terminated(): Promise<void> {
  await waitFinish();
}

export async function workflow_failed_application_error(): Promise<void> {
  throw ApplicationFailure.nonRetryable('boom', 'CorpusBoom', 'detail');
}

export async function activity_heartbeat_details(): Promise<number> {
  const { heartbeat_thrice: hb } = proxyActivities<typeof activities>({
    ...once,
    heartbeatTimeout: '10s',
  });
  return hb();
}

// --- DX2 ----------------------------------------------------------------------

export async function timer_short(): Promise<void> {
  await sleep('1s');
}

export async function timer_cancelled(): Promise<string> {
  const scope = new CancellationScope();
  const long = scope.run(() => sleep('1h'));
  await sleep(100);
  scope.cancel();
  try {
    await long;
  } catch (err) {
    if (isCancellation(err)) return 'cancelled';
    throw err;
  }
  throw new Error('the long timer was not cancelled');
}

export async function activity_retry_backoff_3_attempts(): Promise<number> {
  const { fail_twice } = proxyActivities<typeof activities>({
    startToCloseTimeout: '10s',
    retry: { initialInterval: '1s', backoffCoefficient: 2, maximumAttempts: 3 },
  });
  return fail_twice();
}

export async function activity_start_to_close_timeout(): Promise<string> {
  const { sleep_for } = proxyActivities<typeof activities>({ startToCloseTimeout: '1s', retry: { maximumAttempts: 1 } });
  try {
    await sleep_for(5000);
    return 'no timeout';
  } catch (err) {
    return timeoutKind(err);
  }
}

export async function activity_schedule_to_start_timeout(): Promise<string> {
  const { greet: nobody } = proxyActivities<typeof activities>({
    taskQueue: 'corpus-no-worker',
    scheduleToStartTimeout: '1s',
    startToCloseTimeout: '1s',
    retry: { maximumAttempts: 1 },
  });
  try {
    await nobody('nobody');
    return 'no timeout';
  } catch (err) {
    return timeoutKind(err);
  }
}

export async function activity_heartbeat_timeout(): Promise<string> {
  const { sleep_for } = proxyActivities<typeof activities>({
    startToCloseTimeout: '30s',
    heartbeatTimeout: '1s',
    retry: { maximumAttempts: 1 },
  });
  try {
    await sleep_for(5000);
    return 'no timeout';
  } catch (err) {
    return timeoutKind(err);
  }
}

export async function signal_with_start(): Promise<string> {
  let v: string | undefined;
  setHandler(goSignal, (s) => {
    v = s;
  });
  await condition(() => v !== undefined);
  return `started with ${v}`;
}

export async function child_greeter(name: string): Promise<string> {
  return greet(name);
}

export async function child_sleeper(): Promise<void> {
  await sleep('1h');
}

export async function child_workflow(): Promise<string> {
  return executeChild(child_greeter, { args: ['child'], workflowId: `${workflowInfo().workflowId}-child` });
}

export async function child_workflow_cancelled(): Promise<string> {
  const scope = new CancellationScope();
  const child = await scope.run(() => startChild(child_sleeper, { workflowId: `${workflowInfo().workflowId}-child` }));
  scope.cancel();
  try {
    await child.result();
  } catch (err) {
    if (isCancellation(err)) return 'child cancelled';
    throw err;
  }
  throw new Error('the child was not cancelled');
}

export async function continue_as_new_3(n: number): Promise<number> {
  if (n < 3) await continueAsNew<typeof continue_as_new_3>(n + 1);
  return n;
}

export async function id_reuse_allow_duplicate_failed_only(fail: boolean): Promise<string> {
  if (fail) throw ApplicationFailure.nonRetryable('first run fails', 'CorpusFirstRun');
  return 'second run';
}

export async function id_conflict_fail(): Promise<void> {
  await waitFinish();
}

export async function local_activity(): Promise<string> {
  const { greet: localGreet } = proxyLocalActivities<typeof activities>({ startToCloseTimeout: '10s' });
  return localGreet('local');
}

export async function side_effect_and_version_marker(): Promise<string> {
  // TypeScript has no SideEffect: workflow code is deterministic by
  // construction. patched() writes the version marker.
  const v = patched('corpus-change') ? 1 : 0;
  return `version ${v}`;
}

// --- DX3 ----------------------------------------------------------------------

export async function search_attributes_upsert(): Promise<void> {
  upsertSearchAttributes({ CorpusKeyword: ['first'] });
  await sleep(100);
  upsertSearchAttributes({ CorpusKeyword: ['second'] });
}

export async function memo(): Promise<void> {
  upsertMemo({ step: 'upserted', n: 2 });
}

export async function large_payload_1mb(): Promise<number> {
  return (await large(1 << 20)).length;
}

// --- DX4 ----------------------------------------------------------------------

async function updates(): Promise<string> {
  let total = 0;
  setHandler(
    incrementUpdate,
    (by: number) => {
      total += by;
      return total;
    },
    {
      validator: (by: number) => {
        if (by < 0) throw new Error('increment must not be negative');
      },
    },
  );
  await waitFinish();
  return `total ${total}`;
}

export async function update_accepted_completed(): Promise<string> {
  return updates();
}

export async function update_rejected(): Promise<string> {
  return updates();
}
