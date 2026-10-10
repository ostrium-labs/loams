// SDK2 Task 0's `typescript_conformance_all_required_fixtures`.
//
// The corpus in `sdks/fixtures` is recorded from a real `loams dev` and from
// `loams-apps-mock`, and this runs every **required** fixture in it through the
// SDK and writes the report the 100% bar is read from (design §44 §10.4, D617).
//
// The rest of the file pins what a client is *for* — a successful call, a
// structured reason, and the unavailable-service path in all three of its shapes
// — through the public facade, which is the surface an application uses. The
// fixture driver beside it answers a different question: not "does the SDK work"
// but "did the suite run every fixture the corpus marks required", and it
// answers it by driving them, so the answer cannot be written down anywhere.
//
// Three things are covered, which between them are what the design asks of a
// conforming SDK:
//
// - a successful call, in every encoding an SDK might pick;
// - a structured-reason error, with `reason` and not the message;
// - the unavailable-service path, in all three of its shapes: the guard that
//   costs no RPC, the refusal a call gets, and the refusal on a stream.

import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { REQUIRED_TESTS, testName } from '../../../../conformance/required.mjs';
import { Loams } from '../src/loams.js';
import { FeatureNotInVariantError, isLoamsError } from '../src/runtime/errors.js';
import { FIXTURES, type FixtureServer, startFixtureServer } from './helpers/server.js';
import { requiredFixtures } from './helpers/corpus.js';
import { TRANSPORT, driveRequiredFixtures } from './helpers/required.js';
import { REPORT, clearReport, testsThisSuiteHas, writeConformanceReport } from './helpers/report.js';

const here = dirname(fileURLToPath(import.meta.url));

let server: FixtureServer;
let loams: Loams;

beforeAll(async () => {
  server = await startFixtureServer();
  loams = new Loams({ endpoint: server.endpoint });
  // Any report from an earlier run goes first, so this run leaves either a
  // report that describes **this** run or none at all. A gate reading the
  // previous run's file is the failure this suite is here to stop.
  await clearReport();
});

afterAll(async () => {
  await server.stop();
});

/** The corpus index, so the suite fails if a recorded case is missing. */
async function corpus(): Promise<{ cases: { name: string; reason: string | null }[] }> {
  const text = await readFile(resolve(FIXTURES, 'index.json'), 'utf8');
  return JSON.parse(text);
}

