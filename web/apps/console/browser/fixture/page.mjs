// The scenarios the console's WebMCP module runs inside a real Chrome
// (AP1d Task 4; design §42 §5, D635 and D638).
//
// **The browser's answer is an input, not a constant.** Measured 2026-10-05,
// under the same `--enable-features=WebMCPTesting,DevToolsWebMCPSupport`:
//
//   | surface                        | Chrome 149.0.7827.155 | Chrome 154.0.8037.57 |
//   |--------------------------------|-----------------------|-----------------------|
//   | `document.modelContext`        | absent                | **present**           |
//   | `navigator.modelContext`       | present               | absent                |
//   | `navigator.modelContextTesting`| present               | absent                |
//
// So what the job asserts is the **invariant**: the console agrees with what
// the browser actually reports, in whichever direction that is. Asserting either
// single answer would have been a coin flip on the runner's Chrome channel.
//
// Three scenarios, in this order, and the order matters:
//
//  1. `untouched` — the real document, exactly as the browser shipped it. The
//     console is run against whatever `document.modelContext` turns out to be
//     and the result is compared with what the browser reported, not with a
//     hardcoded answer. This is the scenario that carries the assertion.
//  2. `native` — what the real API looked like when there was one. Reporting
//     only: a browser's shape must not be able to turn this job red.
//  3. `seam` — a spec-shaped `ModelContext` on the *real* document, so the
//     registration seam runs against real DOM objects: real `AbortSignal`s, a
//     real `EventTarget`, real promises, and the spec's rule that the
//     registration-time signal unregisters. It is a **fixture**, never Chrome's
//     implementation, and it says so in every field it reports.
//
// Scenario 3 runs on **every** build, native API or not. It is the only way to
// observe what the console hands `registerTool` — no real browser will report
// its own arguments back — and the invariants it covers (only trustworthy
// origins, one abort per tool, idempotent `clear()`, a partial failure keeping
// the working tools) are properties of `src/webmcp/index.ts`, not of any one
// Chrome. On a build that has the native API the fixture shadows it for the
// duration and records that it did, in `shadowedNative`.
//
// Everything below must end up JSON-serialisable: it crosses the CDP boundary
// with `returnByValue`, and anything unserialisable comes back as `undefined`
// and fails a test for the wrong reason.

import { detectModelContext, WebMcpTools } from '/webmcp.js';

/** The one property the harness reads back. Set last, so its presence means done. */
const PROBE = '__loamsWebmcpProbe';

/** Two tools, so "one abort per tool" is distinguishable from "one abort". */
const probeTool = (name = 'loams_probe') => ({
  name,
  description: `A probe tool (${name}), so the seam has something to register.`,
  execute: async (input) => `${name}:${JSON.stringify(input ?? {})}`,
});

/** The method names of an object, ignoring `Object.prototype`. */
function methodsOf(value) {
  if (value === null || value === undefined || typeof value !== 'object') return [];
  const names = new Set();
  let cursor = value;
  while (cursor !== null && cursor !== Object.prototype) {
    for (const name of Object.getOwnPropertyNames(cursor)) {
      if (name !== 'constructor' && typeof value[name] === 'function') names.add(name);
    }
    cursor = Object.getPrototypeOf(cursor);
  }
  return [...names].sort();
}

/**
 * Whether an origin is one the spec would accept in `exposedTo`.
 *
 * Written out rather than imported from the module under test: a fixture that
 * validated with the function it is validating would agree with a wrong
 * implementation. This is the Secure Contexts rule from Task 0, kept short.
 */
function isSecureOrigin(origin) {
  let url;
  try {
    url = new URL(origin);
  } catch {
    return false;
  }
  if (url.protocol === 'https:' || url.protocol === 'wss:' || url.protocol === 'file:') return true;
  if (url.protocol !== 'http:') return false;
  const host = url.hostname.replace(/^\[|\]$/g, '');
  return host === 'localhost' || host.endsWith('.localhost') || /^127(?:\.\d{1,3}){3}$/.test(host);
}

/**
 * A `ModelContext` shaped like the IDL AP1d Task 0 read.
 *
 * It enforces the three rules the console has to live with, and each of them is
 * enforced by *behaviour* rather than by inspection, so a wrong implementation
 * cannot pass by construction:
 *
 *  - an `exposedTo` entry that is not potentially trustworthy rejects the whole
 *    registration with a `SecurityError` — so a plaintext origin that leaked
 *    through would cost every tool, which is what makes it observable;
 *  - the registration-time `signal` **unregisters**; it does not cancel a call
 *    already running (which is why the console keeps one controller per tool);
 *  - a duplicate name rejects with `InvalidStateError`, which is what makes the
 *    partial-failure case reachable without stubbing anything.
 *
 * Every abort is counted, and the deletion is not `{ once: true }`, so a
 * `clear()` that aborted twice would be visible rather than silent.
 */
