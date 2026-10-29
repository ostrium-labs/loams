// Where the Runs page reads the run record (design §72 §6.5, D1436; plan FT1, FT0 Task 9):
// the Live app `factory`'s `runs:list` (table `factory_runs`), through the console's own
// Connect transport, which reaches the engine's `loams.live.v1` (see the Live plugin's
// `transport.ts`). The page polls; a Live subscription replaces the poll when the Live
// client grows a typed watch of user functions.

import { createClient, type Transport } from '@connectrpc/connect';
import { live, type liveValue } from '@loams/proto';

export interface RunApproval {
  kind: string;
  id: string;
  state: string;
}

/** A `factory_runs` document, as the page shows it. */
export interface RunRow {
  run: string;
  seq: number;
  project?: string;
  intent?: string;
  stage: string;
  state: string;
  planUrl?: string;
  threads?: string[];
  pr?: { number: number; url: string };
  checks?: string;
  deploy?: { deploymentId: string; app: string; state: string; commit: string };
  approvals?: RunApproval[];
  budget?: { limit: number; spent: number };
  error?: string;
  createdAt: number;
  updatedAt: number;
}

export interface RunsSource {
  list(signal?: AbortSignal): Promise<RunRow[]>;
}

export const FACTORY_LIVE_APP = 'factory';

type Json = null | boolean | number | string | Json[] | { [k: string]: Json };

/** A Live value as plain JSON; an int64 is a number (run records hold small ones). */
export function toJson(v: liveValue.Value | undefined): Json {
  const k = v?.kind;
  switch (k?.case) {
    case 'int64Value':
      return Number(k.value);
    case 'doubleValue':
    case 'boolValue':
    case 'stringValue':
      return k.value;
    case 'arrayValue':
      return k.value.values.map(toJson);
    case 'objectValue':
      return Object.fromEntries(Object.entries(k.value.fields).map(([n, x]) => [n, toJson(x)]));
    default:
      return null;
  }
}

const EMPTY_ARGS = { kind: { case: 'objectValue', value: { fields: {} } } } as const;

/** `runs:list` of the Live app `factory`. */
export function liveRunsSource(transport: Transport): RunsSource {
  const client = createClient(live.LiveService, transport);
  return {
    async list(signal) {
      const res = await client.query(
        { function: 'runs:list', args: EMPTY_ARGS },
        { signal, headers: { 'loams-live-app': FACTORY_LIVE_APP } },
      );
      const rows = toJson(res.result);
      return Array.isArray(rows) ? (rows as unknown as RunRow[]) : [];
    },
  };
}
