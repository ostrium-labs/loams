// Every required fixture, driven through the SDK (design §44 §10.4, D617).
//
// This is the module that makes the 100% bar **checkable** for TypeScript, and
// it is the reference the other twelve SDKs will copy, so the rules it follows
// are the ones worth copying:
//
// - **The required set is read from `manifest.json`**, never written here. A
//   list in the suite would be a list that goes stale, and a corpus that grows
//   is supposed to turn the gate red.
// - **`ran` is what this function returns**, and a fixture is in it only if its
//   own steps were driven *and* held. A fixture whose replay failed is a failure
//   with a name attached, not a line in a report.
// - **The SDK does the work.** The recorded request is decoded into a message
//   and the SDK serialises it again, so the fixture server's byte-for-byte
//   comparison of the request is a real check on this SDK's encoder. The
//   outcome is read back through the SDK's typed error hierarchy (D611).
//
// ## What it drives, and what it cannot
//
// The corpus covers two servers. `loams dev` is the SDK's primary target and its
// cases go out through the same `CallInvoker` every facade method delegates to.
// The `loams-apps-mock` cases are on services that carry **no**
// `loams.options.v1.module` annotation yet — `ApprovalService` and
// `DeviceService` arrive with API1 Tasks 2–4 — so there is no `loams.approvals`
// to call. Those steps are driven through `CallInvoker` against the generated
// descriptor directly, which is the SDK's own call path with the generated
// module wrapper left off: same transport, same credentials, same retry
// decision, same error mapping. That is stated here rather than glossed over.
//
// One recorded shape the SDK's own call path cannot reproduce: **D610 gives
// every mutation an idempotency key**, and seven of the app-mock mutations were
// recorded *without* one. Putting a key on the wire would change the request,
// and the fixture server — correctly — refuses a request that is not the
// recorded one. Those steps go out through the generated client and are still
// read back through the SDK's error mapping; which steps those are is decided
// per step, from the request schema, and not written down here.

import { Code, createClient, type CallOptions, type Transport } from '@connectrpc/connect';
import { createConnectTransport, createGrpcWebTransport } from '@connectrpc/connect-web';
import {
  toJsonString,
  type DescField,
  type DescMessage,
  type DescService,
} from '@bufbuild/protobuf';
import * as approvals from '@loams/proto/approvals';
import * as devices from '@loams/proto/devices';
import * as instance from '@loams/proto/instance';
import * as notifications from '@loams/proto/notifications';
import * as operations from '@loams/proto/operations';
import { LiveService } from '@loams/live/live';
import type { CallBinding } from '../../src/gen/facade.js';
import { CallInvoker } from '../../src/runtime/call.js';
import { toLoamsError, type LoamsError } from '../../src/runtime/errors.js';
import {
  authorizationOf,
  decodeRequest,
  endFramePayload,
  familyOf,
  grpcStatusOf,
  methodOf,
  readFixture,
  requiredFixtures,
  routeOf,
  rpcOf,
  serviceDescriptor,
  type Family,
  type FixtureExpectation,
  type ManifestFixture,
  type RecordedStep,
} from './corpus.js';

/** Every service descriptor this SDK ships, for the ones a fixture may name. */
const SERVICES: readonly DescService[] = [
  instance.InstanceService,
  approvals.ApprovalService,
  devices.DeviceService,
  notifications.NotificationService,
  operations.OperationsService,
  LiveService,
];

/** The field a mutation is keyed by (D610). */
const IDEMPOTENCY_KEY = 'idempotencyKey';
/**
 * The cursor a stream **request** resumes from, and the one a stream **response**
 * hands back — the same value in two directions, which is the whole of R7's
 * resume and why they are named apart here: conflating them would let a step
 * that sent no cursor satisfy an assertion about one that received it.
 */
const RESUME_CURSOR = 'resumeCursor';
const HANDED_CURSOR = 'cursor';

/**
 * The transport a run used, which is what the report records and what
 * `maySkip` keys off.
 *
 * `connect`, not `connect-unary`: the one skip the rule permits is a
 * `transport: grpc-only` fixture on the Connect-unary **fallback**, for a host
 * with no gRPC of its own (D613). This suite speaks gRPC-Web and Connect, so
 * there is no fallback and nothing may be skipped on it.
 */