function specShapedModelContext() {
  const tools = new Map();
  const aborts = new Map();
  const events = new EventTarget();

  const context = {
    async registerTool(tool, options = {}) {
      const exposedTo = options.exposedTo ?? [];
      for (const origin of exposedTo) {
        if (!isSecureOrigin(origin)) {
          throw new DOMException(`exposedTo: "${origin}" is not a secure origin.`, 'SecurityError');
        }
      }
      if (tools.has(tool.name)) {
        throw new DOMException(`"${tool.name}" is already registered.`, 'InvalidStateError');
      }
      const signal = options.signal ?? null;
      tools.set(tool.name, { tool, exposedTo, signal });
      signal?.addEventListener('abort', () => {
        aborts.set(tool.name, (aborts.get(tool.name) ?? 0) + 1);
        tools.delete(tool.name);
      });
      queueMicrotask(() => events.dispatchEvent(new Event('toolchange')));
      return undefined;
    },
    // Same-origin visibility: a bare registration, or one that names this
    // document's origin. Cross-origin reachability is not what this scenario is
    // about, and the console never calls `getTools` anyway.
    async getTools() {
      const own = String(location.origin);
      return [...tools.values()]
        .filter((entry) => entry.exposedTo.length === 0 || entry.exposedTo.includes(own))
        .map((entry) => ({
          name: entry.tool.name,
          description: entry.tool.description,
          origin: own,
        }));
    },
    async executeTool(tool, inputObject, options = {}) {
      const name = typeof tool === 'string' ? tool : tool?.name;
      const entry = tools.get(name);
      if (!entry) throw new DOMException(`"${name}" is not registered.`, 'InvalidStateError');
      return String(
        await entry.tool.execute(inputObject ?? {}, {
          signal: options.signal ?? new AbortController().signal,
        }),
      );
    },
    addEventListener: (type, listener) => events.addEventListener(type, listener),
    removeEventListener: (type, listener) => events.removeEventListener(type, listener),
  };

  return {
    context,
    names: () => [...tools.keys()],
    abortCounts: () => Object.fromEntries([...aborts.entries()].sort()),
    exposedToSeenByRegisterTool: () => [...tools.values()].map((entry) => [...entry.exposedTo]),
  };
}

/**
 * Scenario 1: the document exactly as the browser shipped it.
 *
 * Everything it reports is a **fact about this browser**, and the assertions in
 * `webmcp-detect.test.mjs` are written against those facts rather than against
 * a hardcoded expectation. The detection reason and the registration result are
 * included, and the test requires them to follow `documentModelContext`.
 *
 * The handle is cleared before returning: this scenario registers against
 * whatever the browser really has, and leaving a probe tool registered on a
 * real `document.modelContext` would both leak into later scenarios and be a
 * rude thing to do to the browser it is measuring.
 */
async function untouchedPath() {
  const context = document.modelContext ?? null;
  const detection = detectModelContext(document);
  const handle = await WebMcpTools.register([probeTool()], { doc: document, location });
  const registered = handle === undefined ? [] : handle.registered.map((tool) => tool.name);
  const errors = handle === undefined ? [] : [...handle.errors];
  // `clear()` twice: sign-out and a retry both call it, and on a real browser
  // this must not throw however the API behaves.
  let clearedWithoutThrowing = true;
  try {
    handle?.clear();
    handle?.clear();
  } catch (cause) {
    clearedWithoutThrowing = String(cause?.message ?? cause);
  }
  return {
    secureContext: window.isSecureContext === true,
    origin: String(location.origin),
    documentModelContext: typeof document.modelContext,
    documentModelContextMethods: methodsOf(context),
    documentModelContextIsOwnProperty: Object.hasOwn(document, 'modelContext'),
    navigatorModelContext: typeof navigator.modelContext,
    navigatorModelContextMethods: methodsOf(navigator.modelContext),
    navigatorModelContextTesting: typeof navigator.modelContextTesting,
    navigatorModelContextTestingMethods: methodsOf(navigator.modelContextTesting),
    permissionsPolicyTools: readPermissionsPolicyTools(),
    detectionAvailable: detection.available === true,
    detectionReason: detection.available ? 'available' : detection.reason,
    // The load-bearing identity check: when the API is present, the context the
    // console hands back is `document.modelContext` itself and not either
    // `navigator` spelling. `null` when the API is absent.
    detectionContextIsDocumentAttribute:
      detection.available === true ? detection.context === document.modelContext : null,
    registerResolved: handle === undefined ? 'undefined' : 'a handle',
    registered,
    errors,
    exposedTo: handle === undefined ? [] : [...handle.exposedTo],
    rejected: handle === undefined ? [] : [...handle.rejected],
    clearedWithoutThrowing,
  };
}

function readPermissionsPolicyTools() {
  const features = document.permissionsPolicy?.allowedFeatures;
  if (!features) return 'no-allowedFeatures-map';
  return features.tools === false ? 'denied' : 'allowed';
}

