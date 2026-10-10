// The Graph page's client: loams.graph.v1 (GraphAdminService, GraphService) and
// loams.instance.v1 over the active server's `transport` (R0.20). The calls carry the
// desktop contract of §48 §18.2: max_rows 1000, timeout_ms 30000, consistency strong,
// read_only on unless the person turns it off. The fixtures in
// conformance/graph/desktop/ pin these requests.

import { create } from '@bufbuild/protobuf';
import { Code, ConnectError, createClient, type Transport } from '@connectrpc/connect';
import { errors, graph, instance } from '@loams/proto';

/** What the page asks for on every Execute (§48 §18.2). */
export const PAGE_MAX_ROWS = 1000;
export const PAGE_TIMEOUT_MS = 30_000;
/** "Stream all" stops here: the page holds at most this many rows. */
export const STREAM_MAX_ROWS = 100_000;
/** `ListGraphs` page size. */
export const LIST_PAGE_SIZE = 50;

export type Language = 'gql' | 'cypher';

export interface RunInput {
  namespace: string;
  graph: string;
  statement: string;
  language: Language;
  parameters: Record<string, graph.Value>;
  readOnly: boolean;
}

/** A refused call, decoded: the Connect code, `ErrorInfo.reason` and its metadata (§48 §8.3). */
export interface GraphFailure {
  code: Code;
  /** The code in Connect's wire form, for example `invalid_argument`. */
  codeName: string;
  message: string;
  reason?: string;
  metadata: Record<string, string>;
  gqlstatus?: string;
  /** 1-based line and column, length in characters (R7.4). */
  position?: { line: number; column: number; length: number };
}

const wireCode = (c: Code): string =>
  (Code[c] ?? 'unknown').replace(/[A-Z]/g, (m, i: number) => (i ? '_' : '') + m.toLowerCase());

function positive(v: string | undefined): number | undefined {
  if (v === undefined || !/^\d+$/.test(v)) return undefined;
  const n = Number(v);
  return n > 0 ? n : undefined;
}

/** Decodes any thrown value; a ConnectError's `loams.errors.v1.ErrorInfo` gives the reason. */
export function toFailure(e: unknown): GraphFailure {
  const err = ConnectError.from(e);
  const info = err.findDetails(errors.ErrorInfoSchema)[0];
  const metadata = { ...(info?.metadata ?? {}) };
  const line = positive(metadata.line);
  const column = positive(metadata.column);
  const length = positive(metadata.length);
  return {
    code: err.code,
    codeName: wireCode(err.code),
    message: err.rawMessage,
    reason: info?.reason || undefined,
    metadata,
    gqlstatus: metadata.gqlstatus || undefined,
    position: line && column ? { line, column, length: length ?? 1 } : undefined,
  };
}

const LANG: Record<Language, graph.QueryLanguage> = {
  // GQL is the server's default: the page leaves `language` unset for it, as the fixtures do.
  gql: graph.QueryLanguage.UNSPECIFIED,
  cypher: graph.QueryLanguage.CYPHER,
};

const strong = () =>
  create(graph.ConsistencySchema, { level: { case: 'strong', value: create(graph.StrongSchema) } });

function executeRequest(input: RunInput, maxRows: number): graph.ExecuteRequest {
  return create(graph.ExecuteRequestSchema, {
    namespace: input.namespace,
    graph: input.graph,
    statement: input.statement,
    language: LANG[input.language],
    parameters: input.parameters,
    readOnly: input.readOnly,
    consistency: strong(),
    timeoutMs: PAGE_TIMEOUT_MS,
    maxRows,
  });
}

/** A fresh idempotency key for a create or delete. */
export function idempotencyKey(): string {
  const c = globalThis.crypto;
  if (c?.randomUUID) return c.randomUUID();
  return `k-${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
}

export function createGraphClient(transport: Transport) {
  const inst = createClient(instance.InstanceService, transport);
  const admin = createClient(graph.GraphAdminService, transport);
  const data = createClient(graph.GraphService, transport);

  return {
    getInstance: () => inst.getInstance({}),

    /**
     * The `variant` a server that lists graph unavailable names in `ErrorInfo.metadata`
     * (`feature_not_in_variant`), or undefined when it does not say.
     */
    async notInVariant(): Promise<string | undefined> {
      try {
        await admin.getEngineInfo({});
        return undefined;
      } catch (e) {
        const f = toFailure(e);
        return f.reason === 'feature_not_in_variant' ? f.metadata.variant : undefined;
      }
    },

    listGraphs: (namespace: string, pageToken = '') =>
      admin.listGraphs({ namespace, pageSize: LIST_PAGE_SIZE, pageToken }),

    createGraph: (namespace: string, name: string, languages: Language[]) =>
      admin.createGraph({
        namespace,
        name,
        mode: graph.GraphMode.OWNED,
        languages: languages.map((l) =>
          l === 'gql' ? graph.QueryLanguage.GQL : graph.QueryLanguage.CYPHER,
        ),
        idempotencyKey: idempotencyKey(),
      }),

    deleteGraph: (namespace: string, name: string) =>
      admin.deleteGraph({ namespace, name, idempotencyKey: idempotencyKey() }),

    getSchema: (namespace: string, name: string) => admin.getSchema({ namespace, name }),

    execute: (input: RunInput, signal?: AbortSignal) =>
      data.execute(executeRequest(input, PAGE_MAX_ROWS), { signal }),

    /** "Stream all": the same statement, every row up to the page's cap. */
    executeStream: (input: RunInput, signal?: AbortSignal) =>
      data.executeStream(
        create(graph.ExecuteStreamRequestSchema, {
          request: executeRequest(input, STREAM_MAX_ROWS),
        }),
        { signal },
      ),

    /**
     * EXPLAIN, or PROFILE when `profile`. A PROFILE runs the statement, which must itself
     * begin with PROFILE (R6.8): the page adds the keyword to what it sends when the
     * statement lacks it.
     */
    explain: (input: RunInput, profile: boolean, signal?: AbortSignal) =>
      data.explain(
        create(graph.ExplainRequestSchema, {
          namespace: input.namespace,
          graph: input.graph,
          statement: profile ? withProfile(input.statement) : input.statement,
          language: LANG[input.language],
          parameters: input.parameters,
          ...(profile ? { profile: true, timeoutMs: PAGE_TIMEOUT_MS, consistency: strong() } : {}),
        }),
        { signal },
      ),
  };
}

export type GraphClient = ReturnType<typeof createGraphClient>;

export function withProfile(statement: string): string {
  return /^\s*PROFILE\b/i.test(statement) ? statement : `PROFILE ${statement.trimStart()}`;
}
