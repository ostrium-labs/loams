// The hand-written half of the app modules (SDKP §2.3–§2.5, §4).
//
// The generated facade gives one function per proto method. A developer
// building an app wants a few more, each a thin loop or shorthand over those:
//
// - `documents.upsert`, `delete`, `patch` and `writeBatch` over `write`;
// - `documents.scrollAll` over `scroll` (its cursor is a `DocumentId`, not an
//   AIP-158 string, so the generic `listAll` does not fit it);
// - `search.text`, `search.hybrid` and `search.queryAll` over `search.query`;
// - `sql.records` over `sql.query`, and `sqlRecords` for an answer in hand;
// - `streams.subscribe` over `streams.fetch`;
// - `<call>All` for every paged call (`namespaces.listAll`,
//   `collections.listAll`), over the generic paging iterator.
//
// `attachAppCalls` adds them to the module objects the facade built, and only
// to modules that exist: an SDK whose facade has no `documents` module gets no
// `documents.upsert`. Nothing here names an RPC path or a retry class; those
// stay the generated bindings'.

import { fromJson, toJson, type JsonObject, type JsonValue } from '@bufbuild/protobuf';
import { ListValueSchema, ValueSchema, type ListValue } from '@bufbuild/protobuf/wkt';
import { Code } from '@connectrpc/connect';
import { WriteOpSchema } from '@loams/proto/documents';
import type { ModuleBinding } from './gen/facade.js';
import { idValue, type DocId, type FilterJson } from './builders/filter.js';
import { Search, consistencyJson, rrf, type ConsistencyLevel, type FusionJson } from './builders/search.js';
import { writeBatch, type BatchOptions, type BatchResult } from './runtime/batch.js';
import type { CallOptions } from './runtime/call.js';
import { InvalidArgumentError } from './runtime/errors.js';
import { paginate } from './runtime/pagination.js';
import { subscribe, type OffsetRecord, type Subscription, type SubscribeRequest } from './runtime/subscribe.js';

/** A generated unary call, as the hand-written layer sees it. */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export type UnaryCall = (request: any, options?: CallOptions) => Promise<any>;

/** One document to write. */
export interface DocumentInput {
  id: DocId;
  /** The document's `_source`. */
  source?: JsonObject;
  /** Dense vectors by name. */
  vectors?: Record<string, readonly number[]>;
  /** Sparse vectors by name. */
  sparseVectors?: Record<string, { indices: number[]; values: number[] }>;
}

/** Where a document call goes: the collection, and the namespace when it is
 * not the client's default. */
export interface Target {
  collection: string;
  namespace?: string;
}

/** A document id as `DocumentId`'s proto3 JSON: `{"uint": "1"}`,
 * `{"string": "a"}` or `{"uuid": "…"}`. */
export function documentIdJson(id: DocId): JsonObject {
  if (typeof id === 'string') {
    return { string: id };
  }
  const value = idValue(id);
  return typeof value === 'string' ? { string: value } : (value as JsonObject);
}

function documentJson(doc: DocumentInput): JsonObject {
  const out: JsonObject = { id: documentIdJson(doc.id) };
  if (doc.source !== undefined) {
    out.source = doc.source;
  }
  if (doc.vectors !== undefined) {
    out.vectors = Object.fromEntries(
      Object.entries(doc.vectors).map(([name, values]) => [name, [...values]]),
    );
  }
  if (doc.sparseVectors !== undefined) {
    out.sparseVectors = doc.sparseVectors as unknown as JsonObject;
  }
  return out;
}

/** The `WriteOp`s of the document conveniences, as proto3 JSON. */
export const WriteOps = {
  upsert(doc: DocumentInput): JsonObject {
    return { upsert: documentJson(doc) };
  },
  delete(id: DocId): JsonObject {
    return { delete: { id: documentIdJson(id) } };
  },
  patch(id: DocId, change: PatchChange): JsonObject {
    const patch: JsonObject = { id: documentIdJson(id) };
    if (change.mode !== undefined) {
      patch.mode = change.mode;
    }
    if (change.set !== undefined) {
      patch.source = change.set;
    }
    if (change.unset !== undefined && change.unset.length > 0) {
      patch.deleteKeys = [...change.unset];
    }
    if (change.vectors !== undefined) {
      patch.vectors = change.vectors as unknown as JsonObject;
    }
    if (change.upsert !== undefined) {
      patch.upsert = documentJson(change.upsert);
    }
    return { patch };
  },
} as const;

