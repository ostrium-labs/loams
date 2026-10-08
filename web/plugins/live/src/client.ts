// The Live client the page uses: the generated LiveService over a Connect
// transport, with every value as plain JSON.

import { createClient, type Transport } from '@connectrpc/connect';
import { live } from '@loams/proto';
import { fromJs, type Json, toJs } from './value.js';

export interface IndexInfo {
  name: string;
  fields: string[];
}
export interface TableInfo {
  name: string;
  id: string;
  indexes: IndexInfo[];
}
export type Doc = { [k: string]: Json };

/** The `_system:query` arguments the page builds. */
export interface QueryArgs {
  table: string;
  index?: string;
  eq?: Json[];
  order?: 'asc' | 'desc';
  limit?: number;
}

/** One Transition as the page needs it. A heartbeat has `changed: false`. */
export interface WatchEvent {
  changed: boolean;
  rows?: Doc[];
  error?: string;
}

export interface MutateResult {
  commitTs: string;
  result: Json;
}

const QUERY_ID = 1;

export interface LiveApi {
  tables(signal?: AbortSignal): Promise<TableInfo[]>;
  query(args: QueryArgs, signal?: AbortSignal): Promise<Doc[]>;
  watch(args: QueryArgs, signal: AbortSignal): AsyncIterable<WatchEvent>;
  mutate(fn: string, args: Json, idempotencyKey: string): Promise<MutateResult>;
}

const asDocs = (v: Json): Doc[] => (Array.isArray(v) ? (v as Doc[]) : []);

export function createLiveApi(transport: Transport): LiveApi {
  const client = createClient(live.LiveService, transport);
  return {
    async tables(signal) {
      const res = await client.query({ function: '_system:tables', args: fromJs({}) }, { signal });
      return (toJs(res.result) as unknown as TableInfo[]).map((t) => ({
        name: t.name,
        id: String(t.id),
        indexes: t.indexes ?? [],
      }));
    },
    async query(args, signal) {
      const res = await client.query({ function: '_system:query', args: fromJs(args) }, { signal });
      return asDocs(toJs(res.result));
    },
    async *watch(args, signal) {
      const stream = client.watch(
        {
          start: {
            case: 'initial',
            value: {
              version: 1n,
              queries: [{ queryId: QUERY_ID, function: '_system:query', args: fromJs(args) }],
            },
          },
        },
        { signal },
      );
      for await (const t of stream) {
        const u = t.updates.find((x) => x.queryId === QUERY_ID);
        if (!u) {
          yield { changed: false };
          continue;
        }
        if (u.update.case === 'value') yield { changed: true, rows: asDocs(toJs(u.update.value)) };
        else if (u.update.case === 'error') yield { changed: true, error: u.update.value.message };
        else yield { changed: true, rows: [] };
      }
    },
    async mutate(fn, args, idempotencyKey) {
      const res = await client.mutate({ function: fn, args: fromJs(args), idempotencyKey });
      return { commitTs: res.commitTs.toString(), result: toJs(res.result) };
    },
  };
}
