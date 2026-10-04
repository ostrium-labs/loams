// The `loams-apps-mock` harness (SDK1 Task 4, design §44 §10.4).
//
// `loams-apps-mock` is the second conformance target, and the one the real
// server cannot replace. `loams dev` can answer a successful unary call and a
// structured-reason refusal; it cannot produce a **paged** list, a mutation that
// honours an **idempotency key**, or a **resumable** stream, because those RPCs
// do not exist on it yet (design §44 §8, plan [API1] Tasks 2–4). The mock does
// all three for real — its `acceptance` module is the server's own rules — so
// the fixtures those behaviours need come from here (D617's "primary `loams
// dev`, fault injection `loams-apps-mock`").
//
// What this module is for: **one place that boots the mock correctly**, so the
// recorder and the runner do not each reinvent the wait-for-the-port dance. The
// mock prints `loams-apps-mock: http://ADDR` once it is listening and nothing
// before, so that line *is* the readiness signal — there is no log scraping and
// no sleep.
//
// The mock is **stateful**, which is the whole point: an idempotency-key replay
// and a stream resume are only meaningful against state that moved. So the
// harness makes a fresh mock cheap — `startAppsMock()` per scenario — rather
// than trying to reset one server between recordings. A recorded fixture is
// therefore reproducible in isolation, which is what lets CI re-record without
// the whole corpus depending on execution order.
//
// Usage:
//   import { startAppsMock } from './apps-mock.mjs';
//   const mock = await startAppsMock();
//   try { ... } finally { await mock.stop(); }

import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';

/** The loopback prefix the mock binds (D111: it refuses anything else). */
const LOOPBACK = '127.0.0.1';

/**
 * How long to wait for the mock to print its URL.
 *
 * A debug build of the mock links the whole server, so the first run after a
 * `cargo clean` pays tens of seconds of page-in before it prints anything. The
 * timeout is generous for that and fatal only if the mock is genuinely wedged.
 */
const READY_TIMEOUT_MS = 120_000;

/**
 * Locates the `loams-apps-mock` binary.
 *
 * `LOAMS_APPS_MOCK_BIN` wins, so CI can hand in a binary it built once and
 * uploaded, and a developer who already has one does not pay for a second
 * build. Otherwise the binary is taken from cargo's own JSON output, which is
 * the only way to learn where the shared target directory put it: this
 * repository deliberately does not hard-code a target directory, so asking
 * `target/debug/loams-apps-mock` would be wrong on any machine that redirects
 * it.
 *
 * Returns `null` when there is no binary, so a caller can decide whether to
 * skip the app-mock half of the corpus or fail. The runner **fails** on a
 * missing binary for a scenario it is required to record; the fixture *serving*
 * path does not need it at all.
 *
 * @param {{ cwd?: string, env?: NodeJS.ProcessEnv }} [options]
 * @returns {Promise<string | null>}
 */
export async function findAppsMockBin(options = {}) {
  const fromEnv = options.env?.LOAMS_APPS_MOCK_BIN ?? process.env.LOAMS_APPS_MOCK_BIN;
  if (fromEnv) {
    if (!existsSync(fromEnv)) {
      throw new Error(`LOAMS_APPS_MOCK_BIN points at ${fromEnv}, which does not exist`);
    }
    return fromEnv;
  }
  const cwd = options.cwd ?? process.cwd();
  // `--message-format=json` puts one JSON object per compiler line on stdout, so
  // the executable is read out of it rather than guessed at. stderr is dropped:
  // a concurrent agent may hold the cargo lock for minutes and its waiting is
  // not this script's error.
  const proc = spawn(
    'cargo',
    ['build', '-p', 'loams-apps-mock', '--message-format=json', '--quiet'],
    { cwd, stdio: ['ignore', 'pipe', 'ignore'] },
  );
  let out = '';
  proc.stdout.setEncoding('utf8');
  for await (const chunk of proc.stdout) {
    out += chunk;
  }
  const code = await new Promise((resolve) => {
    proc.on('close', resolve);
  });
  if (code !== 0) {
    return null;
  }
  for (const line of out.split('\n')) {
    if (!line.includes('"executable"')) {
      continue;
    }
    try {
      const message = JSON.parse(line);
      const exe = message.executable ?? '';
      if (exe.endsWith('loams-apps-mock')) {
        return exe;
      }
    } catch {
      // A non-JSON line is cargo's own chatter; it is not an executable path.
    }
  }
  return null;
}