/** What a `patch` changes. */
export interface PatchChange {
  /** The keys to merge into `_source`. */
  set?: JsonObject;
  /** The top-level keys to remove. */
  unset?: string[];
  /** `merge_deep` (the default), `merge_top` or `replace`. */
  mode?: 'merge_deep' | 'merge_top' | 'replace';
  /** Vectors to set; `null` removes one. */
  vectors?: Record<string, readonly number[] | null>;
  /** The document to write when there is none to patch. */
  upsert?: DocumentInput;
}

function opsRequest(target: Target, ops: JsonObject[]) {
  return {
    ...(target.namespace === undefined ? {} : { namespace: target.namespace }),
    collection: target.collection,
    ops: ops.map((op) => fromJson(WriteOpSchema, op)),
  };
}

function targetOf(collection: string | Target): Target {
  return typeof collection === 'string' ? { collection } : collection;
}

/** The scroll options of `scrollAll`. */
export interface ScrollOptions extends CallOptions {
  /** A typed filter (`ScrollDocumentsRequest.where`, a `Query`'s proto3 JSON). */
  where?: FilterJson;
  /** Documents per page. */
  limit?: number;
  select?: JsonObject;
  /** How fresh each page must be. */
  read?: ConsistencyLevel;
}

/** The document conveniences over `documents.write` and `documents.scroll`. */
export function documentCalls(documents: { write: UnaryCall; scroll?: UnaryCall }) {
  return {
    /** Writes (creates or replaces) documents in one call. Answers the write's
     * response, whose `token` is its consistency token. */
    upsert(collection: string | Target, docs: DocumentInput[], options?: CallOptions) {
      return documents.write(opsRequest(targetOf(collection), docs.map(WriteOps.upsert)), options);
    },
    /** Removes documents by id in one call. */
    delete(collection: string | Target, ids: DocId[], options?: CallOptions) {
      return documents.write(opsRequest(targetOf(collection), ids.map(WriteOps.delete)), options);
    },
    /** Changes part of one document. */
    patch(collection: string | Target, id: DocId, change: PatchChange, options?: CallOptions) {
      return documents.write(opsRequest(targetOf(collection), [WriteOps.patch(id, change)]), options);
    },
    /** A bulk upsert, chunked, one chunk in flight (R13). */
    writeBatch(
      collection: string | Target,
      docs: Iterable<DocumentInput> | AsyncIterable<DocumentInput>,
      options: BatchOptions & CallOptions = {},
    ): Promise<BatchResult> {
      const target = targetOf(collection);
      return writeBatch<JsonObject>(
        (ops, callOptions) => documents.write(opsRequest(target, ops), callOptions),
        mapOps(docs),
        options,
      );
    },
    /** Every document, page by page, following `next` until it is unset. */
    async *scrollAll(collection: string | Target, options: ScrollOptions = {}) {
      if (documents.scroll === undefined) {
        throw new InvalidArgumentError('this SDK build has no documents.scroll', {
          code: Code.Unimplemented,
        });
      }
      const { where, limit, select, read, ...call } = options;
      const target = targetOf(collection);
      let after: unknown;
      for (;;) {
        const page = await documents.scroll(
          {
            ...(target.namespace === undefined ? {} : { namespace: target.namespace }),
            collection: target.collection,
            // `where` is a `Query`'s proto3 JSON (SDKP-12), which is what a
            // `Filter` already is.
            ...(where === undefined ? {} : { where }),
            ...(limit === undefined ? {} : { limit }),
            ...(select === undefined ? {} : { select }),
            ...(read === undefined ? {} : { consistency: consistencyJsonMessage(read) }),
            ...(after === undefined ? {} : { after }),
          },
          call,
        );
        for (const doc of (page?.documents ?? []) as unknown[]) {
          yield doc;
        }
        if (page?.next === undefined || page.next.id?.case === undefined) {
          return;
        }
        after = page.next;
      }
    },
  };
}

