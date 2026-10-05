// A real Chrome, driven over CDP, for the console's WebMCP detection test
// (AP1d Task 4; design §42 §5, D635 and D638).
//
// There is no browser harness anywhere in this repository, so this is
// deliberately the smallest thing that can answer the one question the vitest
// suite structurally cannot: what does `detectModelContext` do in a browser
// that actually has Chrome's WebMCP machinery switched on? `document.modelContext`
// is `[SecureContext]`, so it never exists under jsdom — every test in
// `test/webmcp.test.ts` runs against an object the test itself built.
//
// Measured on Chrome 149.0.7827.155 on 2026-10-05, with
// `--enable-features=WebMCPTesting,DevToolsWebMCPSupport`: the flag exposes
// `navigator.modelContext` and `navigator.modelContextTesting` and **not**
// `document.modelContext`. So the honest assertion in a real browser is the
// absent-API one — which is also the production path, because WebKit's standards
// position is closed and `oppose` (D635).
//
// Hand-rolled rather than Playwright or Puppeteer, on purpose:
//   - no new dependency, so nothing is downloaded for any other job, and there
//     is no browser binary in the lockfile to keep fresh;
//   - `src/webmcp/index.ts` is loaded through TypeScript's own transpiler, so the
//     test exercises the module as written rather than a bundled copy of it;
//   - only `Target.createTarget` and `Runtime.evaluate` are used, which is the
//     whole of what this test needs.
//
// On the flags: Chrome's own WebMCP page names the local-development switch as
// `chrome://flags/#enable-webmcp-testing`, but neither the `--enable-features=`
// spelling nor the two `navigator` surfaces appear on any primary page, and
// D638 records them as unverified for exactly that reason. They are therefore
// configuration (`LOAMS_WEB_MCP_FLAGS`) and nothing here fails when they stop
// working — the assertion this harness exists for is about the API being
// *absent*, which holds whether or not the flag does anything.

import { spawn } from 'node:child_process';
import { constants as fsConstants } from 'node:fs';
import { access, mkdtemp, readFile, rm } from 'node:fs/promises';
import { createServer } from 'node:http';
import { tmpdir } from 'node:os';
import { delimiter, join } from 'node:path';
import ts from 'typescript';

/** Milliseconds a Chrome launch may take to write its debugging endpoint. */
const LAUNCH_TIMEOUT_MS = 60_000;
/** Milliseconds a single CDP command may take. */
const COMMAND_TIMEOUT_MS = 30_000;
/** Milliseconds the page may take to finish its scenarios. */
const PAGE_TIMEOUT_MS = 60_000;

/** The page sets this once every scenario has run, so the harness can poll it. */
const RESULT_EXPRESSION = 'globalThis.__loamsWebmcpProbe ?? null';

const CHROME_NAMES = [
  'google-chrome-stable',
  'google-chrome',
  'chromium',
  'chromium-browser',
  'chrome',
];

const CHROME_PATHS = [
  '/opt/google/chrome/chrome',
  '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
  'C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe',
  'C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe',
];

/**
 * The Chrome flags the job runs with.
 *
 * `WebMCPTesting` is a Blink runtime feature — confirmed in Chromium's own
 * `third_party/blink/renderer/platform/runtime_enabled_features.json5`, where
 * it is `status: "experimental"` and appears in the `implied_by` list of the
 * origin-trial feature `WebMCP`. That is as far as a primary source takes it:
 * the feature exists, but nothing on a primary page states that enabling it
 * yields `navigator.modelContextTesting` rather than `document.modelContext`,
 * and the measurement above is what settled that.
 *
 * `DevToolsWebMCPSupport` came from a vendor integration guide, could not be
 * confirmed from any primary source, and is measurably inert on Chrome 149
 * (the surface set is byte-identical with and without it). It is kept because
 * removing it would make the job a weaker test if a future Chrome needs it, and
 * D638 says so rather than dropping the claim silently.
 */
export function featureFlags() {
  const configured = process.env.LOAMS_WEB_MCP_FLAGS;
  if (configured !== undefined && configured.trim() !== '') {
    return configured
      .split(',')
      .map((flag) => flag.trim())
      .filter(Boolean);
  }
  return ['--enable-features=WebMCPTesting,DevToolsWebMCPSupport'];
}

/**
 * The Chrome binary to test in, or `undefined` when there is none.
 *
 * `LOAMS_CHROME` wins outright and is an error rather than a fallback when it
 * points at nothing: a browser test that quietly ran against some other browser
 * is worse than one that did not run.
 */
