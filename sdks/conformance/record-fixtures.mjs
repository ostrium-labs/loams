// Records the conformance fixture corpus against real `loams` servers.
//
// This is SDK1 Task 4's recorder. It speaks raw HTTP to whatever endpoint it is
// pointed at, saves the exact status, headers and body bytes of each case under
// `sdks/fixtures/recorded/<server>/`, and writes the projections a per-SDK
// suite reads: `status.json` (what each transport answered), `error.json` (one
// row per recorded `reason`), `state.json` (the ordered multi-step scenarios)
// and `manifest.json` (the single authority on which fixtures are required).
//
// Why a recording at all (design §44 §10.4): the per-SDK suite has to run in CI
// on every PR, and CI cannot afford to boot a Rust server for each of the
// thirteen SDKs on every change. So the corpus is captured once from a real
// server, committed, and replayed by `fixture-server.mjs`. `run.sh` still
// offers the live path for the job that can afford it.
//
// ## The two servers, and why both are needed
//
// `loams dev` is the **primary** target (D617). It answers a successful unary
// call and both structured-reason refusals, in every encoding.
//
// It cannot answer a paged list, an idempotency-keyed mutation or a resumable
// stream, because `loams.collection.v1` has not landed and the app packages are
// not served by it at all (design §44 §8; plan API1 Tasks 2–4). Those fixtures
// come from `loams-apps-mock` (AP0 Task 5), which implements the app services
// for real — its `acceptance` module is the server's own decision rules — and
// records them under `recorded/apps-mock/`.
//
// **Nothing here is hand-written.** Where a behaviour has no server that
// produces it, the recorder says so in `error.json`'s `unproducible` list
// rather than inventing bytes for it. See `docs/sdk/fixtures.md`.
//
// ## Recorded, not derived
//
// Every unary RPC is recorded in every encoding an SDK's transport might pick,
// because the response body is **not interchangeable** between them. The
// browser's gRPC-Web path and the streaming envelope each get a case: the
// unavailable-service path in particular arrives differently on a stream.
//
// Streams are recorded as a **bounded prefix**: a watch stream never ends, so
// the recorder reads N frames or until a deadline, records what arrived, and
// closes. That is a recording of what the server said, not a reconstruction.
//
// Timestamps are the one field that cannot be identical between two runs,
// because the mock's seed is relative to `now`. Every fixture that carries one
// declares the field names in `volatile`, and `verify-corpus.mjs` masks exactly
// those when it compares a re-recording. Nothing else is allowed to drift.
//
// Usage:
//   node sdks/conformance/record-fixtures.mjs                       # both servers
//   node sdks/conformance/record-fixtures.mjs --source loams-dev
//   node sdks/conformance/record-fixtures.mjs --source apps-mock --only error
//   LOAMS_TEST_ENDPOINT=http://127.0.0.1:8080 node …/record-fixtures.mjs
//
// A body is stored base64 when it is not UTF-8 JSON, so a gRPC-Web frame or a
// Connect streaming envelope replays byte for byte.