export const TRANSPORT = 'connect';

/** The transport a family goes out on: Connect over `fetch`, or gRPC-Web. */
function transportFor(family: Family, endpoint: string): Transport {
  const json = family.endsWith('json');
  return family.startsWith('grpc-web')
    ? createGrpcWebTransport({ baseUrl: endpoint, useBinaryFormat: !json })
    : createConnectTransport({ baseUrl: endpoint, useBinaryFormat: !json });
}

/** What one step's replay produced: its frames or its value, and what was thrown. */
interface Outcome {
  /** The response message's descriptor, for reading an enum by name. */
  readonly output: DescMessage;
  /** The response messages, in order. Empty when the call was refused. */
  readonly messages: readonly Record<string, unknown>[];
  /** The same messages in proto JSON, which is how two answers are compared. */
  readonly json: readonly string[];
  /** The mapped failure, or `undefined` when the call answered. */
  readonly error: LoamsError | undefined;
  /**
   * The cursor this step's **request** asked to resume from.
   *
   * Carried separately from the messages because R7 is a claim about the two
   * directions agreeing: the cursor in the request has to be the one a previous
   * response handed out, and reading it back off a response would make that
   * vacuous.
   */
  readonly askedResumeCursor: string | undefined;
}

/**
 * Drives every fixture `manifest.json` marks `required`, and returns the names it
 * actually ran.
 *
 * Each fixture is driven on its own so one failure names one fixture: a suite
 * that missed four of them should learn about all four in one run, and the list
 * returned must not claim the three that held alongside the one that did not.
 */
export async function driveRequiredFixtures(endpoint: string): Promise<string[]> {
  const required = await requiredFixtures();
  const driven: string[] = [];
  const failures: string[] = [];
  const invokers = new Map<Family, CallInvoker>();
  for (const fixture of required) {
    try {
      await driveFixture(fixture, endpoint, (family) => invokerFor(invokers, family, endpoint));
      driven.push(fixture.name);
    } catch (thrown) {
      failures.push(`${fixture.name}: ${thrown instanceof Error ? thrown.message : String(thrown)}`);
    }
  }
  if (failures.length > 0) {
    throw new Error(
      `${failures.length} of ${required.length} required fixture(s) did not hold:\n  - ${failures.join('\n  - ')}`,
    );
  }
  return driven;
}

/**
 * The SDK's own call path for one family: a `CallInvoker` over the transport,
 * with every generated service registered and **no retries**.
 *
 * Zero retries because a replay is not a client: an answer a retry would have
 * asked for again must surface as the failure it is, rather than be retried into
 * a second recorded answer.
 */
function invokerFor(
  cache: Map<Family, CallInvoker>,
  family: Family,
  endpoint: string,
): CallInvoker {
  const existing = cache.get(family);
  if (existing !== undefined) {
    return existing;
  }
  const invoker = new CallInvoker(transportFor(family, endpoint), undefined, 0, undefined);
  for (const service of SERVICES) {
    invoker.register(service.typeName, service);
  }
  cache.set(family, invoker);
  return invoker;
}

/** Drives one fixture's steps in order, asserting each against its recording. */
async function driveFixture(
  fixture: ManifestFixture,
  endpoint: string,
  invokerFor: (family: Family) => CallInvoker,
): Promise<void> {
  const recorded = await readFixture(fixture);
  if (recorded.name !== fixture.name) {
    throw new Error(`${fixture.file} is filed as ${recorded.name}, not ${fixture.name}`);
  }
  const answers: string[][] = [];
  let lastCursor: string | undefined;
  for (const [index, step] of recorded.steps.entries()) {
    const outcome = await replayStep(fixture.name, step, index, endpoint, invokerFor);
    assertStep(fixture.name, index, step, outcome, answers);
    answers.push([...outcome.json]);
    // R7 across steps: a step that resumes must carry the cursor a previous step
    // handed out, not one written into the test. That is the whole invariant of
    // `mock_state_stream_resume`, and it is checked here rather than per fixture.
    const asked = outcome.askedResumeCursor;
    if (asked !== undefined && lastCursor !== undefined && asked !== lastCursor) {
      throw new Error(`step ${index} resumes from ${asked}, but the stream last handed out ${lastCursor}`);
    }
    for (const message of outcome.messages) {
      const cursor = message[HANDED_CURSOR];
      if (typeof cursor === 'string' && cursor !== '') {
        lastCursor = cursor;
      }
    }
  }
}

