// The run record and its events (design §72 §6.5, D1436; plan FT1, FT0 Task 9).
//
// Every transition of a factory run is written twice, both idempotently:
// - to the Live app `factory` (`runs:record` in `live/factory.js`, table `factory_runs`),
//   which the Runs page and phones read; a transition carries the run's sequence number
//   and one at or below the stored one changes nothing;
// - to the stream `factory_events` as a CloudEvent `io.loams.dev.factory.run.<kind>.v1`
//   whose `id` is `<run>/<seq>`, so the stream's per-`source`+`id` dedupe drops a replay.
//
// The workflow numbers its transitions itself (one per step boundary), so a step replayed
// after a crash repeats the same `seq` and both writes are no-ops.

export const RUN_STAGES = [
  'intent',
  'plan',
  'build',
  'review',
  'ci',
  'merge',
  'deploy',
  'close',
] as const;
export type RunStage = (typeof RUN_STAGES)[number];

export const RUN_STATES = ['open', 'waiting', 'paused', 'succeeded', 'failed', 'killed'] as const;
export type RunState = (typeof RUN_STATES)[number];

/** What a transition is, in the event's type. */
export const RUN_TRANSITIONS = [
  'opened',
  'stage',
  'waiting',
  'resumed',
  'paused',
  'succeeded',
  'failed',
  'killed',
] as const;
export type RunTransition = (typeof RUN_TRANSITIONS)[number];

export interface RunApproval {
  kind: string;
  id: string;
  state: string;
}

/** The fields of a `factory_runs` document a transition may set. */
export interface RunPatch {
  project?: string;
  place?: string;
  intent?: string;
  stage?: RunStage;
  state?: RunState;
  planUrl?: string;
  threads?: string[];
  pr?: { number: number; url: string };
  checks?: string;
  deploy?: { deploymentId: string; app: string; state: string; commit: string };
  approvals?: RunApproval[];
  /** Spend in US dollars. */
  budget?: { limit: number; spent: number };
  error?: string;
}

/** A `factory_runs` document as `runs:list` and `runs:get` answer it. */
export interface RunRecord extends RunPatch {
  run: string;
  seq: number;
  stage: RunStage;
  state: RunState;
  createdAt: number;
  updatedAt: number;
}

export interface Transition {
  run: string;
  /** The run's transition number: strictly increasing, the same on a replay. */
  seq: number;
  kind: RunTransition;
  patch: RunPatch;
}

/** Where the record lives (the Live app in production; a fake in tests). */
export interface RunStore {
  record(t: Transition): Promise<{ applied: boolean }>;
}

/** Where the events go (the stream `factory_events`; a fake in tests). */
export type ProduceEvent = (event: RunEvent) => Promise<void>;

/** A CloudEvents 1.0 event, structured mode. */
export interface RunEvent {
  specversion: '1.0';
  id: string;
  source: string;
  type: string;
  subject: string;
  time: string;
  datacontenttype: 'application/json';
  data: { run: string; seq: number } & RunPatch;
}

export const EVENTS_STREAM = 'factory_events';
export const LIVE_APP = 'factory';

export function runEventType(kind: RunTransition): string {
  return `io.loams.dev.factory.run.${kind}.v1`;
}

/** The event of one transition. Its `id` is `<run>/<seq>`, so a replay is the same event. */
export function runEvent(t: Transition, project: string, time: Date): RunEvent {
  return {
    specversion: '1.0',
    id: `${t.run}/${t.seq}`,
    source: `/loams/factory/${encodeURIComponent(project || 'default')}`,
    type: runEventType(t.kind),
    subject: t.run,
    time: time.toISOString(),
    datacontenttype: 'application/json',
    data: { run: t.run, seq: t.seq, ...t.patch },
  };
}

export class RunRecordError extends Error {
  override name = 'RunRecordError';
}

export interface RunRecorder {
  transition(t: Transition): Promise<{ applied: boolean }>;
}

/**
 * Record a run's transitions: the Live record first, then the event (a crash between
 * the two is repaired by the replay, which re-sends the same event id).
 */
