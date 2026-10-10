// A fake server for the page's tests, answering from the desktop contract fixtures of
// GR1 Task 7 (conformance/graph/desktop/*.json, R7.1). It is a Connect router transport,
// so requests and answers cross the real Connect codec. It records every request the
// page sends, in proto3 JSON, so the tests can compare them with the fixtures' requests.

import {
  create,
  type DescMessage,
  fromJson,
  type JsonObject,
  type JsonValue,
  toJson,
} from '@bufbuild/protobuf';
import { Code, ConnectError, createRouterTransport } from '@connectrpc/connect';
import { errors, graph, instance } from '@loams/proto';
import errorDenied from '../../../../conformance/graph/desktop/error_denied.json';
import errorSyntax from '../../../../conformance/graph/desktop/error_syntax.json';
import executeGraph from '../../../../conformance/graph/desktop/execute_graph.json';
import executeTable from '../../../../conformance/graph/desktop/execute_table.json';
import executeTruncated from '../../../../conformance/graph/desktop/execute_truncated.json';
import explain from '../../../../conformance/graph/desktop/explain.json';
import instanceFixture from '../../../../conformance/graph/desktop/instance.json';
import listGraphs from '../../../../conformance/graph/desktop/list_graphs.json';
import schema from '../../../../conformance/graph/desktop/schema.json';
import values from '../../../../conformance/graph/desktop/values.json';

export interface Exchange {
  method: string;
  request: JsonObject;
  response?: JsonValue;
  chunks?: JsonValue[];
  error?: { code: string; message: string; reason: string; metadata?: Record<string, string> };
}

interface FixtureFile {
  description: string;
  exchanges: Exchange[];
}

const files = (...f: unknown[]) => (f as FixtureFile[]).flatMap((x) => x.exchanges);

export const FIXTURES = {
  instance: (instanceFixture as unknown as FixtureFile).exchanges[0] as Exchange,
  listGraphs: (listGraphs as unknown as FixtureFile).exchanges[0] as Exchange,
  schema: (schema as unknown as FixtureFile).exchanges[0] as Exchange,
  executeTable: (executeTable as unknown as FixtureFile).exchanges[0] as Exchange,
  executeGraph: (executeGraph as unknown as FixtureFile).exchanges[0] as Exchange,
  truncated: (executeTruncated as unknown as FixtureFile).exchanges as Exchange[],
  errorSyntax: (errorSyntax as unknown as FixtureFile).exchanges[0] as Exchange,
  errorDenied: (errorDenied as unknown as FixtureFile).exchanges[0] as Exchange,
  explain: (explain as unknown as FixtureFile).exchanges as Exchange[],
  values: (values as unknown as { cases: { name: string; value: JsonValue }[] }).cases,
};

const ALL: Exchange[] = files(
  listGraphs,
  schema,
  executeTable,
  executeGraph,
  executeTruncated,
  errorSyntax,
  errorDenied,
  explain,
);

/** `permission_denied` → Code.PermissionDenied. */
export function codeOf(wire: string): Code {
  const pascal = wire.replace(/(^|_)([a-z])/g, (_m, _u, c: string) => c.toUpperCase());
  return Code[pascal as keyof typeof Code];
}

export function refusal(e: NonNullable<Exchange['error']>): ConnectError {
  return new ConnectError(e.message, codeOf(e.code), undefined, [
    {
      desc: errors.ErrorInfoSchema,
      value: create(errors.ErrorInfoSchema, { reason: e.reason, metadata: e.metadata ?? {} }),
    },
  ]);
}

/** The fields a fixture is found by. The tests compare the rest of the request themselves. */
function key(method: string, req: JsonObject): string {
  const r = (method.endsWith('/ExecuteStream') ? req.request : req) as JsonObject | undefined;
  return JSON.stringify([
    method,
    r?.namespace ?? '',
    r?.graph ?? r?.name ?? '',
    r?.statement ?? '',
    r?.profile ?? false,
  ]);
}

export interface Call {
  method: string;
  request: JsonObject;
}

export type Availability = 'available' | 'not_in_variant' | 'absent';

export interface FakeOptions {
  availability?: Availability;
  /** Extra exchanges, found before the fixtures. */
  exchanges?: Exchange[];
  /** Answers PERMISSION_DENIED to CreateGraph and DeleteGraph. */
  denyAdmin?: boolean;
  /** Called with each Execute; return a response to answer it instead of a fixture. */
  execute?: (req: graph.ExecuteRequest) => graph.ExecuteResponse | undefined;
}

