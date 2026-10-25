// The `Loams` object: one client with namespaced modules (design §44 §7.1).
//
//   const loams = new Loams({ endpoint, auth: apiKey(process.env.LOAMS_API_KEY) });
//   const info = await loams.instance.getInstance({});
//   for await (const t of loams.live.watch({ start: { case: 'initial', value: querySet } })) {}
//
// What is generated and what is hand-written, once more, because it decides
// where a change goes. The **module surface** is generated: `gen/facade.ts` has
// one interface per annotated service and one method per `FacadeOptions`
// call, with the generated message types, and `MODULES` says which service and
// which retry class each call has. The **runtime** behind those methods is
// hand-written, once, in `runtime/`: transport, credentials, retry, errors,
// tokens, pagination, streams. This file is the thin join — it reads the
// generated table, builds one object per module out of it, and hands every call
// to the same invoker. It contains no method names and no RPC paths, which is
// why annotating a proto is enough to add an SDK method.

import { Code } from '@connectrpc/connect';
import type { Transport } from '@connectrpc/connect';
import {
  MODULES,
  PROTO_PACKAGES,
  PROTO_REV,
  SERVICE_DESCRIPTORS,
  type AdminAgentsModule,
  type AdminAuditModule,
  type AdminKeysModule,
  type AdminOrgModule,
  type AdminProjectsModule,
  type AuthModule,
  type CallBinding,
  type CollectionsModule,
  type DocumentsModule,
  type GraphModule,
  type SqlModule,
  type StreamsModule,
  type NamespacesModule,
  type SearchModule,
  type VectorModule,
  type InstanceModule,
  type LiveAdminModule,
  type LiveModule,
  type ModuleBinding,
  type TablesModule,
} from './gen/facade.js';
import { attachAppCalls, type documentCalls, type searchCalls, type sqlCalls, type streamCalls } from './app.js';
import { CollectionHandle, type HandleModules } from './handle.js';
import {
  CallInvoker,
  DEFAULT_TIMEOUT_MS,
  type CallOptions,
  type ConsistencyTokenStore,
} from './runtime/call.js';
import { ConsistencySession } from './runtime/consistency.js';
import { LoamsError } from './runtime/errors.js';
import { paginate, type PageRequestOptions } from './runtime/pagination.js';
import { SystemApi, type VersionReport } from './system.js';
import { DEFAULT_MAX_RETRIES } from './runtime/retry.js';
import { apiKey, envToken, type TokenSource } from './runtime/token-source.js';
import { createLoamsTransport, type TransportOptions } from './runtime/transports.js';
import { watch, type ResumeOptions } from './runtime/streams.js';

/** How to build a `Loams`. */
export interface LoamsOptions {
  /** The instance's base URL, for example `https://acme.loams.dev`. A
   * loopback stack is `http://127.0.0.1:8080`. */
  endpoint: string;
  /**
   * The bearer source. A string is taken as an API key, which is what a
   * script or a CI job has; pass `envToken()` or `oidcExchange()` for the
   * rest. Omitted means an unauthenticated client, which is what
   * `loams.instance.getInstance` needs anyway.
   */
  auth?: TokenSource | string;
  /**
   * The transport. Defaults to Connect over `fetch` pointed at `endpoint`,
   * which is the browser, Node and Bun path (see `runtime/transports.ts`).
   * `@loams/client/node` exports `createNodeTransport` for HTTP/2 gRPC.
   *
   * A transport carries its own base URL, so a transport passed here must be
   * built for the same instance as `endpoint`.
   */
  transport?: Transport;
  /** Transport-level options, used when `transport` is not given. They win
   * over `endpoint` where they overlap, so `baseUrl` here replaces it. */
  transportOptions?: TransportOptions;
  /** Retries after the first attempt, for every call. `0` disables them. */
  maxRetries?: number;
  /**
   * Hold a session consistency token across calls (D609). Off by default:
   * every read is then `STRONG` on its own, which is correct but does not give
   * read-your-writes across processes.
   */
  sessionConsistency?: boolean;
  /**
   * The deadline of every unary call that does not set its own `timeoutMs`
   * (runtime contract R12). Default 30 000 ms; `0` disables it. Streams have
   * no default deadline. The budget covers a call's retries, and a call that
   * runs out raises `DeadlineExceededError` with reason
   * `client_deadline_exceeded`, which is never retried.
   */
  timeoutMs?: number;
  /**
   * The namespace of every request that has a `namespace` field and leaves it
   * empty (runtime contract R11). An explicit `namespace` always wins; with
   * neither, such a call fails with `InvalidArgumentError` before any network
   * call.
   */
  namespace?: string;
}

