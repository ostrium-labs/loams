// Verifies the conformance corpus, and fails loudly.
//
//   node sdks/conformance/verify-corpus.mjs            # the corpus as committed
//   node sdks/conformance/verify-corpus.mjs --drift    # re-record and diff
//   node sdks/conformance/verify-corpus.mjs --quiet
//
// Five things, each of which has been a real way for a fixture corpus to lie:
//
// 1. **Every fixture named in `manifest.json` exists**, and nothing is in the
//    corpus that the manifest does not name. A manifest entry with no file
//    behind it is the dangerous direction: it is the shape "this language may
//    skip it" takes by accident.
// 2. **Every recorded case still says what `manifest.json` says it says.** A
//    fixture whose file has drifted from its `expect` block is a test that
//    asserts the wrong thing, and it passes.
// 3. **`faults.json` and the `FAULTS` table agree.** The catalogue is written by
//    hand and the table is what is served; if they diverge, a suite reads one and
//    gets the other.
// 4. **Every registry reason is either recorded or explicitly listed as
//    unproducible.** `docs/api/reasons.md` promises reasons that no server can
//    raise; the point is that the promise and the gap are both written down, so
//    deleting a row from one without the other failing is impossible.
// 5. With `--drift`, a **re-recording** differs only in the declared volatile
//    fields. Timestamps and the server-generated `instance_id` may move; nothing
//    else may.
//
// Exit code is 0 when everything holds and 1 when anything does not. `--drift`
// needs both servers and is the job that can afford them.

