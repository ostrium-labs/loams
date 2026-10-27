// The call path: the one place a facade method becomes an RPC (design §44 §7.4;
// runtime contract R1-R4).
//
// It does four things the generated facade cannot, and nothing else:
//
// - attaches the bearer from the client's `TokenSource`;
// - retries on the call's class from the generated bindings, with M1.6's
//   backoff numbers, and refreshes the token once on `token_expired`;
// - gives a mutating call an idempotency key once per logical call and reuses
//   it on every retry, so a retried write is the same write (D610);
// - turns whatever is thrown into the typed `LoamsError` hierarchy, so a
//   caller branches on `reason` and never on a message.

import type { DescMessage, DescService } from '@bufbuild/protobuf';
import {
  Code,
  type CallOptions as ConnectCallOptions,
  createClient,
  type Transport,
} from '@connectrpc/connect';
import type { CallBinding } from '../gen/facade.js';
import {
  DeadlineExceededError,
  InvalidArgumentError,
  LoamsError,
  TokenExpiredError,
  toLoamsError,
} from './errors.js';
import { TOKEN_PREFIX } from './consistency.js';
import { DEFAULT_MAX_RETRIES, backoffMs, shouldRetry, sleep } from './retry.js';
import type { TokenSource } from './token-source.js';
import { uuidv7 } from './uuidv7.js';

/**
 * Per-call overrides. Everything here is optional: the client's defaults apply,
 * and the generated binding supplies the retry class.
 *
 * The instance's URL is *not* here: in connect-es it belongs to the transport,
 * so a `Loams` built without a `transport` makes one from its `endpoint`, and
 * one built with a `transport` expects that transport to know the URL.
 */
export interface CallOptions extends ConnectCallOptions {
  /** Retries after the first attempt. `0` disables retrying for this call. */
  maxRetries?: number;
  /** Extra request headers. `Authorization` is set by the client and wins. */
  headers?: Record<string, string>;
  /**
   * The idempotency key for a mutating call. Supply your own to make a retry
   * yours rather than the SDK's; omit it and the SDK mints one UUIDv7 per
   * logical call and reuses it on every retry.
   */
  idempotencyKey?: string;
  /** Overrides the call's retry class for this call only. */
  retrySafe?: boolean;
  /** The consistency token to read at, and the store to record writes in
   * (§44 §7.4, D609). */
  consistency?: ConsistencyOptions;
  /**
   * This call's deadline in milliseconds (runtime contract R12). Unary calls
   * default to the client's `timeoutMs` (30 s); streams have none unless one is
   * given here. `0` means no deadline. The whole call, retries included, has to
   * fit: a call that runs out raises `DeadlineExceededError` with reason
   * `client_deadline_exceeded` and is never retried.
   */
  timeoutMs?: number;
}

/** How consistency tokens are carried (D609). */
export interface ConsistencyOptions {
  /** A `ConsistencyToken` from a previous write, or `'strong'` (the default)
   * / `'eventual'` to read without waiting. */
  token?: string;
  /** Record the consistency token every response carries, so later reads are
   * read-your-writes without the caller threading tokens by hand. Unset means
   * the client's session, when `sessionConsistency` is on; `false` opts this
   * call out. */
  session?: ConsistencyTokenStore | true | false;
}

/** A session's merged consistency token (D609). Off unless a call opts in. */
export interface ConsistencyTokenStore {
  /** The token to attach to the next read in `scope` (a proto package), or
   * undefined for none. */
  current(scope?: string): string | undefined;
  /** Folds a token the server returned, in `scope`, into the session's. */
  record(token: string | undefined, scope?: string): void;
}

/** One attempt of a call, as the transport sees it. */
export type Send = (attempt: Attempt) => Promise<unknown>;

/** What one attempt carries: the request (with the idempotency key already
 * set) and the bearer to send, if any. */
export interface Attempt {
  readonly request: unknown;
  readonly headers: Record<string, string>;
  readonly attempt: number;
  readonly refreshed: boolean;
}