/** Replays one recorded step and returns what the SDK made of it. */
async function replayStep(
  fixture: string,
  step: RecordedStep,
  index: number,
  endpoint: string,
  invokerFor: (family: Family) => CallInvoker,
): Promise<Outcome> {
  const family = familyOf(step.request.headers['content-type']);
  const { service, method } = routeOf(step.request.path);
  const descriptor = serviceDescriptor(SERVICES, service);
  const rpc = methodOf(descriptor, method);
  const request = decodeRequest(rpc.input, step.request);
  const headers: CallOptions['headers'] = {
    // Which recording answers, when several answer the same key, and which step
    // of it. Both are the fixture server's own contract: six recorded scenarios
    // are the same RPC over the same encoding, and two of them send
    // byte-identical requests twice, so the step cannot be inferred.
    'loams-fixture-name': fixture,
    'loams-fixture-step': String(index),
    ...authorizationOf(step.request),
  };
  const asked = (request as Record<string, unknown>)[RESUME_CURSOR];
  const askedResumeCursor = typeof asked === 'string' && asked !== '' ? asked : undefined;
  const binding = bindingFor(descriptor, rpc);
  const empty: Outcome = { output: rpc.output, messages: [], json: [], error: undefined, askedResumeCursor };
  const json = (messages: readonly Record<string, unknown>[]): string[] =>
    messages.map((message) => toJsonString(rpc.output, message as never));

  if (rpc.methodKind === 'server_streaming') {
    const source = keylessMutation(rpc, request)
      ? openRaw(family, endpoint, descriptor, rpc, request, headers)
      : invokerFor(family).stream(binding, request, { headers });
    const collected = await collect(source);
    return { ...collected, output: rpc.output, json: json(collected.messages), askedResumeCursor };
  }

  const call = keylessMutation(rpc, request)
    ? rawCall(family, endpoint, descriptor, rpc, request, headers)
    : invokerFor(family).unary(binding, request, { headers });
  try {
    const value = (await call) as Record<string, unknown>;
    return { ...empty, messages: [value], json: json([value]) };
  } catch (thrown) {
    return { ...empty, error: toLoamsError(thrown, rpcOf(step.request.path)) };
  }
}

/**
 * Whether this step's recorded request is one the SDK's own call path cannot
 * reproduce, because D610's idempotency key would change it.
 *
 * Decided from the request schema rather than from a list of fixtures, so a
 * corpus that grows a keyed mutation is handled by the same rule and not by
 * somebody remembering to move it.
 */
function keylessMutation(rpc: DescService['methods'][number], request: object): boolean {
  const declares = rpc.input.field[IDEMPOTENCY_KEY] !== undefined;
  const carried = (request as Record<string, unknown>)[IDEMPOTENCY_KEY];
  return declares && (typeof carried !== 'string' || carried === '');
}

/**
 * One generated client method, called through with no options type.
 *
 * The two shapes a generated method has — a promise for a unary RPC and an
 * `AsyncIterable` for a server stream — cannot both be typed at once, so the
 * cast is here and the caller decides which it wanted from `methodKind`.
 */
type RawMethod = (request: unknown, options: CallOptions) => unknown;

function rawMethod(
  family: Family,
  endpoint: string,
  descriptor: DescService,
  rpc: DescService['methods'][number],
): RawMethod {
  const client = createClient(descriptor, transportFor(family, endpoint)) as unknown as Record<
    string,
    RawMethod
  >;
  const method = client[rpc.localName];
  if (typeof method !== 'function') {
    throw new Error(`${descriptor.typeName} has no generated client method ${rpc.localName}`);
  }
  return method;
}

function rawCall(
  family: Family,
  endpoint: string,
  descriptor: DescService,
  rpc: DescService['methods'][number],
  request: unknown,
  headers: CallOptions['headers'],
): Promise<unknown> {
  return rawMethod(family, endpoint, descriptor, rpc)(request, { headers }) as Promise<unknown>;
}