export async function findChrome() {
  const explicit = process.env.LOAMS_CHROME;
  if (explicit !== undefined && explicit !== '') {
    if (await isExecutable(explicit)) return explicit;
    throw new Error(`LOAMS_CHROME is set to "${explicit}", which is not an executable file.`);
  }
  for (const directory of (process.env.PATH ?? '').split(delimiter).filter(Boolean)) {
    for (const name of CHROME_NAMES) {
      const candidate = join(directory, name);
      if (await isExecutable(candidate)) return candidate;
    }
  }
  for (const candidate of CHROME_PATHS) {
    if (await isExecutable(candidate)) return candidate;
  }
  return undefined;
}

async function isExecutable(path) {
  try {
    await access(path, fsConstants.X_OK);
    return true;
  } catch {
    return false;
  }
}

/** The browser's own version string, for the CI log. Never a gate. */
export function chromeVersion(binary) {
  return new Promise((resolve) => {
    const child = spawn(binary, ['--version'], { stdio: ['ignore', 'pipe', 'ignore'] });
    let out = '';
    child.stdout.setEncoding('utf8');
    child.stdout.on('data', (chunk) => {
      out += chunk;
    });
    child.on('error', () => resolve('unknown'));
    child.on('close', () => resolve(out.trim() || 'unknown'));
    setTimeout(() => child.kill(), 10_000).unref?.();
  });
}

/**
 * Serves the fixture and the transpiled `webmcp` module from loopback.
 *
 * Loopback rather than `file:` because the fixture is an ES module, and Chrome
 * refuses module scripts over `file:`. Loopback is also a potentially
 * trustworthy origin, so `isSecureContext` is true and the page is in exactly
 * the context the spec's `[SecureContext]` is about — which is the whole point,
 * since a non-secure page would make the absent-API assertion true for a
 * different reason than the one under test.
 */
async function startServer() {
  const routes = new Map([
    [
      '/',
      { file: new URL('./fixture/index.html', import.meta.url), type: 'text/html; charset=utf-8' },
    ],
    [
      '/page.mjs',
      {
        file: new URL('./fixture/page.mjs', import.meta.url),
        type: 'text/javascript; charset=utf-8',
      },
    ],
    ['/webmcp.js', { body: transpiledWebmcpModule, type: 'text/javascript; charset=utf-8' }],
  ]);
  const server = createServer((request, response) => {
    const method = request.method ?? 'GET';
    const path = (request.url ?? '/').split('?')[0];
    const route = method === 'GET' || method === 'HEAD' ? routes.get(path) : undefined;
    if (!route) {
      response.writeHead(404, { 'content-type': 'text/plain; charset=utf-8' });
      response.end('no such fixture route\n');
      return;
    }
    const send = (body) => {
      response.writeHead(200, { 'content-type': route.type, 'cache-control': 'no-store' });
      response.end(method === 'HEAD' ? undefined : body);
    };
    const fail = (cause) => {
      response.writeHead(500, { 'content-type': 'text/plain; charset=utf-8' });
      response.end(`the fixture failed to load: ${cause?.stack ?? cause}\n`);
    };
    if (route.file) readFile(route.file, 'utf8').then(send, fail);
    else route.body().then(send, fail);
  });
  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', resolve);
  });
  const address = server.address();
  if (address === null || typeof address === 'string') {
    await new Promise((resolve) => server.close(resolve));
    throw new Error('the fixture server did not bind a TCP port');
  }
  return {
    origin: `http://127.0.0.1:${address.port}`,
    close: () => new Promise((resolve) => server.close(resolve)),
  };
}

let webmcpModule;

/**
 * `src/webmcp/index.ts` as an ES module, through TypeScript's transpiler.
 *
 * The module imports nothing, so a transpile is the whole build. It is compiled
 * from source on every run rather than reused from `dist/`, so this test cannot
 * pass against a stale bundle.
 */
async function transpiledWebmcpModule() {
  if (webmcpModule !== undefined) return webmcpModule;
  const source = await readFile(new URL('../src/webmcp/index.ts', import.meta.url), 'utf8');
  const { outputText, diagnostics } = ts.transpileModule(source, {
    fileName: 'webmcp.ts',
    reportDiagnostics: true,
    compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022 },
  });
  const errors = (diagnostics ?? []).filter(
    (diagnostic) => diagnostic.category === ts.DiagnosticCategory.Error,
  );
  if (errors.length > 0) {
    const rendered = errors
      .map((diagnostic) => ts.flattenDiagnosticMessageText(diagnostic.messageText, ' '))
      .join('; ');
    throw new Error(`src/webmcp/index.ts did not transpile: ${rendered}`);
  }
  webmcpModule = outputText;
  return webmcpModule;
}

