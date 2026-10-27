import { readdirSync, readFileSync } from 'node:fs';
import type { Listing } from '../src/types.js';

export const TESTDATA = new URL('../../../schemas/marketplace/testdata/', import.meta.url);

export function fixture(name: string): Listing {
  return JSON.parse(readFileSync(new URL(name, TESTDATA), 'utf8')) as Listing;
}

export function validFixtures(): string[] {
  return readdirSync(new URL('valid/', TESTDATA))
    .filter((f) => f.endsWith('.json'))
    .sort()
    .map((f) => `valid/${f}`);
}

interface Op {
  op: 'set' | 'remove';
  path: string;
  value?: unknown;
}

export interface Case {
  name: string;
  rule: string;
  base: string;
  patch: Op[];
}

export function invalidCases(): Case[] {
  return (JSON.parse(readFileSync(new URL('invalid.json', TESTDATA), 'utf8')) as { cases: Case[] }).cases;
}

/** Applies `set` and `remove` JSON Pointer operations to a copy. */
export function patched<T>(doc: T, ops: Op[]): T {
  const out = structuredClone(doc) as unknown;
  for (const op of ops) {
    const tokens = op.path.split('/').slice(1).map((t) => t.replace(/~1/g, '/').replace(/~0/g, '~'));
    const last = tokens.pop()!;
    let target = out as Record<string, unknown> | unknown[];
    for (const token of tokens) {
      target = (Array.isArray(target) ? target[Number(token)] : target[token]) as Record<string, unknown> | unknown[];
    }
    if (Array.isArray(target)) {
      if (op.op === 'remove') target.splice(Number(last), 1);
      else target[Number(last)] = op.value;
    } else if (op.op === 'remove') {
      delete target[last];
    } else {
      target[last] = op.value;
    }
  }
  return out as T;
}