function openRaw(
  family: Family,
  endpoint: string,
  descriptor: DescService,
  rpc: DescService['methods'][number],
  request: unknown,
  headers: CallOptions['headers'],
): AsyncIterable<unknown> {
  return rawMethod(family, endpoint, descriptor, rpc)(request, { headers }) as AsyncIterable<unknown>;
}

/**
 * A server stream, collected frame by frame.
 *
 * A recorded stream is a bounded **prefix** — a real stream never ends — so the
 * recording has no end frame and the client reports a missing one once the
 * recorded frames are through. That is a property of the recording, not of the
 * SDK, so the frames are what is asserted and the truncation is carried as the
 * error rather than swallowed.
 */
async function collect(source: AsyncIterable<unknown>): Promise<{
  messages: Record<string, unknown>[];
  error: LoamsError | undefined;
}> {
  const messages: Record<string, unknown>[] = [];
  try {
    for await (const message of source) {
      messages.push(message as Record<string, unknown>);
    }
    return { messages, error: undefined };
  } catch (thrown) {
    return { messages, error: toLoamsError(thrown) };
  }
}

/** The binding table's row for a generated method, built from its descriptor. */
function bindingFor(descriptor: DescService, rpc: DescService['methods'][number]): CallBinding {
  return {
    module: descriptor.name.replace(/Service$/, ''),
    name: rpc.localName,
    protoName: rpc.localName,
    method: rpc.name,
    rpc: `${descriptor.typeName}/${rpc.name}`,
    service: descriptor.typeName,
    package: descriptor.typeName.split('.').slice(0, -1).join('.'),
    idempotency: 'none',
    // `manual`: the invoker retries a mutation only once it carries a key, and
    // decides that itself from the request schema.
    retry: 'manual',
    streaming: rpc.methodKind === 'server_streaming' ? 'server' : 'unary',
    pagination: null,
  };
}

/**
 * The Connect code a recorded refusal carries.
 *
 * Read from the recording's own bytes, in the order the protocol files them,
 * because **an HTTP status cannot tell these refusals apart**: `400` covers
 * `invalid_argument` *and* `failed_precondition`, and a gRPC-Web refusal is a
 * `200` with the code in its trailers. A driver that mapped the status would be
 * asserting a coincidence, and would start failing the moment the corpus grew a
 * refusal in a combination the table had no row for.
 */
function recordedCode(step: RecordedStep): Code | undefined {
  if (step.expect?.grpcStatus !== undefined) {
    return step.expect.grpcStatus as Code;
  }
  // Connect unary: `{"code": "unimplemented", …}`.
  const body = step.response.body;
  if (body !== undefined) {
    const code = codeNameOf(body, 'code');
    if (code !== undefined) {
      return codeFromName(code);
    }
  }
  // gRPC-Web: the code is in a trailing header block, under a 200.
  const grpcStatus = grpcStatusOf(step.response);
  if (grpcStatus !== undefined) {
    return grpcStatus as Code;
  }
  // Connect stream: the failure is in the end-of-stream frame.
  const end = endFramePayload(step.response);
  if (end !== undefined) {
    try {
      const parsed = JSON.parse(end) as { error?: { code?: unknown } };
      if (typeof parsed.error?.code === 'string') {
        return codeFromName(parsed.error.code);
      }
    } catch {
      return undefined;
    }
  }
  return undefined;
}

/** A Connect error's code as a string, from a recorded body or frame. */
function codeNameOf(value: unknown, key: string): string | undefined {
  if (typeof value !== 'object' || value === null) {
    return undefined;
  }
  const code = (value as Record<string, unknown>)[key];
  return typeof code === 'string' ? code : undefined;
}

/**
 * The canonical `snake_case` name of every Connect code.
 *
 * Written out rather than imported because connect-es keeps its
 * `codeFromString` private, and this table is the **protocol's** vocabulary, not
 * the SDK's: deriving the expectation from the SDK's own registry would make
 * the assertion agree with whatever the registry says, which is exactly the
 * thing under test. The numbers are gRPC's, and the two agree.
 */
