// The conformance fixture server (design §44 §10.4, SDK1 Task 4).
//
// Serves the recorded corpus over real HTTP so an SDK's conformance suite runs
// against something that speaks the wire, with nothing to install but Node. The
// thirteen SDKs all run their suite against this, which is what makes the suite
// comparable between them: same bytes, same status codes, same trailers.
//
//   node sdks/conformance/fixture-server.mjs [--port 0] [--fixtures <dir>]
//
// It prints `{"url":"http://127.0.0.1:PORT"}` on stdout once it is listening,
// then serves until it is killed.
//
// A request that matches no recorded case is a **404 with a body naming the
// gap**, never a silent success. A conformance suite that passed because the
// fixture server answered everything with 200 would be worse than no suite, so
// an unrecorded path fails loudly and the corpus has to grow.
//
// ## Three things it does beyond replaying bytes
//
// **Faults.** A request carrying `loams-test-fault: <name>` gets that fault
// from `faults.mjs` instead of a recording. Faults are the one thing in the
// corpus that is *not* a recording — no healthy server produces a retryable
// `unavailable` on demand — so they are marked as injected everywhere they
// appear, and the server says so in `/__fixtures/faults`.
//
// **Scenario steps.** A single-request fixture is a `method + path + content
// type` lookup, but a *scenario* is an ordered sequence whose steps are often
// byte-identical: the idempotency replay sends the same request twice. Replaying
// step 0 for step 1 would make the second call answer with the first call's
// bytes and prove nothing. So a scenario step is selected by the
// `loams-fixture-step` header, which every language can send and none of them
// has to know about to run the rest of the corpus.
//
// **Streams.** A recorded stream is a bounded prefix, so it is written as the
// frames it is rather than as one buffer: the client sees them arrive one at a
// time, which is the only way a heartbeat or a cursor is observable at all.

import { createServer } from 'node:http';
import { readFile, readdir } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { FAULTS, REQUIRED_FAULTS, faultsCatalogue, injectFault } from './faults.mjs';

const here = dirname(fileURLToPath(import.meta.url));

/** Which step of a scenario a request wants; absent means the first. */
const STEP_HEADER = 'loams-fixture-step';
/**
 * Which fixture a request wants, when more than one answers the same key.
 *
 * Several watch-stream fixtures are all `WatchApprovals` over Connect JSON, so
 * the request alone cannot say which recording it is asking for. Requiring the
 * header rather than picking one is the point: a suite that meant the heartbeat
 * fixture and got the snapshot-reset one would pass on the wrong bytes.
 */
const NAME_HEADER = 'loams-fixture-name';
/** The prefix for the harness's own endpoints, which are not part of the API. */
const HARNESS = '/__fixtures';

function arg(name, fallback) {
  const at = process.argv.indexOf(`--${name}`);
  return at >= 0 && at + 1 < process.argv.length ? process.argv[at + 1] : fallback;
}

/**
 * The encoding of a content type, which is what a recorded case is keyed on:
 * `json`, `proto`, `grpc_web`, `grpc_web_json` and `connect` each have their own
 * response bytes. The gRPC-Web variants are kept apart because a client that
 * asks for gRPC-Web with protobuf must not be handed gRPC-Web with JSON, which
 * is the kind of mismatch that shows up as a parse error inside somebody's SDK
 * rather than as a failure here.
 */
function family(contentType) {
  const type = (contentType ?? '').split(';')[0].trim();
  if (type === 'application/grpc-web+json') {
    return 'grpc_web_json';
  }
  if (type.startsWith('application/grpc-web')) {
    return 'grpc_web';
  }
  if (type.startsWith('application/connect')) {
    return 'connect';
  }
  if (type === 'application/json') {
    return 'json';
  }
  if (type === 'application/proto') {
    return 'proto';
  }
  return type === '' ? 'none' : type;
}

/** The steps of a fixture: a scenario has several, a single-request case one. */
function stepsOf(fixture) {
  return Array.isArray(fixture.steps) ? fixture.steps : [fixture];
}

/**
 * Every recorded case, from both sources, keyed by request shape.
 *
 * `recorded/apps-mock/` is walked as well as `recorded/`: the flat directory is
 * the thirteen `loams dev` cases and is read first, so their keys resolve to
 * exactly what they did before any of this existed.
 *
 * A fixture is registered under the key of **every step it has**, not just its
 * first. A scenario is allowed to touch more than one RPC — the stream-resume
 * one opens a watch, decides an approval and opens the watch again — and a key
 * built from step 0 alone would leave steps 1 and 2 with no route at all.
 *
 * A key maps to a **list**, because a key stopped being unique when the app-mock
 * scenarios arrived: six recorded scenarios are `DecideApproval` over Connect
 * JSON and five are `WatchApprovals`. The list is what makes that visible, and
 * `loams-fixture-name` is what resolves it.
 */