describe('@loams/client conformance', () => {
  it('typescript_conformance_all_required_fixtures', async () => {
    if (server.live) {
      // A live `loams dev` does not serve the app packages at all, so the
      // `loams-apps-mock` half of the corpus cannot exist in this run and there
      // is nothing honest to report. Writing a partial one would put a file in
      // front of `check-languages.mjs` that describes fewer fixtures than the
      // suite really runs, which is the thing the report must never be.
      expect(existsSync(REPORT)).toBe(false);
      return;
    }

    // The gate's whole input, derived by driving the corpus rather than by
    // listing it: `ran` below is what came back, and it has to be the manifest's
    // required set exactly — no more (a name that is not in the corpus is a
    // failure) and no less (a missing one is the bar the runner enforces).
    const ran = await driveRequiredFixtures(server.endpoint);
    const required = (await requiredFixtures()).map((fixture) => fixture.name);
    expect([...ran].sort()).toEqual([...required].sort());

    // The report this run leaves behind, and the 100% bar is read from it
    // (`check-languages.mjs --check typescript`, which is what CI runs).
    expect(await writeConformanceReport(ran, TRANSPORT)).toEqual([...ran].sort());

    // From here the suite checks what a client is for, through the public
    // facade. The fixture list above is the same corpus driven the same way; what
    // follows is what each of those answers has to *mean*.
    const { cases } = await corpus();
    const expected = [
      // A successful call, in each encoding a client might pick.
      'instance_get_instance_json',
      'instance_get_instance_proto',
      'instance_get_instance_grpc_web',
      'instance_get_instance_grpc_web_json',
      // A structured-reason error, in each encoding.
      'instance_who_am_i_json',
      'instance_who_am_i_proto',
      'instance_who_am_i_grpc_web',
      'instance_who_am_i_grpc_web_json',
      // The unavailable-service path, unary and on a stream.
      'live_query_json',
      'live_query_proto',
      'live_query_grpc_web',
      'live_query_grpc_web_json',
      'live_watch',
    ];
    expect(cases.map((entry) => entry.name).sort()).toEqual([...expected].sort());

    // A successful call. `GetInstance` needs no auth, which is why it is the
    // first thing any client calls.
    const info = await loams.instance.getInstance({});
    expect(info.name).toBe('Loams');
    expect(info.apiVersions).toContain('loams.instance.v1');
    expect(info.services?.length).toBeGreaterThan(0);

    // A structured-reason error. The reason is what the SDK reads; the message
    // is for a person and is not asserted on.
    const failed = await loams.instance.whoAmI({}).then(
      () => undefined,
      (error: unknown) => error,
    );
    expect(isLoamsError(failed)).toBe(true);
    expect(failed).toMatchObject({ reason: 'not_implemented' });

    // The unavailable-service path, three ways.
    //
    // 1. The guard, from the catalogue, spending no RPC on a call that cannot
    //    work. `loams.live` and `loams.tables` are the same service, so the
    //    guard is asked about the package.
    await expect(loams.system.guard('live')).rejects.toBeInstanceOf(
      FeatureNotInVariantError,
    );
    await expect(loams.system.guard('instance')).resolves.toBeUndefined();

    // 2. The refusal a call gets when the caller skips the guard. This is the
    //    typed surface: the reason is in the union, and the variant is read out
    //    of the metadata rather than parsed out of the message.
    const refused = await loams.tables.query({}).then(
      () => undefined,
      (error: unknown) => error,
    );
    expect(refused).toBeInstanceOf(FeatureNotInVariantError);
    expect(refused).toMatchObject({
      reason: 'feature_not_in_variant',
      variant: 'standard',
    });

    // 3. The refusal on a server stream, which arrives inside the Connect
    //    envelope rather than as an HTTP status. A client that only reads
    //    status codes sees a 200 here, so this is the case that distinguishes a
    //    real Connect implementation from a status-code-only fake.
    const streamed: unknown[] = [];
    await expect(
      (async () => {
        for await (const transition of loams.live.watch({})) {
          streamed.push(transition);
        }
      })(),
    ).rejects.toBeInstanceOf(FeatureNotInVariantError);
    expect(streamed).toHaveLength(0);

    // The catalogue answers the same question the refusals do, from one call.
    const catalogue = await loams.system.catalogue();
    expect(catalogue.served).toContain('loams.instance.v1');
    expect(catalogue.unavailable).toContain('loams.live.v1');
    expect(catalogue.services.find((s) => s.package === 'loams.live.v1')?.unstable).toBe(true);
  });

  it('has all six canonical tests, which the report claims by name', async () => {
    // The six names are the contract two languages are compared by, and the
    // report lists them. A renamed test would otherwise be reported as present,
    // so the report discovers them here: one out of the suite's own sources, one
    // out of the module that owns them.
    expect(await testsThisSuiteHas()).toEqual(REQUIRED_TESTS.map((short) => testName('typescript', short)));
  });

  it('reports the proto revision and the server packages (R9)', async () => {
    const report = await loams.system.version();
    expect(report.protoRev).toBe(loams.protoRev);
    // `loams.live.v1` is served as `unavailable` in the standard variant, and
    // `GetInstance.api_versions` lists only what is served, so a mismatch here
    // is the server's, not the SDK's.
    expect(report.apiVersions).toEqual(['loams.instance.v1']);
    expect(report.compatible).toBe(false);
    expect(report.missing).toContain('loams.live.v1');
  });

  it('is a browser-shaped client: no Node-only import on the default path', async () => {
    // The default entry point must not reach for `node:http2`, or a bundler
    // would follow it and fail. This is a static check because the failure it
    // prevents is a build failure in someone else's project.
    // The check is on imports, not on the word: the prose names `node:http2`
    // to explain what it is kept away from.
    for (const file of [
      '../src/runtime/transports.ts',
      '../src/index.ts',
      '../src/loams.ts',
      '../src/system.ts',
      '../src/runtime/call.ts',
    ]) {
      const source = await readFile(resolve(here, file), 'utf8');
      expect(source, file).not.toMatch(/from ['"]node:/);
      expect(source, file).not.toMatch(/require\(['"]node:/);
    }
    // And the Node transport is behind the `/node` subpath.
    const node = await readFile(resolve(here, '../src/node.ts'), 'utf8');
    expect(node).toContain('@connectrpc/connect-node');
  });
});