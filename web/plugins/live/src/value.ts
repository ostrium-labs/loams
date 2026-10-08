// Plain JSON <-> loams.live.v1.Value. A document's integers are int64, which
// JavaScript shows as a number when it is safe and as a decimal string when
// not; bytes show as `{ "$bytes": "<hex>" }`.

import { create, type MessageInitShape } from '@bufbuild/protobuf';
import { liveValue } from '@loams/proto';

export type Json = null | boolean | number | string | Json[] | { [k: string]: Json };

export function toJs(v: liveValue.Value | undefined): Json {
  const k = v?.kind;
  switch (k?.case) {
    case 'int64Value':
      return k.value >= BigInt(Number.MIN_SAFE_INTEGER) &&
        k.value <= BigInt(Number.MAX_SAFE_INTEGER)
        ? Number(k.value)
        : k.value.toString();
    case 'doubleValue':
    case 'boolValue':
    case 'stringValue':
      return k.value;
    case 'bytesValue':
      return { $bytes: Array.from(k.value, (b) => b.toString(16).padStart(2, '0')).join('') };
    case 'arrayValue':
      return k.value.values.map(toJs);
    case 'objectValue':
      return Object.fromEntries(Object.entries(k.value.fields).map(([n, x]) => [n, toJs(x)]));
    default:
      return null;
  }
}

type Init = MessageInitShape<typeof liveValue.ValueSchema>;

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
    const fields: Record<string, Init> = {};
    for (const [k, v] of Object.entries(x)) fields[k] = init(v);
    return { kind: { case: 'objectValue', value: { fields } } };
  }
  throw new Error(`cannot encode a ${typeof x} as a Live value`);
}

export function fromJs(x: unknown): liveValue.Value {
  return create(liveValue.ValueSchema, init(x));
}