/** Everything the retry loop needs, so it can be tested without a transport. */
export interface RetryPlan {
  /** The call's retry class from the generated binding. */
  readonly retrySafe: boolean;
  readonly maxRetries: number;
  readonly signal?: AbortSignal;
  readonly onRefresh?: () => Promise<void>;
  /** True once the call's own deadline has passed: no further attempt. */
  readonly expired?: () => boolean;
}

/**
 * Runs `send` until it answers or the plan says stop, and returns whatever it
 * last threw as a `LoamsError`.
 *
 * Exported so the tests can drive it with a stub `send`: the retry policy,
 * the idempotency-key lifecycle and the refresh-once behaviour are the SDK's
 * own logic and are pinned without a server.
 */
export async function callWithRetry(
  request: unknown,
  send: Send,
  plan: RetryPlan,
  rpc?: string,
): Promise<unknown> {
  let refreshed = false;
  for (let attempt = 0; ; attempt += 1) {
    try {
      return await send({ request, headers: {}, attempt, refreshed });
    } catch (thrown) {
      const error = toLoamsError(thrown, rpc);
      // R1: a 401 whose reason is `token_expired` gets exactly one refresh and
      // one retry. A source that cannot refresh (`apiKey`) makes this a no-op,
      // and a second expiry is reported rather than looped on.
      if (error instanceof TokenExpiredError && !refreshed && plan.onRefresh !== undefined) {
        refreshed = true;
        await plan.onRefresh();
        attempt -= 1;
        continue;
      }
      if (
        plan.expired?.() === true ||
        !shouldRetry(error, plan.retrySafe, attempt, plan.maxRetries)
      ) {
        throw error;
      }
      await sleep(backoffMs(attempt), plan.signal);
    }
  }
}

/**
 * Gives a mutating call its idempotency key, once per logical call.
 *
 * D610: a mutation may only be retried when it carries one, and the same key
 * has to go out on every attempt or the retry is a second write. So the key is
 * decided here, before the first attempt, and the request object is not
 * rewritten afterwards.
 *
 * A message without an `idempotency_key` field is left alone: proto3 messages
 * are plain objects here, so the field's presence is the test.
 */
export function withIdempotencyKey(
  request: unknown,
  supplied: string | undefined,
  /** Whether the request *schema* declares an `idempotency_key` field. */
  declared = false,
): { request: unknown; keyed: boolean } {
  if (typeof request !== 'object' || request === null) {
    return { request, keyed: false };
  }
  const message = request as Record<string, unknown>;
  // Either the schema declares the field or the object carries it; without one
  // of those two there is nothing to key, and a request that has no key field
  // must be left exactly as the caller wrote it.
  if (!declared && !('idempotencyKey' in message)) {
    return { request, keyed: false };
  }
  if (typeof message.idempotencyKey === 'string' && message.idempotencyKey !== '') {
    return { request, keyed: true };
  }
  return {
    request: { ...message, idempotencyKey: supplied ?? uuidv7() },
    keyed: true,
  };
}

/**
 * A stream method's return value, as one async iterable.
 *
 * connect-es types a server stream as an iterable that may be awaited; the SDK
 * hands callers a plain `AsyncIterable`, which is what `for await` wants
 * (design §44 §7.1).
 */
async function* streamOf(source: AsyncIterable<unknown> | AsyncIterable<Promise<unknown>>) {
  yield* source as AsyncIterable<unknown>;
}

/** The request-message field a mutation is keyed by (D610). Read from the
 * generated schema rather than guessed from the object a caller built. */
const IDEMPOTENCY_KEY = 'idempotencyKey';

/**
 * The session a call asked for: its own store if it brought one, the client's
 * if it asked for the session's, none otherwise.
 */
function resolveSession(
  options: CallOptions['consistency'],
  clientSession: ConsistencyTokenStore | undefined,
): ConsistencyTokenStore | undefined {
  const session = options?.session;
  if (session === false) {
    return undefined;
  }
  if (session === undefined || session === true) {
    return clientSession;
  }
  return session;
}

/** Whether a response string is something other than a consistency token. */
function isForeignToken(value: string): boolean {
  return !value.startsWith(TOKEN_PREFIX);
}