/**
 * Starts `loams-apps-mock` on a free loopback port and waits until it answers.
 *
 * The heartbeat is shortened to one second by default: the recorded stream
 * fixtures need a heartbeat between the snapshot and the end of the recorded
 * prefix, and the production 15 s period would make recording take a quarter of
 * a minute per stream for no extra coverage. A suite that wants the production
 * period passes `heartbeatSecs: 15`.
 *
 * @param {object} [options]
 * @param {string} [options.bin] Path to the binary; found with `findAppsMockBin` when absent.
 * @param {number} [options.heartbeatSecs] Heartbeat period of every watch stream.
 * @param {string} [options.cwd] Repository root the binary is built in.
 * @returns {Promise<{ url: string, port: number, heartbeatSecs: number, stop: () => Promise<void> }>}
 */
export async function startAppsMock(options = {}) {
  const bin = options.bin ?? (await findAppsMockBin({ cwd: options.cwd }));
  if (!bin) {
    throw new Error(
      'could not find or build loams-apps-mock; set LOAMS_APPS_MOCK_BIN or run ' +
        '`cargo build -p loams-apps-mock`',
    );
  }
  const heartbeatSecs = options.heartbeatSecs ?? 1;
  const proc = spawn(bin, ['--listen', `${LOOPBACK}:0`, '--heartbeat-secs', String(heartbeatSecs)], {
    cwd: options.cwd ?? process.cwd(),
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  const stderr = [];
  proc.stderr.setEncoding('utf8');
  proc.stderr.on('data', (chunk) => stderr.push(chunk));

  const url = await new Promise((resolve, reject) => {
    let buffered = '';
    const timer = setTimeout(() => {
      cleanup();
      reject(
        new Error(
          `loams-apps-mock did not print its URL within ${READY_TIMEOUT_MS}ms\n${stderr.join('')}`,
        ),
      );
    }, READY_TIMEOUT_MS);
    const onData = (chunk) => {
      buffered += chunk;
      const match = buffered.match(/loams-apps-mock:\s+(http:\/\/\S+)/);
      if (match) {
        cleanup();
        resolve(match[1].trim());
      }
    };
    const onExit = (code) => {
      cleanup();
      reject(new Error(`loams-apps-mock exited with ${code}\n${stderr.join('')}`));
    };
    const onError = (error) => {
      cleanup();
      reject(error);
    };
    function cleanup() {
      clearTimeout(timer);
      proc.stdout.off('data', onData);
      proc.off('exit', onExit);
      proc.off('error', onError);
    }
    proc.stdout.setEncoding('utf8');
    proc.stdout.on('data', onData);
    proc.on('exit', onExit);
    proc.on('error', onError);
  });

  return {
    url,
    port: Number(new URL(url).port),
    heartbeatSecs,
    async stop() {
      if (proc.exitCode !== null || proc.signalCode !== null) {
        return;
      }
      // SIGTERM, then SIGKILL: a watch stream never ends on its own, so a mock
      // with a connection still open does not exit on SIGTERM alone and the
      // next scenario would collide with its port.
      proc.kill('SIGTERM');
      const gone = await Promise.race([
        new Promise((resolve) => proc.once('exit', () => resolve(true))),
        new Promise((resolve) => setTimeout(() => resolve(false), 2_000)),
      ]);
      if (!gone) {
        proc.kill('SIGKILL');
        await new Promise((resolve) => proc.once('exit', resolve));
      }
    },
  };
}

/**
 * The two documented fake credentials (§37 §12).
 *
 * They are exported rather than written into the fixtures so a test that needs
 * a stale session says `STALE` instead of pasting a bearer token, and so there
 * is exactly one place a reader can go to learn that neither string is a
 * secret.
 */
export const TOKENS = {
  /** A session authenticated now: decides `STEP_UP_SESSION` approvals. */
  FRESH: 'Bearer mock-access-usr_omar',
  /** The same principal, authenticated 10 minutes ago: `step_up_required`. */
  STALE: 'Bearer mock-stale-usr_omar',
  /** A fresh session for the requester of the seed approvals, who may not decide. */
  REQUESTER: 'Bearer mock-access-usr_dana',
};