const CODE_NAMES: Readonly<Record<Code, string>> = {
  [Code.Canceled]: 'canceled',
  [Code.Unknown]: 'unknown',
  [Code.InvalidArgument]: 'invalid_argument',
  [Code.DeadlineExceeded]: 'deadline_exceeded',
  [Code.NotFound]: 'not_found',
  [Code.AlreadyExists]: 'already_exists',
  [Code.PermissionDenied]: 'permission_denied',
  [Code.ResourceExhausted]: 'resource_exhausted',
  [Code.FailedPrecondition]: 'failed_precondition',
  [Code.Aborted]: 'aborted',
  [Code.OutOfRange]: 'out_of_range',
  [Code.Unimplemented]: 'unimplemented',
  [Code.Internal]: 'internal',
  [Code.Unavailable]: 'unavailable',
  [Code.DataLoss]: 'data_loss',
  [Code.Unauthenticated]: 'unauthenticated',
};

/** The code a `snake_case` protocol name names, or `undefined`. */
function codeFromName(name: string): Code | undefined {
  for (const [code, spelling] of Object.entries(CODE_NAMES)) {
    if (spelling === name) {
      return Number(code) as Code;
    }
  }
  return undefined;
}

/** The gRPC code an HTTP status carries when nothing in the body says better. */
function codeFor(status: number): Code {
  switch (status) {
    case 400:
      return Code.InvalidArgument;
    case 401:
      return Code.Unauthenticated;
    case 403:
      return Code.PermissionDenied;
    case 404:
      return Code.NotFound;
    case 408:
    case 504:
      return Code.DeadlineExceeded;
    case 409:
      return Code.Aborted;
    case 412:
      return Code.FailedPrecondition;
    case 429:
      return Code.ResourceExhausted;
    case 501:
      return Code.Unimplemented;
    case 503:
      return Code.Unavailable;
    default:
      return Code.Internal;
  }
}

/**
 * Whether the only thing that went wrong is that the recording stopped.
 *
 * `conformance.test.mjs` records a stream as a prefix because a stream never
 * ends, and a client that reached the end of a truncated recording correctly
 * says so. Accepting *any* error here would make the truncation a loophole, so
 * it is narrowed twice: the recording has to declare itself truncated, and the
 * error has to be the missing end frame and nothing else.
 */
function isEndOfRecording(step: RecordedStep, error: LoamsError): boolean {
  return (
    step.response.truncated === true &&
    error.code === Code.Unknown &&
    /missing EndStreamResponse/.test(error.message)
  );
}

/**
 * Whether a step's recorded answer is a refusal.
 *
 * Three signals, because the corpus files a refusal three ways: an HTTP status
 * of 400 or more (Connect), a `grpc-status` in the trailers under a `200`
 * (gRPC-Web), and a declared `reason` — which is what makes the difference
 * visible, since `mock_status_unauthenticated` declares `reason: null` and is
 * still a refusal, and a gRPC-Web refusal declares a reason under a `200`.
 */
function isRefusal(step: RecordedStep): boolean {
  const expectation = step.expect ?? {};
  return (
    expectation.grpcStatus !== undefined ||
    (expectation.status ?? 200) >= 400 ||
    'reason' in expectation
  );
}