export function fakeServer(opts: FakeOptions = {}) {
  const calls: Call[] = [];
  const exchanges = [...(opts.exchanges ?? []), ...ALL];
  const listed = fromJson(graph.ListGraphsResponseSchema, FIXTURES.listGraphs.response ?? {});
  let graphs = [...listed.graphs];

  const record = <T extends DescMessage>(method: string, schemaOf: T, req: unknown) => {
    const request = toJson(schemaOf, req as never) as JsonObject;
    calls.push({ method, request });
    return request;
  };
  const find = (method: string, request: JsonObject) => {
    const k = key(method, request);
    const ex = exchanges.find((e) => e.method === method && key(method, e.request) === k);
    if (!ex) throw new ConnectError(`no fixture for ${k}`, Code.Unimplemented);
    if (ex.error) throw refusal(ex.error);
    return ex;
  };
  const S = 'loams.graph.v1.GraphService/';
  const A = 'loams.graph.v1.GraphAdminService/';

  const transport = createRouterTransport(({ service }) => {
    service(instance.InstanceService, {
      getInstance: (req) => {
        record(
          'loams.instance.v1.InstanceService/GetInstance',
          instance.GetInstanceRequestSchema,
          req,
        );
        const res = fromJson(instance.GetInstanceResponseSchema, FIXTURES.instance.response ?? {});
        res.instanceId = '01JTESTSERVER';
        const availability = opts.availability ?? 'available';
        if (availability === 'absent') {
          res.services = [];
          res.apiVersions = [];
        } else if (availability === 'not_in_variant') {
          for (const s of res.services) s.available = false;
          res.apiVersions = [];
        }
        return res;
      },
    });
    service(graph.GraphAdminService, {
      getEngineInfo: () => {
        if (opts.availability === 'not_in_variant') {
          throw refusal({
            code: 'unimplemented',
            message: 'GraphAdminService/GetEngineInfo is not in the standard variant',
            reason: 'feature_not_in_variant',
            metadata: { variant: 'standard' },
          });
        }
        return create(graph.EngineInfoSchema, { engineVersion: 'grafeo 0.5.43' });
      },
      listGraphs: (req) => {
        record(`${A}ListGraphs`, graph.ListGraphsRequestSchema, req);
        return create(graph.ListGraphsResponseSchema, {
          graphs: graphs.filter((g) => g.namespace === req.namespace),
        });
      },
      createGraph: (req) => {
        record(`${A}CreateGraph`, graph.CreateGraphRequestSchema, req);
        if (opts.denyAdmin) {
          throw refusal({
            code: 'permission_denied',
            message: 'graph admin needs the admin role',
            reason: 'permission_denied',
          });
        }
        const g = create(graph.GraphSchema$, {
          namespace: req.namespace,
          name: req.name,
          id: 'gr_01JTESTGRAPH0000000000000',
          mode: req.mode,
          languages: req.languages,
          state: graph.GraphState.READY,
          version: 1n,
        });
        graphs = [...graphs, g].sort((a, b) => a.name.localeCompare(b.name));
        return g;
      },
      deleteGraph: (req) => {
        record(`${A}DeleteGraph`, graph.DeleteGraphRequestSchema, req);
        if (opts.denyAdmin) {
          throw refusal({
            code: 'permission_denied',
            message: 'graph admin needs the admin role',
            reason: 'permission_denied',
          });
        }
        graphs = graphs.filter((g) => g.name !== req.name);
        return { id: 'op-1', kind: 'graph.delete' };
      },
      getSchema: (req) => {
        const request = record(`${A}GetSchema`, graph.GetSchemaRequestSchema, req);
        return fromJson(graph.GraphSchemaSchema, find(`${A}GetSchema`, request).response ?? {});
      },
    });
    service(graph.GraphService, {
      execute: (req) => {
        const request = record(`${S}Execute`, graph.ExecuteRequestSchema, req);
        const custom = opts.execute?.(req);
        if (custom) return custom;
        return fromJson(graph.ExecuteResponseSchema, find(`${S}Execute`, request).response ?? {});
      },
      async *executeStream(req) {
        const request = record(`${S}ExecuteStream`, graph.ExecuteStreamRequestSchema, req);
        for (const c of find(`${S}ExecuteStream`, request).chunks ?? []) {
          yield fromJson(graph.ResultChunkSchema, c);
        }
      },
      explain: (req) => {
        const request = record(`${S}Explain`, graph.ExplainRequestSchema, req);
        return fromJson(graph.PlanSchema, find(`${S}Explain`, request).response ?? {});
      },
    });
  });
  return { transport, calls, methods: () => calls.map((c) => c.method.split('/')[1]) };
}

/** A storage that lives for one test. */
export function memoryStorage(): Storage {
  const m = new Map<string, string>();
  return {
    get length() {
      return m.size;
    },
    clear: () => m.clear(),
    getItem: (k) => m.get(k) ?? null,
    key: (i) => [...m.keys()][i] ?? null,
    removeItem: (k) => void m.delete(k),
    setItem: (k, v) => void m.set(k, String(v)),
  };
}
