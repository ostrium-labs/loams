// The conformance fault injector (SDK1 Task 4, design §44 §10.4).
//
// ## These are not recordings
//
// Everything else in `sdks/fixtures` was captured from a real server, because a
// hand-written expectation only proves the SDK agrees with whoever wrote it.
// The faults in `sdks/fixtures/faults.json` are the deliberate exception, and
// they are labelled as such in that file. They cannot be anything else: a
// retryable `unavailable` needs a dependency to be down, a mid-stream disconnect
// needs something to drop the connection, and `token_expired` needs a token
// endpoint that does not exist yet (MT, API1 Task 7). A healthy server will not
// produce them on demand, which is exactly why the plan asks for a fault
// injector beside the real target rather than instead of it.
//
// What is still real: the **bytes on the wire**. A fault is a response this
// module writes — status, headers, Connect error envelope, gRPC-Web trailer
// frame — in the shapes a real server of this API would use, because the SDK
// under test cannot tell a synthesised failure from a genuine one and must
// handle both identically. That is the whole point: if the injected shape drifts
// from the real one, an SDK passes here and fails in production, so
// `faults.json` pins the shapes and the shapes are checked against the
// recordings by `verify-corpus.mjs`.
//
// ## How a fault is selected
//
// A request carries `loams-test-fault: <name>`. A header, not a path, because
// all thirteen languages can set one and none of them should have to learn a
// fake URL shape to test a retry. Nothing is injected unless the header is
// present, so the same proxy serves the plain corpus unchanged.
//
// `loams-test-fault-after: <n>` asks a `stream_drop` to stop after n frames
// instead of after the first one, which is what makes "resume from the last
// cursor you applied" testable: the client has to have applied a cursor before
// the drop for the resume to have anything to resume from.

import { createServer } from 'node:http';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));

/** The header that asks for a fault. Absent means no fault. */
export const FAULT_HEADER = 'loams-test-fault';
/** The header that says how far into a stream the fault bites. */
export const FAULT_AFTER_HEADER = 'loams-test-fault-after';

/**
 * One `map<string, string>` entry, which is what `ErrorInfo.metadata` is: a
 * repeated message on field 2, each with `key` on field 1 and `value` on field 2
 * (`proto/loams/errors/v1/errors.proto`).
 *
 * Encoded as protobuf rather than as a JSON blob, on purpose. An SDK reads this
 * with a generated `ErrorInfo` type, so a fault that put a JSON string where the
 * map is would let an SDK that cannot read the map pass the fixture.
 */
function mapEntry(key, value) {
  const keyBytes = Buffer.from(key, 'utf8');
  const valueBytes = Buffer.from(value, 'utf8');
  return Buffer.concat([
    Buffer.from([0x0a, keyBytes.length]),
    keyBytes,
    Buffer.from([0x12, valueBytes.length]),
    valueBytes,
  ]);
}

/** Connect's `ErrorInfo` (`loams.errors.v1`), serialized the way a server does. */
function errorInfo(reason, metadata) {
  const parts = [];
  // Field 1, wire type 2: a length-delimited string.
  const reasonBytes = Buffer.from(reason, 'utf8');
  parts.push(Buffer.from([0x0a, reasonBytes.length]), reasonBytes);
  for (const [key, value] of Object.entries(metadata ?? {})) {
    const entry = mapEntry(key, String(value));
    // Field 2, wire type 2, repeated: one entry per key.
    parts.push(Buffer.from([0x12, entry.length]), entry);
  }
  return Buffer.concat(parts);
}

/** The base64 a Connect JSON error puts in `details[].value`. */
function detailValue(reason, metadata) {
  return errorInfo(reason, metadata).toString('base64');
}

/**
 * A Connect JSON unary error body, shaped the way connect-rust shapes one.
 *
 * The field order matters to nobody and the presence of every field matters to
 * everybody: an SDK that reads `details[0].type` to decide what it is holding
 * has to find the type URL there, and one that reads `details[0].value` has to
 * find base64 protobuf. A fault that omits either would let a broken SDK pass.
 */
function connectErrorBody(code, message, reason, metadata) {
  return JSON.stringify({
    code,
    message,
    details: reason ? [{ type: 'loams.errors.v1.ErrorInfo', value: detailValue(reason, metadata) }] : [],
  });
}

/** A gRPC-Web trailers frame: the high flag bit, then the trailers as text. */
function trailersFrame(trailers) {
  const payload = Buffer.from(
    Object.entries(trailers)
      .map(([key, value]) => `${key}: ${value}`)
      .join('\r\n') + '\r\n',
    'utf8',
  );
  const header = Buffer.alloc(5);
  header[0] = 0x80;
  header.writeUInt32BE(payload.length, 1);
  return Buffer.concat([header, payload]);
}