async function load(fixturesDir) {
  const byKey = new Map();
  for (const dir of [join(fixturesDir, 'recorded'), join(fixturesDir, 'recorded', 'apps-mock')]) {
    let files;
    try {
      files = await readdir(dir);
    } catch {
      continue;
    }
    for (const file of files.filter((name) => name.endsWith('.json')).sort()) {
      const fixture = JSON.parse(await readFile(join(dir, file), 'utf8'));
      const steps = stepsOf(fixture);
      // `default` marks the `loams dev` half. It is what a request with no
      // `loams-fixture-name` gets where a key overlaps, because that is what
      // every suite resolved before the app-mock scenarios existed and changing
      // it would break them. See the resolution rule in the request handler.
      const entry = { name: fixture.name, steps, isDefault: !dir.endsWith('apps-mock') };
      const seen = new Set();
      for (const step of steps) {
        const key = keyOf(step.request);
        if (seen.has(key)) {
          // One entry per key per fixture: two steps of the same fixture on the
          // same key are what `loams-fixture-step` is for.
          continue;
        }
        seen.add(key);
        const list = byKey.get(key) ?? [];
        list.push(entry);
        byKey.set(key, list);
      }
    }
  }
  return byKey;
}

/** The `method path family` key a recorded request is looked up by. */
function keyOf(request) {
  return `${request.method} ${request.path} ${family(request.headers['content-type'])}`;
}

/** The request bytes a recorded step was made with. */
function expectedBody(step) {
  return step.request.body === undefined
    ? Buffer.from(step.request.bodyBase64 ?? '', 'base64')
    : Buffer.from(step.request.body, 'utf8');
}

/** The response bytes a recorded step answered with. */
function recordedBody(step) {
  return step.response.body === undefined
    ? Buffer.from(step.response.bodyBase64 ?? '', 'base64')
    : Buffer.from(JSON.stringify(step.response.body), 'utf8');
}

/** Reassembles the recorded stream prefix into its individual frames. */
function recordedFrames(step) {
  const bytes = Buffer.from(step.response.bodyBase64 ?? '', 'base64');
  const frames = step.response.frames ?? [];
  const out = frames.map((frame) => {
    const payload = Buffer.from(frame.payload, 'base64');
    const header = Buffer.alloc(5);
    header[0] = frame.flags ?? 0;
    header.writeUInt32BE(payload.length, 1);
    return Buffer.concat([header, payload]);
  });
  if (out.length > 0) {
    return out;
  }
  // A recording with no parsed frames (the `loams dev` streaming refusal) is one
  // frame; splitting it is what lets `stream_drop` cut it mid-answer.
  if (bytes.length === 0) {
    return [];
  }
  const framesOut = [];
  let at = 0;
  while (at + 5 <= bytes.length) {
    const length = bytes.readUInt32BE(at + 1);
    if (at + 5 + length > bytes.length) {
      break;
    }
    framesOut.push(bytes.subarray(at, at + 5 + length));
    at += 5 + length;
  }
  return framesOut;
}

const fixturesDir = arg('fixtures', join(here, '..', 'fixtures'));
const cases = await load(fixturesDir);
if (cases.size === 0) {
  console.error(`no recorded cases under ${fixturesDir}/recorded`);
  process.exit(2);
}

const manifest = JSON.parse(
  await readFile(join(fixturesDir, 'manifest.json'), 'utf8').catch(() => '{}'),
);