/** Scenario 2: what the real API is, when this build has one. Never asserted. */
function nativePath() {
  const context = document.modelContext ?? null;
  if (typeof document.modelContext !== 'object' || document.modelContext === null) {
    return {
      exercised: false,
      reason:
        'document.modelContext is absent in this build, so there is no real API to report. That is the production path: WebKit standards-positions WebMCP as `oppose` (D635).',
    };
  }
  // `[SameObject]` in the IDL: the attribute must hand back the identical
  // object every time. Compared against a read taken before the method list was
  // walked, so this is a re-read rather than a self-comparison.
  const reread = document.modelContext;
  return {
    exercised: true,
    isChromeImplementation: true,
    constructorName: context.constructor?.name ?? null,
    methods: methodsOf(context),
    sameObject: reread === context,
  };
}

/**
 * Scenario 3: a spec-shaped context on the real document, on every build.
 *
 * When the browser already has a native API this shadows it, which is recorded
 * in `shadowedNative` and asserted, rather than skipped: skipping is what made
 * the previous version of this file assert nothing at all on a build that had
 * the API, and the console's behaviour is identical either way — the module
 * only ever reads `document.modelContext`.
 */
async function seamPath() {
  const hadNative = typeof document.modelContext === 'object' && document.modelContext !== null;
  const { context, names, abortCounts, exposedToSeenByRegisterTool } = specShapedModelContext();
  try {
    Object.defineProperty(document, 'modelContext', {
      value: context,
      configurable: true,
      enumerable: false,
      writable: false,
    });
  } catch (cause) {
    // Only reachable if a future engine makes the attribute non-configurable on
    // the instance. Reported rather than swallowed: the scenario genuinely did
    // not run, and saying so is the honest thing.
    return {
      exercised: false,
      reason: `could not shadow document.modelContext: ${cause?.message ?? cause}`,
    };
  }
  try {
    const detection = detectModelContext(document);
    // The call shape a real caller uses: *agent* origins only. The console adds
    // its own origin by itself, and listing it here as an agent as well put it
    // in `exposedTo` twice — harmless per the spec, but it only became visible
    // once this file stopped comparing `exposedTo` by substring.
    const agents = ['https://agent.example', 'http://plaintext.example'];
    const handle = await WebMcpTools.register([probeTool(), probeTool('loams_probe_two')], {
      doc: document,
      location,
      agents,
    });
    const registered = handle === undefined ? [] : handle.registered.map((tool) => tool.name);
    const afterRegister = names();
    const exposedToSeenBeforeClear = exposedToSeenByRegisterTool();

    handle?.clear();
    const afterClear = names();
    const abortsAfterClear = abortCounts();

    // `clear()` is idempotent: sign-out and a retry both call it, and a second
    // call must not abort a second time.
    handle?.clear();
    const clearsTwiceCleanly = names().length === 0;
    const abortsAfterSecondClear = abortCounts();

    // The tool actually runs. A fresh registration, because `clear()` above
    // unregistered the first one.
    const seam = await WebMcpTools.register([probeTool()], { doc: document, location });
    const executed = await context.executeTool(
      { name: 'loams_probe' },
      { ping: true },
      { signal: new AbortController().signal },
    );
    seam?.clear();
    const afterSeamClear = names();

    // One duplicate name, so the second registration rejects on its own and the
    // first must survive it: a single malformed tool may not cost the rest.
    const partial = await WebMcpTools.register([probeTool(), probeTool()], {
      doc: document,
      location,
    });
    const partialRegistered =
      partial === undefined ? [] : partial.registered.map((tool) => tool.name);
    const partialErrors = partial === undefined ? [] : [...partial.errors];
    const partialSurvivors = names();
    partial?.clear();
    const partialAfterClear = names();

    return {
      exercised: true,
      isSpecShapedFake: true,
      shadowedNative: hadNative,
      detectionAvailable: detection.available === true,
      exposedTo: handle === undefined ? [] : [...handle.exposedTo],
      rejected: handle === undefined ? [] : [...handle.rejected],
      errors: handle === undefined ? [] : [...handle.errors],
      registered,
      afterRegister,
      afterClear,
      abortsAfterClear,
      abortsAfterSecondClear,
      clearsTwiceCleanly,
      executed: String(executed),
      afterSeamClear,
      partialRegistered,
      partialErrors,
      partialSurvivors,
      partialAfterClear,
      exposedToRecordedByTheFixture: exposedToSeenBeforeClear,
    };
  } finally {
    Reflect.deleteProperty(document, 'modelContext');
  }
}

async function run() {
  const probe = {};
  try {
    probe.untouched = await untouchedPath();
    probe.native = nativePath();
    probe.seam = await seamPath();
    probe.ok = true;
    probe.error = null;
  } catch (cause) {
    probe.ok = false;
    probe.error = cause?.stack ?? String(cause);
    document.getElementById('status').textContent = `failed: ${probe.error}`;
  }
  globalThis[PROBE] = probe;
}

await run();
