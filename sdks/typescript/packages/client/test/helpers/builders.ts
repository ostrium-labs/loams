// The builder corpus's neutral notation, interpreted with the public builders
// (`sdks/fixtures/builders/README.md`). Shared by `builders.test.ts` and the
// app scenario replay, which builds a step's request from its builder case.

import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import type { JsonObject, JsonValue } from '@bufbuild/protobuf';
import { Filter, type FilterJson } from '../../src/builders/filter.js';
import { Schema } from '../../src/builders/schema.js';
import { Search, Sort, dbsf, rrf, weighted } from '../../src/builders/search.js';
import { FIXTURES } from './server.js';

export interface Call {
  op: string;
  args?: JsonValue[];
}

export interface Case {
  name: string;
  builder: 'schema' | 'filter' | 'search';
  message: string;
  spec: unknown;
  expect: JsonObject;
}

const DIR = join(FIXTURES, 'builders');
export const cases: Case[] = readdirSync(DIR)
  .filter((file) => file.endsWith('.json'))
  .sort()
  .map((file) => JSON.parse(readFileSync(join(DIR, file), 'utf8')) as Case);

const camel = (name: string) => name.replace(/_([a-z0-9])/g, (_m, c: string) => c.toUpperCase());

const isCall = (value: unknown): value is Call =>
  typeof value === 'object' && value !== null && !Array.isArray(value) && 'op' in value;

/** An options object, with its keys in TypeScript case. */
function options(value: JsonValue | undefined): Record<string, unknown> | undefined {
  if (value === undefined || value === null || typeof value !== 'object' || Array.isArray(value)) {
    return undefined;
  }
  return Object.fromEntries(Object.entries(value).map(([key, v]) => [camel(key), v]));
}

/** A call's arguments, with nested calls evaluated. */
export function args(call: Call): unknown[] {
  return (call.args ?? []).map((arg) => (isCall(arg) ? evaluate(arg) : arg));
}

/** A filter, fusion or sort call. */
export function evaluate(call: Call): JsonObject {
  const a = args(call);
  switch (call.op) {
    case 'rrf':
      return rrf(a[0] as number | undefined);
    case 'dbsf':
      return dbsf();
    case 'weighted':
      return weighted(...(a as number[]));
    case 'score':
      return Sort.score(a[0] as 'asc' | 'desc' | undefined);
    case 'pk':
      return Sort.pk(a[0] as 'asc' | 'desc' | undefined);
    case 'field':
      return Sort.field(a[0] as string, a[1] as 'asc' | 'desc' | undefined, a[2] as 'first' | 'last' | undefined);
    default:
      return filter(call.op, a);
  }
}

export function filter(op: string, a: unknown[]): FilterJson {
  const verb = camel(op === 'phrase' ? 'phrase' : op);
  switch (verb) {
    case 'range':
      return Filter.range(a[0] as string, options(a[1] as JsonValue) ?? {});
    case 'match':
      return Filter.match(a[0] as string, a[1] as string, options(a[2] as JsonValue));
    case 'ids':
      return Filter.ids(...(a as Parameters<typeof Filter.ids>));
    case 'and':
    case 'or':
    case 'not':
    case 'filter':
      return (Filter[verb] as (...q: FilterJson[]) => FilterJson)(...(a as FilterJson[]));
    default: {
      const fn = (Filter as Record<string, unknown>)[verb];
      if (typeof fn !== 'function') {
        throw new Error(`no filter verb ${op}`);
      }
      return (fn as (...x: unknown[]) => FilterJson)(...a);
    }
  }
}

export function schema(calls: Call[]): JsonObject {
  let builder = new Schema();
  for (const call of calls) {
    const a = args(call);
    const verb = camel(call.op) as keyof Schema;
    const fn = builder[verb] as unknown as (...x: unknown[]) => Schema;
    const last = a[a.length - 1];
    const opts = options(last as JsonValue);
    builder = fn.apply(builder, opts === undefined ? a : [...a.slice(0, -1), opts]);
  }
  return builder.toJson();
}

export function search(spec: { on: string; calls: Call[] }): Search {
  let builder = Search.on(spec.on);
  for (const call of spec.calls) {
    const a = args(call);
    const verb = camel(call.op) as keyof Search;
    const fn = builder[verb] as unknown as (...x: unknown[]) => Search;
    if (typeof fn !== 'function') {
      throw new Error(`no search verb ${call.op}`);
    }
    // A trailing plain object is the call's options; nested calls are already
    // values, and `after`'s list is a list.
    const last = call.args?.[call.args.length - 1];
    const opts = !isCall(last) ? options(last) : undefined;
    builder = fn.apply(builder, opts === undefined ? a : [...a.slice(0, -1), opts]);
  }
  return builder;
}

/** A builder case by name. */
export function builderCase(name: string): Case {
  const found = cases.find((entry) => entry.name === name);
  if (found === undefined) {
    throw new Error(`no builder case ${name}`);
  }
  return found;
}
