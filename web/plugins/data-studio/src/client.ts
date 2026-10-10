// The data plane client behind Data Studio: the loams.collection.v1 Connect
// services over the active server's `transport`, plus the REST SQL route
// (R0.10) over `platform.fetch`. Everything the pages see is plain JSON.

import { create, fromJson, type JsonObject, type JsonValue, toJson } from '@bufbuild/protobuf';
import { ValueSchema } from '@bufbuild/protobuf/wkt';
import { createClient, type Transport } from '@connectrpc/connect';
import { collection, documents, query } from '@loams/proto';

export type CollectionInfo = collection.CollectionInfo;

/** One document as the grid and the side panel show it. */
export interface DocRow {
  /** The id as text (a uint is a decimal string). */
  id: string;
  source: JsonObject;
  vectors: Record<string, JsonValue>;
}

export interface ScrollPage {
  rows: DocRow[];
  /** The cursor for the next page, or undefined on the last page. */
  next?: documents.DocumentId;
}

export interface SqlResult {
  columns: { name: string; type: string }[];
  rows: JsonValue[][];
  truncated: boolean;
}

export interface SearchHit {
  id: string;
  score: number;
  source: JsonObject;
}

export interface CollectionSpec {
  name: string;
  /** The REST schema JSON, carried verbatim (see connect_collections.rs). */
  schema: JsonObject;
}

/** An error from the REST SQL route, `{error, message}`. */
export class SqlError extends Error {
  constructor(
    readonly code: string,
    message: string,
    readonly status: number,
  ) {
    super(message);
    this.name = 'SqlError';
  }
}

export function idText(id: documents.DocumentId | undefined): string {
  switch (id?.id.case) {
    case 'uint':
      return id.id.value.toString();
    case 'string':
    case 'uuid':
      return id.id.value;
    default:
      return '';
  }
}

/** A document id from a JSON value: only a JSON integer is a uint; a string stays a string (a UUID string is a uuid). */
export function toDocumentId(value: unknown): documents.DocumentId {
  if (typeof value === 'number' && Number.isSafeInteger(value) && value >= 0) {
    return create(documents.DocumentIdSchema, { id: { case: 'uint', value: BigInt(value) } });
  }
  if (typeof value === 'string') {
    const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(value);
    return create(documents.DocumentIdSchema, {
      id: uuid ? { case: 'uuid', value } : { case: 'string', value },
    });
  }
  throw new Error(
    `A document id must be a string or a non-negative integer, got ${String(value)}.`,
  );
}

function toRow(d: documents.Document): DocRow {
  const source: JsonObject = d.source ?? {};
  const fields: JsonObject = {};
  for (const [k, v] of Object.entries(d.fields)) fields[k] = toJson(ValueSchema, v);
  const vectors: Record<string, JsonValue> = {};
  for (const [k, v] of Object.entries(d.vectors)) vectors[k] = toJson(ValueSchema, v);
  return {
    id: idText(d.id),
    source: Object.keys(source).length > 0 ? source : fields,
    vectors,
  };
}

export function createDataClient(
  transport: Transport,
  fetch: typeof globalThis.fetch,
  baseUrl = '',
) {
  const namespaces = createClient(collection.NamespaceService, transport);
  const collections = createClient(collection.CollectionService, transport);
  const docs = createClient(documents.DocumentService, transport);
  const search = createClient(query.QueryService, transport);

  return {
    async listCollections(ns: string): Promise<CollectionInfo[]> {
      const out: CollectionInfo[] = [];
      let pageToken = '';
      do {
        const res = await collections.listCollections({ namespace: ns, pageSize: 100, pageToken });
        out.push(...res.collections);
        pageToken = res.nextPageToken;
      } while (pageToken);
      return out;
    },

    getCollection(ns: string, name: string): Promise<CollectionInfo> {
      return collections.getCollection({ namespace: ns, collection: name });
    },

    async createNamespace(ns: string): Promise<void> {
      await namespaces.createNamespace({ namespace: ns });
    },

    createCollection(ns: string, spec: CollectionSpec): Promise<CollectionInfo> {
      return collections.createCollection({
        namespace: ns,
        name: spec.name,
        schema: spec.schema,
      });
    },

    async scroll(
      ns: string,
      coll: string,
      opts: { pageToken?: documents.DocumentId; limit: number },
    ): Promise<ScrollPage> {
      const res = await docs.scrollDocuments({
        namespace: ns,
        collection: coll,
        limit: opts.limit,
        ...(opts.pageToken ? { pageToken: opts.pageToken } : {}),
      });
      return { rows: res.documents.map(toRow), next: res.next };
    },

    async count(ns: string, coll: string, filter?: JsonValue): Promise<number | undefined> {
      const res = await docs.countDocuments({
        namespace: ns,
        collection: coll,
        ...(filter === undefined ? {} : { filter: fromJson(ValueSchema, filter) }),
      });
      return res.count === undefined ? undefined : Number(res.count);
    },

    /** `ir` is the SearchRequest in proto JSON, without namespace and collection. */
    async search(ns: string, coll: string, ir: JsonObject): Promise<SearchHit[]> {
      const request = fromJson(query.SearchRequestSchema, {
        ...ir,
        namespace: ns,
        collection: coll,
      });
      const res = await search.search(request);
      return res.hits.map((h) => ({
        id: idText(h.pk),
        score: h.score,
        source: h.source ?? {},
      }));
    },

    /** Upserts a batch of `{id, source, vectors}`; returns the write token. */
    async write(
      ns: string,
      coll: string,
      batch: { id: unknown; source: JsonObject; vectors?: Record<string, JsonValue> }[],
      idempotencyKey: string,
    ): Promise<string> {
      const res = await docs.writeDocuments({
        namespace: ns,
        collection: coll,
        idempotencyKey,
        ops: batch.map((d) =>
          create(documents.WriteOpSchema, {
            op: {
              case: 'upsert',
              value: create(documents.WriteDocumentSchema, {
                id: toDocumentId(d.id),
                source: d.source,
                vectors: Object.fromEntries(
                  Object.entries(d.vectors ?? {}).map(([k, v]) => [k, fromJson(ValueSchema, v)]),
                ),
              }),
            },
          }),
        ),
      });
      return res.token;
    },

    async sql(ns: string, text: string): Promise<SqlResult> {
      const res = await fetch(`${baseUrl}/v1/namespaces/${encodeURIComponent(ns)}/sql`, {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ query: text }),
        credentials: 'include',
      });
      const body = (await res.json().catch(() => ({}))) as Record<string, unknown>;
      if (!res.ok) {
        throw new SqlError(
          String(body.error ?? body.code ?? 'error'),
          String(body.message ?? res.statusText),
          res.status,
        );
      }
      return body as unknown as SqlResult;
    },
  };
}

export type DataClient = ReturnType<typeof createDataClient>;

/** A collection's schema as the REST schema JSON. */
export function schemaOf(info: CollectionInfo): JsonObject {
  return info.schema ?? {};
}

/** The dense vector fields of a schema: name and dimension. */
export function vectorFields(schema: JsonObject): { name: string; dim: number }[] {
  const list = Array.isArray(schema.vectors) ? schema.vectors : [];
  return list.flatMap((v) => {
    if (typeof v !== 'object' || v === null || Array.isArray(v)) return [];
    return [{ name: String(v.name ?? ''), dim: Number(v.dim ?? 0) }];
  });
}
