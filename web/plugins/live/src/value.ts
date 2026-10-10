// Plain JSON <-> loams.live.v1.Value: one canonical form shared with the agent
// tools (apps/desktop-electron/src/main/agent-tools/live.ts).
//
// - An int64 inside the JS safe range is a number; outside it, `{"$int64":"<decimal>"}`.
// - Bytes are `{"$bytes":"<base64>"}`.
// - `fromJs` accepts both wrappers, so what the page shows can be pasted back.
// - `parseJson` reads raw text without losing big integers.

import { create, type MessageInitShape } from '@bufbuild/protobuf';
import { base64Decode, base64Encode } from '@bufbuild/protobuf/wire';
import { liveValue } from '@loams/proto';

export type Json = null | boolean | number | string | Json[] | { [k: string]: Json };

const MIN = BigInt(Number.MIN_SAFE_INTEGER);
const MAX = BigInt(Number.MAX_SAFE_INTEGER);
const I64_MIN = -(2n ** 63n);
const I64_MAX = 2n ** 63n - 1n;

export function toJs(v: liveValue.Value | undefined): Json {
  const k = v?.kind;
  switch (k?.case) {
    case 'int64Value':
      return k.value >= MIN && k.value <= MAX ? Number(k.value) : { $int64: k.value.toString() };
    case 'doubleValue':
    case 'boolValue':
    case 'stringValue':
      return k.value;
    case 'bytesValue':
      return { $bytes: base64Encode(k.value) };
    case 'arrayValue':
      return k.value.values.map(toJs);
    case 'objectValue':
      return Object.fromEntries(Object.entries(k.value.fields).map(([n, x]) => [n, toJs(x)]));
    default:
      return null;
  }
}

type Init = MessageInitShape<typeof liveValue.ValueSchema>;

/** The single string of a `{ "<key>": "<string>" }` wrapper, else undefined. */
function wrapper(x: object, key: string): string | undefined {
  const keys = Object.keys(x);
  const v = (x as Record<string, unknown>)[key];
  return keys.length === 1 && keys[0] === key && typeof v === 'string' ? v : undefined;
}

function int64(text: string): bigint {
  if (!/^-?\d+$/.test(text)) throw new Error(`$int64 takes a decimal integer, not "${text}"`);
  const n = BigInt(text);
  if (n < I64_MIN || n > I64_MAX) throw new Error(`${text} is outside the int64 range`);
  return n;
}

function init(x: unknown): Init {
  if (x === null || x === undefined) return { kind: { case: 'nullValue', value: {} } };
  if (typeof x === 'boolean') return { kind: { case: 'boolValue', value: x } };
  if (typeof x === 'string') return { kind: { case: 'stringValue', value: x } };
  if (typeof x === 'number')
    return Number.isSafeInteger(x)
      ? { kind: { case: 'int64Value', value: BigInt(x) } }
      : { kind: { case: 'doubleValue', value: x } };
  if (Array.isArray(x)) return { kind: { case: 'arrayValue', value: { values: x.map(init) } } };
  if (typeof x === 'object') {
    const big = wrapper(x, '$int64');
    if (big !== undefined) return { kind: { case: 'int64Value', value: int64(big) } };
    const bytes = wrapper(x, '$bytes');
    if (bytes !== undefined) return { kind: { case: 'bytesValue', value: base64Decode(bytes) } };
    const fields: Record<string, Init> = {};
    for (const [k, v] of Object.entries(x)) fields[k] = init(v);
    return { kind: { case: 'objectValue', value: { fields } } };
  }
  throw new Error(`cannot encode a ${typeof x} as a Live value`);
}

export function fromJs(x: unknown): liveValue.Value {
  return create(liveValue.ValueSchema, init(x));
}

type Reviver = (key: string, value: unknown, context: { source?: string }) => unknown;

/**
 * JSON.parse that keeps integers beyond 2^53 exact: they come back as
 * `{ $int64: "<decimal>" }`. Throws on a syntax error or an integer outside int64.
 */
export function parseJson(text: string): Json {
  const reviver: Reviver = (_k, value, ctx) => {
    if (typeof value === 'number' && ctx.source && /^-?\d+$/.test(ctx.source)) {
      if (!Number.isSafeInteger(value)) return { $int64: int64(ctx.source).toString() };
    }
    return value;
  };
  return JSON.parse(text, reviver as never) as Json;
}