/** The unary deadline when neither the client nor the call sets one (R12). */
export const DEFAULT_TIMEOUT_MS = 30_000;

/** The response fields a consistency token is read from (R4, SDKP §2.1). */
const TOKEN_FIELDS = ['consistencyToken', 'token', 'readToken'] as const;

/** The response header a consistency token is read from (R4). */
export const CONSISTENCY_TOKEN_HEADER = 'loams-consistency-token';

/**
 * The packages whose `namespace` field is a Loams namespace (R11) and whose
 * `consistency` field reads at a consistency token (R4). Other packages have
 * fields of the same name that mean something else: `loams.live.v1`'s
 * `CreateAppRequest.namespace` is free text naming an app's environment, and
 * must never be filled from, or refused for lack of, the client's default.
 */
export const APP_PACKAGES: ReadonlySet<string> = new Set([
  'loams.collection.v1',
  'loams.graph.v1',
  'loams.sql.v1',
  'loams.stream.v1',
]);

/**
 * The packages whose reads take the session's token as `at_least` (R4,
 * SDKP-17): collections and SQL. A graph's token names one graph's stream and
 * a Graph read refuses any other, so graph reads never take the session's.
 */
export const SESSION_PACKAGES: ReadonlySet<string> = new Set(['loams.collection.v1', 'loams.sql.v1']);

/** What the invoker reads off a request schema once, per RPC. */
interface RequestShape {
  /** The request declares `idempotency_key` (D610). */
  readonly keyed: boolean;
  /** The request declares a string `namespace` (R11). */
  readonly namespaced: boolean;
  /** How to say `at_least = token` in this request's `consistency` field, if it
   * has one (R4). */
  readonly atLeast: ((token: string) => Record<string, unknown>) | undefined;
}

/** How a request's `consistency` message spells "at least this token". Both
 * shapes in the protos are covered: a plain `at_least` field
 * (`loams.collection.v1.Consistency`) and an `at_least` arm of a oneof
 * (`loams.graph.v1.Consistency`). */
function atLeastOf(input: DescMessage): RequestShape['atLeast'] {
  const field = input.field.consistency;
  if (field === undefined || field.fieldKind !== 'message') {
    return undefined;
  }
  const atLeast = field.message.field.atLeast;
  if (atLeast === undefined || atLeast.fieldKind !== 'scalar') {
    return undefined;
  }
  const oneof = atLeast.oneof;
  if (oneof === undefined) {
    return (token) => ({ atLeast: token });
  }
  return (token) => ({ [oneof.localName]: { case: 'atLeast', value: token } });
}

/** Options the invoker applies to every call (R11, R12). */
export interface InvokerDefaults {
  /** The namespace a request with an empty `namespace` field is sent with. */
  namespace?: string;
  /** The unary deadline; `0` disables it. */
  timeoutMs?: number;
}

/**
 * The client's call path: a generated `CallBinding` plus a request becomes a
 * response message, with everything the SDK owns applied on the way.
 */
export class CallInvoker {
  private readonly clients = new Map<string, Record<string, unknown>>();
  private readonly descriptors = new Map<string, DescService>();
  /** What each RPC's request schema declares, per RPC. */
  private readonly shapes = new Map<string, RequestShape>();
  private readonly defaults: InvokerDefaults;

  constructor(
    private readonly transport: Transport,
    private readonly tokenSource: TokenSource | undefined,
    private readonly maxRetries: number,
    private readonly consistency: ConsistencyTokenStore | undefined,
    defaults: InvokerDefaults = {},
  ) {
    this.defaults = defaults;
  }

  /** The client's default namespace, if it has one (R11). */
  get namespace(): string | undefined {
    return this.defaults.namespace;
  }