/**
 * The faults, as data.
 *
 * Kept next to the code that serves them rather than only in
 * `sdks/fixtures/faults.json` so the two cannot disagree: `faults.json` is
 * written by hand (nothing records a fault) and this table is what a test
 * asserts against, so a change to one that is not a change to the other shows up
 * as a failing shape rather than as silence.
 *
 * `grpcStatus` is the gRPC numbering, which differs from the Connect code for
 * the same class: `unavailable` is code 13 over gRPC and 503 over HTTP.
 */
export const FAULTS = {
  unavailable: {
    about:
      'A retryable refusal (R2): the dependency this read needs is down. An SDK ' +
      'retries this one, with full jitter, base 100 ms, capped at 2 s, three ' +
      'retries — and stops after that rather than spinning.',
    code: 'unavailable',
    reason: 'unavailable',
    httpStatus: 503,
    grpcStatus: 14, // UNAVAILABLE
    retryable: true,
    pinnedBy: ['R2'],
  },
  resource_exhausted: {
    about:
      'The other retryable class, from backpressure. Distinct from ' +
      '`unavailable` because a caller usually wants to slow down rather than ' +
      'back off: this is the one carrying `retry_after_ms`.',
    code: 'resource_exhausted',
    reason: 'resource_exhausted',
    httpStatus: 429,
    grpcStatus: 8, // RESOURCE_EXHAUSTED
    metadata: { retry_after_ms: 250 },
    retryable: true,
    pinnedBy: ['R2'],
  },
  deadline_exceeded: {
    about:
      "The caller's own deadline passed. Retryable, but only because the client " +
      'chose a deadline it can extend; a server that answers this has not done ' +
      'anything wrong.',
    code: 'deadline_exceeded',
    reason: 'deadline_exceeded',
    httpStatus: 504,
    grpcStatus: 4, // DEADLINE_EXCEEDED
    retryable: true,
    pinnedBy: ['R2'],
  },
  retry_after: {
    about:
      'R2\'s last sentence: a server-sent `RetryInfo.retry_delay` replaces the ' +
      'computed backoff, up to 30 s. **No proto carries `RetryInfo` yet**, so ' +
      'this is injected with the headers a gRPC retry policy would read and the ' +
      'header the API reserves — which is why it is the one fault whose shape is ' +
      'most likely to be wrong, and the one to watch when `RetryInfo` lands.',
    code: 'unavailable',
    reason: 'unavailable',
    httpStatus: 503,
    grpcStatus: 14,
    // `grpc-retry-pushback-ms` is what a gRPC client reads; the `loams-` one is
    // what the API would use once there is a proto to put it in.
    trailers: { 'grpc-retry-pushback-ms': '1500', 'loams-retry-after-ms': '1500' },
    retryable: true,
    pinnedBy: ['R2'],
  },
  token_expired: {
    about:
      'R1\'s trigger, and the reason it is in the registry: a rejected access ' +
      'token, as distinct from a token that was never sent. An SDK refreshes ' +
      '**once** and retries once, and reports a second expiry rather than ' +
      'looping. No server can produce this today — `loams dev` has no ' +
      'authentication at all — so without this fault R1 would be untestable ' +
      'against anything.',
    code: 'unauthenticated',
    reason: 'token_expired',
    httpStatus: 401,
    grpcStatus: 16, // UNAUTHENTICATED
    retryable: false,
    pinnedBy: ['R1'],
  },
  stream_drop: {
    about:
      'The stream dies mid-flight. The server sent a cursor and then stopped ' +
      'without an end frame, which is what a proxy timeout or a mobile ' +
      'handover looks like to a client. R7 says the client re-opens from the ' +
      'last cursor it applied and does not re-yield what it already yielded; ' +
      'this is the only way to check that, because a stream that ends cleanly ' +
      'proves nothing about resuming.',
    code: null,
    reason: null,
    stream: true,
    retryable: true,
    pinnedBy: ['R7'],
  },
};

/** The faults a language must exercise, in the order R2 and R1 name them. */
export const REQUIRED_FAULTS = [
  'unavailable',
  'resource_exhausted',
  'deadline_exceeded',
  'retry_after',
  'token_expired',
  'stream_drop',
];

/** Whether `name` is a fault this build knows how to inject. */
export function isFault(name) {
  return Object.hasOwn(FAULTS, name);
}

