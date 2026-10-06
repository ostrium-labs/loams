// The recorded conformance corpus, read as data (design §44 §10.4, D617).
//
// Everything the required-fixture driver needs to know about a fixture comes
// from here and from `manifest.json`: where the recording is, what its steps
// were, and what each step has to hold. **Nothing is written down twice** — the
// names, the required set, the recorded request and the expectation are all read
// from the corpus, so a fixture that moves cannot leave the suite asserting the
// old thing.
//
// The two files the corpus is filed under are both walked: `recorded/` is the
// `loams dev` half and `recorded/apps-mock/` the `loams-apps-mock` half, and
// `manifest.json`'s `file` field says which one a fixture is in, so no
// directory is guessed.

import {
  fromBinary,
  fromJsonString,
  type DescMessage,
  type DescService,
  type MessageShape,
} from '@bufbuild/protobuf';
import { readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { FIXTURES } from './server.js';

/** What `manifest.json` says about one fixture. */
export interface ManifestFixture {
  readonly name: string;
  readonly kind: string;
  readonly server: string;
  readonly required: boolean;
  readonly transport?: string;
  readonly reason?: string | null;
  /** The recording, relative to the fixture root. */
  readonly file: string;
}

/** What a fixture's step has to hold, as the recorder filed it. */
export interface FixtureExpectation {
  readonly status?: number;
  /** The gRPC status a `200` carries in its trailers instead of a status code. */
  readonly grpcStatus?: number;
  /** `null` means the server sent no `ErrorInfo` at all, which is a fact. */
  readonly reason?: string | null;
  readonly state?: string;
  readonly revision?: string;
  readonly apiVersions?: readonly string[];
  readonly identicalToStep?: number;
  readonly frames?: number;
  readonly frameKinds?: readonly string[];
  readonly cursor?: string;
  readonly snapshotReset?: boolean;
  /** The recording is a prefix of a stream; stated rather than implied. */
  readonly truncated?: boolean;
}

/** One recorded request, as the recorder filed it. */
export interface RecordedRequest {
  readonly method: string;
  readonly path: string;
  readonly headers: Record<string, string>;
  readonly body?: string;
  readonly bodyBase64?: string;
}

/** The recorded answer, in whichever encoding it was captured in. */
export interface RecordedResponse {
  readonly status: number;
  readonly headers: Record<string, string>;
  /** A Connect unary answer, as JSON. */
  readonly body?: unknown;
  /** A gRPC-Web or streaming answer, as the bytes on the wire. */
  readonly bodyBase64?: string;
  /** The frames a streaming answer is made of, when the recorder split them. */
  readonly frames?: readonly { flags?: number; payload: string }[];
  /** True when the recording is a prefix and the stream never ended. */
  readonly truncated?: boolean;
}

/** One step of a recording: a request, and what it answered with. */
export interface RecordedStep {
  readonly request: RecordedRequest;
  readonly response: RecordedResponse;
  readonly expect?: FixtureExpectation;
}

/** One recording: a single request, or a scenario's ordered steps. */
export interface RecordedFixture {
  readonly name: string;
  readonly kind: string;
  readonly transport: string;
  readonly reason: string | null;
  readonly steps: readonly RecordedStep[];
}

/** The manifest, which is the authority on the 100% bar. */
export async function manifest(): Promise<ManifestFixture[]> {
  const parsed = JSON.parse(await readFile(join(FIXTURES, 'manifest.json'), 'utf8')) as {
    fixtures?: ManifestFixture[];
  };
  if (!Array.isArray(parsed.fixtures)) {
    throw new Error(`${join(FIXTURES, 'manifest.json')} has no fixtures array`);
  }
  return parsed.fixtures;
}

/**
 * The fixtures `manifest.json` marks `required`, in manifest order.
 *
 * Read from the manifest rather than written out, because a list written here
 * is a list that goes stale: the corpus growing is supposed to turn the gate
 * red, and a suite carrying its own copy of the bar would go on passing.
 */
export async function requiredFixtures(): Promise<ManifestFixture[]> {
  return (await manifest()).filter((fixture) => fixture.required);
}

/**
 * The recording behind a fixture.
 *
 * A fixture with no file behind it is an error rather than a skip: that is the
 * case where a language would be excused for a test nobody wrote.
 */
export async function readFixture(fixture: ManifestFixture): Promise<RecordedFixture> {
  const file = join(FIXTURES, fixture.file);
  const recorded = JSON.parse(await readFile(file, 'utf8')) as RecordedFixture & {
    steps?: RecordedStep[];
  };
  // A single-request fixture is the recording; a scenario is `steps`. Which one
  // it is read from the file rather than decided from the fixture's name.
  const steps =
    recorded.steps ??
    ([
      {
        request: (recorded as unknown as { request: RecordedStep['request'] }).request,
        response: (recorded as unknown as { response: RecordedStep['response'] }).response,
        expect: (recorded as unknown as { expect?: FixtureExpectation }).expect,
      },
    ] as RecordedStep[]);
  if (steps.length === 0 || steps[0]?.request === undefined) {
    throw new Error(`${file} has no request to replay`);
  }
  return { ...recorded, steps };
}

/** The wire families the corpus is filed under, one transport each. */
export type Family =
  | 'json'
  | 'proto'
  | 'grpc-web+json'
  | 'grpc-web+proto'
  | 'connect+json'
  | 'connect+proto';

/**
 * The family a `content-type` belongs to, which is both the codec and the
 * transport a call has to go out on.
 *
 * The families are kept apart exactly as `encodings.mjs` keeps them apart: a
 * client that asked for protobuf must not be handed JSON, and that shows up as
 * a parse error inside somebody's SDK rather than as a failure in the harness.
 */
export function familyOf(contentType: string | undefined): Family {
  const value = (contentType ?? '').split(';')[0]?.trim().toLowerCase() ?? '';
  switch (value) {
    case 'application/json':
      return 'json';
    case 'application/proto':
      return 'proto';
    case 'application/grpc-web+json':
      return 'grpc-web+json';
    case 'application/grpc-web+proto':
      return 'grpc-web+proto';
    case 'application/connect+json':
      return 'connect+json';
    case 'application/connect+proto':
      return 'connect+proto';
    default:
      throw new Error(`the recorded request declares an encoding this SDK does not speak: '${value}'`);
  }
}

/** Whether a family puts its message in a 5-byte envelope before the payload. */
export function isFramed(family: Family): boolean {
  return family.startsWith('grpc-web') || family.startsWith('connect');
}

/** The bytes a recorded request carried, envelope and all. */
export function requestBytes(request: RecordedRequest): Buffer {
  return request.body === undefined
    ? Buffer.from(request.bodyBase64 ?? '', 'base64')
    : Buffer.from(request.body, 'utf8');
}

/** The bytes a recorded response carried, in whichever field it was filed in. */
export function responseBytes(response: RecordedResponse): Buffer {
  if (response.bodyBase64 !== undefined) {
    return Buffer.from(response.bodyBase64, 'base64');
  }
  return typeof response.body === 'string'
    ? Buffer.from(response.body, 'utf8')
    : Buffer.from(JSON.stringify(response.body ?? null), 'utf8');
}

/** Flags in a Connect streaming frame: bit 0 compressed, bit 1 end-of-stream. */
const FLAG_END = 0x02;

/**
 * The Connect end-of-stream frame's payload, when the body carries one.
 *
 * A Connect stream's failure is not a status and not a frame: it is the
 * `EndStreamResponse` at the end, whose `error.code` is the code the server
 * raised. A client reads it there, so the driver has to look there too.
 */
export function endFramePayload(response: RecordedResponse): string | undefined {
  for (const frame of response.frames ?? []) {
    if ((frame.flags ?? 0) & FLAG_END) {
      return Buffer.from(frame.payload, 'base64').toString('utf8');
    }
  }
  const raw = responseBytes(response);
  let at = 0;
  while (at + 5 <= raw.length) {
    const flags = raw[at] ?? 0;
    const length = raw.readUInt32BE(at + 1);
    const payload = raw.subarray(at + 5, at + 5 + length);
    if (payload.length !== length) {
      return undefined;
    }
    if (flags & FLAG_END) {
      return payload.toString('utf8');
    }
    at += 5 + length;
  }
  return undefined;
}

/**
 * The `grpc-status` a gRPC-Web answer carries in its trailers.
 *
 * gRPC-Web answers a refusal with a `200` and the code in a trailing header
 * block inside the last frame, which is the whole reason the corpus records one
 * refusal in four encodings: a client that reads status codes sees success.
 */
export function grpcStatusOf(response: RecordedResponse): number | undefined {
  const match = /grpc-status:\s*(\d+)/.exec(responseBytes(response).toString('latin1'));
  return match?.[1] === undefined ? undefined : Number(match[1]);
}

/**
 * The recorded request as a protobuf-es message, so the SDK **serialises it
 * itself** rather than the suite handing the recording back.
 *
 * That is the point of the driver: the fixture server compares the request it
 * is sent against the recorded one byte for byte, so if this SDK's encoder ever
 * drifts — field order, an enum's spelling, a `uint64` as a string — the replay
 * stops matching and the test fails. Replaying the recorded bytes with `fetch`
 * would pass no matter what the SDK wrote.
 */
export function decodeRequest<Desc extends DescMessage>(
  input: Desc,
  request: RecordedRequest,
): MessageShape<Desc> {
  const family = familyOf(request.headers['content-type']);
  const raw = requestBytes(request);
  // gRPC-Web and Connect both wrap the message in a 5-byte envelope (`flags`,
  // then a big-endian length); a unary Connect request is bare.
  const payload = isFramed(family) && raw.length >= 5 ? raw.subarray(5) : raw;
  return family.endsWith('json')
    ? fromJsonString(input, payload.toString('utf8'))
    : fromBinary(input, new Uint8Array(payload));
}

/**
 * The `package.Service` and `Method` a recorded path names.
 *
 * Read off the path rather than from a table, because the path is what the call
 * actually goes to: a table would be a second list of RPCs to keep in step with
 * the protos, and it would quietly skip a fixture whose service this SDK had
 * never heard of instead of saying so.
 */
export function routeOf(path: string): { readonly service: string; readonly method: string } {
  const parts = path.split('/');
  const method = parts[parts.length - 1];
  const service = parts[parts.length - 2];
  if (method === undefined || service === undefined || service === '') {
    throw new Error(`the recorded path ${path} names no service`);
  }
  return { service, method };
}

/** The RPC a recorded step's path names, as `package.Service/Method`. */
export function rpcOf(path: string): string {
  const { service, method } = routeOf(path);
  return `${service}/${method}`;
}

/** The bearer a recorded request carried, if it had one. */
export function authorizationOf(request: RecordedRequest): Record<string, string> {
  const bearer = request.headers['authorization'];
  return bearer === undefined ? {} : { authorization: bearer };
}

/** The service descriptor a recorded path names, out of the ones this SDK has. */
export function serviceDescriptor(
  services: readonly DescService[],
  service: string,
): DescService {
  const found = services.find((candidate) => candidate.typeName === service);
  if (found === undefined) {
    throw new Error(
      `this SDK has no generated descriptor for ${service}; the corpus drives it and the ` +
        'fixture cannot be run until the SDK speaks the service',
    );
  }
  return found;
}

/** The method a recorded path names, matching either spelling of its name. */
export function methodOf(service: DescService, method: string): DescService['methods'][number] {
  const found = service.methods.find(
    (candidate) => candidate.name === method || candidate.localName === method,
  );
  if (found === undefined) {
    throw new Error(`${service.typeName} has no method ${method}`);
  }
  return found;
}