/** The environment `Loams.fromEnv` reads (R1). */
export interface LoamsEnvironment {
  readonly LOAMS_ENDPOINT?: string;
  readonly LOAMS_API_KEY?: string;
  readonly LOAMS_TOKEN?: string;
  readonly LOAMS_NAMESPACE?: string;
  readonly [name: string]: string | undefined;
}

export type { Catalogue, VersionReport } from './system.js';

/** The generated modules, keyed by name. A module the generator has not seen
 * does not appear, so `loams.vector` is absent until `QueryService/Search`
 * carries its facade options (API1 Task 2). */
export interface LoamsModules {
  readonly instance: InstanceModule;
  readonly live: LiveModule;
  readonly tables: TablesModule;
  readonly [module: string]: unknown;
}

/** `loams.live`: the session calls, with `loams.live.admin` under it. */
export type LiveNamespace = LiveModule & {
  /** `loams.live.admin` — apps, deploy, roll back, index status and logs. */
  readonly admin: LiveAdminModule;
};

/** A paged call's `<call>All` iterator, which `attachAppCalls` adds. */
type AllOf<Item> = (request?: object, options?: CallOptions) => AsyncGenerator<Item, void, undefined>;

/** `loams.namespaces`, with `listAll`. */
export type NamespacesNamespace = NamespacesModule & { readonly listAll: AllOf<unknown> };
/** `loams.collections`, with `listAll` and `listVersionsAll`. */
export type CollectionsNamespace = CollectionsModule & {
  readonly listAll: AllOf<unknown>;
  readonly listVersionsAll: AllOf<unknown>;
};
/** `loams.documents`, with the hand-written conveniences (SDKP §2.3). */
export type DocumentsNamespace = DocumentsModule & ReturnType<typeof documentCalls>;
/** `loams.search`, with `text`, `hybrid` and `queryAll` (SDKP §2.4). */
export type SearchNamespace = SearchModule & ReturnType<typeof searchCalls>;
/** `loams.sql`, with `records` (SDKP §2.5). */
export type SqlNamespace = SqlModule & ReturnType<typeof sqlCalls>;
/** `loams.streams`, with `subscribe` and `<call>All` (SDKP §2.5, R14). */
export type StreamsNamespace = StreamsModule &
  ReturnType<typeof streamCalls> & { readonly listAll: AllOf<unknown> };

/** `loams.admin`: the console's administration modules (design §44 §7.2). */
export interface AdminNamespace {
  readonly agents: AdminAgentsModule;
  readonly audit: AdminAuditModule;
  readonly keys: AdminKeysModule;
  readonly org: AdminOrgModule;
  readonly projects: AdminProjectsModule;
}

/** One SDK, over one instance. */
export class Loams {
  /** `loams.instance` — what this instance is, and who the caller is. */
  readonly instance: InstanceModule;
  /** `loams.live` — the live sync session half, and `loams.live.admin`.
   * Its package is `unstable`. */
  readonly live: LiveNamespace;
  /** `loams.tables` — the table half of the same service (design §44 §7.2). */
  readonly tables: TablesModule;
  /** `loams.admin` — org, projects, agents, keys and audit. */
  readonly admin: AdminNamespace;
  /** `loams.auth` — sessions and sign-in. */
  readonly auth: AuthModule;
  /** `loams.namespaces` — the containers every collection and stream live in. */
  readonly namespaces: NamespacesNamespace;
  /** `loams.collections` — collections, their schemas, aliases and versions. */
  readonly collections: CollectionsNamespace;
  /** `loams.documents` — write, read, scroll and count documents. */
  readonly documents: DocumentsNamespace;
  /** `loams.search` — text, vector and hybrid search (`Search` builds one). */
  readonly search: SearchNamespace;
  /** `loams.vector` — the kNN half of `search`, by its own name (Q613). */
  readonly vector: VectorModule;
  /** `loams.graph` — Loams Graph, queried with GQL. Its package is `unstable`. */
  readonly graph: GraphModule;
  /** `loams.sql` — one read-only SQL statement over a namespace's collections. */
  readonly sql: SqlNamespace;
  /** `loams.streams` — a namespace's logs: produce, fetch, CloudEvents and `subscribe`. */
  readonly streams: StreamsNamespace;
  /** The module catalogue, feature detection and the version check. */
  readonly system: SystemApi;
  /** Every generated module, by its dotted name (`'live.admin'`): the
   * catalogue a caller iterates. The same objects are reachable as
   * properties, `loams.live.admin` for `'live.admin'`. */
  readonly modules: Readonly<Record<string, unknown>>;
  /** The default namespace (R11), if the client has one. */
  readonly namespace: string | undefined;

  readonly #invoker: CallInvoker;
  readonly #consistency: ConsistencySession | undefined;

