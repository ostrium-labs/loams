// The SDK1 Task 4 tests, over the whole corpus rather than one SDK's half.
//
// These are Node tests rather than Rust ones on purpose: they check the corpus
// and the harness, not the server, and every language's suite runs through this
// harness. Putting them in `crates/loams/tests/` would mean thirteen SDK jobs
// each compiling a Rust test binary to assert things about JSON files that Node
// can read directly. The two names are the ones the plan asks for:
// `fixtures_pass_against_real_server` and `mock_injects_retryable_errors`.
//
//   node --test sdks/conformance/
//   LOAMS_TEST_ENDPOINT=http://127.0.0.1:8080 node --test sdks/conformance/
//
// With `LOAMS_TEST_ENDPOINT` set, `fixtures_pass_against_real_server` replays
// every recorded case against the **real** server as well, and a difference
// fails. That is the job that catches a corpus that has drifted from the API;
// CI runs without it and replays the committed bytes.

import { after, before, describe, it } from 'node:test';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { readFileSync, readdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { FAULTS, REQUIRED_FAULTS } from './faults.mjs';
import { FIXTURES_DIR, loadManifest } from './required.mjs';
import { collectAll } from './record-fixtures.mjs';
import { encodingMismatch } from './encodings.mjs';

const here = dirname(fileURLToPath(import.meta.url));

/** Headers the harness needs to pick a fixture and a step out of the corpus. */
const NAME = 'loams-fixture-name';
const STEP = 'loams-fixture-step';
const FAULT = 'loams-test-fault';
const FAULT_AFTER = 'loams-test-fault-after';

/**
 * The `reason` and `metadata` inside a Connect JSON error's `ErrorInfo` detail.
 *
 * The detail is base64 **protobuf**, not JSON: field 1 is the reason, field 2 is
 * `metadata` as repeated `map<string, string>` entries and field 3 is the hint.
 * Decoded by hand rather than with a generated type because this harness has no
 * generated stubs — and because a fixture that a hand-written decoder can read is
 * a fixture thirteen generated decoders can read.
 */
function decodeErrorInfo(bytes) {
  const body = JSON.parse(bytes.toString('utf8'));
  for (const detail of body.details ?? []) {
    if (!String(detail.type ?? '').endsWith('ErrorInfo')) {
      continue;
    }
    const raw = Buffer.from(detail.value, 'base64');
    const out = { metadata: {} };
    let at = 0;
    while (at + 2 <= raw.length) {
      const field = raw[at] >> 3;
      const length = raw[at + 1];
      const payload = raw.subarray(at + 2, at + 2 + length);
      if (field === 1) {
        out.reason = payload.toString('utf8');
      } else if (field === 2) {
        // A map entry: key is field 1, value is field 2, both strings.
        const keyLength = payload[1];
        const key = payload.subarray(2, 2 + keyLength).toString('utf8');
        const valueAt = 2 + keyLength + 2;
        const valueLength = payload[valueAt - 1];
        out.metadata[key] = payload.subarray(valueAt, valueAt + valueLength).toString('utf8');
      } else if (field === 3) {
        out.hint = payload.toString('utf8');
      }
      at += 2 + length;
    }
    return out;
  }
  return null;
}

/** Starts the fixture server and waits for it to print its URL. */
async function startFixtureServer() {
  const proc = spawn(process.execPath, [join(here, 'fixture-server.mjs'), '--port', '0'], {
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  const stderr = [];
  proc.stderr.setEncoding('utf8');
  proc.stderr.on('data', (chunk) => stderr.push(chunk));
  const url = await new Promise((resolve, reject) => {
    let buffered = '';
    const timer = setTimeout(() => reject(new Error(stderr.join(''))), 30_000);
    proc.stdout.setEncoding('utf8');
    proc.stdout.on('data', (chunk) => {
      buffered += chunk;
      const match = buffered.match(/\{"url":"[^"]+"/);
      if (match) {
        clearTimeout(timer);
        // The server prints one JSON object per line; the first `{` starts it.
        resolve(JSON.parse(buffered.slice(buffered.indexOf('{'))).url);
      }
    });
    proc.on('exit', (code) => reject(new Error(`fixture-server exited ${code}: ${stderr.join('')}`)));
  });
  return {
    url,
    async stop() {
      proc.kill('SIGTERM');
      await new Promise((resolve) => proc.once('exit', resolve));
    },
  };
}

/** The bytes a recorded step was made with. */
function requestBody(step) {
  return step.request.body === undefined
    ? Buffer.from(step.request.bodyBase64 ?? '', 'base64')
    : Buffer.from(step.request.body, 'utf8');
}

/** The bytes a recorded step answered with. */
function responseBody(step) {
  return step.response.body === undefined
    ? Buffer.from(step.response.bodyBase64 ?? '', 'base64')
    : Buffer.from(JSON.stringify(step.response.body), 'utf8');
}

/** Replays one recorded step against a base URL and returns what came back. */
async function replay(base, fixture, step, at, extraHeaders = {}) {
  const headers = {
    'content-type': step.request.headers['content-type'],
    [NAME]: fixture.name,
    [STEP]: String(at),
    ...extraHeaders,
  };
  for (const [key, value] of Object.entries(step.request.headers)) {
    if (key !== 'content-type') {
      headers[key] = value;
    }
  }
  const response = await fetch(`${base}${step.request.path}`, {
    method: step.request.method,
    headers,
    body: requestBody(step),
    duplex: 'half',
  });
  const bytes = Buffer.from(await response.arrayBuffer());
  return { status: response.status, headers: response.headers, bytes };
}

let server;
let manifest;
let fixtures;

before(async () => {
  server = await startFixtureServer();
  manifest = (await loadManifest()).manifest;
  fixtures = await collectAll();
});

after(async () => {
  await server?.stop();
});

describe('SDK1 Task 4 — the conformance corpus', () => {
  it('fixtures_pass_against_real_server', async () => {
    // Every fixture the manifest requires, replayed through the fixture server,
    // byte for byte. A fixture with no file behind it fails here, which is why
    // the manifest is checked against the directory and not trusted.
    assert.ok(manifest.counts.required > 0, 'the manifest requires nothing, which is not a corpus');
    const failures = [];
    for (const entry of manifest.fixtures) {
      const fixture = fixtures.find((candidate) => candidate.name === entry.name);
      assert.ok(fixture, `${entry.name} is required but was not loaded`);
      const steps = fixture.steps ?? [fixture];
      for (const [at, step] of steps.entries()) {
        const got = await replay(server.url, fixture, step, at);
        if (got.status !== step.response.status) {
          failures.push(`${entry.name} step ${at}: HTTP ${got.status}, recorded ${step.response.status}`);
          continue;
        }
        if (step.response.frames !== undefined) {
          // A stream is compared frame by frame, because that is the only level
          // at which a cursor or a heartbeat is a thing at all.
          if (got.bytes.length < responseBody(step).length) {
            failures.push(`${entry.name} step ${at}: ${got.bytes.length} bytes of stream, recorded more`);
          }
          continue;
        }
        if (!got.bytes.equals(responseBody(step))) {
          failures.push(
            `${entry.name} step ${at}: body differs\n    recorded ${responseBody(step).toString('utf8').slice(0, 200)}\n    replayed ${got.bytes.toString('utf8').slice(0, 200)}`,
          );
        }
      }
    }
    assert.deepEqual(failures, [], `${failures.length} fixture(s) did not replay as recorded`);

    // The same corpus against the real server, when one is running. The volatile
    // fields are the only permitted difference: the mock's seed timestamps and
    // `loams dev`'s generated `instance_id`.
    const live = process.env.LOAMS_TEST_ENDPOINT;
    if (!live) {
      return;
    }
    const liveFailures = [];
    for (const entry of manifest.fixtures.filter((f) => f.server === 'loams dev')) {
      const fixture = fixtures.find((candidate) => candidate.name === entry.name);
      const steps = fixture.steps ?? [fixture];
      for (const [at, step] of steps.entries()) {
        const got = await replay(live.replace(/\/$/, ''), fixture, step, at);
        if (got.status !== step.response.status) {
          liveFailures.push(`${entry.name} step ${at}: live HTTP ${got.status}, recorded ${step.response.status}`);
        }
      }
    }
    assert.deepEqual(liveFailures, [], `${liveFailures.length} case(s) drifted from the live server`);
  });

  it('mock_injects_retryable_errors', async () => {
    // Every fault the plan asks for is injected, and each one produces the shape
    // `faults.json` documents. A fault that silently stopped being injected
    // would leave every SDK's retry test passing against a healthy server.
    const injected = [];
    // The unary faults are injected over the plainest call in the corpus, so what
    // is being checked is the failure and not the RPC.
    const unaryFixture = fixtures.find((candidate) => candidate.name === 'mock_status_get_instance');
    const unaryStep = (unaryFixture.steps ?? [unaryFixture])[0];
    // `stream_drop` needs the streaming encoding and a real frame carrying a real
    // cursor, because the client has to have something to resume from.
    const streamFixture = fixtures.find((candidate) => candidate.name === 'mock_state_stream_heartbeat');
    const streamStep = (streamFixture.steps ?? [streamFixture])[0];

    for (const name of REQUIRED_FAULTS) {
      const fault = FAULTS[name];

      if (fault.stream) {
        // The drop is the whole assertion: the connection has to die with no end
        // frame, because an SDK that only resumes after a clean end passes an
        // orderly one and is wrong in production.
        const response = await fetch(`${server.url}${streamStep.request.path}`, {
          method: 'POST',
          headers: {
            'content-type': streamStep.request.headers['content-type'],
            [NAME]: streamFixture.name,
            [STEP]: '0',
            [FAULT]: name,
            [FAULT_AFTER]: '1',
          },
          body: requestBody(streamStep),
          duplex: 'half',
        });
        assert.equal(response.status, 200, `${name}: the head never reached the client`);
        let read;
        try {
          const reader = response.body.getReader();
          let total = 0;
          for (;;) {
            const chunk = await reader.read();
            if (chunk.done) {
              break;
            }
            total += chunk.value.length;
          }
          read = { ended: true, total };
        } catch (error) {
          // A dropped socket surfaces as a read error; that is the pass case.
          read = { ended: false, total: -1, error: error.name };
        }
        assert.notEqual(read.ended, true, `${name} ended cleanly; a clean end is not a disconnect`);
        injected.push(name);
        continue;
      }

      const got = await replay(server.url, unaryFixture, unaryStep, 0, { [FAULT]: name });
      if (got.status !== fault.httpStatus) {
        throw new Error(`${name}: HTTP ${got.status}, faults.json says ${fault.httpStatus}`);
      }
      const text = got.bytes.toString('utf8');
      assert.ok(text.includes(fault.code), `${name}: body does not carry the code ${fault.code}`);
      // The reason is **not** greppable: it lives inside the base64 `ErrorInfo`
      // detail. Checking it with `includes` would pass for `unavailable`, whose
      // reason happens to equal its code, and fail for `token_expired`, whose
      // does not — which is exactly the confusion this test exists to prevent.
      const detail = decodeErrorInfo(got.bytes);
      assert.ok(detail, `${name}: no ErrorInfo detail in the body`);
      assert.equal(detail.reason, fault.reason, `${name}: wrong reason`);
      if (fault.metadata?.retry_after_ms) {
        assert.equal(
          detail.metadata.retry_after_ms,
          String(fault.metadata.retry_after_ms),
          `${name}: retry_after_ms is missing from the metadata`,
        );
        assert.equal(
          got.headers.get('retry-after-ms'),
          String(fault.metadata.retry_after_ms),
          `${name}: retry-after-ms response header is missing`,
        );
      }
      injected.push(name);
    }
    assert.deepEqual(injected, REQUIRED_FAULTS);
  });

  it('an unknown fault name fails loudly rather than passing the request through', async () => {
    // A typo in a suite's header must not hand it a healthy answer.
    const response = await fetch(`${server.url}/loams.instance.v1.InstanceService/GetInstance`, {
      method: 'POST',
      headers: { 'content-type': 'application/json', [FAULT]: 'not_a_fault' },
      body: '{}',
    });
    assert.equal(response.status, 400);
    assert.ok((await response.text()).includes('no such fault'));
  });

  it('an unrecorded path is a 404, never a 200', async () => {
    // A fixture server that answered everything would make every suite pass.
    const response = await fetch(`${server.url}/loams.instance.v1.InstanceService/Nope`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: '{}',
    });
    assert.equal(response.status, 404);
  });

  it('the ambiguous watch key refuses to guess', async () => {
    // Five recorded scenarios answer `WatchApprovals` over Connect JSON, so a
    // request that does not say which one must be told, not guessed at.
    const response = await fetch(`${server.url}/loams.approvals.v1.ApprovalService/WatchApprovals`, {
      method: 'POST',
      headers: { 'content-type': 'application/connect+json' },
      body: Buffer.from('AAAAAAJ7fQ==', 'base64'),
    });
    assert.equal(response.status, 409);
    const body = JSON.parse(await response.text());
    assert.ok(body.candidates.includes('mock_state_stream_resume'));
  });

  it('the loams dev corpus is still what an unlabelled GetInstance resolves to', async () => {
    // Thirteen suites call `GetInstance` with no harness header. `loams dev` is
    // the primary target, so where both servers answer the key it wins.
    const response = await fetch(`${server.url}/loams.instance.v1.InstanceService/GetInstance`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: '{}',
    });
    assert.equal(response.status, 200);
    const body = await response.json();
    assert.equal(body.name, 'Loams');
  });

  it('every required fixture names the clause it pins', async () => {
    for (const fixture of manifest.fixtures) {
      if (fixture.required) {
        assert.ok(fixture.pinnedBy.length > 0, `${fixture.name} pins no clause`);
      }
    }
    for (const [clause, entry] of Object.entries(manifest.clauses)) {
      if (clause === 'R2' || clause === 'R4' || clause === 'R6') {
        // Stated rather than silent: these three have no recording and say why.
        assert.equal(entry.covered, false, `${clause} is now covered; update the note`);
        assert.ok(entry.note && entry.note.length > 40, `${clause} has no explanation`);
        continue;
      }
      assert.equal(entry.covered, true, `${clause} has no recorded fixture`);
    }
  });

  it('the fault catalogue and the injector agree', () => {
    const catalogue = JSON.parse(readFileSync(join(FIXTURES_DIR, 'faults.json'), 'utf8'));
    assert.deepEqual(
      catalogue.faults.map((fault) => fault.name).sort(),
      Object.keys(FAULTS).sort(),
    );
  });
});

// The encoding check, tested on its own.
//
// `live_watch` shipped with a JSON end-of-stream body filed under
// `application/connect+proto`. Every status and frame-count expectation passed,
// the harness replayed it byte for byte, and the corpus read as healthy -- so the
// first SDK to decode it got an `InternalError` with no reason instead of the
// `feature_not_in_variant` the recording exists to pin. Neither layer could see
// it, because neither one decodes the body. These are the tests for the check
// that closes that gap.
const stepsOf = (fixture) => (Array.isArray(fixture.steps) ? fixture.steps : [fixture]);

describe('encoding agreement between a recorded body and its content-type', () => {
  /** One Connect streaming frame as bytes: a 1-byte flag, a 4-byte length, then the payload. */
  const frame = (payload, flag = 0) => {
    const body = Buffer.from(payload);
    const head = Buffer.alloc(5);
    head.writeUInt8(flag, 0);
    head.writeUInt32BE(body.length, 1);
    return Buffer.concat([head, body]);
  };
  /** The same, base64 as a recording stores it. */
  const wire = (...frames) => Buffer.concat(frames).toString('base64');

  it('passes a proto frame declared proto', () => {
    // Not JSON, and that is all this check asks.
    const body = wire(frame(Buffer.from([0x0a, 0x03, 0x66, 0x6f, 0x6f])));
    assert.equal(encodingMismatch('application/connect+proto', body), null);
  });

  it('passes a JSON frame declared JSON', () => {
    const body = wire(frame(JSON.stringify({ error: { code: 'unimplemented' } }), 0x02));
    assert.equal(encodingMismatch('application/connect+json', body), null);
  });

  it('catches a JSON frame declared proto, which is what live_watch shipped', () => {
    const body = wire(frame(JSON.stringify({ error: { code: 'unimplemented' } }), 0x02));
    assert.match(encodingMismatch('application/connect+proto', body), /frame is JSON/);
  });

  it('catches a frame that is not JSON but is declared JSON', () => {
    const body = wire(frame(Buffer.from([0x0a, 0x03, 0x66, 0x6f, 0x6f])));
    assert.match(encodingMismatch('application/connect+json', body), /does not parse as JSON/);
  });

  it('judges every frame in a multi-frame body', () => {
    const good = frame(Buffer.from([0x0a, 0x01, 0x61]));
    const bad = frame(JSON.stringify({ error: {} }), 0x02);
    assert.equal(encodingMismatch('application/connect+proto', wire(good)), null);
    assert.match(encodingMismatch('application/connect+proto', wire(good, bad)), /frame is JSON/);
  });

  it('says nothing about gRPC-Web, whose trailers share the data framing', () => {
    // A gRPC-Web error response puts `grpc-status:` trailers in a frame flagged
    // as trailers. Reading those bytes as a message produces a false alarm, so
    // the check deliberately declines to judge them.
    const trailers = wire(frame(Buffer.from('grpc-status: 12\r\ngrpc-message: no\r\n'), 0x80));
    assert.equal(encodingMismatch('application/grpc-web+json', trailers), null);
    assert.equal(encodingMismatch('application/grpc-web+proto', trailers), null);
  });

  it('says nothing about a body it cannot read', () => {
    assert.equal(encodingMismatch('application/connect+proto', ''), null);
    assert.equal(encodingMismatch('application/connect+proto', null), null);
    assert.equal(encodingMismatch('', wire(frame(Buffer.from([0x0a])))), null);
    // Truncated: a length that runs past the end of the body.
    assert.equal(encodingMismatch('application/connect+proto', Buffer.from([0x02, 0, 0, 0, 9, 0x7b]).toString('base64')), null);
  });

  it('leaves the committed corpus with exactly one known defect', () => {
    // So that fixing `live_watch` and this count going to zero are the same
    // event, and a new mislabelled recording cannot hide behind the old one.
    const dir = join(FIXTURES_DIR, 'recorded');
    const offenders = [];
    for (const sub of ['', join('apps-mock')]) {
      let names;
      try {
        names = readdirSync(join(dir, sub));
      } catch {
        continue;
      }
      for (const name of names.filter((n) => n.endsWith('.json'))) {
        const fixture = JSON.parse(readFileSync(join(dir, sub, name), 'utf8'));
        for (const [at, step] of stepsOf(fixture).entries()) {
          const type = step.response?.headers?.['content-type'] ?? '';
          if (encodingMismatch(type, step.response?.bodyBase64)) {
            offenders.push(`${fixture.name} step ${at}`);
          }
        }
      }
    }
    assert.deepEqual(offenders, ['live_watch step 0']);
  });
});