async function launchChrome(binary, flags) {
  const userDataDir = await mkdtemp(join(tmpdir(), 'loams-webmcp-chrome-'));
  const headless = process.env.LOAMS_CHROME_HEADLESS ?? 'new';
  const args = [
    `--user-data-dir=${userDataDir}`,
    // Port 0: the kernel picks, so a parallel job on the same runner cannot
    // collide with this one.
    '--remote-debugging-port=0',
    '--no-first-run',
    '--no-default-browser-check',
    '--disable-background-networking',
    '--disable-component-update',
    '--disable-dev-shm-usage',
    '--disable-gpu',
    // Ephemeral runners have no user-namespace sandbox available. This profile
    // is a fresh temp directory with nothing in it, and nothing here is ever
    // pointed at a real browser profile.
    '--no-sandbox',
    ...(headless === '0' || headless === 'false' ? [] : [`--headless=${headless}`]),
    ...flags,
    ...splitExtraArgs(process.env.LOAMS_CHROME_EXTRA_ARGS),
  ];
  const child = spawn(binary, args, { stdio: ['ignore', 'ignore', 'pipe'] });
  let stderr = '';
  let failure;
  child.stderr.setEncoding('utf8');
  child.stderr.on('data', (chunk) => {
    stderr = (stderr + chunk).slice(-8192);
  });
  child.once('error', (cause) => {
    failure ??= new Error(`could not start ${binary}: ${cause.message}`);
  });
  child.once('exit', (code, signal) => {
    failure ??= new Error(
      `${binary} exited before it was ready (code ${code}, signal ${signal ?? 'none'}).\n${stderr}`,
    );
  });
  return { child, userDataDir, failure: () => failure };
}

function splitExtraArgs(raw) {
  if (raw === undefined || raw.trim() === '') return [];
  return raw
    .split(' ')
    .map((arg) => arg.trim())
    .filter(Boolean);
}