export function createRunRecorder(deps: {
  store: RunStore;
  produce: ProduceEvent;
  project: string;
  now?: () => Date;
}): RunRecorder {
  const now = deps.now ?? (() => new Date());
  return {
    async transition(t) {
      if (!t.run || !Number.isSafeInteger(t.seq) || t.seq < 1) {
        throw new RunRecordError(`a transition needs a run and a positive seq, not ${t.seq}`);
      }
      if (!RUN_TRANSITIONS.includes(t.kind)) {
        throw new RunRecordError(`unknown transition ${String(t.kind)}`);
      }
      const result = await deps.store.record(t);
      await deps.produce(runEvent(t, deps.project, now()));
      return result;
    },
  };
}

// ---- The Live and stream clients (Connect JSON and REST over fetch) ----

type LiveValue = Record<string, unknown>;

/** Plain JSON to `loams.live.v1.Value` proto JSON; integers are int64. */
export function toLiveValue(x: unknown): LiveValue {
  if (x === null || x === undefined) return { nullValue: {} };
  if (typeof x === 'boolean') return { boolValue: x };
  if (typeof x === 'string') return { stringValue: x };
  if (typeof x === 'number') {
    return Number.isSafeInteger(x) ? { int64Value: String(x) } : { doubleValue: x };
  }
  if (Array.isArray(x)) return { arrayValue: { values: x.map(toLiveValue) } };
  if (typeof x === 'object') {
    const fields: Record<string, LiveValue> = {};
    for (const [k, v] of Object.entries(x)) if (v !== undefined) fields[k] = toLiveValue(v);
    return { objectValue: { fields } };
  }
  throw new RunRecordError(`not a Live value: ${typeof x}`);
}

/** `loams.live.v1.Value` proto JSON to plain JSON; an int64 is a number. */
export function fromLiveValue(v: unknown): unknown {
  const value = (v ?? {}) as Record<string, unknown>;
  if ('int64Value' in value) return Number(value.int64Value);
  if ('doubleValue' in value) return Number(value.doubleValue);
  if ('boolValue' in value) return value.boolValue;
  if ('stringValue' in value) return value.stringValue;
  if ('arrayValue' in value) {
    const values = (value.arrayValue as { values?: unknown[] } | undefined)?.values ?? [];
    return values.map(fromLiveValue);
  }
  if ('objectValue' in value) {
    const fields =
      (value.objectValue as { fields?: Record<string, unknown> } | undefined)?.fields ?? {};
    return Object.fromEntries(Object.entries(fields).map(([k, f]) => [k, fromLiveValue(f)]));
  }
  if ('bytesValue' in value) return value.bytesValue;
  return null;
}

/**
 * The Live app `factory` over Connect JSON. FT0 talks to the local engine's Live,
 * which has no authentication in the light stack (ruling FT0-9-2); no token is sent.
 */
export function liveRunStore(baseUrl: string, fetchFn: typeof fetch = fetch): RunStore {
  const url = `${baseUrl.replace(/\/+$/, '')}/loams.live.v1.LiveService/Mutate`;
  return {
    async record(t) {
      const response = await fetchFn(url, {
        method: 'POST',
        headers: {
          'content-type': 'application/json',
          'connect-protocol-version': '1',
          'loams-live-app': LIVE_APP,
        },
        body: JSON.stringify({
          function: 'runs:record',
          args: toLiveValue({ run: t.run, seq: t.seq, patch: t.patch }),
          idempotencyKey: `${t.run}/${t.seq}`,
        }),
      });
      const reply = (await response.json().catch(() => ({}))) as Record<string, unknown>;
      if (!response.ok) {
        throw new RunRecordError(
          `runs:record: ${String(reply.code ?? response.status)}: ${String(reply.message ?? '')}`,
        );
      }
      const result = fromLiveValue(reply.result) as { applied?: boolean } | null;
      return { applied: result?.applied === true };
    },
  };
}

/** `factory_events` in namespace `ns`, through the engine's streams REST API. */
export function streamProducer(
  baseUrl: string,
  ns: string,
  fetchFn: typeof fetch = fetch,
): ProduceEvent {
  const url = `${baseUrl.replace(/\/+$/, '')}/v1/namespaces/${encodeURIComponent(ns)}/streams/${EVENTS_STREAM}/events?partition=0`;
  return async (event) => {
    const response = await fetchFn(url, {
      method: 'POST',
      headers: { 'content-type': 'application/cloudevents+json' },
      body: JSON.stringify(event),
    });
    if (!response.ok) {
      throw new RunRecordError(`${EVENTS_STREAM}: HTTP ${response.status}`);
    }
  };
}