  constructor(options: LoamsOptions) {
    // The endpoint lives on the transport, which is where connect-es reads it.
    const transport =
      options.transport ??
      createLoamsTransport({ baseUrl: options.endpoint, ...options.transportOptions });
    const tokenSource =
      options.auth === undefined
        ? undefined
        : typeof options.auth === 'string'
          ? apiKey(options.auth)
          : options.auth;
    // The session store is built first so the invoker can hold it: a call
    // that asks for the session's consistency token reads and records through
    // the same store the client exposes.
    this.#consistency = options.sessionConsistency === true ? new ConsistencySession() : undefined;
    this.#invoker = new CallInvoker(
      transport,
      tokenSource,
      options.maxRetries ?? DEFAULT_MAX_RETRIES,
      this.#consistency,
      {
        namespace: options.namespace,
        timeoutMs: options.timeoutMs ?? DEFAULT_TIMEOUT_MS,
      },
    );
    this.namespace = options.namespace;

    this.modules = buildModules(this.#invoker);
    attachAppCalls(this.modules as Record<string, unknown>, MODULES);
    const tree = nestModules(this.modules);
    this.instance = tree.instance as InstanceModule;
    this.live = tree.live as LiveNamespace;
    this.tables = tree.tables as TablesModule;
    this.admin = tree.admin as AdminNamespace;
    this.auth = tree.auth as AuthModule;
    this.namespaces = tree.namespaces as NamespacesNamespace;
    this.collections = tree.collections as CollectionsNamespace;
    this.documents = tree.documents as DocumentsNamespace;
    this.search = tree.search as SearchNamespace;
    this.vector = tree.vector as VectorModule;
    this.graph = tree.graph as GraphModule;
    this.sql = tree.sql as SqlNamespace;
    this.streams = tree.streams as StreamsNamespace;
    this.system = new SystemApi(
      (request, callOptions) => this.instance.getInstance(request, callOptions),
      {
        endpoint: options.endpoint,
        maxRetries: options.maxRetries,
        timeoutMs: options.timeoutMs,
      },
    );
  }

  /**
   * A client from the environment: `LOAMS_ENDPOINT`, then `LOAMS_API_KEY` or
   * `LOAMS_TOKEN`, then `LOAMS_NAMESPACE` (R1). `options` wins over the
   * environment where both say something.
   */
  static fromEnv(
    options: Partial<LoamsOptions> = {},
    environment: LoamsEnvironment = readEnvironment(),
  ): Loams {
    const endpoint = options.endpoint ?? environment.LOAMS_ENDPOINT;
    if (endpoint === undefined || endpoint === '') {
      throw new LoamsError('Loams.fromEnv: LOAMS_ENDPOINT is not set', {
        code: Code.InvalidArgument,
        metadata: { field: 'LOAMS_ENDPOINT' },
      });
    }
    const hasKey =
      (environment.LOAMS_API_KEY ?? '') !== '' || (environment.LOAMS_TOKEN ?? '') !== '';
    const namespace = options.namespace ?? environment.LOAMS_NAMESPACE;
    return new Loams({
      ...options,
      endpoint,
      ...(options.auth === undefined && hasKey ? { auth: envToken(environment) } : {}),
      ...(namespace === undefined || namespace === '' ? {} : { namespace }),
    });
  }

  /**
   * A handle on one collection: its document and search calls with
   * `collection` (and `namespace`) filled in (SDKP §2.8). Holds no state.
   */
  collection(name: string, options: { namespace?: string } = {}): CollectionHandle {
    return new CollectionHandle(this.modules as HandleModules, name, options);
  }

  /** The session consistency token store, when `sessionConsistency` is on. */
  get consistency(): ConsistencyTokenStore | undefined {
    return this.#consistency;
  }

  /** The generated module bindings, as the SDK sees them. */
  get bindings(): readonly ModuleBinding[] {
    return MODULES;
  }

  /** Every item of a paged call (D617's iterator).
   *
   * `loams.paginate('collections', 'listCollections', { namespace })`. The
   * per-call alias §44 §7.4 sketches (`loams.collections.listAll`) arrives with
   * the first paged RPC, when there is a generated signature to hang it on;
   * until then this is the same iterator under its module and call name.
   */
  paginate<Request extends object, Response extends object, Item>(
    module: string,
    call: string,
    request: Request,
    options: CallOptions & PageRequestOptions = {},
  ): AsyncGenerator<Item, void, undefined> {
    const binding = this.binding(module, call);
    const method = this.modules[module] as Record<string, unknown> | undefined;
    const fn = method?.[call];
    if (typeof fn !== 'function') {
      throw new LoamsError(`loams.${module} has no call ${call}`, { code: Code.Internal });
    }
    return paginate<Request, Response, Item>(
      binding,
      fn as (request: Request, options?: CallOptions) => Promise<Response>,
      request,
      options,
    );
  }