import { mkdir, readdir, readFile, writeFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { startAppsMock, TOKENS } from './apps-mock.mjs';

const here = dirname(fileURLToPath(import.meta.url));
function arg(name, fallback) {
  const at = process.argv.indexOf(`--${name}`);
  return at >= 0 && at + 1 < process.argv.length ? process.argv[at + 1] : fallback;
}

/**
 * Where the corpus is written.
 *
 * Overridable so `verify-corpus.mjs --drift` can re-record into a temporary
 * directory and compare. Re-recording in place and then diffing is the obvious
 * thing and it is wrong twice over: it dirties the working tree, so a check
 * cannot tell "the corpus drifted" from "the check ran", and a failure leaves
 * the corpus half-rewritten for whatever runs next.
 */
const fixturesDir = arg('fixtures', join(here, '..', 'fixtures'));

/** The commit the corpus was recorded from, so a diff can be traced to a build. */
const COMMIT = arg('commit', process.env.LOAMS_FIXTURE_COMMIT ?? 'unknown');

// ===========================================================================
// Source 1: `loams dev` — the primary target.
// ===========================================================================

/** The RPCs the corpus covers, and what each one is. */
const RPCS = [
  {
    name: 'instance_get_instance',
    path: '/loams.instance.v1.InstanceService/GetInstance',
    about:
      'A successful unary call, no auth: what this instance is. This is the ' +
      'case a client makes before anything else, on a cold start.',
    reason: null,
    pinnedBy: ['R5', 'R9'],
  },
  {
    name: 'instance_who_am_i',
    path: '/loams.instance.v1.InstanceService/WhoAmI',
    about:
      'A structured-reason error: `unimplemented` carrying an ErrorInfo whose ' +
      'reason is `not_implemented`, because this build has no authentication ' +
      'yet. An SDK branches on the reason, never on the message.',
    reason: 'not_implemented',
    pinnedBy: ['R8'],
  },
  {
    name: 'live_query',
    path: '/loams.live.v1.LiveService/Query',
    about:
      'The unavailable-service path on a unary RPC: `loams.live.v1` is a ' +
      '`full`-variant engine, so every one of its RPCs answers `unimplemented` ' +
      'with reason `feature_not_in_variant` and the variant in `metadata`.',
    reason: 'feature_not_in_variant',
    pinnedBy: ['R5', 'R8'],
  },
];

/** The encodings a case is recorded in. */
const ENCODINGS = [
  {
    name: 'json',
    about: 'The proto3 JSON mapping, which is what `curl` sends (design §44 §4).',
    contentType: 'application/json',
    body: '{}',
  },
  {
    name: 'proto',
    about: 'The binary encoding, which is what an SDK sends by default.',
    contentType: 'application/proto',
    bodyBase64: '',
  },
  {
    name: 'grpc_web',
    about:
      'gRPC-Web, the framing a browser sends. One length-prefixed frame: flag 0 ' +
      'and a zero-length message.',
    contentType: 'application/grpc-web+proto',
    // Five bytes: one flag byte and a four-byte big-endian length, both zero.
    bodyBase64: 'AAAAAAA=',
  },
  {
    name: 'grpc_web_json',
    about: 'gRPC-Web carrying the proto3 JSON mapping, which a browser SDK can ask for.',
    contentType: 'application/grpc-web+json',
    // The same frame, with `{}` as its payload: flag 0, length 2.
    bodyBase64: 'AAAAAAJ7fQ==',
  },
];

/**
 * The refusal on a server stream. It is its own case because the answer is not
 * an HTTP status: the refusal arrives inside the Connect streaming envelope, so
 * a client that only reads status codes sees a 200 and no error.
 */
const STREAM_CASE = {
  name: 'live_watch',
  path: '/loams.live.v1.LiveService/Watch',
  about:
    'The unavailable-service path on a server stream: the refusal arrives ' +
    'inside the Connect streaming envelope, not as an HTTP status. An SDK that ' +
    'reports it has to read the envelope.',
  reason: 'feature_not_in_variant',
  contentType: 'application/connect+proto',
  // flags 0, length 0, then a zero-length request message.
  bodyBase64: 'AAAAAAA=',
};

/** Headers worth recording; `date` and friends change on every call. */
const KEEP = ['content-type', 'grpc-status', 'grpc-message', 'loams-consistency-token'];

/**
 * Whether the `loams dev` case is required of every SDK.
 *
 * These thirteen files predate `manifest.json` and are byte-for-byte what
 * SDK2 Task 0 recorded, so their required-ness is declared here rather than
 * written into them. Every one of them is required: a language that cannot run
 * `GetInstance` cannot call anything at all, and the rest are the three
 * behaviours R5 and R8 name.
 */
const LOAMS_DEV_REQUIRED = true;

/**
 * The `loams dev` fields that cannot be identical between two runs.
 *
 * `GetInstance` answers with an `instance_id` that `loams dev` generates from
 * its data directory, so every fresh `loams dev` reports a different one. The
 * committed bytes are still the recording — they are what the SDKs assert
 * against — but a re-recording is a *new* recording and this field legitimately
 * moves, so the drift check masks it and nothing else.
 *
 * The two JSON and gRPC-Web-JSON cases mask the one field; the binary and
 * gRPC-Web-binary cases mask the whole body, because a ULID is baked into the
 * serialized bytes and there is no field to point at. That is a coarser mask,
 * so it is stated here rather than buried: the JSON encodings pin those bodies
 * field by field and are the ones that catch a message change.
 *
 * Declared here and not in the fixture files because those thirteen are
 * Task 0's bytes and nothing should rewrite them; `manifest.json` is where a
 * reader looks.
 */
const VOLATILE_BY_FIXTURE = {
  instance_get_instance_json: ['body.instanceId'],
  instance_get_instance_proto: ['response.bodyBase64'],
  instance_get_instance_grpc_web: ['response.bodyBase64'],
  instance_get_instance_grpc_web_json: ['response.bodyBase64'],
};

/** Builds the `loams dev` case list: each RPC in each encoding, plus the stream. */
function loamsDevCases() {
  const out = [];
  for (const rpc of RPCS) {
    for (const encoding of ENCODINGS) {
      out.push({
        name: `${rpc.name}_${encoding.name}`,
        about: `${rpc.about} (${encoding.about})`,
        kind: rpc.reason ? 'error' : 'unary',
        transport: encoding.name.startsWith('grpc_web') ? 'grpc-web' : 'connect',
        required: LOAMS_DEV_REQUIRED,
        pinnedBy: rpc.pinnedBy,
        request: {
          method: 'POST',
          path: rpc.path,
          headers: { 'content-type': encoding.contentType },
          ...(encoding.bodyBase64 === undefined
            ? { body: encoding.body }
            : { bodyBase64: encoding.bodyBase64 }),
        },
        // gRPC-Web always answers 200 and puts the code in trailers, so the
        // HTTP status only says something on the Connect paths.
        expect: {
          status: encoding.name.startsWith('grpc_web') ? 200 : rpc.reason ? 501 : 200,
          reason: rpc.reason ?? undefined,
        },
        framed: encoding.name.startsWith('grpc_web'),
      });
    }
  }
  out.push({
    name: STREAM_CASE.name,
    about: STREAM_CASE.about,
    kind: 'error',
    transport: 'connect',
    required: LOAMS_DEV_REQUIRED,
    pinnedBy: ['R5', 'R7', 'R8'],
    request: {
      method: 'POST',
      path: STREAM_CASE.path,
      headers: { 'content-type': STREAM_CASE.contentType },
      bodyBase64: STREAM_CASE.bodyBase64,
    },
    expect: { status: 200, reason: STREAM_CASE.reason },
    framed: true,
  });
  return out;
}

// ===========================================================================
// Source 2: `loams-apps-mock` — the stateful app packages.
// ===========================================================================

/** The Connect JSON encoding, which the mock serves for every app RPC. */
const JSON_CT = 'application/json';
/** The Connect streaming encoding, for the watch RPCs. */
const STREAM_CT = 'application/connect+json';
/** The binary unary encoding, so the corpus proves the two agree. */
const PROTO_CT = 'application/proto';

const APPROVALS = '/loams.approvals.v1.ApprovalService';
const DEVICES = '/loams.devices.v1.DeviceService';
const NOTIFICATIONS = '/loams.notifications.v1.NotificationService';
const OPERATIONS = '/loams.operations.v1.OperationsService';

/** One frame of the Connect streaming request envelope: flags, length, body. */
function frame(json) {
  const payload = Buffer.from(json, 'utf8');
  const header = Buffer.alloc(5);
  header.writeUInt32BE(payload.length, 1);
  return Buffer.concat([header, payload]).toString('base64');
}

/**
 * The fields whose value is a function of *when* the mock was started.
 *
 * `loams-apps-mock`'s seed is `Seed::demo_at(now)`, so every timestamp it
 * answers with differs between two runs while everything else is byte-identical.
 * These are named so `verify-corpus.mjs` can mask exactly them; a difference
 * anywhere else fails the drift check.
 *
 * **Bare names** — masked wherever they appear, because the mock nests them to
 * whatever depth the message puts them and listing every path would mean editing
 * this list every time a message gains a field.
 *
 * A recorded **stream** additionally carries its frames base64-encoded, where a
 * field name cannot reach. Those fixtures list `response.bodyBase64` and
 * `frames.payload` too, which is a coarser mask: the frame *count* and flags are
 * still compared, and the same data is compared field by field in the JSON
 * encodings of the unary cases.
 */
const VOLATILE_TIME_FIELDS = [
  'createdAt',
  'updatedAt',
  'expiresAt',
  'decidedAt',
  'startedAt',
  'finishedAt',
  'lastSeenAt',
  'at',
  'created_at',
  'updated_at',
  'expires_at',
  'decided_at',
  'started_at',
  'finished_at',
  'last_seen_at',
];

/** The extra entries a recorded stream needs, on top of the time fields. */
const VOLATILE_STREAM_FIELDS = ['response.bodyBase64', 'frames.payload'];

/**
 * The extra entry a scenario whose second step is the **binary** encoding needs.
 *
 * `GetInstance.min_app_versions` is a Rust map, and a protobuf map has no
 * defined serialization order, so the binary encoding of the same message is
 * different bytes on every run. The proto3 JSON step next to it compares the
 * same fields exactly and by name, so the coverage that is lost here is
 * recovered there — which is stated rather than assumed.
 */
const VOLATILE_BINARY_MAP_FIELDS = ['response.bodyBase64'];

/**
 * The scenarios recorded from `loams-apps-mock`.
 *
 * Each scenario runs against a **freshly started** mock, because the mock is
 * stateful and the point of most of these is state moving. One mock per
 * scenario also means a fixture replays in isolation, with no ordering
 * dependency between fixtures — which is what lets a language's suite replay
 * exactly the one scenario it is testing.
 *
 * `required: false` marks a fixture that is **not** part of the 100% bar: it is
 * recorded because it is informative, and a language may legitimately not reach
 * it yet. The rule in `required.mjs` only ever blocks on `required: true`.
 */
const APPS_MOCK_SCENARIOS = [
  // ---- `error`: one real refusal per reason a server can actually raise ----
  {
    name: 'mock_error_not_implemented',
    kind: 'error',
    transport: 'connect',
    required: true,
    pinnedBy: ['R8'],
    reason: 'not_implemented',
    about:
      'A stub handler that has not landed: `unimplemented` with reason ' +
      '`not_implemented`. This is the reason R8 says is distinct from ' +
      '`feature_not_in_variant` — one means "never built", the other "not in ' +
      'this build variant", and an SDK that conflates them cannot tell whether ' +
      'to offer a feature or report the binary as wrong.',
    steps: [
      {
        about: 'SendTestNotification is a declared RPC whose handler is a stub.',
        request: {
          method: 'POST',
          path: `${DEVICES}/SendTestNotification`,
          headers: { 'content-type': JSON_CT, authorization: TOKENS.FRESH },
          body: '{}',
        },
        expect: { status: 501, reason: 'not_implemented' },
      },
    ],
  },
  {
    name: 'mock_error_reason_required',
    kind: 'error',
    transport: 'connect',
    required: true,
    pinnedBy: ['R8'],
    reason: 'reason_required',
    about:
      'A `invalid_argument` refusal that is a **policy** rule, not a parse ' +
      'failure: rejecting an approval, or approving a destructive one, needs a ' +
      'reason. The message is for a person; the branch is `reason_required`.',
    steps: [
      {
        about: 'Reject without a reason.',
        request: {
          method: 'POST',
          path: `${APPROVALS}/DecideApproval`,
          headers: { 'content-type': JSON_CT, authorization: TOKENS.FRESH },
          body: JSON.stringify({
            approvalId: 'apr_01J9ZCREATEKEY',
            revision: '1',
            decision: 'DECISION_KIND_REJECT',
          }),
        },
        expect: { status: 400, reason: 'reason_required' },
      },
    ],
  },
  {
    name: 'mock_error_approval_expired',
    kind: 'error',
    transport: 'connect',
    required: true,
    pinnedBy: ['R8'],
    reason: 'approval_expired',
    about:
      'A `failed_precondition` whose condition is **time**, not state: the ' +
      'approval was still pending and is no longer. The seed carries one ' +
      'expired approval precisely so this is reachable without waiting.',
    steps: [
      {
        request: {
          method: 'POST',
          path: `${APPROVALS}/DecideApproval`,
          headers: { 'content-type': JSON_CT, authorization: TOKENS.FRESH },
          body: JSON.stringify({
            approvalId: 'apr_01J9ZEXPIRED',
            revision: '2',
            decision: 'DECISION_KIND_APPROVE',
          }),
        },
        expect: { status: 400, reason: 'approval_expired' },
      },
    ],
  },
  {
    name: 'mock_error_approval_already_decided',
    kind: 'error',
    transport: 'connect',
    required: true,
    pinnedBy: ['R8'],
    reason: 'approval_already_decided',
    about:
      'The double-submit refusal. It is recorded as its own scenario rather ' +
      'than as a second step of `mock_state_idempotent_decide`, because it is ' +
      'the answer to a **different** request (no idempotency key) and a client ' +
      'must be able to tell "you already did this" from "here is the same ' +
      'result again".',
    steps: [
      {
        about: 'Decide without an idempotency key.',
        request: {
          method: 'POST',
          path: `${APPROVALS}/DecideApproval`,
          headers: { 'content-type': JSON_CT, authorization: TOKENS.FRESH },
          body: JSON.stringify({
            approvalId: 'apr_01J9ZCREATEKEY',
            revision: '1',
            decision: 'DECISION_KIND_APPROVE',
          }),
        },
        expect: { status: 200 },
      },
      {
        about: 'The same decision again, with no key to identify it.',
        request: {
          method: 'POST',
          path: `${APPROVALS}/DecideApproval`,
          headers: { 'content-type': JSON_CT, authorization: TOKENS.FRESH },
          body: JSON.stringify({
            approvalId: 'apr_01J9ZCREATEKEY',
            revision: '1',
            decision: 'DECISION_KIND_APPROVE',
          }),
        },
        expect: { status: 400, reason: 'approval_already_decided' },
      },
    ],
  },
  {
    name: 'mock_error_approval_stale_revision',
    kind: 'error',
    transport: 'connect',
    required: true,
    pinnedBy: ['R8'],
    reason: 'approval_stale_revision',
    about:
      'Optimistic concurrency as a *reason*: the caller decided revision 9, ' +
      'the server is on revision 1. An SDK surfaces this as "reload it" rather ' +
      'than as a generic precondition failure, because the caller can act on it.',
    steps: [
      {
        request: {
          method: 'POST',
          path: `${APPROVALS}/DecideApproval`,
          headers: { 'content-type': JSON_CT, authorization: TOKENS.FRESH },
          body: JSON.stringify({
            approvalId: 'apr_01J9ZCREATEKEY',
            revision: '9',
            decision: 'DECISION_KIND_APPROVE',
          }),
        },
        expect: { status: 400, reason: 'approval_stale_revision' },
      },
    ],
  },
  {
    name: 'mock_error_requester_cannot_approve',
    kind: 'error',
    transport: 'connect',
    required: true,
    pinnedBy: ['R8'],
    reason: 'requester_cannot_approve',
    about:
      'A `permission_denied` that names the *shape* of the refusal: an agent ' +
      'requested the operation, so the human who asked the agent cannot be the ' +
      'one to approve it. Two actors, one credential, and the answer is about ' +
      'the actor chain rather than the role.',
    steps: [
      {
        about: 'Dana decides what the agent requested on her behalf.',
        request: {
          method: 'POST',
          path: `${APPROVALS}/DecideApproval`,
          headers: { 'content-type': JSON_CT, authorization: TOKENS.REQUESTER },
          body: JSON.stringify({
            approvalId: 'apr_01J9ZCREATEKEY',
            revision: '1',
            decision: 'DECISION_KIND_REJECT',
            reason: 'no longer needed',
          }),
        },
        expect: { status: 403, reason: 'requester_cannot_approve' },
      },
    ],
  },
  {
    name: 'mock_error_step_up_required',
    kind: 'error',
    transport: 'connect',
    required: true,
    pinnedBy: ['R1', 'R8'],
    reason: 'step_up_required',
    about:
      'The `token_expired` sibling R1 needs and the reason the registry has for ' +
      'it: an `unauthenticated` on a credential that is *valid but too old* for ' +
      'this decision. An SDK must refresh once and retry (R1); `token_expired` ' +
      'itself no server can produce yet — see the `unproducible` list in ' +
      '`error.json`.',
    steps: [
      {
        about: 'A session from 10 minutes ago decides a STEP_UP_SESSION approval.',
        request: {
          method: 'POST',
          path: `${APPROVALS}/DecideApproval`,
          headers: { 'content-type': JSON_CT, authorization: TOKENS.STALE },
          body: JSON.stringify({
            approvalId: 'apr_01J9ZDROPDOCS',
            revision: '1',
            decision: 'DECISION_KIND_APPROVE',
            reason: 'approved after review',
          }),
        },
        expect: { status: 401, reason: 'step_up_required' },
      },
    ],
  },
  {
    name: 'mock_error_encodings',
    kind: 'error',
    transport: 'connect',
    required: true,
    pinnedBy: ['R8', 'R10'],
    reason: 'not_implemented',
    about:
      'The **same** refusal in every encoding, which is what proves the SDK ' +
      'has to read the body and not the status. Connect answers 501 with a JSON ' +
      'error; the gRPC-Web encodings answer 200 and put `grpc-status: 12` in ' +
      'the trailers. A client that reads only status codes passes the first and ' +
      'silently succeeds on the others.',
    steps: [
      {
        about: 'Connect, proto3 JSON.',
        request: {
          method: 'POST',
          path: `${DEVICES}/SendTestNotification`,
          headers: { 'content-type': JSON_CT, authorization: TOKENS.FRESH },
          body: '{}',
        },
        expect: { status: 501, reason: 'not_implemented' },
      },
      {
        about: 'Connect, binary protobuf.',
        request: {
          method: 'POST',
          path: `${DEVICES}/SendTestNotification`,
          headers: { 'content-type': PROTO_CT, authorization: TOKENS.FRESH },
          bodyBase64: '',
        },
        expect: { status: 501, reason: 'not_implemented' },
      },
      {
        about: 'gRPC-Web, binary protobuf: HTTP 200, the code is in the trailers.',
        request: {
          method: 'POST',
          path: `${DEVICES}/SendTestNotification`,
          headers: {
            'content-type': 'application/grpc-web+proto',
            authorization: TOKENS.FRESH,
          },
          bodyBase64: 'AAAAAAA=',
        },
        expect: { status: 200, grpcStatus: 12, reason: 'not_implemented' },
      },
      {
        about: 'gRPC-Web, proto3 JSON.',
        request: {
          method: 'POST',
          path: `${DEVICES}/SendTestNotification`,
          headers: {
            'content-type': 'application/grpc-web+json',
            authorization: TOKENS.FRESH,
          },
          bodyBase64: 'AAAAAAJ7fQ==',
        },
        expect: { status: 200, grpcStatus: 12, reason: 'not_implemented' },
      },
    ],
  },

  // ---- `state`: the ordered scenarios the stateful clauses need ----
  {
    name: 'mock_state_idempotent_decide',
    kind: 'state',
    transport: 'connect',
    required: true,
    pinnedBy: ['R3'],
    about:
      'R3, recorded rather than argued: **the same** `DecideApproval` sent ' +
      'twice with the same `idempotency_key`, against a mock whose approval ' +
      'state really moved between the two calls. Both answers are identical ' +
      'bytes and the approval is at revision 2, so the second call did not ' +
      'write. A client that regenerates the key per attempt (or drops it on the ' +
      'retry) turns this into a second write — which is the failure the field ' +
      'exists to prevent.',
    steps: [
      {
        about: 'First attempt. Writes.',
        request: {
          method: 'POST',
          path: `${APPROVALS}/DecideApproval`,
          headers: { 'content-type': JSON_CT, authorization: TOKENS.FRESH },
          body: JSON.stringify({
            approvalId: 'apr_01J9ZCREATEKEY',
            revision: '1',
            decision: 'DECISION_KIND_APPROVE',
            idempotencyKey: 'conformance-idempotency-1',
          }),
        },
        expect: { status: 200, state: 'APPROVAL_STATE_APPROVED', revision: '2' },
      },
      {
        about: 'The retry, byte-identical including the key. Does not write again.',
        request: {
          method: 'POST',
          path: `${APPROVALS}/DecideApproval`,
          headers: { 'content-type': JSON_CT, authorization: TOKENS.FRESH },
          body: JSON.stringify({
            approvalId: 'apr_01J9ZCREATEKEY',
            revision: '1',
            decision: 'DECISION_KIND_APPROVE',
            idempotencyKey: 'conformance-idempotency-1',
          }),
        },
        expect: {
          status: 200,
          state: 'APPROVAL_STATE_APPROVED',
          revision: '2',
          identicalToStep: 0,
        },
      },
    ],
  },
  {
    name: 'mock_state_stream_resume',
    kind: 'state',
    transport: 'connect',
    required: true,
    pinnedBy: ['R7'],
    about:
      'R7 end to end, which `loams dev` cannot do because `LiveService/Watch` ' +
      'is `feature_not_in_variant` in every variant. Three steps: open the ' +
      'stream and take the snapshot and its cursor; **change something while ' +
      'the client is disconnected**; resume from that cursor. The resumed ' +
      'stream sends the change, not the snapshot, which is exactly "no loss and ' +
      'no re-yield": a client that re-opened from scratch would re-send the ' +
      'snapshot, and one that resumed from nothing would miss the decision.',
    steps: [
      {
        about: 'Open, watching pending and decided. One frame: the snapshot, cursor c0.',
        stream: { untilFrames: 1, timeoutMs: 10_000 },
        request: {
          method: 'POST',
          path: `${APPROVALS}/WatchApprovals`,
          headers: { 'content-type': STREAM_CT, authorization: TOKENS.FRESH },
          bodyBase64: frame(
            JSON.stringify({ states: ['APPROVAL_STATE_PENDING', 'APPROVAL_STATE_APPROVED'] }),
          ),
        },
        expect: { status: 200, frames: 1, frameKinds: ['snapshot'], cursor: 'c0' },
      },
      {
        about: 'The client is disconnected. The approval is decided.',
        request: {
          method: 'POST',
          path: `${APPROVALS}/DecideApproval`,
          headers: { 'content-type': JSON_CT, authorization: TOKENS.FRESH },
          body: JSON.stringify({
            approvalId: 'apr_01J9ZCREATEKEY',
            revision: '1',
            decision: 'DECISION_KIND_APPROVE',
            idempotencyKey: 'conformance-stream-resume-1',
          }),
        },
        expect: { status: 200, state: 'APPROVAL_STATE_APPROVED' },
      },
      {
        about: 'Resume from c0. The change as an upsert, not a snapshot.',
        stream: { untilFrames: 1, timeoutMs: 10_000 },
        request: {
          method: 'POST',
          path: `${APPROVALS}/WatchApprovals`,
          headers: { 'content-type': STREAM_CT, authorization: TOKENS.FRESH },
          bodyBase64: frame(
            JSON.stringify({
              states: ['APPROVAL_STATE_PENDING', 'APPROVAL_STATE_APPROVED'],
              resumeCursor: 'c0',
            }),
          ),
        },
        expect: {
          status: 200,
          frames: 1,
          frameKinds: ['upsert'],
          snapshotReset: false,
        },
      },
    ],
  },
  {
    name: 'mock_state_stream_resume_remove',
    kind: 'state',
    transport: 'connect',
    required: true,
    pinnedBy: ['R7'],
    about:
      'The same resume, watched with the **default** filter (pending only), and ' +
      'it is a different frame. Deciding the approval takes it *out of* the ' +
      'filter, so the resumed stream sends `remove`, not `upsert`. An SDK that ' +
      'handles only `upsert` leaves the decided approval on screen forever, ' +
      'which is the "quietly stale sync UI" R7 calls worse than an error.',
    steps: [
      {
        about: 'Open with no filter: pending only.',
        stream: { untilFrames: 1, timeoutMs: 10_000 },
        request: {
          method: 'POST',
          path: `${APPROVALS}/WatchApprovals`,
          headers: { 'content-type': STREAM_CT, authorization: TOKENS.FRESH },
          bodyBase64: frame('{}'),
        },
        expect: { status: 200, frames: 1, frameKinds: ['snapshot'], cursor: 'c0' },
      },
      {
        about: 'The approval is decided while the client is disconnected.',
        request: {
          method: 'POST',
          path: `${APPROVALS}/DecideApproval`,
          headers: { 'content-type': JSON_CT, authorization: TOKENS.FRESH },
          body: JSON.stringify({
            approvalId: 'apr_01J9ZCREATEKEY',
            revision: '1',
            decision: 'DECISION_KIND_APPROVE',
            idempotencyKey: 'conformance-stream-remove-1',
          }),
        },
        expect: { status: 200, state: 'APPROVAL_STATE_APPROVED' },
      },
      {
        about: 'Resume from c0. It left the filter, so: remove.',
        stream: { untilFrames: 1, timeoutMs: 10_000 },
        request: {
          method: 'POST',
          path: `${APPROVALS}/WatchApprovals`,
          headers: { 'content-type': STREAM_CT, authorization: TOKENS.FRESH },
          bodyBase64: frame('{"resumeCursor":"c0"}'),
        },
        expect: {
          status: 200,
          frames: 1,
          frameKinds: ['remove'],
          snapshotReset: false,
        },
      },
    ],
  },
  {
    name: 'mock_state_stream_snapshot_reset',
    kind: 'state',
    transport: 'connect',
    required: true,
    pinnedBy: ['R7'],
    about:
      'The other half of R7: a cursor the server no longer has. The registry ' +
      'says explicitly that `cursor_expired` is **not** a reason — a watch ' +
      'whose cursor is too old resets with a fresh snapshot and ' +
      '`snapshot_reset = true` rather than failing. An SDK that treats this as ' +
      'an error leaves a user watching an empty stream forever.',
    steps: [
      {
        about: 'A cursor from beyond the end of the log.',
        stream: { untilFrames: 1, timeoutMs: 10_000 },
        request: {
          method: 'POST',
          path: `${APPROVALS}/WatchApprovals`,
          headers: { 'content-type': STREAM_CT, authorization: TOKENS.FRESH },
          bodyBase64: frame('{"resumeCursor":"c9999"}'),
        },
        expect: {
          status: 200,
          frames: 1,
          frameKinds: ['snapshot'],
          snapshotReset: true,
        },
      },
    ],
  },
  {
    name: 'mock_state_stream_heartbeat',
    kind: 'state',
    transport: 'connect',
    required: true,
    pinnedBy: ['R7'],
    about:
      'The heartbeat, recorded: an empty frame the server sends on a timer so ' +
      'proxies do not idle the connection out (§37 §8.3). A client must treat it ' +
      'as liveness and **not** as data — a stream that yields heartbeats to a UI ' +
      'is a stream that renders empty rows every fifteen seconds.',
    steps: [
      {
        about: 'Snapshot, then a heartbeat with nothing in it.',
        stream: { untilFrames: 2, timeoutMs: 10_000 },
        request: {
          method: 'POST',
          path: `${APPROVALS}/WatchApprovals`,
          headers: { 'content-type': STREAM_CT, authorization: TOKENS.FRESH },
          bodyBase64: frame('{}'),
        },
        expect: { status: 200, frames: 2, frameKinds: ['snapshot', 'heartbeat'] },
      },
    ],
  },

  // ---- `status`: the success path, and the reads an SDK makes on connect ----
  {
    name: 'mock_status_get_instance',
    kind: 'status',
    transport: 'connect',
    required: true,
    pinnedBy: ['R9'],
    about:
      '`GetInstance` with **no credential at all**, which is the R9 contract: ' +
      'the version check must work before a caller has a token, and the SDK ' +
      'reports `api_versions` beside its own `PROTO_REV`. Note what is absent: ' +
      'there is no `services[]` here, so the R5 catalogue half is still ' +
      'unexercised outside `loams dev` (D600 has not landed in the mock).',
    steps: [
      {
        request: {
          method: 'POST',
          path: '/loams.instance.v1.InstanceService/GetInstance',
          headers: { 'content-type': JSON_CT },
          body: '{}',
        },
        expect: { status: 200, apiVersions: ['loams.instance.v1'] },
      },
      {
        about: 'The same call in binary protobuf: the fields must agree.',
        request: {
          method: 'POST',
          path: '/loams.instance.v1.InstanceService/GetInstance',
          headers: { 'content-type': PROTO_CT },
          bodyBase64: '',
        },
        expect: { status: 200 },
      },
    ],
  },
  {
    name: 'mock_status_unauthenticated',
    kind: 'status',
    transport: 'connect',
    required: true,
    pinnedBy: ['R1', 'R8'],
    about:
      'R1 and R8\'s third case: a failure from **below the API** — no ' +
      'credential at all. The mock answers `unauthenticated` with **no** ' +
      '`ErrorInfo` detail, so there is no `reason` to branch on. An SDK must ' +
      'not invent one: this is the shape R8 says is distinct from a mapped ' +
      'reason, and conflating it with `token_expired` would make a refresh ' +
      'loop on a request that has no credential to refresh.',
    steps: [
      {
        request: {
          method: 'POST',
          path: `${APPROVALS}/ListApprovals`,
          headers: { 'content-type': JSON_CT },
          body: '{}',
        },
        expect: { status: 401, reason: null },
      },
    ],
  },
  {
    name: 'mock_status_not_found_without_reason',
    kind: 'status',
    transport: 'connect',
    required: false,
    pinnedBy: ['R8'],
    about:
      'Recorded because it is a **gap in the server**, and a fixture is the ' +
      'cheapest way to make that visible: `GetApproval` on an unknown id ' +
      'answers `404` with `{"code":"not_found"}` and **no `ErrorInfo` detail**, ' +
      'so there is no `reason` even though `not_found` is in the registry. An ' +
      'SDK mapping `not_found` therefore has to fall back to the Connect code, ' +
      'which is exactly the fallback R8 allows — but the reason is not there. ' +
      'Kept at `required: false`: the missing detail is the server\'s to fix, ' +
      'and a language must not be blocked on it.',
    steps: [
      {
        request: {
          method: 'POST',
          path: `${APPROVALS}/GetApproval`,
          headers: { 'content-type': JSON_CT, authorization: TOKENS.FRESH },
          body: JSON.stringify({ approvalId: 'apr_does_not_exist' }),
        },
        expect: { status: 404, reason: null },
      },
    ],
  },
  {
    name: 'mock_status_list_is_not_paged',
    kind: 'status',
    transport: 'connect',
    required: false,
    pinnedBy: ['R6'],
    about:
      'Recorded to pin a **fact about the corpus**: `ListApprovals` declares ' +
      '`page_size` and answers `next_page_token` in its response message, but ' +
      'the mock honours neither — it returns every matching approval and no ' +
      'token, whatever `page_size` says. So there is still **no paged RPC ' +
      'anywhere** and R6 remains pinned against a stub. This fixture is what ' +
      'turns that claim from an assertion into a recording: when API1 Task 2 ' +
      'lands a real `ListCollections`, this case starts returning a token and ' +
      'the fixture changes.',
    steps: [
      {
        about: 'page_size 1 against a two-approval seed: both come back.',
        request: {
          method: 'POST',
          path: `${APPROVALS}/ListApprovals`,
          headers: { 'content-type': JSON_CT, authorization: TOKENS.FRESH },
          body: JSON.stringify({ pageSize: 1 }),
        },
        expect: { status: 200, itemCount: 2, nextPageToken: null },
      },
    ],
  },
  {
    name: 'mock_status_notifications',
    kind: 'status',
    transport: 'connect',
    required: false,
    pinnedBy: ['R6'],
    about:
      'The one inbox item the seed carries, recorded so a suite can assert on ' +
      'a real `CloudEvent` `type` (`io.loams.dev.…`) without inventing one.',
    steps: [
      {
        request: {
          method: 'POST',
          path: `${NOTIFICATIONS}/ListNotifications`,
          headers: { 'content-type': JSON_CT, authorization: TOKENS.FRESH },
          body: '{}',
        },
        expect: { status: 200, itemCount: 1 },
      },
    ],
  },
  {
    name: 'mock_status_list_devices',
    kind: 'status',
    transport: 'connect',
    required: false,
    pinnedBy: ['R5'],
    about:
      'The paired phone in the seed. `ListDevices` is marked `NO_SIDE_EFFECTS` ' +
      'in the proto, so it is also the one app RPC an SDK should issue as a ' +
      'cacheable GET (design §44 §4).',
    steps: [
      {
        request: {
          method: 'POST',
          path: `${DEVICES}/ListDevices`,
          headers: { 'content-type': JSON_CT, authorization: TOKENS.FRESH },
          body: '{}',
        },
        expect: { status: 200 },
      },
    ],
  },
  {
    name: 'mock_status_list_operations',
    kind: 'status',
    transport: 'connect',
    required: false,
    pinnedBy: ['R6'],
    about: 'The running import in the seed, which is also the `WatchOperations` subject.',
    steps: [
      {
        request: {
          method: 'POST',
          path: `${OPERATIONS}/ListOperations`,
          headers: { 'content-type': JSON_CT, authorization: TOKENS.FRESH },
          body: '{}',
        },
        expect: { status: 200 },
      },
    ],
  },
];

// ===========================================================================
// Recording.
// ===========================================================================

/** Reads the headers worth keeping; `date` and friends change on every call. */
function keptHeaders(headers, extra) {
  const out = {};
  for (const name of [...KEEP, ...extra]) {
    const value = headers.get(name);
    if (value !== null) {
      out[name] = value;
    }
  }
  return out;
}

/**
 * Whether the reason is anywhere in the response: a Connect JSON error carries
 * it base64-encoded inside the `ErrorInfo` detail, so the detail bytes are
 * decoded before the check. On a stream the refusal is in the body itself.
 */
function carriesReason(bytes, reason) {
  const text = bytes.toString('utf8');
  if (text.includes(reason)) {
    return true;
  }
  // The Connect JSON error body puts the ErrorInfo in a `value`, and gRPC-Web
  // puts a google.rpc.Status in a `grpc-status-details-bin` trailer. Both are
  // base64, so any long base64 run in the response is decoded and searched.
  for (const match of text.matchAll(/[A-Za-z0-9+/]{16,}={0,2}/g)) {
    if (Buffer.from(match[0], 'base64').toString('utf8').includes(reason)) {
      return true;
    }
  }
  return false;
}

/** The `reason` in a Connect JSON error body, or `null` if there is no detail. */
function reasonOf(bytes) {
  const text = bytes.toString('utf8');
  if (!text.startsWith('{')) {
    return null;
  }
  let body;
  try {
    body = JSON.parse(text);
  } catch {
    return null;
  }
  for (const detail of body.details ?? []) {
    if (!String(detail.type ?? '').endsWith('ErrorInfo')) {
      continue;
    }
    // The detail is base64 **protobuf**, so the reason is a length-delimited
    // field 1: byte 0 is the tag 0x0a, byte 1 its length, then the bytes.
    const raw = Buffer.from(detail.value ?? '', 'base64');
    if (raw[0] === 0x0a) {
      return raw.subarray(2, 2 + raw[1]).toString('utf8');
    }
  }
  return null;
}

/** The raw body bytes of a recorded step, whichever way it was stored. */
function bodyBytes(step) {
  return step.response.bodyBase64
    ? Buffer.from(step.response.bodyBase64, 'base64')
    : Buffer.from(JSON.stringify(step.response.body), 'utf8');
}

/**
 * The `grpc-status` of a recorded step, wherever it actually is.
 *
 * On the Connect unary paths it is a **header**. On gRPC-Web it is not: a
 * trailers-only answer is one frame with the high flag bit set and the status
 * inside its payload, and the HTTP status is a cheerful 200. An SDK that looks
 * in the wrong place sees a success, so the recorder looks in both and a case
 * may state the number it expects.
 */
function grpcStatusOf(step) {
  const header = step.response.headers['grpc-status'];
  if (header !== undefined) {
    return Number(header);
  }
  const frames = step.response.frames ?? [];
  for (const frame of frames) {
    const flags = step.response.frames === undefined ? (frame.flags ?? 0) : frame.flags;
    if ((flags & 0x80) === 0) {
      continue;
    }
    const text = Buffer.from(frame.payload, 'base64').toString('utf8');
    const match = text.match(/grpc-status:\s*(\d+)/);
    if (match) {
      return Number(match[1]);
    }
  }
  // A non-streamed gRPC-Web answer puts the trailers frame in the body.
  const raw = step.response.bodyBase64;
  if (raw) {
    const bytes = Buffer.from(raw, 'base64');
    if (bytes.length >= 5 && (bytes[0] & 0x80) !== 0) {
      const match = bytes.subarray(5).toString('utf8').match(/grpc-status:\s*(\d+)/);
      if (match) {
        return Number(match[1]);
      }
    }
  }
  return null;
}

/** The oneof field name of a recorded `WatchApprovalsResponse`, in proto3 JSON. */
function frameKind(payload) {
  const text = payload.toString('utf8');
  if (text.includes('"snapshot"')) {
    return 'snapshot';
  }
  if (text.includes('"upsert"')) {
    return 'upsert';
  }
  if (text.includes('"remove"')) {
    return 'remove';
  }
  if (text.includes('"heartbeat"')) {
    return 'heartbeat';
  }
  return 'unknown';
}

/**
 * Reads a bounded prefix of a response body, splitting the Connect envelope
 * into frames as they arrive.
 *
 * A watch stream never ends, so "record the response" has to mean "record the
 * first N frames and close". Stopping at `untilFrames` rather than at a timeout
 * is what makes the recording deterministic: the frames that arrive first do not
 * depend on how fast the machine is, only the heartbeat interval does, and that
 * is a parameter the harness fixes.
 */
async function readFramed(response, untilFrames, timeoutMs) {
  const reader = response.body.getReader();
  const chunks = [];
  const frames = [];
  // `consumed` is how far into `raw` the frames already split off reach. The
  // buffer is rebuilt on every read, so the offset has to be carried alongside
  // it — slicing `raw` itself would re-yield the first frame on the next chunk.
  let raw = Buffer.alloc(0);
  let consumed = 0;
  const deadline = Date.now() + timeoutMs;
  try {
    for (;;) {
      const remaining = deadline - Date.now();
      if (remaining <= 0) {
        break;
      }
      const next = await Promise.race([
        reader.read(),
        new Promise((resolve) => setTimeout(() => resolve({ timeout: true }), remaining)),
      ]);
      if (next.timeout === true || next.done) {
        break;
      }
      chunks.push(Buffer.from(next.value));
      raw = Buffer.concat(chunks);
      for (;;) {
        if (raw.length - consumed < 5) {
          break;
        }
        const flags = raw[consumed];
        const length = raw.readUInt32BE(consumed + 1);
        if (raw.length - consumed < 5 + length) {
          break;
        }
        const payload = raw.subarray(consumed + 5, consumed + 5 + length);
        frames.push({ flags, payload: payload.toString('base64') });
        consumed += 5 + length;
        if (frames.length >= untilFrames) {
          throw { done: true };
        }
      }
    }
  } catch (error) {
    if (error?.done !== true) {
      throw error;
    }
  } finally {
    await reader.cancel().catch(() => {});
  }
  return { frames, prefix: Buffer.concat(chunks) };
}

/** Sends one step and records the answer. */
async function recordStep(base, step, extraHeaders = []) {
  const body = step.request.bodyBase64
    ? Buffer.from(step.request.bodyBase64, 'base64')
    : Buffer.from(step.request.body ?? '', 'utf8');
  const response = await fetch(`${base}${step.request.path}`, {
    method: step.request.method,
    headers: { ...step.request.headers, ...Object.fromEntries(extraHeaders) },
    body,
    duplex: 'half',
  });
  const headers = keptHeaders(response.headers, ['connect-accept-encoding']);

  if (step.stream) {
    const { frames, prefix } = await readFramed(
      response,
      step.stream.untilFrames,
      step.stream.timeoutMs,
    );
    return {
      about: step.about,
      request: step.request,
      response: {
        status: response.status,
        headers,
        frames,
        bodyBase64: prefix.toString('base64'),
        truncated: true,
      },
      expect: step.expect,
    };
  }

  const bytes = Buffer.from(await response.arrayBuffer());
  const text = bytes.toString('utf8');
  const isJson = !step.framed && (text.startsWith('{') || text.startsWith('['));
  return {
    about: step.about,
    request: step.request,
    response: {
      status: response.status,
      headers,
      ...(isJson ? { body: JSON.parse(text) } : { bodyBase64: bytes.toString('base64') }),
    },
    expect: step.expect,
  };
}

/**
 * Checks a recorded step against what the scenario said it would be.
 *
 * The recorder is the last place a wrong expectation can be caught cheaply: once
 * a fixture is committed, thirteen SDKs assert against it, so an expectation
 * that was wrong when it was written becomes thirteen wrong tests. Every check
 * below therefore fails the record rather than warning.
 */
function checkStep(name, at, step, previous) {
  const where = `${name} step ${at}`;
  const expect = step.expect ?? {};
  if (expect.status !== undefined && step.response.status !== expect.status) {
    throw new Error(
      `${where}: HTTP ${step.response.status}, expected ${expect.status}: ` +
        `${JSON.stringify(step.response).slice(0, 400)}`,
    );
  }
  if (expect.grpcStatus !== undefined) {
    const got = grpcStatusOf(step);
    if (got !== expect.grpcStatus) {
      throw new Error(`${where}: grpc-status ${got}, expected ${expect.grpcStatus}`);
    }
  }
  if (expect.reason !== undefined) {
    const bytes = bodyBytes(step);
    if (expect.reason === null) {
      // The point of these cases: no `ErrorInfo` at all, so an SDK must fall
      // back to the Connect code rather than invent a reason.
      if (bytes.includes('"details"')) {
        throw new Error(`${where}: expected no ErrorInfo detail, got ${bytes.toString('utf8').slice(0, 200)}`);
      }
    } else if (!carriesReason(bytes, expect.reason)) {
      throw new Error(
        `${where}: the body does not carry ${expect.reason}: ${bytes.toString('utf8').slice(0, 200)}`,
      );
    }
  }
  if (expect.frames !== undefined && step.response.frames?.length !== expect.frames) {
    throw new Error(
      `${where}: ${step.response.frames?.length} frames, expected ${expect.frames}`,
    );
  }
  if (expect.frameKinds !== undefined) {
    const kinds = (step.response.frames ?? []).map((f) => frameKind(Buffer.from(f.payload, 'base64')));
    if (JSON.stringify(kinds) !== JSON.stringify(expect.frameKinds)) {
      throw new Error(`${where}: frames ${kinds.join(',')}, expected ${expect.frameKinds.join(',')}`);
    }
  }
  if (expect.cursor !== undefined) {
    const first = (step.response.frames ?? [])[0];
    const cursor = first ? JSON.parse(Buffer.from(first.payload, 'base64').toString('utf8')).cursor : null;
    if (cursor !== expect.cursor) {
      throw new Error(`${where}: cursor ${cursor}, expected ${expect.cursor}`);
    }
  }
  if (expect.snapshotReset !== undefined) {
    const first = (step.response.frames ?? [])[0];
    const reset = first
      ? JSON.parse(Buffer.from(first.payload, 'base64').toString('utf8')).snapshotReset === true
      : null;
    if (reset !== expect.snapshotReset) {
      throw new Error(`${where}: snapshot_reset ${reset}, expected ${expect.snapshotReset}`);
    }
  }
  if (expect.state !== undefined || expect.revision !== undefined) {
    const approval = step.response.body?.approval;
    if (!approval) {
      throw new Error(`${where}: no approval in the response`);
    }
    if (expect.state !== undefined && approval.state !== expect.state) {
      throw new Error(`${where}: state ${approval.state}, expected ${expect.state}`);
    }
    if (expect.revision !== undefined && String(approval.revision) !== expect.revision) {
      throw new Error(`${where}: revision ${approval.revision}, expected ${expect.revision}`);
    }
  }
  if (expect.itemCount !== undefined) {
    const lists = [
      step.response.body?.approvals,
      step.response.body?.notifications,
      step.response.body?.operations,
      step.response.body?.devices,
    ].filter(Array.isArray);
    if (lists.length !== 1 || lists[0].length !== expect.itemCount) {
      throw new Error(`${where}: ${lists[0]?.length} items, expected ${expect.itemCount}`);
    }
  }
  if (expect.nextPageToken !== undefined) {
    const got = step.response.body?.nextPageToken ?? null;
    if (got !== expect.nextPageToken) {
      throw new Error(`${where}: next_page_token ${JSON.stringify(got)}, expected ${expect.nextPageToken}`);
    }
  }
  if (expect.apiVersions !== undefined) {
    const got = step.response.body?.apiVersions ?? [];
    for (const want of expect.apiVersions) {
      if (!got.includes(want)) {
        throw new Error(`${where}: api_versions ${JSON.stringify(got)} lacks ${want}`);
      }
    }
  }
  if (expect.identicalToStep !== undefined) {
    const before = previous[expect.identicalToStep];
    const same = JSON.stringify(before.response.body) === JSON.stringify(step.response.body);
    if (!same) {
      throw new Error(
        `${where}: the replayed answer differs from step ${expect.identicalToStep}, so the ` +
          'second call was not recognised as the same logical write',
      );
    }
  }
}

/** Records one `loams-apps-mock` scenario against a mock of its own. */
async function recordScenario(scenario, options) {
  const mock = await startAppsMock({ cwd: options.cwd, heartbeatSecs: options.heartbeatSecs });
  const steps = [];
  try {
    for (const [at, step] of scenario.steps.entries()) {
      const recorded = await recordStep(mock.url, step);
      checkStep(scenario.name, at, recorded, steps);
      steps.push(recorded);
    }
  } finally {
    await mock.stop();
  }
  return {
    name: scenario.name,
    kind: scenario.kind,
    about: scenario.about,
    transport: scenario.transport,
    required: scenario.required,
    pinnedBy: scenario.pinnedBy ?? [],
    reason: scenario.reason ?? null,
    source: {
      server: 'loams-apps-mock',
      commit: COMMIT,
      recordedAt: new Date().toISOString(),
      heartbeatSecs: options.heartbeatSecs,
    },
    volatile: [
      ...VOLATILE_TIME_FIELDS,
      ...(scenario.steps.some((step) => step.stream) ? VOLATILE_STREAM_FIELDS : []),
      ...(scenario.name === 'mock_status_get_instance' ? VOLATILE_BINARY_MAP_FIELDS : []),
    ],
    steps,
  };
}

// ===========================================================================
// Projections.
// ===========================================================================

/** Every recorded fixture, flattened to the shape the projections read. */
function rowsOf(fixtures) {
  const rows = [];
  for (const fixture of fixtures) {
    const steps = fixture.steps ?? [fixture];
    for (const [at, step] of steps.entries()) {
      rows.push({
        fixture: fixture.name,
        step: steps.length > 1 ? at : 0,
        steps: steps.length,
        kind: fixture.kind ?? 'unary',
        about: step.about ?? fixture.about,
        server: fixture.source?.server ?? 'loams dev',
        required: fixture.required ?? true,
        transport: fixture.transport ?? 'connect',
        pinnedBy: fixture.pinnedBy ?? [],
        path: step.request.path,
        method: step.request.method,
        contentType: step.request.headers['content-type'],
        httpStatus: step.response.status,
        grpcStatus:
          step.response.headers['grpc-status'] === undefined
            ? null
            : Number(step.response.headers['grpc-status']),
        connectCode: step.response.body?.code ?? null,
        reason: fixture.reason ?? null,
      });
    }
  }
  return rows;
}

/**
 * `status.json` — what each transport actually answered.
 *
 * This is the file a language's transport test reads instead of hard-coding
 * "Connect is 501 and gRPC-Web is 200": the numbers here are projections of the
 * recordings next to them, and each row names the fixture it came from so a
 * reader can go and read the bytes.
 */
function statusProjection(fixtures) {
  const rows = rowsOf(fixtures);
  const byTransport = {};
  for (const row of rows) {
    byTransport[row.transport] ??= { httpStatuses: {}, grpcStatuses: {}, cases: 0 };
    byTransport[row.transport].httpStatuses[row.httpStatus] =
      (byTransport[row.transport].httpStatuses[row.httpStatus] ?? 0) + 1;
    if (row.grpcStatus !== null) {
      byTransport[row.transport].grpcStatuses[row.grpcStatus] =
        (byTransport[row.transport].grpcStatuses[row.grpcStatus] ?? 0) + 1;
    }
    byTransport[row.transport].cases += 1;
  }
  return {
    about:
      'What each transport answered, projected from the recordings in ' +
      '`recorded/`. A projection, not a source: `record-fixtures.mjs` rewrites ' +
      'this file from the bytes, so it cannot drift from the corpus. ' +
      'Design §44 §10.4.',
    note:
      'HTTP status is not the error. Connect answers a refusal with a status ' +
      'and a JSON body; gRPC-Web answers 200 and puts the code in `grpc-status`. ' +
      'A client that reads only status codes passes the first and silently ' +
      'succeeds on the second.',
    byTransport,
    rows,
  };
}

/**
 * `error.json` — one row per recorded `reason`, and an explicit list of the
 * registry reasons no server can raise yet.
 *
 * The `unproducible` half matters more than the recorded half. An SDK's ' +
 * '`<lang>_error_reason_mapping` test has to cover the whole registry, and today ' +
 * 'it can only do that against a stub for most of it; naming which ones and why ' +
 * 'is what stops that from quietly becoming "the ones we happened to test".
 */
function errorProjection(fixtures) {
  const rows = [];
  for (const fixture of fixtures) {
    if (fixture.kind !== 'error') {
      continue;
    }
    for (const [at, step] of (fixture.steps ?? [fixture]).entries()) {
      rows.push({
        reason: fixture.reason ?? null,
        fixture: fixture.name,
        step: at,
        server: fixture.source?.server ?? 'loams dev',
        rpc: step.request.path.split('/').pop(),
        path: step.request.path,
        contentType: step.request.headers['content-type'],
        httpStatus: step.response.status,
        connectCode: step.response.body?.code ?? null,
        carriesErrorInfo: step.response.body?.details !== undefined,
        required: fixture.required ?? true,
        pinnedBy: fixture.pinnedBy ?? [],
        about: step.about ?? fixture.about,
      });
    }
  }
  return {
    about:
      'One row per recorded structured-reason error, plus the registry reasons ' +
      'no server can raise yet. Generated by `record-fixtures.mjs` from ' +
      '`recorded/`; the registry it is checked against is `docs/api/reasons.md` ' +
      '(D611).',
    recorded: rows,
    unproducible: UNPRODUCIBLE_REASONS,
  };
}

/**
 * The registry reasons no server can produce today, and why.
 *
 * This list is the honest half of `error_reason_mapping`: a language's suite ' +
 * 'covers the whole registry against a stub, and the six reasons above are the ' +
 * 'only ones a recording can back. Deleting a row here without the server ' +
 * 'landing is how a stub quietly becomes the spec.
 */
const UNPRODUCIBLE_REASONS = [
  {
    reason: 'token_expired',
    code: 'unauthenticated',
    why:
      'R1 needs it and no server raises it: `loams dev` has no authentication ' +
      'at all (`WhoAmI` answers `not_implemented`), and the mock only knows ' +
      'fresh and stale sessions, where stale is `step_up_required`. It arrives ' +
      'with the token endpoint (MT, API1 Task 7).',
    standIn: 'mock_error_step_up_required',
  },
  {
    reason: 'device_revoked',
    code: 'unauthenticated',
    why:
      'The mock has `revoked_principals` in its seed but no revoked device is ' +
      'wired to an RPC, so nothing raises it. AP0 Task 5 stubs device ' +
      'verification; the server side is AP4.',
    standIn: null,
  },
  {
    reason: 'decision_proof_invalid',
    code: 'permission_denied',
    why:
      'Decision-proof verification is a stub in `loams-apps-mock`: a request ' +
      'carrying a proof is refused with `not_implemented` rather than judged, ' +
      'because a mock that accepted an unverified proof would be teaching the ' +
      'wrong thing.',
    standIn: null,
  },
  {
    reason: 'pairing_expired',
    code: 'failed_precondition',
    why: 'The pairing grant belongs to the auth plan (MT), not to AP0.',
    standIn: null,
  },
  {
    reason: 'pairing_used',
    code: 'failed_precondition',
    why: 'The pairing grant belongs to the auth plan (MT), not to AP0.',
    standIn: null,
  },
  {
    reason: 'push_target_unknown',
    code: 'not_found',
    why: 'Push targets are `unimplemented` in the mock (AP0 Ruling 9 stub list).',
    standIn: 'mock_error_not_implemented',
  },
  {
    reason: 'feature_not_in_variant',
    code: 'unimplemented',
    why:
      'Recorded, but only from `loams dev` — `loams-apps-mock` serves every ' +
      'package it has, so there is no variant to be outside of. That is the ' +
      'point of the case, and it is why the primary target keeps it.',
    standIn: 'live_query_json',
  },
  {
    reason: 'invalid_argument',
    code: 'invalid_argument',
    why:
      'Nothing raises the generic form: both servers answer a malformed field ' +
      'with a *specific* reason (`reason_required`). The generic rows below ' +
      '`invalid_argument` in the registry are the code-to-class mapping, so an ' +
      'SDK maps them from the code and needs no recording.',
    standIn: null,
  },
  {
    reason: 'invalid_decision',
    code: 'invalid_argument',
    why:
      'The mock does not validate the decision enum: an unrecognised decision ' +
      'value is refused as a malformed field, or not at all, but never as ' +
      '`invalid_decision`. The registry row is for a decision that parses and is ' +
      'still not one of the three legal values.',
    standIn: null,
  },
  {
    reason: 'unauthenticated',
    code: 'unauthenticated',
    why:
      'The mock answers a request with no credential `401 {"code":' +
      '"unauthenticated"}` with **no `ErrorInfo`**, so the reason is genuinely ' +
      'absent even though it is in the registry. Recorded as ' +
      '`mock_status_unauthenticated`. Distinct from `token_expired` on purpose: ' +
      'one has a credential to refresh and the other has nothing to refresh.',
    standIn: 'mock_status_unauthenticated',
  },
  {
    reason: 'failed_precondition',
    code: 'failed_precondition',
    why:
      'Only the specific preconditions are raised (`approval_expired`, ' +
      '`approval_already_decided`, `approval_stale_revision`), each with a reason ' +
      'of its own. The generic row is the code-to-class mapping.',
    standIn: 'mock_error_approval_stale_revision',
  },
  {
    reason: 'not_found',
    code: 'not_found',
    why:
      'The mock answers `404 {"code":"not_found"}` with **no** `ErrorInfo`, so ' +
      'the reason is genuinely absent. Recorded as ' +
      '`mock_status_not_found_without_reason` so the gap is visible rather ' +
      'than assumed.',
    standIn: 'mock_status_not_found_without_reason',
  },
  {
    reason: 'already_exists',
    code: 'already_exists',
    why:
      'No naming RPC is served: `CreatePairing` and `CreateNamespace` are stubs ' +
      'or absent, so nothing can collide yet.',
    standIn: null,
  },
  {
    reason: 'permission_denied',
    code: 'permission_denied',
    why: 'Only the specific `requester_cannot_approve` is raised today.',
    standIn: 'mock_error_requester_cannot_approve',
  },
  {
    reason: 'resource_exhausted',
    code: 'resource_exhausted',
    why:
      'Backpressure and quota refusals need a loaded server. This is also the ' +
      'code R2 retries on, so it is the one a fault injector must synthesise ' +
      '(`mock_injects_retryable_errors`): no server produces it on demand.',
    standIn: null,
  },
  {
    reason: 'unavailable',
    code: 'unavailable',
    why:
      'A dependency being down. Same as above: only a fault injector can produce ' +
      'it on demand, which is why SDK1 Task 4 asks for one.',
    standIn: null,
  },
  {
    reason: 'deadline_exceeded',
    code: 'deadline_exceeded',
    why: "The caller's own deadline. Only reproducible with a delay the mock does not have.",
    standIn: null,
  },
  {
    reason: 'aborted',
    code: 'aborted',
    why: 'A concurrent write winning. Needs two racing writers against one resource.',
    standIn: null,
  },
  {
    reason: 'internal',
    code: 'internal',
    why: 'A bug. A recording of one would pin a bug that ought to be fixed.',
    standIn: null,
  },
];

/**
 * `state.json` — the ordered multi-step scenarios, and what each one proves.
 *
 * This is the file the stateful clauses read. Unlike a single-request fixture a
 * scenario is only meaningful **in order**, so each row states the invariant a
 * language asserts across the whole sequence rather than per step.
 */
function stateProjection(fixtures) {
  const scenarios = fixtures
    .filter((fixture) => Array.isArray(fixture.steps))
    .map((fixture) => ({
      fixture: fixture.name,
      about: fixture.about,
      required: fixture.required ?? true,
      pinnedBy: fixture.pinnedBy ?? [],
      transport: fixture.transport ?? 'connect',
      server: fixture.source?.server ?? 'loams dev',
      steps: fixture.steps.map((step, at) => ({
        step: at,
        about: step.about ?? null,
        method: step.request.method,
        path: step.request.path,
        contentType: step.request.headers['content-type'],
        httpStatus: step.response.status,
        expect: step.expect,
      })),
      invariant: INVARIANTS[fixture.name] ?? null,
      volatile: fixture.volatile ?? [],
    }));
  return {
    about:
      'The ordered scenarios: what has to hold **across** the steps, which is ' +
      'where R3 and R7 live. Single-request cases are in `status.json`. ' +
      'Generated by `record-fixtures.mjs`; see `docs/sdk/fixtures.md`.',
    scenarios,
  };
}

/** The cross-step invariant each scenario exists to pin. */
const INVARIANTS = {
  mock_state_idempotent_decide:
    'One logical write. Step 0 and step 1 are the same request byte for byte; ' +
    'the two answers are equal and the approval is still at revision 2.',
  mock_state_stream_resume:
    'No loss and no re-yield. Step 0 takes the snapshot and its cursor, the ' +
    'approval is decided while the client is disconnected, and step 2 resumes ' +
    'from that cursor: it sends the `upsert`, not the snapshot, and does not ' +
    'set `snapshot_reset`.',
  mock_state_stream_resume_remove:
    'The same resume under the default pending-only filter sends `remove`, ' +
    'because deciding took the approval out of the filter. Handling `upsert` ' +
    'alone leaves a decided approval on screen forever.',
  mock_state_stream_snapshot_reset:
    'An unusable cursor is not an error. The registry says `cursor_expired` is ' +
    'not a reason, so the stream resets with a fresh snapshot and ' +
    '`snapshot_reset = true`.',
  mock_state_stream_heartbeat:
    'A heartbeat is liveness, not data. The second frame is empty and must not ' +
    'reach a caller as a value.',
  mock_error_approval_already_decided:
    'The double-submit refusal, which is distinct from the idempotent replay: ' +
    'no key means the server cannot recognise the second call as the same one.',
};

// ===========================================================================
// Entry point.
// ===========================================================================

/** Writes one recorded fixture and returns its index row. */
async function writeFixture(directory, fixture) {
  await mkdir(directory, { recursive: true });
  await writeFile(join(directory, `${fixture.name}.json`), `${JSON.stringify(fixture, null, 2)}\n`, 'utf8');
  console.log(
    `recorded ${fixture.name} (${fixture.kind}, ${fixture.steps.length} step` +
      `${fixture.steps.length === 1 ? '' : 's'}${fixture.required ? '' : ', not required'})`,
  );
  return fixture;
}

/**
 * The 13 legacy cases, written with the field order and the projection of
 * Task 0's recorder so re-recording leaves them byte-for-byte unchanged.
 */
async function recordLoamsDev(base, outDir, fixtures) {
  for (const testCase of loamsDevCases()) {
    const recorded = await recordStep(base, testCase);
    if (recorded.response.status !== testCase.expect.status) {
      throw new Error(
        `${testCase.name}: status ${recorded.response.status}, expected ${testCase.expect.status}: ` +
          `${JSON.stringify(recorded.response).slice(0, 400)}`,
      );
    }
    const bytes = recorded.response.bodyBase64
      ? Buffer.from(recorded.response.bodyBase64, 'base64')
      : Buffer.from(JSON.stringify(recorded.response.body), 'utf8');
    if (testCase.expect.reason && !carriesReason(bytes, testCase.expect.reason)) {
      throw new Error(`${testCase.name}: the body does not carry ${testCase.expect.reason}`);
    }
    const grpcStatus = recorded.response.headers['grpc-status'];
    if (!testCase.expect.reason && grpcStatus !== undefined && grpcStatus !== '0') {
      throw new Error(`${testCase.name}: grpc-status ${grpcStatus}`);
    }
    // Exactly Task 0's shape: name, about, request, response, expect. The
    // `index.json` reader in every SDK's suite and the fixture server both key
    // off those, and re-recording must not produce a diff in them.
    const flat = {
      name: testCase.name,
      about: testCase.about,
      request: testCase.request,
      response: recorded.response,
      expect: testCase.expect,
    };
    await writeFile(
      join(outDir, `${testCase.name}.json`),
      `${JSON.stringify(flat, null, 2)}\n`,
      'utf8',
    );
    console.log(`recorded ${flat.name} (${flat.response.status})`);
    fixtures.push({ ...flat, kind: testCase.kind, transport: testCase.transport, required: testCase.required });
  }
}

async function main() {
  const source = arg('source', 'all');
  const only = arg('only', null);
  const cwd = process.cwd();
  const fixtures = [];

  if (source === 'all' || source === 'loams-dev') {
    const base = process.env.LOAMS_TEST_ENDPOINT;
    if (!base) {
      throw new Error(
        'LOAMS_TEST_ENDPOINT is not set; `loams dev` must be running to record the primary ' +
          'half of the corpus (see sdks/conformance/run.sh)',
      );
    }
    const outDir = join(fixturesDir, 'recorded');
    await mkdir(outDir, { recursive: true });
    await recordLoamsDev(base.replace(/\/$/, ''), outDir, fixtures);
  }

  if (source === 'all' || source === 'apps-mock') {
    const outDir = join(fixturesDir, 'recorded', 'apps-mock');
    for (const scenario of APPS_MOCK_SCENARIOS) {
      if (only && scenario.kind !== only) {
        continue;
      }
      fixtures.push(await writeFixture(outDir, await recordScenario(scenario, { cwd, heartbeatSecs: 1 })));
    }
  }

  await mkdir(fixturesDir, { recursive: true });
  const index = fixtures
    .filter((fixture) => !fixture.source)
    .map((entry) => ({
      name: entry.name,
      about: entry.about,
      path: entry.request.path,
      contentType: entry.request.headers['content-type'],
      status: entry.response.status,
      reason: entry.expect.reason ?? null,
    }));
  if (index.length > 0) {
    await writeFile(
      join(fixturesDir, 'index.json'),
      `${JSON.stringify(
        {
          about:
            'The conformance corpus (design §44 §10.4, SDK1 Task 4). Recorded from ' +
            'a real `loams dev` by sdks/conformance/record-fixtures.mjs and ' +
            'replayed by sdks/conformance/fixture-server.mjs.',
          cases: index,
        },
        null,
        2,
      )}\n`,
      'utf8',
    );
    console.log(`wrote ${index.length} loams dev cases`);
  }

  const everything = await collectAll();
  await writeFile(
    join(fixturesDir, 'status.json'),
    `${JSON.stringify(statusProjection(everything), null, 2)}\n`,
    'utf8',
  );
  await writeFile(
    join(fixturesDir, 'error.json'),
    `${JSON.stringify(errorProjection(everything), null, 2)}\n`,
    'utf8',
  );
  await writeFile(
    join(fixturesDir, 'state.json'),
    `${JSON.stringify(stateProjection(everything), null, 2)}\n`,
    'utf8',
  );
  await writeFile(
    join(fixturesDir, 'manifest.json'),
    `${JSON.stringify(await manifestProjection(everything), null, 2)}\n`,
    'utf8',
  );
  console.log('wrote status.json, error.json, state.json, manifest.json');
}

/**
 * `manifest.json` — the single authority on what a language must run.
 *
 * `required.mjs` reads this and nothing else, so the skip rule has exactly one
 * definition. Every row names the clause it pins, so "which test covers R7" has
 * an answer that is not somebody's memory.
 */
async function manifestProjection(fixtures) {
  const rows = [];
  for (const fixture of fixtures) {
    const steps = fixture.steps ?? [fixture];
    rows.push({
      name: fixture.name,
      kind: fixture.kind ?? 'unary',
      server: fixture.source?.server ?? 'loams dev',
      file: fixture.source?.file ?? `recorded/${fixture.name}.json`,
      required: fixture.required ?? true,
      // `grpc-only` marks a fixture that cannot be replayed over the
      // Connect-unary fallback (D613). None are today; the field is here so
      // adding one is a data change and not a code change.
      transport: fixture.transport ?? 'connect',
      grpcOnly: (fixture.transport ?? 'connect') === 'grpc-only',
      pinnedBy: fixture.pinnedBy ?? [],
      reason: fixture.reason ?? null,
      steps: steps.length,
      volatile: fixture.volatile ?? VOLATILE_BY_FIXTURE[fixture.name] ?? [],
      about: fixture.about,
    });
  }
  rows.sort((a, b) => a.name.localeCompare(b.name));
  const required = rows.filter((row) => row.required).map((row) => row.name);
  const byClause = {};
  for (const row of rows) {
    for (const clause of row.pinnedBy) {
      (byClause[clause] ??= { fixtures: [], required: [] });
      byClause[clause].fixtures.push(row.name);
      if (row.required) {
        byClause[clause].required.push(row.name);
      }
    }
  }
  // Every clause R1–R10 gets a row, including the ones with no fixture. A clause
  // that is silently absent from this table reads exactly like one that was
  // covered, which is how a stub quietly becomes the specification.
  const clauses = {};
  for (const clause of CLAUSES) {
    const found = byClause[clause.id];
    clauses[clause.id] = {
      title: clause.title,
      covered: Boolean(found && found.required.length > 0),
      fixtures: found?.fixtures ?? [],
      note: found && found.required.length > 0 ? null : clause.note,
    };
  }
  return {
    about:
      'The authority on the 100% bar (design §44 §10.4, D617). ' +
      '`sdks/conformance/required.mjs` reads this file and nothing else, so ' +
      '"a language cannot skip a required fixture" has one definition rather ' +
      'than one per runner.',
    rule:
      'A language passes conformance when it has run every fixture with ' +
      '`required: true`. The only permitted skip is a fixture marked ' +
      '`transport: grpc-only` on the Connect-unary fallback transport (D613, ' +
      'for Ruby and PHP hosts without a native gRPC extension). Every other ' +
      'skip is a failure, and a fixture that is absent from this file is a ' +
      'failure too.',
    recordedFrom: {
      'loams dev': 'crates/loams — the primary target',
      'loams-apps-mock': 'crates/loams-apps-mock — the app packages and state',
    },
    volatile:
      'The response fields a re-recording is allowed to move: the ' +
      'server-generated `instance_id`, and the mock seed timestamps. ' +
      '`verify-corpus.mjs --drift` masks exactly these and fails on any other ' +
      'difference.',
    counts: { total: rows.length, required: required.length, optional: rows.length - required.length },
    clauses,
    required,
    byClause: Object.fromEntries(
      Object.entries(byClause).map(([clause, entry]) => [
        clause,
        entry.required.length > 0 ? entry.required : entry.fixtures,
      ]),
    ),
    fixtures: rows,
  };
}

/**
 * The runtime contract's clauses and, for the ones nothing can record yet, why.
 *
 * `docs/sdk/runtime-contract.md` is the normative text; this is the coverage
 * side of it, and it is generated so the two cannot be quoted out of context.
 */
const CLAUSES = [
  {
    id: 'R1',
    title: 'Credentials: bearer from a token source, one refresh on `token_expired`.',
    note: null,
  },
  {
    id: 'R2',
    title: 'Retry classes and backoff.',
    note:
      '**No fixture, and none is possible from either server today.** A ' +
      'retryable `unavailable` needs a dependency to be down and a ' +
      '`resource_exhausted` needs a loaded server; `RetryInfo` is on no proto ' +
      'at all (runtime-contract.md, R2). `mock_injects_retryable_errors` is the ' +
      'SDK1 Task 4 answer and it is **not built** — see `faults.json`. R2 is ' +
      'pinned against a stub in every language.',
  },
  {
    id: 'R3',
    title: 'One idempotency key per logical call, reused on every retry.',
    note: null,
  },
  {
    id: 'R4',
    title: 'Consistency tokens on reads and writes.',
    note:
      '**No fixture, and none is possible.** No RPC carries a ' +
      '`consistency_token`: `ListCollections` and the write paths arrive with ' +
      'API1 Tasks 2–4, and the token\'s encoding is not in the protos yet ' +
      '(runtime-contract.md, R4). R4 is pinned against a stub, deliberately — a ' +
      'silently-merged token reads stale data, which is worse than a failure.',
  },
  {
    id: 'R5',
    title: 'Unavailable services: the catalogue and the refusal.',
    note: null,
  },
  {
    id: 'R6',
    title: 'Pagination: `page_size` in, `next_page_token` out, an iterator of items.',
    note:
      '**No paged RPC exists on any server**, so there is nothing to record. ' +
      '`mock_status_list_is_not_paged` is the recording that pins this fact: ' +
      '`ListApprovals` declares both fields and honours neither, so there is ' +
      'still no end-to-end page to page. R6 is pinned against a stub; the ' +
      'end-to-end half arrives with API1 Task 2. Marked `required: false` ' +
      'precisely so no language is blocked on it.',
  },
  {
    id: 'R7',
    title: 'Streams: an async iterable that resumes from a cursor without loss.',
    note: null,
  },
  {
    id: 'R8',
    title: 'Errors: the code gives the class, the `reason` is the branch.',
    note: null,
  },
  {
    id: 'R9',
    title: 'Version reporting: `PROTO_REV` beside the server\'s `api_versions`.',
    note: null,
  },
  {
    id: 'R10',
    title: 'The browser is a first-class target: gRPC-Web with no Node built-in.',
    note:
      'Partly covered. `mock_error_encodings` records the gRPC-Web encodings, ' +
      'and R10\'s *static* half (no Node built-in at an SDK\'s default entry ' +
      'point) is a property of the SDK, not of a server. What no fixture can ' +
      'pin is a CORS preflight: that needs an origin and a server that answers ' +
      'preflights, and this repository has neither (runtime-contract.md, R10).',
  },
];

/**
 * The metadata for the thirteen `loams dev` cases, by name.
 *
 * Those files are Task 0's bytes and carry only `name`, `about`, `request`,
 * `response` and `expect` — no `kind`, no `pinnedBy`, no required flag. Adding
 * those keys would rewrite thirteen files that thirteen SDKs already read, and
 * the reader in every suite keys off exactly those five. So the metadata is
 * declared once here and merged in when the projections are built.
 */
function loamsDevMetadata() {
  const out = {};
  for (const testCase of loamsDevCases()) {
    out[testCase.name] = {
      kind: testCase.kind,
      transport: testCase.transport,
      required: testCase.required,
      pinnedBy: testCase.pinnedBy,
    };
  }
  return out;
}

/**
 * Reads every recorded fixture off disk, both sources, in a stable order.
 *
 * The projections are written from what is on disk rather than from what this
 * process just recorded, so `--source loams-dev` alone still refreshes
 * `manifest.json` against the committed app-mock half.
 */
export async function collectAll() {
  const out = [];
  const legacy = loamsDevMetadata();
  const flatDir = join(fixturesDir, 'recorded');
  for (const [dir, server] of [
    [flatDir, 'loams dev'],
    [join(flatDir, 'apps-mock'), 'loams-apps-mock'],
  ]) {
    let names;
    try {
      names = await readdir(dir);
    } catch {
      continue;
    }
    for (const name of names.sort()) {
      if (!name.endsWith('.json')) {
        continue;
      }
      const fixture = JSON.parse(await readFile(join(dir, name), 'utf8'));
      // The thirteen `loams dev` cases predate `source`; the app-mock ones carry
      // it. Filled in here so a projection never has to branch on it. `file` is
      // the path **relative to the corpus root**, because "is the manifest's
      // recorded path the one the file is actually at" is a check worth making
      // and guessing the directory is how that check gets faked.
      const relative = `recorded/${dir.endsWith('apps-mock') ? 'apps-mock/' : ''}${name}`;
      fixture.source = { server, file: relative };
      Object.assign(fixture, legacy[fixture.name] ?? {});
      out.push(fixture);
    }
  }
  return out;
}

if (import.meta.url === `file://${process.argv[1]}`) {
  await main();
}