/** Asserts one step's outcome against what the recording says it must be. */
function assertStep(
  fixture: string,
  index: number,
  step: RecordedStep,
  outcome: Outcome,
  answers: readonly (readonly string[])[],
): void {
  const where = `${fixture} step ${index}`;
  const expectation: FixtureExpectation = step.expect ?? {};
  if (isRefusal(step)) {
    const error = outcome.error;
    if (error === undefined) {
      throw new Error(
        `${where} is recorded as a refusal and the SDK returned ${outcome.messages.length} message(s)`,
      );
    }
    const wanted = recordedCode(step) ?? codeFor(expectation.status ?? 500);
    if (error.code !== wanted) {
      throw new Error(`${where} is recorded as code ${wanted} and the SDK raised ${error.code}: ${error.message}`);
    }
    if ('reason' in expectation) {
      const declared = expectation.reason ?? undefined;
      if (error.reason !== declared) {
        throw new Error(
          `${where} carries reason ${JSON.stringify(declared)} and the SDK read ${JSON.stringify(error.reason)}`,
        );
      }
    }
    return;
  }

  if (outcome.error !== undefined) {
    if (isEndOfRecording(step, outcome.error)) {
      // A recorded stream is a bounded **prefix**: the recorder closed the
      // connection, so there is no end frame and the client reports the missing
      // one once the recorded frames are through. The frames are asserted below;
      // this is the recording's truncation, not the SDK's behaviour, and it is
      // accepted only when the recording says it was truncated and only for that
      // one error.
    } else {
      throw new Error(`${where} answers ${expectation.status ?? 200} and the SDK raised ${outcome.error.message}`);
    }
  }
  if (expectation.frames !== undefined && outcome.messages.length !== expectation.frames) {
    throw new Error(
      `${where} recorded ${expectation.frames} frame(s) and the SDK read ${outcome.messages.length}`,
    );
  }
  if (expectation.frameKinds !== undefined) {
    const kinds = outcome.messages.map((message) => eventOf(message));
    if (kinds.join(',') !== expectation.frameKinds.join(',')) {
      throw new Error(
        `${where} recorded frames [${expectation.frameKinds.join(', ')}] and the SDK read [${kinds.join(', ')}]`,
      );
    }
  }
  if (expectation.cursor !== undefined) {
    const cursors = outcome.messages.map((message) => message[HANDED_CURSOR]);
    if (!cursors.includes(expectation.cursor)) {
      throw new Error(
        `${where} recorded cursor ${expectation.cursor} and the SDK read [${cursors.map(String).join(', ')}]`,
      );
    }
  }
  if (expectation.snapshotReset !== undefined) {
    const read = outcome.messages.map((message) => message['snapshotReset']);
    if (!read.includes(expectation.snapshotReset)) {
      throw new Error(
        `${where} recorded snapshotReset ${String(expectation.snapshotReset)} and the SDK read [${read.map(String).join(', ')}]`,
      );
    }
  }
  const answer = outcome.messages[0] ?? {};
  if (expectation.apiVersions !== undefined) {
    const served = (answer['apiVersions'] as string[] | undefined) ?? [];
    for (const version of expectation.apiVersions) {
      if (!served.includes(version)) {
        throw new Error(`${where} serves [${served.join(', ')}], which does not include ${version}`);
      }
    }
  }
  const approval = (answer['approval'] ?? {}) as Record<string, unknown>;
  if (expectation.state !== undefined && approval['state'] !== undefined) {
    // An enum is a number in memory and a name in the recording, so it is read
    // back through the **response's own descriptor** and compared by name.
    // Comparing `String(2)` against `APPROVAL_STATE_APPROVED` would pass for
    // every approval state in the proto and fail for every reason that matters.
    const read = enumName(outcome.output, ['approval', 'state'], approval['state']);
    if (read !== expectation.state) {
      throw new Error(`${where} answers state ${String(read)}, not ${expectation.state}`);
    }
  }
  if (expectation.revision !== undefined && approval['revision'] !== undefined) {
    if (String(approval['revision']) !== expectation.revision) {
      throw new Error(`${where} answers revision ${String(approval['revision'])}, not ${expectation.revision}`);
    }
  }
  if (expectation.identicalToStep !== undefined) {
    const earlier = answers[expectation.identicalToStep];
    if (earlier === undefined || earlier.join('\n') !== outcome.json.join('\n')) {
      throw new Error(
        `${where} is recorded as byte-identical to step ${expectation.identicalToStep}, and the SDK's two answers differ`,
      );
    }
  }
}

/** The branch of a stream message's `oneof`, which is what a frame *is*. */
function eventOf(message: Record<string, unknown>): string {
  const event = (message['event'] ?? {}) as { case?: unknown };
  return typeof event.case === 'string' ? event.case : 'none';
}

/**
 * The name of an enum value, read through the message's own descriptor.
 *
 * `path` walks down to the field, which has to be an enum: a number in a
 * message is unreadable on its own, and the recording spells it out.
 */
function enumName(root: DescMessage, path: readonly string[], value: unknown): string | undefined {
  let descriptor: DescMessage | undefined = root;
  let field: DescField | undefined;
  for (const segment of path) {
    field = descriptor?.field[segment];
    if (field === undefined) {
      return undefined;
    }
    descriptor = (field as { message?: DescMessage }).message;
  }
  const values = (field as { enum?: { values: readonly { name: string; number: number }[] } }).enum
    ?.values;
  return values?.find((candidate) => candidate.number === Number(value))?.name;
}