  /**
   * A module's Connect client, built once from the service descriptor
   * `protoc-gen-es` generated. This is what makes the facade generated rather
   * than hand-written: the SDK never writes a client, it wraps the stubs.
   */
  register(service: string, descriptor: DescService): void {
    if (this.clients.has(service)) {
      return;
    }
    this.descriptors.set(service, descriptor);
    for (const method of Object.values(descriptor.method)) {
      // Whether this RPC takes an idempotency key is a property of its request
      // message, so it is read from the generated schema rather than inferred
      // from the object a caller happened to build. A caller who omits an
      // `optional` field still gets a key, and `MutateRequest` (which has one)
      // is never confused with `DeployRequest` (which does not).
      const namespace = method.input.field.namespace;
      const pkg = descriptor.file.proto.package ?? '';
      const app = APP_PACKAGES.has(pkg);
      this.shapes.set(`${service}/${method.name}`, {
        keyed: method.input.field[IDEMPOTENCY_KEY] !== undefined,
        namespaced:
          app &&
          namespace !== undefined &&
          namespace.fieldKind === 'scalar' &&
          namespace.oneof === undefined,
        atLeast: SESSION_PACKAGES.has(pkg) ? atLeastOf(method.input) : undefined,
      });
    }
    this.clients.set(service, createClient(descriptor, this.transport) as unknown as Record<
      string,
      unknown
    >);
  }

  /** What this call's request message declares. */
  private shape(binding: CallBinding): RequestShape {
    return (
      this.shapes.get(`${binding.service}/${binding.method}`) ?? {
        keyed: false,
        namespaced: false,
        atLeast: undefined,
      }
    );
  }

  /** Whether this call's request message declares an `idempotency_key`. */
  private takesIdempotencyKey(binding: CallBinding): boolean {
    return this.shape(binding).keyed;
  }

  /**
   * The request as it goes out: the default namespace filled in (R11) and the
   * session's token set as `consistency.at_least` when the caller left
   * `consistency` unset (R4). A request the caller set either on is left as
   * written: an explicit argument always wins.
   */
  private prepare(binding: CallBinding, request: unknown, consistency: string | undefined): unknown {
    if (typeof request !== 'object' || request === null) {
      return request;
    }
    const shape = this.shape(binding);
    let message = request as Record<string, unknown>;
    if (shape.namespaced) {
      const given = message.namespace;
      if (typeof given !== 'string' || given === '') {
        const fallback = this.defaults.namespace;
        if (fallback === undefined || fallback === '') {
          throw new InvalidArgumentError(
            `${binding.rpc} needs a namespace: pass one, or give the client a default namespace`,
            {
              code: Code.InvalidArgument,
              reason: 'namespace_required',
              rpc: binding.rpc,
              metadata: { field: 'namespace' },
            },
          );
        }
        message = { ...message, namespace: fallback };
      }
    }
    if (consistency !== undefined && shape.atLeast !== undefined && message.consistency === undefined) {
      message = { ...message, consistency: shape.atLeast(consistency) };
    }
    return message;
  }

  /** The deadline a unary call runs under, in milliseconds, or undefined. */
  private unaryTimeout(options: CallOptions): number | undefined {
    const ms = options.timeoutMs ?? this.defaults.timeoutMs ?? DEFAULT_TIMEOUT_MS;
    return ms > 0 ? ms : undefined;
  }

  /** The connect-es method a binding names, or a clear internal error. */
  private method(binding: CallBinding): (request: unknown, options: CallOptions) => Promise<unknown> {
    const client = this.clients.get(binding.service);
    // The connect-es client is keyed by the RPC's own local name
    // (`createNamespace`), not by the facade name (`create`): the two differ
    // whenever `FacadeOptions.name` renames a call, which every app module does.
    const descriptor = this.descriptors.get(binding.service);
    const local =
      descriptor?.methods.find((candidate) => candidate.name === binding.method)?.localName ??
      binding.name;
    const method = client?.[local];
    if (typeof method !== 'function') {
      throw new LoamsError(
        `the generated client for ${binding.service} has no method ${binding.name}`,
        { code: Code.Internal, rpc: binding.rpc },
      );
    }
    return method as (request: unknown, options: CallOptions) => Promise<unknown>;
  }

