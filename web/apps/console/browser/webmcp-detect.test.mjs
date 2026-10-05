// The console's WebMCP feature detection, in a real Chrome (AP1d Task 4;
// design §42 §5, D635 and D638).
//
// `test/webmcp.test.ts` covers the module's logic against objects the test
// builds. This file covers the one thing that cannot be built: what a browser
// that actually carries Chrome's WebMCP machinery does to `document.modelContext`.
// That attribute is `[SecureContext]`, so it never exists under jsdom, and a
// jsdom test can only ever prove the console agrees with a fake.
//
// **This file asserts the invariant, not one browser's answer.** Measured
// 2026-10-05, under the same `--enable-features=WebMCPTesting,DevToolsWebMCPSupport`:
//
//   | surface                         | Chrome 149.0.7827.155 | Chrome 154.0.8037.57 |
//   |---------------------------------|-----------------------|-----------------------|
//   | `document.modelContext`         | absent                | present               |
//   | `navigator.modelContext`        | present               | absent                |
//   | `navigator.modelContextTesting` | present               | absent                |
//
// The availability of the real API moves with the Chrome channel, and the decoy
// moves the other way, so pinning either one would have been a coin flip on the
// runner image. So the load-bearing assertion is: **whatever the browser
// reports, the console agrees with it.** When `document.modelContext` is absent
// that means `not-exposed` and a `register` that resolves `undefined`; when it
// is present, detection is available, `register` returns a handle, and the
// handle's `exposedTo` carries only potentially-trustworthy origins. Both
// directions are asserted; neither is assumed.
//
// Three further properties hold unconditionally, which is what makes the job
// worth running on a channel that keeps moving:
//
//  - the `navigator.modelContext` / `navigator.modelContextTesting` decoys are
//    **never** mistaken for `document.modelContext`. On 149 the decoys are
//    present and the API is not; on 154 the API is present and the decoys are
//    not. Each build therefore rules out one half of that mistake, and the
//    identity check `detection.context === document.modelContext` rules out the
//    other half on any build that has the API;
//  - a plaintext `http://` origin never reaches `registerTool`;
//  - `clear()` is idempotent, one abort per registered tool, and a partial
//    registration failure keeps the working tools.
//
// Every assertion holds whether or not the flags still work. If a future Chrome
// renames the features, this file keeps passing and its diagnostics change —
// the point of the scenario-by-scenario reporting at the top is that a channel
// change is visible in the log rather than only as a red build.

import assert from 'node:assert/strict';
import { after, before, describe, it } from 'node:test';
import { featureFlags, findChrome, runInChrome } from './harness.mjs';

const chromeBinary = await findChrome();

