// loams.graph.v1.Value as the page shows it: typed cell text, parameters from JSON, and
// the nodes and relationships a result holds for the graph view.
//
// Integers stay exact: INT64 and UINT64 are bigints and print as their decimal text.
// proto3 JSON leaves out an id of 0, and protobuf-es reads a missing id as 0n, so the
// element with id 0 needs no special case here (R7.7).

import { create, type JsonValue } from '@bufbuild/protobuf';
import { NullValue } from '@bufbuild/protobuf/wkt';
import { graph } from '@loams/proto';

type Value = graph.Value;

const pad = (n: number | bigint, w = 2) => n.toString().padStart(w, '0');

function dateText(d: graph.Date | undefined): string {
  if (!d) return '';
  const y = d.year < 0 ? `-${pad(-d.year, 4)}` : pad(d.year, 4);
  return `${y}-${pad(d.month)}-${pad(d.day)}`;
}

function timeText(t: graph.LocalTime | undefined): string {
  if (!t) return '';
  const base = `${pad(t.hour)}:${pad(t.minute)}:${pad(t.second)}`;
  return t.nanosecond ? `${base}.${pad(t.nanosecond, 9).replace(/0+$/, '')}` : base;
}

function offsetText(seconds: number): string {
  if (seconds === 0) return 'Z';
  const sign = seconds < 0 ? '-' : '+';
  const abs = Math.abs(seconds);
  const s = abs % 60;
  const base = `${sign}${pad(Math.floor(abs / 3600))}:${pad(Math.floor((abs % 3600) / 60))}`;
  return s ? `${base}:${pad(s)}` : base;
}

function durationText(d: graph.Duration | undefined): string {
  if (!d) return 'PT0S';
  const sec = d.nanos / 1_000_000_000n;
  const frac = d.nanos % 1_000_000_000n;
  let out = 'P';
  if (d.months) out += `${d.months}M`;
  if (d.days) out += `${d.days}D`;
  if (d.nanos || out === 'P') {
    const f = frac ? `.${pad(frac < 0n ? -frac : frac, 9).replace(/0+$/, '')}` : '';
    const neg = d.nanos < 0n && sec === 0n ? '-' : '';
    out += `T${neg}${sec}${f}S`;
  }
  return out;
}

function hex(bytes: Uint8Array): string {
  const shown = Array.from(bytes.slice(0, 32), (b) => b.toString(16).padStart(2, '0')).join('');
  return `0x${shown}${bytes.length > 32 ? '…' : ''}`;
}

function floatText(f: number): string {
  if (Number.isNaN(f)) return 'NaN';
  if (!Number.isFinite(f)) return f > 0 ? 'Infinity' : '-Infinity';
  return Number.isInteger(f) ? f.toFixed(1) : String(f);
}

function props(map: { [key: string]: Value }): string {
  const keys = Object.keys(map).sort();
  if (keys.length === 0) return '';
  return ` {${keys.map((k) => `${k}: ${valueText(map[k], true)}`).join(', ')}}`;
}

export function nodeText(n: graph.Node): string {
  return `(#${n.id}${n.labels.map((l) => `:${l}`).join('')}${props(n.properties)})`;
}

export function relationshipText(r: graph.Relationship): string {
  return `[#${r.id}:${r.type} ${r.src}→${r.dst}${props(r.properties)}]`;
}

/** A value as one line of text. Strings are bare at the top level and quoted inside. */
export function valueText(v: Value | undefined, nested = false): string {
  const k = v?.kind;
  switch (k?.case) {
    case undefined:
    case 'null':
      return 'null';
    case 'boolean':
      return String(k.value);
    case 'int64':
    case 'uint64':
      return k.value.toString();
    case 'float64':
      return floatText(k.value);
    case 'string':
      return nested ? `'${k.value.replace(/\\/g, '\\\\').replace(/'/g, "\\'")}'` : k.value;
    case 'bytes':
      return hex(k.value);
    case 'decimal':
      return k.value.value;
    case 'date':
      return dateText(k.value);
    case 'localTime':
      return timeText(k.value);
    case 'zonedTime':
      return `${timeText(k.value.time)}${offsetText(k.value.offsetSeconds)}`;
    case 'localDatetime':
      return `${dateText(k.value.date)}T${timeText(k.value.time)}`;
    case 'zonedDatetime':
      return `${dateText(k.value.local?.date)}T${timeText(k.value.local?.time)}${offsetText(k.value.offsetSeconds)}`;
    case 'duration':
      return durationText(k.value);
    case 'list':
      return `[${k.value.values.map((x) => valueText(x, true)).join(', ')}]`;
    case 'map':
      return `{${Object.keys(k.value.entries)
        .sort()
        .map((key) => `${key}: ${valueText(k.value.entries[key], true)}`)
        .join(', ')}}`;
    case 'node':
      return nodeText(k.value);
    case 'relationship':
      return relationshipText(k.value);
    case 'path': {
      const { nodes, relationships } = k.value;
      let out = nodes[0] ? nodeText(nodes[0]) : '()';
      relationships.forEach((r, i) => {
        const next = nodes[i + 1];
        const forward = !next || r.dst === next.id;
        out += forward
          ? `-${relationshipText(r)}->${next ? nodeText(next) : '()'}`
          : `<-${relationshipText(r)}-${nodeText(next)}`;
      });
      return out;
    }
    case 'vector':
      return `vector[${k.value.values.length}]`;
    case 'counter': {
      const sum = (m: { [key: string]: bigint }) => Object.values(m).reduce((a, b) => a + b, 0n);
      return (sum(k.value.positive) - sum(k.value.negative)).toString();
    }
  }
}