  /** One unary call: the whole runtime contract, applied to one binding. */
  async unary(binding: CallBinding, request: unknown, options: CallOptions = {}) {
    const session = resolveSession(options.consistency, this.consistency);
    // An explicit token wins; otherwise the session's, which is what makes a
    // session read-your-writes rather than record it and never send it (R4).
    // R4 (SDKP-17): one token per proto package, so a collection write's token
    // never becomes a graph read's `at_least`.
    const consistency = options.consistency?.token ?? session?.current(binding.package);
    const prepared = this.prepare(binding, request, consistency);
    const { request: keyed, keyed: hasKey } = withIdempotencyKey(
      prepared,
      options.idempotencyKey,
      this.takesIdempotencyKey(binding),
    );
    const maxRetries = options.maxRetries ?? this.maxRetries;
    // D610: a `safe` call always retries; a mutation retries once it carries an
    // idempotency key, which `withIdempotencyKey` has just decided.
    const retrySafe = options.retrySafe ?? (binding.retry === 'safe' || hasKey);
    const method = this.method(binding);

    // R12: one budget for the whole call, retries included. The SDK runs its
    // own timer rather than trusting the transport's, so the budget holds over
    // every transport, and so "the client ran out" is told apart from a
    // server's `deadline_exceeded`, which may be retried.
    const timeoutMs = this.unaryTimeout(options);
    const deadline = timeoutMs === undefined ? undefined : Date.now() + timeoutMs;
    const controller = new AbortController();
    let expired = false;
    const onCallerAbort = () => controller.abort(options.signal?.reason);
    if (options.signal?.aborted === true) {
      controller.abort(options.signal.reason);
    } else {
      options.signal?.addEventListener('abort', onCallerAbort, { once: true });
    }
    const timer =
      timeoutMs === undefined
        ? undefined
        : setTimeout(() => {
            expired = true;
            controller.abort(new Error('client deadline exceeded'));
          }, timeoutMs);
    let headerToken: string | undefined;
    const onHeader = (headers: Headers | undefined) => {
      // A stub transport may answer without headers at all.
      headerToken = headers?.get?.(CONSISTENCY_TOKEN_HEADER) ?? undefined;
      if (headers !== undefined) {
        options.onHeader?.(headers);
      }
    };

    try {
      const result = (await callWithRetry(
        keyed,
        async () =>
          method(keyed, {
            ...options,
            signal: controller.signal,
            ...(deadline === undefined
              ? {}
              : { timeoutMs: Math.max(1, deadline - Date.now()) }),
            onHeader,
            headers: await this.headers(options, consistency, this.shape(binding).atLeast === undefined),
          }),
        {
          retrySafe,
          maxRetries,
          signal: controller.signal,
          onRefresh: this.tokenSource?.refresh?.bind(this.tokenSource),
          expired: () => expired,
        },
        binding.rpc,
      )) as Record<string, unknown> | undefined;
      this.recordConsistency(session, result, headerToken, binding.package);
      return result;
    } catch (thrown) {
      if (expired) {
        throw new DeadlineExceededError(
          `${binding.rpc} did not answer within the client's ${timeoutMs} ms deadline`,
          {
            code: Code.DeadlineExceeded,
            reason: 'client_deadline_exceeded',
            rpc: binding.rpc,
            metadata: { timeout_ms: String(timeoutMs) },
            cause: thrown,
          },
        );
      }
      throw toLoamsError(thrown, binding.rpc);
    } finally {
      if (timer !== undefined) {
        clearTimeout(timer);
      }
      options.signal?.removeEventListener('abort', onCallerAbort);
    }
  }

  /** The headers a call carries: the caller's, plus the bearer and the
   * consistency token the runtime owns. */
  private async headers(
    options: CallOptions,
    consistency: string | undefined,
    asHeader = true,
  ): Promise<Record<string, string>> {
    const headers: Record<string, string> = { ...options.headers };
    const bearer = await this.tokenSource?.token();
    if (bearer !== undefined) {
      headers.authorization = `Bearer ${bearer}`;
    }
    // A request with a `consistency` field carries the token there (R4); one
    // without carries it as a header, which is all such an RPC can read.
    if (consistency !== undefined && asHeader) {
      headers['loams-consistency'] = consistency;
    }
    return headers;
  }

