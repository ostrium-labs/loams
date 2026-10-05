// The scenarios the console's WebMCP module runs inside a real Chrome
// (AP1d Task 4; design §42 §5, D635 and D638).
//
// Three scenarios, in this order, and the order matters:
//
//  1. `absent` — the untouched document. This is the assertion the job exists
//     for. Chrome's local-development WebMCP flag turns on
//     `navigator.modelContext` and `navigator.modelContextTesting` and not
//     `document.modelContext`, so `detectModelContext` must report
//     `not-exposed` and `WebMcpTools.register` must resolve `undefined`. That is
//     D568's absent path, and per D635 it is the *normal* one: WebKit's
//     standards position on WebMCP is closed and `oppose`.
//  2. `native` — the real API, if this browser happens to expose it. It cannot
//     be reached today without an origin-trial token for the console's origin,
//     which CI does not have, so this reports why it was skipped instead of
//     pretending to have run. It is written against the surface rather than
//     against a flag so that it starts exercising the real thing the day a
//     token exists, with no change here.
//  3. `present` — a spec-shaped `document.modelContext` installed on the *real*
//     document, so the registration seam runs against real DOM objects: real
//     `AbortSignal`s, a real `EventTarget`, real promises, and the spec's rule
//     that the registration-time signal unregisters. It is a fixture, not
//     Chrome's implementation, and it is labelled as one wherever it is
//     reported. Its value is that it can be trusted about the *console's*
//     behaviour while still being real about the browser around it.
//
// Everything below must end up JSON-serialisable: it crosses the CDP boundary
// with `returnByValue`, and anything unserialisable comes back as `undefined`
// and fails a test for the wrong reason.

import { detectModelContext, WebMcpTools } from '/webmcp.js';

/** The one property the harness reads back. Set last, so its presence means done. */
const PROBE = '__loamsWebmcpProbe';

const probeTool = () => ({
  name: 'loams_probe',
  description: 'A probe tool, so the seam has something to register.',
  execute: async (input) => `loams_probe:${JSON.stringify(input ?? {})}`,
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
 * validates with the function it is validating would agree with a wrong
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
 * A `ModelContext` shaped like the IDL AP1d Task 0 read, for the present-API
 * scenario. It enforces the two rules the console has to live with:
 *
 *  - an `exposedTo` entry that is not potentially trustworthy rejects the whole
 *    registration with a `SecurityError`;
 *  - the registration-time `signal` **unregisters**; it does not cancel a call
 *    already running (which is why the console keeps one controller per tool).
 *
 * A duplicate name rejects with `InvalidStateError`, which is what makes the
 * partial-failure case reachable without stubbing anything.
 */
function specShapedModelContext() {
  const tools = new Map();
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
      signal?.addEventListener('abort', () => tools.delete(tool.name), { once: true });
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

  return { context, names: () => [...tools.keys()], entries: () => tools };
}

/** Scenario 1: the untouched document, which is the production case. */
function absentPath() {
  const detection = detectModelContext(document);
  return WebMcpTools.register([probeTool()], { doc: document, location }).then((handle) => ({
    secureContext: window.isSecureContext === true,
    origin: String(location.origin),
    documentModelContext: typeof document.modelContext,
    navigatorModelContext: typeof navigator.modelContext,
    navigatorModelContextMethods: methodsOf(navigator.modelContext),
    navigatorModelContextTesting: typeof navigator.modelContextTesting,
    navigatorModelContextTestingMethods: methodsOf(navigator.modelContextTesting),
    documentPermissionsPolicy: typeof document.permissionsPolicy,
    detectionReason: detection.available ? 'available' : detection.reason,
    registerResolved: handle === undefined ? 'undefined' : 'a handle',
  }));
}

/** Scenario 2: the real API, when a build actually exposes it. */
async function nativePath() {
  if (typeof document.modelContext !== 'object' || document.modelContext === null) {
    return {
      exercised: false,
      reason:
        "document.modelContext is absent, which is the production path: the API is behind an origin trial and CI has no token for the console origin. Chrome's local-development flag exposes navigator.modelContext and navigator.modelContextTesting instead.",
    };
  }
  try {
    const context = document.modelContext;
    const handle = await WebMcpTools.register([probeTool()], { doc: document, location });
    const registered = handle === undefined ? [] : handle.registered.map((tool) => tool.name);
    const tools = await context.getTools();
    handle?.clear();
    return {
      exercised: true,
      registered,
      visibleToGetTools: tools.map((tool) => tool.name),
      errors: handle === undefined ? [] : [...handle.errors],
    };
  } catch (cause) {
    // Reported, never asserted: a native run must not be able to turn this job
    // red on a browser's behaviour rather than on the console's.
    return { exercised: false, reason: `the native API threw: ${cause?.message ?? cause}` };
  }
}

/** Scenario 3: a spec-shaped context on the real document. */
async function presentPath() {
  if (typeof document.modelContext === 'object' && document.modelContext !== null) {
    return {
      exercised: false,
      reason: 'document.modelContext is native in this build; the fixture does not shadow it.',
    };
  }
  const { context, names, entries } = specShapedModelContext();
  Object.defineProperty(document, 'modelContext', {
    value: context,
    configurable: true,
    enumerable: false,
    writable: false,
  });
  try {
    const detection = detectModelContext(document);
    const agents = [String(location.origin), 'https://agent.example', 'http://plaintext.example'];
    const handle = await WebMcpTools.register([probeTool()], { doc: document, location, agents });
    const registered = handle === undefined ? [] : handle.registered.map((tool) => tool.name);
    const afterRegister = names();
    handle?.clear();
    const afterClear = names();
    // `clear()` is idempotent: sign-out can call it twice.
    const clearsTwiceCleanly = (() => {
      handle?.clear();
      return true;
    })();

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
      detectionAvailable: detection.available === true,
      exposedTo: handle === undefined ? [] : [...handle.exposedTo],
      rejected: handle === undefined ? [] : [...handle.rejected],
      errors: handle === undefined ? [] : [...handle.errors],
      registered,
      afterRegister,
      afterClear,
      clearsTwiceCleanly,
      executed: String(executed),
      afterSeamClear,
      partialRegistered,
      partialErrors,
      partialSurvivors,
      partialAfterClear,
      exposedToRecordedByTheFixture: [...entries().values()].map((entry) => entry.exposedTo),
    };
  } finally {
    Reflect.deleteProperty(document, 'modelContext');
  }
}

async function run() {
  const probe = {};
  try {
    probe.absent = await absentPath();
    probe.native = await nativePath();
    probe.present = await presentPath();
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