/** A `Consistency` init object for a call that takes one directly. */
function consistencyJsonMessage(level: ConsistencyLevel) {
  const json = consistencyJson(level);
  if ('atLeast' in json) {
    return { atLeast: json.atLeast as string };
  }
  return { freshness: json.freshness === 'FRESHNESS_EVENTUAL' ? 2 : 1 };
}

async function* mapOps(docs: Iterable<DocumentInput> | AsyncIterable<DocumentInput>) {
  for await (const doc of docs as AsyncIterable<DocumentInput>) {
    yield WriteOps.upsert(doc);
  }
}

/** The options of the search shorthands. */
export interface SearchShorthandOptions extends CallOptions {
  namespace?: string;
  filter?: FilterJson;
  limit?: number;
}

/** The search conveniences over `search.query`. */
export function searchCalls(search: { query: UnaryCall }) {
  const run = (builder: Search, options: CallOptions) => search.query(builder.build(), options);
  return {
    /** A text search: one field is a `match`, several a `multi_match`, none a
     * `query_string`. */
    text(
      collection: string,
      text: string,
      options: SearchShorthandOptions & { fields?: string[] } = {},
    ) {
      const { namespace, filter, limit, fields, ...call } = options;
      let builder = Search.on(collection, { namespace }).text(text, { fields });
      if (filter !== undefined) builder = builder.filter(filter);
      if (limit !== undefined) builder = builder.limit(limit);
      return run(builder, call);
    },
    /** Text plus vector, fused (RRF by default). */
    hybrid(
      collection: string,
      text: string,
      vector: readonly number[],
      options: SearchShorthandOptions & {
        field: string;
        fields?: string[];
        fusion?: FusionJson;
        k?: number;
      },
    ) {
      const { namespace, filter, limit, fields, field, fusion, k, ...call } = options;
      let builder = Search.on(collection, { namespace })
        .text(text, { fields, k })
        .vector(field, vector, { k })
        .fuse(fusion ?? rrf());
      if (filter !== undefined) builder = builder.filter(filter);
      if (limit !== undefined) builder = builder.limit(limit);
      return run(builder, call);
    },
    /**
     * Every hit, page after page, following `search_after` from the last hit's
     * `sortValues` until a page comes back short. The sort must end in a
     * primary-key tiebreaker (`Sort.pk()`), or two hits with equal sort values
     * could straddle a page and be skipped or repeated; without one this
     * refuses with `invalid_argument` before any call.
     */
    async *queryAll(builder: Search, options: CallOptions = {}) {
      if (!builder.hasTiebreak) {
        throw new InvalidArgumentError(
          'search.queryAll needs a sort that ends in the primary key, such as .sort(Sort.score(), Sort.pk())',
          { code: Code.InvalidArgument, reason: 'search_after_needs_tiebreak', metadata: { field: 'sort' } },
        );
      }
      const limit = (builder.toJson().limit as number | undefined) ?? 10;
      let page = builder;
      for (;;) {
        const answer = await run(page, options);
        const hits = (answer?.hits ?? []) as { sortValues?: unknown[] }[];
        for (const hit of hits) {
          yield hit;
        }
        const last = hits[hits.length - 1];
        if (hits.length < limit || last === undefined || last.sortValues === undefined) {
          return;
        }
        page = builder.after(last.sortValues as JsonValue[]);
      }
    },
  };
}

/** The parts of a `sql.query` answer `sqlRecords` reads. */
export interface SqlAnswer {
  columns: readonly { name: string }[];
  rows: readonly ListValue[];
}

/**
 * A `sql.query` answer's rows as one object per row, keyed by column name
 * (SDKP §2.5). A value is its JSON: a 64-bit integer past 2^53 is the decimal
 * string the server sent, so it stays exact. A column name that repeats keeps
 * its last value, as a JavaScript object must.
 */
export function sqlRecords(answer: SqlAnswer): Record<string, JsonValue>[] {
  const names = answer.columns.map((column) => column.name);
  return answer.rows.map((row) => {
    const values = toJson(ListValueSchema, row) as JsonValue[];
    const record: Record<string, JsonValue> = {};
    names.forEach((name, at) => {
      record[name] = values[at] ?? null;
    });
    return record;
  });
}