import { readdir, readFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { FAULTS, REQUIRED_FAULTS } from './faults.mjs';
import { FIXTURES_DIR, loadFaults, loadManifest, maySkip } from './required.mjs';
import { encodingMismatch } from './encodings.mjs';

const here = dirname(fileURLToPath(import.meta.url));

function arg(name, fallback) {
  const at = process.argv.indexOf(`--${name}`);
  return at >= 0 && at + 1 < process.argv.length ? process.argv[at + 1] : fallback;
}
const has = (name) => process.argv.includes(`--${name}`);

const quiet = has('quiet');
const log = (...parts) => {
  if (!quiet) {
    console.log(...parts);
  }
};

/** The steps of a fixture: a scenario has several, a single-request case one. */
function stepsOf(fixture) {
  return Array.isArray(fixture.steps) ? fixture.steps : [fixture];
}

/** Every `.json` fixture file on disk, by name, from both source directories. */
async function filesOnDisk(dir = FIXTURES_DIR) {
  const out = new Map();
  for (const sub of ['recorded', join('recorded', 'apps-mock')]) {
    let names;
    try {
      names = await readdir(join(dir, sub));
    } catch {
      continue;
    }
    for (const name of names.filter((n) => n.endsWith('.json')).sort()) {
      const fixture = JSON.parse(await readFile(join(dir, sub, name), 'utf8'));
      out.set(fixture.name, { fixture, file: `${sub}/${name}` });
    }
  }
  return out;
}

const problems = [];
const fail = (message) => problems.push(message);

// ---- 1 and 2: the manifest and the corpus agree -----------------------------

const { manifest, byName } = await loadManifest();
const onDisk = await filesOnDisk();

for (const name of manifest.required) {
  if (!onDisk.has(name)) {
    fail(`manifest.json requires ${name} but there is no recorded file for it`);
  }
}
for (const fixture of manifest.fixtures) {
  const entry = onDisk.get(fixture.name);
  if (!entry) {
    fail(`manifest.json names ${fixture.name} (${fixture.file}), which does not exist`);
    continue;
  }
  if (entry.file !== fixture.file) {
    fail(`${fixture.name} is at ${entry.file}, but manifest.json says ${fixture.file}`);
  }
  const steps = stepsOf(entry.fixture);
  if (steps.length !== fixture.steps) {
    fail(`${fixture.name} has ${steps.length} step(s); manifest.json says ${fixture.steps}`);
  }
  // The five `loams dev` cases carry `expect` per case; the app-mock scenarios
  // carry it per step. Both are checked against what the recording actually is,
  // which is the only way an expectation that was wrong when it was written gets
  // caught rather than copied into thirteen SDKs.
  for (const [at, step] of steps.entries()) {
    const expect = step.expect;
    if (!expect) {
      fail(`${fixture.name} step ${at} has no expectation to check`);
      continue;
    }
    if (expect.status !== undefined && step.response.status !== expect.status) {
      fail(
        `${fixture.name} step ${at} recorded HTTP ${step.response.status}, its expectation says ${expect.status}`,
      );
    }
    if (fixture.reason && !steps.some((step) => step.expect?.reason === fixture.reason)) {
      fail(
        `${fixture.name} is catalogued as reason ${fixture.reason} but no step expects it; ` +
          'the steps say ' +
          JSON.stringify(steps.map((step) => step.expect?.reason ?? null)),
      );
    }
    const responseType = step.response?.headers?.['content-type'] ?? '';
    const mismatch = encodingMismatch(responseType, step.response?.bodyBase64);
    if (mismatch) {
      fail(`${fixture.name} step ${at} ${mismatch}`);
    }
    const expectedFrames = expect.frames ?? steps.filter((s) => s.response.frames).length;
    if (step.response.frames && step.response.frames.length !== expectedFrames) {
      fail(
        `${fixture.name} step ${at} recorded ${step.response.frames.length} frame(s), its expectation says ${expectedFrames}`,
      );
    }
  }
}

for (const name of onDisk.keys()) {
  if (!byName.has(name)) {
    fail(`${name} is recorded but manifest.json does not name it, so nothing requires it`);
  }
}

// A required fixture has to say which clause it pins, or "which test covers R7"
// has no answer and a clause can quietly lose its coverage.
for (const fixture of manifest.fixtures) {
  if (fixture.required && fixture.pinnedBy.length === 0) {
    fail(`${fixture.name} is required but names no runtime-contract clause`);
  }
}

// ---- 3: the fault catalogue and the injector agree ---------------------------

const catalogue = await loadFaults();
const catalogued = new Map(catalogue.faults.map((fault) => [fault.name, fault]));
for (const name of Object.keys(FAULTS)) {
  if (!catalogued.has(name)) {
    fail(`faults.json does not describe ${name}, which the injector serves`);
  }
}
for (const [name, fault] of catalogued) {
  const served = FAULTS[name];
  if (!served) {
    fail(`faults.json describes ${name}, which the injector cannot serve`);
    continue;
  }
  if ((served.code ?? null) !== (fault.connectCode ?? null)) {
    fail(`${name}: faults.json says Connect code ${fault.connectCode}, the injector serves ${served.code}`);
  }
  if ((served.reason ?? null) !== (fault.reason ?? null)) {
    fail(`${name}: faults.json says reason ${fault.reason}, the injector serves ${served.reason}`);
  }
  if (served.httpStatus !== fault.httpStatus) {
    fail(`${name}: faults.json says HTTP ${fault.httpStatus}, the injector serves ${served.httpStatus}`);
  }
  if (served.grpcStatus !== fault.grpcStatus) {
    fail(`${name}: faults.json says gRPC status ${fault.grpcStatus}, the injector serves ${served.grpcStatus}`);
  }
  if (Boolean(served.retryable) !== Boolean(fault.retryable)) {
    fail(`${name}: faults.json says retryable ${fault.retryable}, the injector serves ${served.retryable}`);
  }
  if (JSON.stringify(served.metadata ?? null) !== JSON.stringify(fault.metadata ?? null)) {
    fail(`${name}: faults.json and the injector disagree about the metadata`);
  }
  if (JSON.stringify(served.trailers ?? null) !== JSON.stringify(fault.trailers ?? null)) {
    fail(`${name}: faults.json and the injector disagree about the trailers`);
  }
  if (Boolean(served.stream ?? false) !== Boolean(fault.stream ?? false)) {
    fail(`${name}: faults.json and the injector disagree about whether it is a stream`);
  }
  if (fault.required !== true) {
    fail(`${name} is injected but not marked required in faults.json`);
  }
}
for (const name of REQUIRED_FAULTS) {
  if (!catalogued.has(name)) {
    fail(`the injector requires the fault ${name} and faults.json does not list it`);
  }
}

// ---- 4: every registry reason is recorded or declared unproducible -----------

const errorProjection = JSON.parse(await readFile(join(FIXTURES_DIR, 'error.json'), 'utf8'));
const registry = await readFile(join(here, '..', '..', 'docs', 'api', 'reasons.md'), 'utf8').catch(
  () => null,
);
if (registry === null) {
  fail('docs/api/reasons.md is missing, so the corpus cannot be checked against the registry');
} else {
  const declared = new Set(
    [...registry.matchAll(/^\| `([a-z0-9_]+)` \|/gm)].map((match) => match[1]),
  );
  const recorded = new Set(
    errorProjection.recorded.filter((row) => row.reason).map((row) => row.reason),
  );
  const unproducible = new Set(errorProjection.unproducible.map((row) => row.reason));
  for (const reason of declared) {
    if (recorded.has(reason) || unproducible.has(reason)) {
      continue;
    }
    fail(
      `the registry promises ${reason}, and error.json neither records it nor lists it as ` +
        'unproducible — an SDK would have to guess what it means',
    );
  }
  for (const reason of unproducible) {
    if (!declared.has(reason)) {
      fail(`error.json lists ${reason} as unproducible, but the registry has no such reason`);
    }
  }
  log(
    `registry: ${declared.size} reasons, ${recorded.size} recorded, ${unproducible.size} declared unproducible`,
  );
}

// ---- the skip rule itself is still expressible ------------------------------

for (const fixture of manifest.fixtures) {
  if (fixture.transport === 'grpc-only' && !fixture.grpcOnly) {
    fail(`${fixture.name} says transport grpc-only but is not flagged grpcOnly`);
  }
  const verdict = maySkip(fixture, { transport: 'connect' });
  if (verdict.ok) {
    fail(`${fixture.name} could be skipped on a plain Connect transport, which the rule forbids`);
  }
}

// ---- 5: optional drift check -------------------------------------------------

/**
 * Replaces the values a re-recording is allowed to move.
 *
 * Two kinds of entry, because the fields come in two shapes:
 *
 * - **`createdAt`** — a bare field name, masked **anywhere** it appears. The
 *   mock's timestamps are nested to whatever depth the message puts them
 *   (`steps.0.response.body.approvals.0.createdAt`), and listing every path
 *   would mean editing this file every time a message gains a field.
 * - **`response.bodyBase64`** — a dotted path, masked only there. Needed for the
 *   two binary `loams dev` encodings, where the generated `instance_id` is baked
 *   into the serialized bytes and there is no field to point at. A path is
 *   matched as a **suffix** of the location, so `body.instanceId` matches
 *   `response.body.instanceId` without the fixture having to spell out the
 *   envelope around it.
 *
 * A dotted entry is deliberately *not* treated as a field name: masking
 * `bodyBase64` everywhere would also mask `request.bodyBase64`, and the request
 * bytes are exactly what a corpus must pin.
 */
function maskVolatile(value, entries) {
  const names = new Set();
  const paths = [];
  for (const entry of entries) {
    if (entry.includes('.')) {
      paths.push(entry.split('.'));
    } else {
      names.add(entry);
    }
  }
  const suffix = (trail, path) => {
    // Array indices are dropped before matching: a path names fields, and the
    // fixture wraps them in whatever number of steps and frames it happens to
    // have. `frames.payload` has to match
    // `steps.0.response.frames.1.payload`, which is the only way a path written
    // once can keep working when a scenario gains a step.
    const fields = trail.filter((segment) => !/^\d+$/.test(segment));
    return (
      fields.length >= path.length &&
      path.every((segment, at) => fields[fields.length - path.length + at] === segment)
    );
  };

  const walk = (node, trail) => {
    if (Array.isArray(node)) {
      return node.map((item, at) => walk(item, [...trail, at]));
    }
    if (node === null || typeof node !== 'object') {
      return node;
    }
    const out = {};
    for (const [key, inner] of Object.entries(node)) {
      const here = [...trail, key];
      if (names.has(key) || paths.some((path) => suffix(here, path))) {
        out[key] = '<volatile>';
      } else {
        out[key] = walk(inner, here);
      }
    }
    return out;
  };
  return walk(value, []);
}

/**
 * Canonical JSON: object keys sorted, everything else untouched.
 *
 * Compared before stringifying because proto3 JSON object key order is **not**
 * meaningful, and two real fields of the corpus do not have a stable one:
 * `GetInstance.min_app_versions` is a Rust map, so its order changes between
 * runs. Comparing serialized text would call that a drift on every single
 * re-recording, and the fix a developer reaches for — re-recording until it goes
 * away — is worse than the noise.
 */
function canonical(value) {
  if (Array.isArray(value)) {
    return value.map(canonical);
  }
  if (value === null || typeof value !== 'object') {
    return value;
  }
  return Object.fromEntries(
    Object.keys(value)
      .sort()
      .map((key) => [key, canonical(value[key])]),
  );
}

/** Compares two recordings with the volatile fields masked out. */
function diffMasked(name, before, after, volatile) {
  const a = JSON.stringify(canonical(maskVolatile(before, volatile)));
  const b = JSON.stringify(canonical(maskVolatile(after, volatile)));
  if (a === b) {
    return null;
  }
  // The first differing character is worth more than "they differ": a fixture
  // that stopped matching because a field was renamed is a different bug from
  // one that drifted a byte, and the message says which.
  let at = 0;
  while (at < a.length && at < b.length && a[at] === b[at]) {
    at += 1;
  }
  return (
    `${name} differs beyond its declared volatile fields\n` +
    `    committed: …${a.slice(Math.max(0, at - 60), at + 90)}\n` +
    `    recorded:  …${b.slice(Math.max(0, at - 60), at + 90)}`
  );
}

if (has('drift')) {
  const { spawnSync } = await import('node:child_process');
  const { mkdtemp, rm } = await import('node:fs/promises');
  const { tmpdir } = await import('node:os');
  // A temporary corpus, never the real one: a drift check that rewrites the
  // corpus cannot tell "it drifted" from "I ran", and a failure leaves the
  // corpus half-rewritten for whatever runs next.
  const scratch = await mkdtemp(join(tmpdir(), 'loams-fixture-drift-'));
  try {
    const recorded = spawnSync(
      process.execPath,
      [
        join(here, 'record-fixtures.mjs'),
        '--source',
        'all',
        '--fixtures',
        scratch,
        '--commit',
        arg('commit', 'drift-check'),
      ],
      { encoding: 'utf8', env: process.env },
    );
    if (recorded.status !== 0) {
      fail(`re-recording failed: ${recorded.stderr || recorded.stdout}`);
    } else {
      const fresh = await filesOnDisk(scratch);
      const seen = new Set();
      for (const [name, entry] of onDisk) {
        seen.add(name);
        const other = fresh.get(name);
        if (!other) {
          fail(`${name} was not produced by the re-recording`);
          continue;
        }
        // `source` is provenance, not a recorded byte: the commit and the moment
        // of the recording differ by construction on every run. Everything else
        // in the file is compared.
        const strip = (fixture) => {
          if (!fixture.source) {
            return fixture;
          }
          const { recordedAt, commit, ...rest } = fixture.source;
          void recordedAt;
          void commit;
          return { ...fixture, source: rest };
        };
        const volatile = byName.get(name)?.volatile ?? [];
        const message = diffMasked(name, strip(entry.fixture), strip(other.fixture), volatile);
        if (message) {
          fail(message);
        }
      }
      for (const name of fresh.keys()) {
        if (!seen.has(name)) {
          fail(`the re-recording produced ${name}, which is not in the committed corpus`);
        }
      }
      log(`drift: compared ${onDisk.size} fixtures against a fresh recording of both servers`);
    }
  } finally {
    await rm(scratch, { recursive: true, force: true });
  }
}

// ---- report -----------------------------------------------------------------

if (problems.length === 0) {
  log(
    `corpus ok: ${manifest.counts.total} fixtures, ${manifest.counts.required} required, ` +
      `${catalogue.faults.length} injected faults, ${Object.keys(FAULTS).length} served`,
  );
  process.exit(0);
}
console.error(`${problems.length} problem(s) with the conformance corpus:`);
for (const problem of problems) {
  console.error(`  - ${problem}`);
}
process.exit(1);
