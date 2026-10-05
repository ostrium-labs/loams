// The console's WebMCP feature detection, in a real Chrome (AP1d Task 4;
// design §42 §5, D635 and D638).
//
// `test/webmcp.test.ts` covers the module's logic against objects the test
// builds. This file covers the one thing that cannot be built: what a browser
// that actually carries Chrome's WebMCP machinery does to `document.modelContext`.
// That attribute is `[SecureContext]`, so it never exists under jsdom, and a
// jsdom test can only ever prove the console agrees with a fake.
//
// What this file is *not* is a test of WebMCP's present-API path in a browser.
// Chrome's local-development flag exposes `navigator.modelContext` and
// `navigator.modelContextTesting`, not `document.modelContext`; measured on
// Chrome 149.0.7827.155 on 2026-10-05, with `--enable-features=WebMCPTesting`
// and with `--enable-features=WebMCP` too, `document.modelContext` stays
// undefined. Reaching it needs an origin-trial token for the console's origin,
// which CI cannot assume. So the load-bearing assertion here is the absent-API
// one — which is also the path that matters in production, because WebKit's
// standards position on WebMCP is closed and `oppose` (D635).
//
// Every assertion below therefore holds whether or not the flags still work. If
// a future Chrome renames the features, this file keeps passing and its
// diagnostics stop reporting a decoy; the `native` scenario starts exercising
// the real API on its own, and nothing here needs to change.

import assert from 'node:assert/strict';
import { after, before, describe, it } from 'node:test';
import { featureFlags, findChrome, runInChrome } from './harness.mjs';

const chromeBinary = await findChrome();

// On CI a missing browser is a failure, not a skip. A browser job that skips
// itself reports green while having tested nothing, which is the exact failure
// mode this file exists to rule out.
if (!chromeBinary && process.env.CI) {
  throw new Error(
    'No Chrome or Chromium binary was found. This job exists to test a real browser, so it refuses ' +
      'to skip. Install Chrome on the runner image or set LOAMS_CHROME.',
  );
}

describe("the console's WebMCP detection in a real Chrome (D638)", {
  skip: chromeBinary ? false : 'no Chrome or Chromium binary found; set LOAMS_CHROME to run this',
}, () => {
  let run;

  before(async () => {
    run = await runInChrome({ binary: chromeBinary });
    // Loud, because a reader of the CI log should be able to tell which browser
    // and which flags produced the verdicts below without opening the source.
    console.log(`# chrome: ${run.version}`);
    console.log(`# binary: ${run.binary}`);
    console.log(`# flags: ${run.flags.join(' ')}`);
    console.log(`# origin: ${run.origin}`);
  });

  after(async () => {
    await run?.close();
  });

  const absent = () => run.result.absent;
  const present = () => run.result.present;

  it('finishes_every_scenario_in_the_page', () => {
    // First, so a fixture that throws reports as itself rather than as a
    // `TypeError` from whichever assertion read a field it never wrote.
    assert.equal(run.result.ok, true, `the fixture page failed:\n${run.result.error}`);
    for (const scenario of ['absent', 'native', 'present']) {
      assert.ok(run.result[scenario], `the fixture reported no ${scenario} scenario`);
    }
  });

  it('runs_the_page_in_a_secure_context', () => {
    // Load-bearing. `document.modelContext` is `[SecureContext]`; in an insecure
    // page it would be absent for a reason that has nothing to do with the
    // console, and the assertions below would prove nothing.
    assert.equal(absent().secureContext, true);
    assert.match(absent().origin, /^http:\/\/127\.0\.0\.1:\d+$/);
  });

  it('does_not_expose_document_model_context_under_chromes_local_webmcp_flag', () => {
    console.log(`# navigator.modelContext: ${absent().navigatorModelContext}`);
    console.log(
      `# navigator.modelContext methods: ${absent().navigatorModelContextMethods.join(', ')}`,
    );
    console.log(
      `# navigator.modelContextTesting: ${absent().navigatorModelContextTesting} ` +
        `(${absent().navigatorModelContextTestingMethods.join(', ')})`,
    );
    assert.equal(absent().documentModelContext, 'undefined');
  });

  it('reports_not_exposed_from_a_real_browser', () => {
    assert.equal(absent().detectionReason, 'not-exposed');
  });

  it('registers_nothing_and_resolves_undefined_so_the_console_runs_unchanged', () => {
    // D568, on the path D635 says is normal. `undefined`, not a throw and not an
    // empty handle: nothing to unregister, nothing to keep alive, no tools.
    assert.equal(absent().registerResolved, 'undefined');
  });

  it('never_confuses_the_navigator_testing_surfaces_for_the_api', () => {
    // The decoy is the whole reason this job is worth having. A console that
    // feature-detected `navigator.modelContext` — the older spelling, which this
    // Chrome does expose under the flag — would pass a jsdom test and then find
    // nothing in any real browser.
    const decoy =
      absent().navigatorModelContext !== 'undefined' ||
      absent().navigatorModelContextTesting !== 'undefined';
    assert.equal(absent().detectionReason, 'not-exposed');
    if (decoy) {
      console.log('# a navigator decoy was present and was correctly not used');
    } else {
      console.log('# no navigator decoy in this build; the assertion above still holds');
    }
  });

  it('registers_and_unregisters_through_a_spec_shaped_context', () => {
    assert.equal(present().exercised, true);
    assert.equal(present().detectionAvailable, true);
    assert.deepEqual(present().registered, ['loams_probe']);
    assert.deepEqual(present().afterRegister, ['loams_probe']);
    // The registration-time signal unregisters; that is why `clear()` works.
    assert.deepEqual(present().afterClear, []);
    assert.deepEqual(present().errors, []);
    assert.equal(present().clearsTwiceCleanly, true);
  });

  it('exposes_the_console_origin_and_secure_agents_only', () => {
    // A plaintext `http://` agent origin rejects the whole registration with a
    // SecurityError, so it must be dropped before it is ever passed through.
    assert.ok(present().exposedTo.includes(run.origin));
    assert.ok(present().exposedTo.includes('https://agent.example'));
    assert.deepEqual(present().rejected, ['http://plaintext.example']);
    for (const exposedTo of present().exposedToRecordedByTheFixture) {
      assert.ok(!exposedTo.includes('http://plaintext.example'));
    }
  });

  it('keeps_the_working_tools_when_one_registration_fails', () => {
    assert.deepEqual(present().partialRegistered, ['loams_probe']);
    assert.equal(present().partialErrors.length, 1);
    assert.match(present().partialErrors[0], /^loams_probe: /);
    assert.deepEqual(present().partialSurvivors, ['loams_probe']);
    assert.deepEqual(present().partialAfterClear, []);
  });

  it('runs_a_registered_tool_through_the_seam', () => {
    assert.equal(present().executed, 'loams_probe:{"ping":true}');
    assert.deepEqual(present().afterSeamClear, []);
  });

  it('reports_the_native_api_path_without_gating_on_it', () => {
    // Never an assertion. Today it is skipped, because the API is behind an
    // origin trial; when a token exists this starts reporting a real run, and a
    // browser's behaviour must not be able to turn this job red.
    assert.equal(typeof run.result.native.exercised, 'boolean');
    if (!run.result.native.exercised) console.log(`# native path: ${run.result.native.reason}`);
    else console.log(`# native path: ${JSON.stringify(run.result.native)}`);
  });

  it('used_the_configured_webmcp_flags', () => {
    assert.deepEqual(run.flags, featureFlags());
  });
});