/**
 * Answers one request with a fault, or returns `false` so the recorded corpus
 * handles it.
 *
 * `sendFrames` is how the fixture server streams: it is called with one
 * serialized frame at a time and returns nothing, and only `stream_drop` uses
 * it. Everything else is a complete unary answer.
 *
 * The gRPC-Web path follows the **recorded** rule rather than the Connect one:
 * HTTP 200 with the code in the trailers. A fault that answered 503 on gRPC-Web
 * would be a fault no browser client could parse, and the point is to make the
 * SDK's ordinary handling apply unchanged.
 */
export function injectFault(request, response, sendFrames) {
  const name = request.headers[FAULT_HEADER];
  if (!name) {
    return false;
  }
  if (!isFault(name)) {
    // An unknown fault name is an error, not a pass-through. A typo in a test's
    // header would otherwise mean the SDK was handed a healthy answer and
    // "passed" a fault it never saw.
    response.writeHead(400, { 'content-type': 'application/json' });
    response.end(
      JSON.stringify({
        error: `no such fault: ${name}`,
        known: Object.keys(FAULTS),
      }),
    );
    return true;
  }
  const fault = FAULTS[name];
  const contentType = String(request.headers['content-type'] ?? '');
  const framed = contentType.startsWith('application/grpc-web');

  if (fault.stream) {
    dropStream(request, response, contentType, sendFrames);
    return true;
  }

  if (framed) {
    // A trailers-only gRPC-Web answer: one frame with the high bit set and the
    // status in it, which is byte-for-byte the shape the corpus recorded for
    // `live_query_grpc_web`.
    const trailers = {
      'grpc-status': String(fault.grpcStatus),
      'grpc-message': fault.code,
      ...(fault.trailers ?? {}),
    };
    if (fault.trailers) {
      // The pushback rides in the frame: a trailers-only answer has no other
      // place to put a header, and a client that reads `grpc-retry-pushback-ms`
      // reads it there.
      for (const [key, value] of Object.entries(fault.trailers)) {
        trailers[key] = String(value);
      }
    }
    response.writeHead(200, { 'content-type': contentType });
    response.end(trailersFrame(trailers));
    return true;
  }

  response.writeHead(fault.httpStatus, {
    'content-type': 'application/json',
    ...(fault.metadata?.retry_after_ms
      ? { 'retry-after-ms': String(fault.metadata.retry_after_ms) }
      : {}),
  });
  response.end(
    Buffer.from(
      connectErrorBody(
        fault.code,
        `${fault.code}: injected by the conformance fault injector`,
        fault.reason,
        fault.metadata,
      ),
      'utf8',
    ),
  );
  return true;
}

/**
 * Sends `loams-test-fault-after` real frames, then kills the socket.
 *
 * The frames are the corpus's own — a real snapshot with real cursors — with the
 * connection cut on top, so "resume from the last cursor you applied" is
 * testable: the client has to have applied a cursor before the drop for the
 * resume to have anything to resume from.
 *
 * `destroy` and not `end`, and no `grpc-status: 0` anywhere. An orderly end
 * tells the client the stream is finished, and an SDK that only resumes after a
 * clean end would pass this and be wrong in production.
 */
function dropStream(request, response, contentType, sendFrames) {
  const after = Number(request.headers[FAULT_AFTER_HEADER] ?? 1);
  response.writeHead(200, { 'content-type': contentType });
  // The head goes out before any frame, so the client's transport is up and the
  // drop is mid-**body**. Without this the socket can be cut before the response
  // exists, and the client gets a connection error instead of a truncated stream
  // — which would test "does it survive a refused connection", not "does it
  // resume from the last cursor it applied".
  response.flushHeaders();
  let seen = 0;
  // `write(frame, callback)` resolves once the bytes are handed to the kernel, so
  // the destroy happens after the last frame is really out. Destroying on the
  // same tick discards the buffered write and the client never saw the cursor.
  let flushed = Promise.resolve();
  sendFrames((frame) => {
    if (seen >= after) {
      return false;
    }
    seen += 1;
    flushed = new Promise((resolve) => {
      response.write(frame, resolve);
    });
    return seen < after;
  });
  flushed.then(() => {
    request.socket.destroy();
  });
}

/**
 * `faults.json`, the catalogue a language's suite reads.
 *
 * Written by hand, because nothing records a fault; `FAULTS` above is what the
 * injector actually serves, and `verify-corpus.mjs` fails when the two disagree.
 */
export function faultsCatalogue() {
  return JSON.parse(readFileSync(join(here, '..', 'fixtures', 'faults.json'), 'utf8'));
}