/** The value's type as the cell labels it, for example `INT64` or `NODE`. */
export function valueType(v: Value | undefined): string {
  const c = v?.kind.case ?? 'null';
  return c.replace(/[A-Z]/g, (m) => `_${m}`).toUpperCase();
}

/**
 * Parameters from the editor's JSON object. An integral number is INT64 (exact only up
 * to 2^53 in JSON; write a bigger one as `{"$int64": "…"}`), any other number FLOAT64.
 */
export function parametersFromJson(text: string): Record<string, Value> {
  const trimmed = text.trim();
  if (!trimmed) return {};
  let parsed: unknown;
  try {
    parsed = JSON.parse(trimmed);
  } catch (e) {
    throw new Error(`Parameters are not JSON: ${(e as Error).message}`);
  }
  if (parsed === null || typeof parsed !== 'object' || Array.isArray(parsed)) {
    throw new Error('Parameters must be a JSON object, for example {"name": "Keanu Reeves"}.');
  }
  const out: Record<string, Value> = {};
  for (const [k, v] of Object.entries(parsed as Record<string, JsonValue>)) out[k] = toValue(v);
  return out;
}

function toValue(v: JsonValue): Value {
  const make = (kind: Value['kind']) => create(graph.ValueSchema, { kind });
  if (v === null) return make({ case: 'null', value: NullValue.NULL_VALUE });
  if (typeof v === 'boolean') return make({ case: 'boolean', value: v });
  if (typeof v === 'number') {
    return Number.isSafeInteger(v)
      ? make({ case: 'int64', value: BigInt(v) })
      : make({ case: 'float64', value: v });
  }
  if (typeof v === 'string') return make({ case: 'string', value: v });
  if (Array.isArray(v)) {
    return make({ case: 'list', value: create(graph.ListValueSchema, { values: v.map(toValue) }) });
  }
  const keys = Object.keys(v);
  if (keys.length === 1 && keys[0] === '$int64' && typeof v.$int64 === 'string') {
    if (!/^-?\d+$/.test(v.$int64)) throw new Error(`"$int64" must be an integer: ${v.$int64}`);
    return make({ case: 'int64', value: BigInt(v.$int64) });
  }
  const entries: Record<string, Value> = {};
  for (const [k, x] of Object.entries(v)) entries[k] = toValue(x as JsonValue);
  return make({ case: 'map', value: create(graph.MapValueSchema, { entries }) });
}

/** The nodes and relationships of a result, in order of first appearance. */
export interface Elements {
  nodes: Map<string, graph.Node>;
  relationships: Map<string, graph.Relationship>;
}

/**
 * Collects every Node, Relationship and Path in `rows` (lists and maps are searched too).
 * Any other value is ignored: in a result with ORDER BY, Grafeo 0.5.43 answers a
 * relationship as INT64 0 (R7.6), and that stays a number in the table.
 */
export function collectElements(rows: graph.Row[]): Elements {
  const out: Elements = { nodes: new Map(), relationships: new Map() };
  const addNode = (n: graph.Node) => {
    const key = n.id.toString();
    if (!out.nodes.has(key)) out.nodes.set(key, n);
  };
  const addRel = (r: graph.Relationship) => {
    const key = r.id.toString();
    if (!out.relationships.has(key)) out.relationships.set(key, r);
  };
  const walk = (v: Value | undefined) => {
    const k = v?.kind;
    switch (k?.case) {
      case 'node':
        addNode(k.value);
        break;
      case 'relationship':
        addRel(k.value);
        break;
      case 'path':
        for (const n of k.value.nodes) addNode(n);
        for (const r of k.value.relationships) addRel(r);
        break;
      case 'list':
        for (const x of k.value.values) walk(x);
        break;
      case 'map':
        for (const x of Object.values(k.value.entries)) walk(x);
        break;
    }
  };
  for (const row of rows) for (const v of row.values) walk(v);
  return out;
}
