// The collection handle (SDKP §2.8).
//
//   const notes = loams.collection('notes');
//   await notes.upsert([{ id: 1, source: { title: 'Refunds' } }]);
//   const hits = await notes.search(notes.query().text('refund', { fields: ['body'] }));
//
// Every call delegates to the module call of the same meaning with
// `collection` (and `namespace`, when the handle was given one) filled in. The
// handle holds no state of its own: two handles on the same collection are
// interchangeable, and a handle is as cheap as the object literal it is.

import { fromJson, type JsonObject } from '@bufbuild/protobuf';
import { Code } from '@connectrpc/connect';
import { DocumentIdSchema } from '@loams/proto/documents';
import type { DocumentInput, PatchChange, ScrollOptions, Target, UnaryCall } from './app.js';
import type { DocId, FilterJson } from './builders/filter.js';
import { Search, type VectorOptions } from './builders/search.js';
import type { BatchOptions, BatchResult } from './runtime/batch.js';
import type { CallOptions } from './runtime/call.js';
import { LoamsError } from './runtime/errors.js';
import { documentIdJson } from './app.js';

/** The module calls a handle delegates to. Each is optional so a handle can
 * be built over a facade that lacks a module; calling into a missing one is a
 * clear error rather than a `TypeError`. */
export interface HandleModules {
  documents?: {
    upsert?: (target: Target, docs: DocumentInput[], options?: CallOptions) => Promise<unknown>;
    delete?: (target: Target, ids: DocId[], options?: CallOptions) => Promise<unknown>;
    patch?: (target: Target, id: DocId, change: PatchChange, options?: CallOptions) => Promise<unknown>;
    writeBatch?: (
      target: Target,
      docs: Iterable<DocumentInput> | AsyncIterable<DocumentInput>,
      options?: BatchOptions & CallOptions,
    ) => Promise<BatchResult>;
    scrollAll?: (target: Target, options?: ScrollOptions) => AsyncGenerator<unknown>;
    get?: UnaryCall;
    count?: UnaryCall;
  };
  search?: { query?: UnaryCall };
  collections?: { get?: UnaryCall; drop?: UnaryCall };
}

function need<F>(fn: F | undefined, name: string): F {
  if (fn === undefined) {
    throw new LoamsError(`this SDK's facade has no ${name}`, { code: Code.Unimplemented });
  }
  return fn;
}

/** A collection, by name, with the document and search calls bound to it. */
export class CollectionHandle {
  readonly name: string;
  readonly namespace: string | undefined;
  readonly #modules: HandleModules;

  constructor(modules: HandleModules, name: string, options: { namespace?: string } = {}) {
    this.#modules = modules;
    this.name = name;
    this.namespace = options.namespace;
  }

  get #target(): Target {
    return this.namespace === undefined
      ? { collection: this.name }
      : { collection: this.name, namespace: this.namespace };
  }

  /** The request fields every call carries. */
  get #fields(): { collection: string; namespace?: string } {
    return this.#target;
  }

  /** Writes (creates or replaces) documents. */
  upsert(docs: DocumentInput[], options?: CallOptions) {
    return need(this.#modules.documents?.upsert, 'documents.upsert')(this.#target, docs, options);
  }

  /** Reads documents by id. */
  get(ids: DocId[], options: CallOptions & { select?: JsonObject } = {}) {
    const { select, ...call } = options;
    return need(this.#modules.documents?.get, 'documents.get')(
      {
        ...this.#fields,
        ids: ids.map((id) => fromJson(DocumentIdSchema, documentIdJson(id))),
        ...(select === undefined ? {} : { select }),
      },
      call,
    );
  }

  /** Removes documents by id. */
  delete(ids: DocId[], options?: CallOptions) {
    return need(this.#modules.documents?.delete, 'documents.delete')(this.#target, ids, options);
  }

  /** Changes part of one document. */
  patch(id: DocId, change: PatchChange, options?: CallOptions) {
    return need(this.#modules.documents?.patch, 'documents.patch')(this.#target, id, change, options);
  }

  /** A chunked bulk upsert (R13). */
  writeBatch(
    docs: Iterable<DocumentInput> | AsyncIterable<DocumentInput>,
    options?: BatchOptions & CallOptions,
  ) {
    return need(this.#modules.documents?.writeBatch, 'documents.writeBatch')(this.#target, docs, options);
  }

  /** How many documents match `where` (every document without one). */
  count(options: CallOptions & { where?: FilterJson } = {}) {
    const { where, ...call } = options;
    return need(this.#modules.documents?.count, 'documents.count')(
      { ...this.#fields, ...(where === undefined ? {} : { where }) },
      call,
    );
  }

  /** Every document, page by page. */
  scrollAll(options?: ScrollOptions) {
    return need(this.#modules.documents?.scrollAll, 'documents.scrollAll')(this.#target, options);
  }

  /** A search builder already pointed at this collection. */
  query(): Search {
    return Search.on(this.name, this.namespace === undefined ? {} : { namespace: this.namespace });
  }

  /** Runs a search; the builder's collection is replaced by this one. */
  search(builder: Search, options?: CallOptions) {
    const json = builder.toJson();
    json.collection = this.name;
    if (this.namespace !== undefined) {
      json.namespace = this.namespace;
    }
    return need(this.#modules.search?.query, 'search.query')(Search.fromJson(json).build(), options);
  }

  /** A kNN search on one vector field. */
  vectorSearch(
    field: string,
    vector: readonly number[],
    options: CallOptions & VectorOptions & { limit?: number; filter?: FilterJson } = {},
  ) {
    const { limit, filter, k, exact, nprobes, refineFactor, ef, oversampling, distance, ...call } = options;
    let builder = this.query().vector(field, vector, { k, exact, nprobes, refineFactor, ef, oversampling, distance });
    if (filter !== undefined) builder = builder.filter(filter);
    if (limit !== undefined) builder = builder.limit(limit);
    return this.search(builder, call);
  }

  /** The collection's schema and stats. */
  info(options?: CallOptions) {
    return need(this.#modules.collections?.get, 'collections.get')(this.#fields, options);
  }

  /** Drops the collection. */
  drop(options?: CallOptions) {
    return need(this.#modules.collections?.drop, 'collections.drop')(this.#fields, options);
  }
}