  /** A server stream that reconnects from its cursor (runtime contract R7). */
  stream<Message, Request extends object>(
    module: string,
    call: string,
    request: Request,
    options: CallOptions & ResumeOptions<Message, Request>,
  ): AsyncGenerator<Message, void, undefined> {
    const binding = this.binding(module, call);
    const method = this.modules[module] as Record<string, unknown> | undefined;
    const fn = method?.[call];
    if (typeof fn !== 'function') {
      throw new LoamsError(`loams.${module} has no call ${call}`, { code: Code.Internal });
    }
    return watch<Message, Request>(
      binding,
      fn as (request: unknown, options?: CallOptions) => AsyncIterable<Message>,
      request,
      options,
    );
  }

  /** The binding a module and call name identify, or a clear error. */
  binding(module: string, call: string): CallBinding {
    const found = MODULES.find((entry) => entry.name === module)?.calls.find(
      (entry) => entry.name === call,
    );
    if (found === undefined) {
      throw new LoamsError(`loams.${module} has no generated call ${call}`, {
        code: Code.Internal,
      });
    }
    return found;
  }

  /** The proto revision this SDK declares (§44 §10.3). */
  get protoRev(): string {
    return PROTO_REV;
  }

  /** The proto packages this SDK speaks. */
  get protoPackages(): readonly string[] {
    return PROTO_PACKAGES.filter((name) => name.startsWith('loams.'));
  }

  /** Forgets the cached service catalogue, so the next check calls again. */
  invalidateCatalogue(): void {
    this.system.invalidate();
  }
}

/** The process environment, where there is one (Node, Bun); empty in a
 * browser. */
function readEnvironment(): LoamsEnvironment {
  const scope = globalThis as { process?: { env?: Record<string, string | undefined> } };
  return scope.process?.env ?? {};
}

/**
 * Builds one object per generated module: one function per generated call,
 * each delegating to the same invoker.
 *
 * A server-streaming call returns the `AsyncIterable` directly rather than a
 * promise of one, so `for await (const t of loams.live.watch(...))` reads the
 * way design §44 §7.1 says it should.
 */
function buildModules(invoker: CallInvoker): Record<string, unknown> {
  const modules: Record<string, unknown> = {};
  for (const module of MODULES) {
    // A module may span two services (`loams.graph` is `GraphAdminService`
    // and `GraphService`), so every call's own service is registered.
    const services = new Set([module.service, ...module.calls.map((call) => call.service)]);
    let missing = false;
    for (const service of services) {
      const descriptor = SERVICE_DESCRIPTORS[service];
      if (descriptor === undefined) {
        missing = true;
        continue;
      }
      invoker.register(service, descriptor);
    }
    if (missing) {
      continue;
    }
    const calls: Record<string, unknown> = {
      module: module.name,
      service: module.service,
      unstable: module.unstable,
    };
    for (const binding of module.calls) {
      // A server-streaming call returns the `AsyncIterable` directly rather
      // than a promise of one, so `for await (const t of loams.live.watch(..))`
      // reads the way design §44 §7.1 says it should. `loams.stream()` wraps
      // the same call with cursor resume.
      calls[binding.name] =
        binding.streaming === 'server'
          ? (request: unknown, options?: CallOptions) => invoker.stream(binding, request, options)
          : (request: unknown, options?: CallOptions) => invoker.unary(binding, request, options);
    }
    modules[module.name] = calls;
  }
  return modules;
}

/**
 * The module tree a dotted name implies: `'live.admin'` becomes `live.admin`
 * and `'admin.org'` becomes `admin.org`.
 *
 * A child is attached to its parent module object when the parent exists
 * (`live` is a module with calls of its own), and to a plain namespace object
 * when it does not (`admin` has only children). The flat catalogue is left as
 * it is; the tree reuses its objects, so `loams.live.admin` and
 * `loams.modules['live.admin']` are the same object.
 */
function nestModules(flat: Record<string, unknown>): Record<string, unknown> {
  const tree: Record<string, unknown> = {};
  // Parents before children, so a module with calls of its own is the node
  // its children hang from rather than being replaced by a namespace.
  const names = Object.keys(flat).sort((a, b) => a.split('.').length - b.split('.').length);
  for (const name of names) {
    const path = name.split('.');
    const leaf = path.pop() as string;
    let node = tree;
    for (const part of path) {
      let next = node[part] as Record<string, unknown> | undefined;
      if (next === undefined) {
        next = {};
        node[part] = next;
      }
      node = next;
    }
    if (leaf in node) {
      throw new LoamsError(`the module ${name} collides with a call of the same name`, {
        code: Code.Internal,
      });
    }
    node[leaf] = flat[name];
  }
  return tree;
}