function delay(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function readDevToolsEndpoint(userDataDir, state) {
  const portFile = join(userDataDir, 'DevToolsActivePort');
  const deadline = Date.now() + LAUNCH_TIMEOUT_MS;
  while (Date.now() < deadline) {
    if (state.failure()) throw state.failure();
    try {
      const text = await readFile(portFile, 'utf8');
      const [port, path] = text.split('\n');
      if (port !== undefined && path !== undefined && port.trim() !== '' && path.trim() !== '') {
        return `ws://127.0.0.1:${port.trim()}${path.trim()}`;
      }
    } catch {
      // Chrome has not written the file yet.
    }
    await delay(50);
  }
  throw new Error(`Chrome did not write DevToolsActivePort within ${LAUNCH_TIMEOUT_MS} ms.`);
}

/** The slice of the DevTools protocol this harness needs, over Node's WebSocket. */
function connect(webSocketUrl) {
  const socket = new WebSocket(webSocketUrl);
  const pending = new Map();
  const handlers = new Set();
  let nextId = 1;
  let broken;

  const failPending = (cause) => {
    for (const entry of pending.values()) {
      clearTimeout(entry.timer);
      entry.reject(cause);
    }
    pending.clear();
  };

  socket.addEventListener('message', (event) => {
    let message;
    try {
      message = JSON.parse(typeof event.data === 'string' ? event.data : String(event.data));
    } catch {
      return;
    }
    const entry = message.id === undefined ? undefined : pending.get(message.id);
    if (entry) {
      pending.delete(message.id);
      clearTimeout(entry.timer);
      if (message.error)
        entry.reject(new Error(`${message.error.message} (CDP ${message.error.code})`));
      else entry.resolve(message.result);
      return;
    }
    for (const handler of handlers) handler(message);
  });

  socket.addEventListener('close', () => {
    broken ??= new Error('the CDP connection closed');
    failPending(broken);
  });
  socket.addEventListener('error', () => {
    broken ??= new Error(`the CDP connection to ${webSocketUrl} failed`);
    failPending(broken);
  });

  const opened = new Promise((resolve, reject) => {
    socket.addEventListener('open', resolve, { once: true });
    socket.addEventListener('error', () => reject(new Error(`could not open ${webSocketUrl}`)), {
      once: true,
    });
  });

  return {
    opened,
    on(handler) {
      handlers.add(handler);
      return () => handlers.delete(handler);
    },
    send(method, params = {}, sessionId) {
      if (broken) return Promise.reject(broken);
      const id = nextId;
      nextId += 1;
      return new Promise((resolve, reject) => {
        const timer = setTimeout(() => {
          pending.delete(id);
          reject(new Error(`CDP ${method} timed out after ${COMMAND_TIMEOUT_MS} ms`));
        }, COMMAND_TIMEOUT_MS);
        pending.set(id, { resolve, reject, timer });
        const frame = { id, method, params };
        if (sessionId !== undefined) frame.sessionId = sessionId;
        socket.send(JSON.stringify(frame));
      });
    },
    close() {
      try {
        socket.close();
      } catch {
        // Already closed.
      }
    },
  };
}

async function awaitPageResult(cdp, sessionId, exceptions) {
  const deadline = Date.now() + PAGE_TIMEOUT_MS;
  let lastProblem = 'the page never reported a result';
  let polls = 0;
  while (Date.now() < deadline) {
    try {
      const evaluated = await cdp.send(
        'Runtime.evaluate',
        { expression: RESULT_EXPRESSION, returnByValue: true, awaitPromise: false },
        sessionId,
      );
      const value = evaluated?.result?.value;
      if (value !== null && value !== undefined) return value;
      lastProblem = 'the page reported nothing yet';
    } catch (cause) {
      // The execution context does not exist until the page commits; keep
      // polling rather than failing the first poll.
      lastProblem = cause.message;
    }
    polls += 1;
    // An uncaught exception means the module never finished evaluating, so the
    // probe will never be set. Waiting out the full timeout would turn a typo
    // in the fixture into a minute of CI for the same answer.
    if (exceptions.length > 0 && polls > 3) {
      throw new Error(
        `the fixture page threw before it reported a result:\n${exceptions.join('\n')}`,
      );
    }
    await delay(100);
  }
  const recorded = exceptions.length > 0 ? `\nuncaught in the page:\n${exceptions.join('\n')}` : '';
  throw new Error(`${lastProblem} after ${PAGE_TIMEOUT_MS} ms.${recorded}`);
}

/**
 * Runs the fixture page in a real Chrome and returns what it found.
 *
 * The caller owns `close()`; every path that can throw closes the server and
 * the temp profile on the way out, so a failed run does not leave a browser
 * behind.
 */
export async function runInChrome(options = {}) {
  const binary = options.binary ?? (await findChrome());
  if (!binary) {
    throw new Error('no Chrome or Chromium binary was found; set LOAMS_CHROME to point at one.');
  }
  const version = options.version ?? (await chromeVersion(binary));
  const flags = options.flags ?? featureFlags();
  const site = await startServer();
  let state;
  let cdp;
  try {
    state = await launchChrome(binary, flags);
    const endpoint = await readDevToolsEndpoint(state.userDataDir, state);
    cdp = connect(endpoint);
    await cdp.opened;
    const exceptions = [];
    cdp.on((message) => {
      if (message.method !== 'Runtime.exceptionThrown') return;
      const details = message.params?.exceptionDetails;
      exceptions.push(
        details?.exception?.description ?? details?.text ?? 'an unknown page exception',
      );
    });
    const { targetId } = await cdp.send('Target.createTarget', { url: site.origin });
    const { sessionId } = await cdp.send('Target.attachToTarget', { targetId, flatten: true });
    await cdp.send('Runtime.enable', {}, sessionId);
    const result = await awaitPageResult(cdp, sessionId, exceptions);
    return {
      binary,
      version,
      flags,
      origin: site.origin,
      exceptions,
      result,
      close: () => teardown(state, cdp, site),
    };
  } catch (cause) {
    // `cdp` is passed on even when it is half-built, so a failure after the
    // socket opened can still send `Browser.close` instead of waiting out the
    // SIGKILL timer. Cleanup is wrapped so that a failure *in cleanup* — `rm` on
    // a profile the runner has already reaped, most likely — cannot replace the
    // original cause, which is the one that says what actually went wrong.
    try {
      if (state) await teardown(state, cdp, site);
      else {
        if (cdp) cdp.close();
        await site.close();
      }
    } catch {
      // Keep the original failure.
    }
    throw cause;
  }
}

async function teardown(state, cdp, site) {
  if (cdp) {
    // `Browser.close` is on the browser session, so no sessionId.
    await cdp.send('Browser.close').catch(() => undefined);
    cdp.close();
  }
  await new Promise((resolve) => {
    if (state.child.exitCode !== null || state.child.signalCode !== null) {
      resolve();
      return;
    }
    const timer = setTimeout(() => {
      state.child.kill('SIGKILL');
      resolve();
    }, 5_000);
    state.child.once('exit', () => {
      clearTimeout(timer);
      resolve();
    });
  });
  await rm(state.userDataDir, { recursive: true, force: true });
  await site.close();
}
