// The Resonate envelope over the desktop proxy: every call is
// `POST /durable/` with `{kind, head: {corrId, version}, data}`, and the
// answer is `{kind, head: {corrId, status, version}, data}`. A status of 400
// or more carries the message as `data` (a string). Shapes follow the pinned
// resonate checkout (e360669, impl/sdk/ts/src/network/types.ts).

export const ENVELOPE_VERSION = '2026-04-01';

export type PromiseState =
  | 'pending'
  | 'resolved'
  | 'rejected'
  | 'rejected_canceled'
  | 'rejected_timedout';
export type TaskState = 'pending' | 'acquired' | 'suspended' | 'halted' | 'fulfilled';

export const PROMISE_STATES: PromiseState[] = [
  'pending',
  'resolved',
  'rejected',
  'rejected_canceled',
  'rejected_timedout',
];
export const TASK_STATES: TaskState[] = ['pending', 'acquired', 'suspended', 'halted', 'fulfilled'];

/** A payload: `data` is base64 of the bytes (the SDKs write base64 JSON). */
export interface Value {
  headers?: Record<string, string>;
  data?: string;
}

export interface PromiseRecord {
  id: string;
  state: PromiseState;
  param: Value;
  value: Value;
  tags: Record<string, string>;
  timeoutAt: number;
  createdAt: number;
  settledAt?: number;
}

export interface TaskRecord {
  id: string;
  state: TaskState;
  version: number;
  resumes: string[] | number | boolean;
  ttl?: number;
  pid?: string;
}

export interface ScheduleRecord {
  id: string;
  cron: string;
  promiseId: string;
  /** Milliseconds each created promise lives. */
  promiseTimeout: number;
  promiseParam: Value;
  promiseTags: Record<string, string>;
  createdAt: number;
  nextRunAt: number;
  lastRunAt?: number;
}

/** A failed call: the envelope's `head.status` (or the HTTP status) and its message. */
export class EnvelopeError extends Error {
  readonly status: number;
  constructor(status: number, message: string) {
    super(message);
    this.name = 'EnvelopeError';
    this.status = status;
  }
}

export type Send = (kind: string, data: any) => Promise<Record<string, any>>;

/** `envelope(fetch)(kind, data)` resolves to the response `data`, or throws an EnvelopeError. */
export function envelope(
  fetchImpl: typeof globalThis.fetch,
  baseUrl = '',
): (kind: string, data: unknown) => Promise<Record<string, any>> {
  return async (kind, data) => {
    const body = {
      kind,
      head: { corrId: crypto.randomUUID(), version: ENVELOPE_VERSION },
      data,
    };
    const res = await fetchImpl(`${baseUrl}/durable/`, {
      method: 'POST',
      credentials: 'include',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(body),
    });
    let json: Record<string, any> | undefined;
    try {
      json = await res.json();
    } catch {
      throw new EnvelopeError(res.status, `unexpected answer (HTTP ${res.status})`);
    }
    // The proxy's own refusals are {code, message}, not an envelope.
    if (!json?.head) throw new EnvelopeError(res.status, json?.message ?? `HTTP ${res.status}`);
    const status: number = json.head.status ?? res.status;
    if (status >= 400) {
      throw new EnvelopeError(
        status,
        typeof json.data === 'string' ? json.data : JSON.stringify(json.data),
      );
    }
    return json.data;
  };
}

export interface Page<T> {
  items: T[];
  cursor?: string;
}

/** Typed calls over `send`. */
export function createDurableApi(send: Send) {
  return {
    async searchPromises(q: {
      state?: PromiseState;
      tags?: Record<string, string>;
      limit?: number;
      cursor?: string;
    }): Promise<Page<PromiseRecord>> {
      const d = await send('promise.search', q);
      return { items: d.promises ?? [], cursor: d.cursor };
    },
    async getPromise(id: string): Promise<PromiseRecord> {
      return (await send('promise.get', { id })).promise;
    },
    async createPromise(p: {
      id: string;
      timeoutAt: number;
      param: Value;
      tags: Record<string, string>;
    }): Promise<PromiseRecord> {
      return (await send('promise.create', p)).promise;
    },
    async cancelPromise(id: string): Promise<PromiseRecord> {
      return (await send('promise.settle', { id, state: 'rejected_canceled', value: {} })).promise;
    },
    async searchSchedules(
      q: { limit?: number; cursor?: string } = {},
    ): Promise<Page<ScheduleRecord>> {
      const d = await send('schedule.search', q);
      return { items: d.schedules ?? [], cursor: d.cursor };
    },
    async createSchedule(s: {
      id: string;
      cron: string;
      promiseId: string;
      promiseTimeout: number;
      promiseParam: Value;
      promiseTags: Record<string, string>;
    }): Promise<ScheduleRecord> {
      return (await send('schedule.create', s)).schedule;
    },
    async deleteSchedule(id: string): Promise<void> {
      await send('schedule.delete', { id });
    },
    async searchTasks(q: {
      state?: TaskState;
      limit?: number;
      cursor?: string;
    }): Promise<Page<TaskRecord>> {
      const d = await send('task.search', q);
      return { items: d.tasks ?? [], cursor: d.cursor };
    },
    async getTask(id: string): Promise<TaskRecord> {
      return (await send('task.get', { id })).task;
    },
  };
}

export type DurableApi = ReturnType<typeof createDurableApi>;

// ---- payloads ----

function fromBase64(b64: string): string | undefined {
  try {
    const bin = atob(b64);
    return new TextDecoder('utf-8', { fatal: true }).decode(
      Uint8Array.from(bin, (c) => c.charCodeAt(0)),
    );
  } catch {
    return undefined;
  }
}

export function toBase64(text: string): string {
  let bin = '';
  for (const b of new TextEncoder().encode(text)) bin += String.fromCharCode(b);
  return btoa(bin);
}

export type Decoded =
  | { kind: 'empty' }
  | { kind: 'json'; text: string; raw: string }
  /** Not valid JSON (or not text): show the base64, with a toggle to the text when it is text. */
  | { kind: 'base64'; raw: string; text?: string };

/** Pretty JSON when the payload is base64 of JSON, otherwise the base64 itself. */
export function decodeValue(v: Value | undefined): Decoded {
  const raw = v?.data;
  if (!raw) return { kind: 'empty' };
  const text = fromBase64(raw);
  if (text === undefined) return { kind: 'base64', raw };
  try {
    return { kind: 'json', text: JSON.stringify(JSON.parse(text), null, 2), raw };
  } catch {
    return { kind: 'base64', raw, text };
  }
}

/** A promise's param as the wire value: base64 of the JSON text, or empty. */
export function encodeParam(json: string): Value {
  const t = json.trim();
  if (!t) return { headers: {}, data: '' };
  return { headers: {}, data: toBase64(JSON.stringify(JSON.parse(t))) };
}

// ---- tags ----

/** `k=v` -> [k, v]; undefined when malformed. The first `=` splits (values may hold `=`). */
export function parseTag(s: string): [string, string] | undefined {
  const i = s.indexOf('=');
  if (i < 1) return undefined;
  return [s.slice(0, i).trim(), s.slice(i + 1).trim()];
}