const server = createServer((request, response) => {
  if (request.url === `${HARNESS}/manifest`) {
    // The required-fixture list over HTTP, so a suite in any of the thirteen
    // languages reads the authority instead of hard-coding names of its own.
    response.writeHead(200, { 'content-type': 'application/json' });
    response.end(JSON.stringify(manifest));
    return;
  }
  if (request.url === `${HARNESS}/faults`) {
    response.writeHead(200, { 'content-type': 'application/json' });
    response.end(JSON.stringify(faultsCatalogue()));
    return;
  }
  const chunks = [];
  request.on('data', (chunk) => chunks.push(chunk));
  request.on('end', () => {
    const key = `${request.method} ${request.url} ${family(request.headers['content-type'])}`;
    const candidates = cases.get(key);
    if (candidates === undefined) {
      response.writeHead(404, { 'content-type': 'application/json' });
      response.end(
        JSON.stringify({
          error: `no recorded fixture for ${key}`,
          recorded: [...cases.keys()],
        }),
      );
      return;
    }

    // Which recording answers this request.
    //
    // Two cases, and the split is deliberate:
    //
    // - Exactly one candidate: answer it.
    // - More than one, and one of them is a `loams dev` case: answer that one.
    //   `GetInstance` over Connect JSON is recorded from **both** servers, and a
    //   suite that has always called it with no header has to keep working;
    //   `loams dev` is the primary target (D617), so it is the default.
    // - More than one and no `loams dev` case: **409**, naming the candidates.
    //   Five recorded scenarios are all `WatchApprovals` over Connect JSON, and a
    //   silent pick would hand a suite the bytes of a fixture it did not ask for.
    let found = candidates[0];
    const named = request.headers[NAME_HEADER];
    if (candidates.length > 1) {
      if (named) {
        found = candidates.find((entry) => entry.name === named);
        if (found === undefined) {
          response.writeHead(404, { 'content-type': 'application/json' });
          response.end(
            JSON.stringify({
              error: `no fixture named ${named} for ${key}`,
              candidates: candidates.map((entry) => entry.name),
            }),
          );
          return;
        }
      } else {
        found = candidates.find((entry) => entry.isDefault);
        if (found === undefined) {
          response.writeHead(409, { 'content-type': 'application/json' });
          response.end(
            JSON.stringify({
              error: `${candidates.length} fixtures answer ${key}; say which with ${NAME_HEADER}`,
              header: NAME_HEADER,
              candidates: candidates.map((entry) => entry.name),
            }),
          );
          return;
        }
      }
    }

    // Faults are answered before the recording is consulted, because a fault is a
    // different answer to the *same* request: `unavailable` on `GetInstance` is
    // the R2 fixture and must not be confused with the recorded success.
    if (
      injectFault(request, response, (sendFrame) => {
        for (const frame of recordedFrames(found.steps[found.steps.length - 1])) {
          if (sendFrame(frame) === false) {
            break;
          }
        }
      })
    ) {
      return;
    }

    const wanted = request.headers[STEP_HEADER];
    // With no step header the step is inferred: the first step of this fixture
    // whose request is the one being made. That is what lets a suite name a
    // scenario and walk it without counting steps, and it is why the header is
    // only genuinely needed when one fixture has two steps on the same key —
    // the idempotency replay, which sends the same request twice.
    let at = Number(wanted ?? Number.NaN);
    if (!Number.isInteger(at)) {
      at = found.steps.findIndex((step) => keyOf(step.request) === key);
    }
    if (at < 0) {
      at = 0;
    }
    if (at < 0 || at >= found.steps.length) {
      response.writeHead(404, { 'content-type': 'application/json' });
      response.end(
        JSON.stringify({
          error: `${found.name} has ${found.steps.length} step(s); step ${wanted ?? '(inferred)'} is not one of them`,
          steps: found.steps.length,
          at,
        }),
      );
      return;
    }
    const step = found.steps[at];

    // The recorded case also carries the request that produced it, and it is
    // checked. A replay that ignores what the client sent would pass an SDK
    // that frames a gRPC-Web message wrongly or posts the wrong payload, which
    // is the class of bug a recorded corpus exists to catch.
    const sent = Buffer.concat(chunks);
    const expected = expectedBody(step);
    if (!sent.equals(expected)) {
      response.writeHead(400, { 'content-type': 'application/json' });
      response.end(
        JSON.stringify({
          error: `the request does not match the recorded one for ${found.name} step ${at}`,
          expected: expected.toString('base64'),
          sent: sent.toString('base64'),
        }),
      );
      return;
    }

    const headers = { ...step.response.headers };
    if (step.response.frames !== undefined) {
      // A stream is written frame by frame so the client observes them arriving
      // separately, which is the only way a cursor or a heartbeat is observable
      // at all. An orderly `end` follows, and that is right: the *recording* of a
      // stream is a prefix a server closed. `stream_drop` is what removes the
      // orderly part.
      response.writeHead(step.response.status, headers);
      for (const frame of recordedFrames(step)) {
        response.write(frame);
      }
      response.end();
      return;
    }
    response.writeHead(step.response.status, headers);
    response.end(recordedBody(step));
  });
});

server.listen(Number(arg('port', '0')), '127.0.0.1', () => {
  const address = server.address();
  const all = [...cases.values()].flat();
  process.stdout.write(
    `${JSON.stringify({
      url: `http://127.0.0.1:${address.port}`,
      keys: cases.size,
      fixtures: all.length,
      ambiguousKeys: [...cases.values()].filter((list) => list.length > 1).length,
      scenarios: all.filter((entry) => entry.steps.length > 1).length,
      faults: REQUIRED_FAULTS,
      required: manifest.required?.length ?? 0,
    })}\n`,
  );
});

for (const signal of ['SIGINT', 'SIGTERM']) {
  process.on(signal, () => {
    server.close(() => process.exit(0));
  });
}

export { FAULTS, REQUIRED_FAULTS, faultsCatalogue, manifest };