  /**
   * One server stream, with the errors mapped: no resume, no cursor tracking.
   *
   * The mapping is not optional here. A refusal on a stream arrives inside the
   * Connect envelope, not as an HTTP status, so a caller that iterated the raw
   * iterable would see a `ConnectError` with no `reason` — the one place in the
   * SDK where `reason` could go missing. `watch()` adds the resume on top.
   */
  stream(binding: CallBinding, request: unknown, options: CallOptions = {}): AsyncIterable<unknown> {
    const method = this.method(binding) as unknown as (
      request: unknown,
      options: CallOptions,
    ) => AsyncIterable<unknown>;
    const session = resolveSession(options.consistency, this.consistency);
    const consistency = options.consistency?.token ?? session?.current(binding.package);
    const prepared = this.prepare(binding, request, consistency);
    const asHeader = this.shape(binding).atLeast === undefined;
    const self = this;
    const refresh = this.tokenSource?.refresh?.bind(this.tokenSource);
    // Synchronous, because `loams.live.watch(...)` has to hand back an
    // `AsyncIterable` rather than a promise of one (design §44 §7.1). The
    // bearer is fetched when the caller first steps the stream rather than when
    // the call is made: a stream that never authenticates is not a stream.
    const invoke = async function* (): AsyncGenerator<unknown, void, undefined> {
      let refreshed = false;
      let yielded = false;
      for (;;) {
        const headers = await self.headers(options, consistency, asHeader);
        const source = self.mapped(
          streamOf(method(prepared, { ...options, headers })),
          binding.rpc,
        );
        try {
          for await (const message of source) {
            yielded = true;
            yield message;
          }
          return;
        } catch (thrown) {
          const error = toLoamsError(thrown, binding.rpc);
          // R1 on a stream: one refresh and one re-open, and only if nothing
          // has been yielded yet. Once messages are flowing the caller is
          // holding a position in the stream, and re-opening it is `watch()`'s
          // job (it resumes from the cursor); replaying from the start here
          // would duplicate everything the caller has already seen.
          if (
            error instanceof TokenExpiredError &&
            !refreshed &&
            refresh !== undefined &&
            !yielded
          ) {
            refreshed = true;
            await refresh();
            continue;
          }
          throw error;
        }
      }
    };
    return invoke();
  }

  /** Wraps an iterable so every throw becomes a `LoamsError`. */
  private mapped(source: AsyncIterable<unknown>, rpc: string): AsyncIterable<unknown> {
    const invoke = async function* (): AsyncGenerator<unknown, void, undefined> {
      try {
        yield* source;
      } catch (thrown) {
        throw toLoamsError(thrown, rpc);
      }
    };
    return invoke();
  }

  /** Merges every consistency token a response carried into the session:
   * `consistency_token`, `token` and `read_token`, and the
   * `loams-consistency-token` header (R4, SDKP §2.1). */
  private recordConsistency(
    session: ConsistencyTokenStore | undefined,
    result: Record<string, unknown> | undefined,
    headerToken?: string,
    scope?: string,
  ): void {
    if (session === undefined) {
      return;
    }
    const tokens: string[] = [];
    for (const field of TOKEN_FIELDS) {
      const value = result?.[field];
      if (typeof value === 'string' && value !== '') {
        tokens.push(value);
      }
    }
    if (headerToken !== undefined && headerToken !== '') {
      tokens.push(headerToken);
    }
    for (const token of tokens) {
      // `token` is a common field name; only a `v1:` string is a consistency
      // token, and anything else (a page token, a bearer) is not the
      // session's business.
      if (isForeignToken(token)) {
        continue;
      }
      try {
        session.record(token, scope);
      } catch {
        // The RPC already succeeded. A token the session cannot parse must not
        // turn a committed write into a thrown error, because a caller that
        // retries on that error performs the write twice. The session counts
        // it (`ConsistencySession.rejected`) instead.
      }
    }
  }


}