/** The options of `sql.records`. */
export interface SqlRecordsOptions extends CallOptions {
  namespace?: string;
  maxRows?: number;
}

/** The SQL conveniences over `sql.query`. */
export function sqlCalls(sql: { query: UnaryCall }) {
  return {
    /**
     * One read-only statement with its `$1…$n` values, answered as records
     * (`sqlRecords`). The values are JSON scalars; the server casts each to the
     * type its placeholder was inferred as.
     */
    async records(
      statement: string,
      params: readonly JsonValue[] = [],
      options: SqlRecordsOptions = {},
    ): Promise<Record<string, JsonValue>[]> {
      const { namespace, maxRows, ...call } = options;
      const answer = (await sql.query(
        {
          sql: statement,
          params: params.map((value) => fromJson(ValueSchema, value)),
          ...(namespace !== undefined ? { namespace } : {}),
          ...(maxRows !== undefined ? { maxRows } : {}),
        },
        call,
      )) as SqlAnswer;
      return sqlRecords(answer);
    },
  };
}

/** The stream conveniences over `streams.fetch`. */
export function streamCalls(streams: { fetch: UnaryCall }) {
  return {
    /** Every record from `fromOffset` on, as an async iterable (R14). */
    subscribe<R extends OffsetRecord = OffsetRecord>(
      request: SubscribeRequest,
      options: CallOptions = {},
    ): Subscription<R> {
      return subscribe<R>(
        (fetchRequest, loopOptions) =>
          streams.fetch(fetchRequest, { ...options, signal: loopOptions?.signal }),
        request,
        { signal: options.signal },
      );
    },
  };
}

/**
 * Adds the hand-written calls to the generated module objects, in place.
 *
 * Every paged call gets `<call>All`; `documents`, `search` and `streams` get
 * their conveniences when the facade generated them. A name the facade
 * already uses is never overwritten.
 */
const UNSAFE_KEYS = new Set(['__proto__', 'constructor', 'prototype']);

export function attachAppCalls(
  modules: Record<string, unknown>,
  bindings: readonly ModuleBinding[],
): void {
  const add = (module: string, calls: Record<string, unknown>) => {
    // Module names come from the generated bindings, but an own-property
    // check keeps a name such as `__proto__` from reaching Object.prototype.
    if (!Object.hasOwn(modules, module) || UNSAFE_KEYS.has(module)) {
      return;
    }
    const target = modules[module] as Record<string, unknown> | undefined;
    if (target === undefined) {
      return;
    }
    for (const [name, fn] of Object.entries(calls)) {
      if (!UNSAFE_KEYS.has(name) && !(name in target)) {
        target[name] = fn;
      }
    }
  };
  for (const module of bindings) {
    const target = modules[module.name] as Record<string, unknown> | undefined;
    if (target === undefined) {
      continue;
    }
    for (const binding of module.calls) {
      if (binding.pagination === null || binding.streaming !== 'unary') {
        continue;
      }
      const call = target[binding.name] as UnaryCall | undefined;
      if (typeof call !== 'function') {
        continue;
      }
      add(module.name, {
        [`${binding.name}All`]: (request: object = {}, options?: CallOptions) =>
          paginate(binding, call, request, options),
      });
    }
  }
  const documents = modules.documents as { write?: UnaryCall; scroll?: UnaryCall } | undefined;
  if (typeof documents?.write === 'function') {
    add('documents', documentCalls(documents as { write: UnaryCall; scroll?: UnaryCall }));
  }
  const search = modules.search as { query?: UnaryCall } | undefined;
  if (typeof search?.query === 'function') {
    add('search', searchCalls(search as { query: UnaryCall }));
  }
  const sql = modules.sql as { query?: UnaryCall } | undefined;
  if (typeof sql?.query === 'function') {
    add('sql', sqlCalls(sql as { query: UnaryCall }));
  }
  const streams = modules.streams as { fetch?: UnaryCall } | undefined;
  if (typeof streams?.fetch === 'function') {
    add('streams', streamCalls(streams as { fetch: UnaryCall }));
  }
}