// On CI a missing browser is a failure, not a skip. A browser job that skips
// itself reports green while having tested nothing, which is the exact failure
// mode this file exists to rule out. Compared strictly, because GitHub sets
// `CI=true` and a developer's `CI=0` must not turn a local run into a failure.
if (!chromeBinary && process.env.CI === 'true') {
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
  });

  after(async () => {
    await run?.close();
  });

  const untouched = () => run.result.untouched;
  const seam = () => run.result.seam;
  /** Whether this build actually ships `document.modelContext`. */
  const native = () => untouched().documentModelContext === 'object';

  /**
   * Whether the Secure Contexts rule would accept an origin in `exposedTo`.
   *
   * Written out here rather than imported from `src/webmcp/index.ts`, for the
   * same reason `fixture/page.mjs` writes its own: an assertion that validates
   * with the function under test agrees with a wrong implementation.
   */
  const trustworthy = (origin) => {
    const url = new URL(origin);
    if (url.protocol === 'https:' || url.protocol === 'wss:' || url.protocol === 'file:')
      return true;
    if (url.protocol !== 'http:') return false;
    const host = url.hostname.replace(/^\[|\]$/g, '');
    return (
      host === 'localhost' ||
      host.endsWith('.localhost') ||
      /^127(?:\.\d{1,3}){3}$/.test(host) ||
      host === '::1'
    );
  };

  it('finishes_every_scenario_in_the_page', () => {
    // First, so a fixture that throws reports as itself rather than as a
    // `TypeError` from whichever assertion read a field it never wrote.
    assert.equal(run.result.ok, true, `the fixture page failed:\n${run.result.error}`);
    for (const scenario of ['untouched', 'native', 'seam']) {
      assert.ok(run.result[scenario], `the fixture reported no ${scenario} scenario`);
    }
  });

  it('runs_the_page_in_a_secure_context', () => {
    // Load-bearing. `document.modelContext` is `[SecureContext]`; in an insecure
    // page it would be absent for a reason that has nothing to do with the
    // console, and the assertions below would prove nothing.
    assert.equal(untouched().secureContext, true);
    assert.match(untouched().origin, /^http:\/\/127\.0\.0\.1:\d+$/);
  });

  it('reports_the_browsers_own_answer_as_diagnostics', () => {
    // Never an assertion on the values themselves: this exists so that a Chrome
    // channel change shows up in the CI log rather than only as a red build.
    const u = untouched();
    console.log(`# chrome: ${run.version}`);
    console.log(`# binary: ${run.binary}`);
    console.log(`# flags: ${run.flags.join(' ')}`);
    console.log(`# origin: ${u.origin}`);
    console.log(`# typeof document.modelContext: ${u.documentModelContext}`);
    console.log(
      `# document.modelContext methods: ${u.documentModelContextMethods.join(', ') || 'none'}`,
    );
    console.log(`# typeof navigator.modelContext: ${u.navigatorModelContext}`);
    console.log(
      `# navigator.modelContext methods: ${u.navigatorModelContextMethods.join(', ') || 'none'}`,
    );
    console.log(
      `# typeof navigator.modelContextTesting: ${u.navigatorModelContextTesting}` +
        ` (${u.navigatorModelContextTestingMethods.join(', ') || 'none'})`,
    );
    console.log(`# permissions policy "tools": ${u.permissionsPolicyTools}`);
    console.log(`# native model context: ${JSON.stringify(run.result.native)}`);
    assert.equal(typeof u.documentModelContext, 'string');
  });

  it('agrees_with_the_browser_about_whether_the_api_is_there', () => {
    // THE invariant, in both directions. D638 is not a claim that the API is
    // absent; it is a claim that the console's answer tracks the browser's.
    const u = untouched();
    if (native()) {
      // Present: detection is available, the context it returns is
      // `document.modelContext` itself, and a handle comes back.
      assert.equal(u.detectionReason, 'available');
      assert.equal(u.detectionAvailable, true);
      assert.equal(u.detectionContextIsDocumentAttribute, true);
      assert.equal(u.registerResolved, 'a handle');
    } else {
      // Absent: D568's path, and with WebKit's position `oppose` (D635) the
      // production one. `undefined`, not a throw and not an empty handle:
      // nothing to unregister, nothing to keep alive, no tools.
      assert.equal(u.detectionReason, 'not-exposed');
      assert.equal(u.registerResolved, 'undefined');
    }
  });

  it('never_confuses_the_navigator_testing_surfaces_for_the_api', () => {
    // The decoy is the reason this job is worth having. A console that
    // feature-detected `navigator.modelContext` — the older spelling, which
    // Chrome 149 exposes under the flag — would pass a jsdom test and then find
    // nothing in any real browser.
    //
    // On 149 the decoys are present and the API is not, so this build rules out
    // a detection that reports "available" off `navigator` alone. On 154 the
    // API is present and the decoys are not, and the identity check in the test
    // above rules out a detection that returned `navigator`'s object under the
    // document's name. Both halves, whichever channel runs the job.
    const u = untouched();
    const decoyPresent =
      u.navigatorModelContext !== 'undefined' || u.navigatorModelContextTesting !== 'undefined';
    if (decoyPresent) {
      console.log('# a navigator decoy is present in this build and was correctly not used');
      assert.equal(
        u.detectionContextIsDocumentAttribute,
        null,
        'the decoy must not be what detection resolved to',
      );
    } else {
      console.log('# no navigator decoy in this build; the assertion above still holds');
    }
    // Either way: the answer follows the document attribute and nothing else.
    assert.equal(u.detectionAvailable, native());
    assert.equal(u.detectionReason, native() ? 'available' : 'not-exposed');
  });

  it('clears_without_throwing_on_a_real_document', () => {
    // Sign-out calls `clear()`, and a retry calls it again. Whatever the
    // browser's API actually is, that must not throw.
    assert.equal(untouched().clearedWithoutThrowing, true);
  });

  it('runs_the_seam_against_a_spec_shaped_context_on_the_real_document', () => {
    // The seam runs on every build, shadowing the native API when there is one.
    // The previous version of this file refused to shadow a real API and so
    // asserted nothing at all on any build that had one — which is exactly the
    // build CI runs. What it shadowed is recorded rather than assumed.
    assert.equal(seam().exercised, true, `the seam scenario did not run: ${seam().reason}`);
    assert.equal(seam().isSpecShapedFake, true);
    assert.equal(seam().shadowedNative, native());
    assert.equal(seam().detectionAvailable, true);
    assert.deepEqual(seam().registered, ['loams_probe', 'loams_probe_two']);
    assert.deepEqual(seam().afterRegister, ['loams_probe', 'loams_probe_two']);
    assert.deepEqual(seam().errors, []);
  });

  it('exposes_the_console_origin_and_secure_agents_only', () => {
    // A plaintext `http://` agent origin rejects the whole registration with a
    // SecurityError, so it must be dropped before it is ever passed through.
    // Compared as parsed origins rather than by substring, because a substring
    // check would also accept `https://agent.example.attacker.test` — which is
    // the mistake CodeQL flagged here as an incomplete URL substring
    // sanitization, and a real one to make in a security check.
    assert.deepEqual([...seam().exposedTo].sort(), [run.origin, 'https://agent.example'].sort());
    assert.deepEqual(seam().rejected, ['http://plaintext.example']);
    for (const recorded of seam().exposedToRecordedByTheFixture) {
      for (const origin of recorded) {
        assert.ok(trustworthy(origin), `${origin} is not a potentially-trustworthy origin`);
      }
      assert.deepEqual([...recorded].sort(), [...seam().exposedTo].sort());
    }
    // And nothing untrustworthy was ever offered to `registerTool`.
    for (const recorded of seam().exposedToRecordedByTheFixture) {
      assert.equal(recorded.length, 2);
    }
  });

  it('unregisters_one_abort_per_tool_and_stays_idempotent', () => {
    // The registration-time signal *unregisters*; it does not cancel a call
    // already running. So sign-out needs one abort per tool, not one global
    // flag, and a second `clear()` must not abort a second time.
    assert.deepEqual(seam().afterClear, []);
    assert.deepEqual(seam().abortsAfterClear, { loams_probe: 1, loams_probe_two: 1 });
    assert.equal(seam().clearsTwiceCleanly, true);
    assert.deepEqual(seam().abortsAfterSecondClear, { loams_probe: 1, loams_probe_two: 1 });
  });

  it('keeps_the_working_tools_when_one_registration_fails', () => {
    assert.deepEqual(seam().partialRegistered, ['loams_probe']);
    assert.equal(seam().partialErrors.length, 1);
    assert.match(seam().partialErrors[0], /^loams_probe: /);
    assert.deepEqual(seam().partialSurvivors, ['loams_probe']);
    assert.deepEqual(seam().partialAfterClear, []);
  });

  it('runs_a_registered_tool_through_the_seam', () => {
    assert.equal(seam().executed, 'loams_probe:{"ping":true}');
    assert.deepEqual(seam().afterSeamClear, []);
  });

  it('reports_the_native_api_path_without_gating_on_it', () => {
    // Never an assertion on the shape. A browser's implementation is not the
    // console's, and must not be able to turn this job red.
    assert.equal(typeof run.result.native.exercised, 'boolean');
    if (!run.result.native.exercised) console.log(`# native path: ${run.result.native.reason}`);
    else console.log(`# native path: ${JSON.stringify(run.result.native)}`);
  });

  it('used_the_configured_webmcp_flags', () => {
    assert.deepEqual(run.flags, featureFlags());
  });
});
