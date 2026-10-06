// The required-fixture rule (design §44 §10.4, D617).
//
// **This file is the only definition of it.** `run.sh`, `run-test.sh` and
// `run-all.sh` all import this and none of them re-states the rule, because the
// plan's global constraint ("a language cannot skip a required fixture;
// `transport:grpc-only` fixtures skip only on the Connect-unary fallback") has
// already been written down three times in three languages' worth of prose and
// drifted each time. Prose is not enforcement.
//
// ## What the rule is
//
// A language is publishable only when it has run **every** fixture in
// `manifest.json` with `required: true`. There is exactly one permitted skip:
//
//   a fixture marked `transport: grpc-only`, on the Connect-unary fallback
//   transport (D613 — Ruby and PHP hosts without a native gRPC extension).
//
// Everything else fails. In particular:
//
// - a **missing** fixture is a failure, not a warning: a corpus entry with no
//   file behind it means a language is being excused for a test nobody wrote;
// - an **unknown** fixture name in a language's report is a failure, because a
//   typo in a suite that silently reduces coverage is worse than no suite;
// - a fixture the language reports as skipped for any other reason is a
//   failure, whatever the reason text says.
//
// ## How a language reports
//
// A suite writes `sdks/fixtures/results/<language>.json`:
//
//   { "tests": [...], "ran": [...], "skipped": [{ "fixture": "…", "reason": "…" }] }
//
// `ran` is the list of fixture names the suite actually exercised. It is the
// only thing that counts; a test that passes without touching a required
// fixture has not run it. The runner compares `ran` against `required` and
// exits non-zero on the difference.

import { readFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));

/** The fixture corpus root. */
export const FIXTURES_DIR = join(here, '..', 'fixtures');

/**
 * How to run one of a language's six named tests.
 *
 * `verified: true` means the command has been run against this repository and
 * works. `verified: false` means it is the command that language's task names,
 * written down so the runner has one place to look — and **the runner refuses to
 * run it**, because a runner that executes a command nobody has executed turns a
 * missing SDK into a confusing CI failure instead of an honest "not wired up
 * yet". Each language's task flips its own flag when its suite is green.
 *
 * The command is `[program, args...]` and is run in `dir`, with the fixture
 * endpoint already in the environment. `-t <name>` filters by test name in every
 * runner this table covers, which is what makes one test runnable in isolation
 * without the other five.
 */
export const LANGUAGES = {
  typescript: {
    // The SDK is a package in the **web** pnpm workspace, not a workspace of its
    // own: that is how `@loams/client` resolves `@loams/proto` and `@loams/live`
    // and shares a lockfile with the console that already consumes them (see
    // `web/pnpm-workspace.yaml`). So the command runs from `web` and filters by
    // package, and running it from `sdks/typescript` fails with a workspace
    // resolution error that says nothing about conformance.
    dir: 'web',
    /**
     * Where the suite's **source** is, which is not where it runs from.
     *
     * The drift check reads the sources to see which tests and fixtures a
     * language names. Pointing it at `dir` would scan the console instead and
     * report a green TypeScript SDK as testing nothing.
     */
    sources: 'sdks/typescript',
    verified: true,
    runOne: ['pnpm', ['--filter', '@loams/client', 'exec', 'vitest', 'run', '-t']],
    runAll: ['pnpm', ['--filter', '@loams/client', 'test']],
  },
  python: {
    dir: 'sdks/python',
    verified: false,
    runOne: ['python', ['-m', 'pytest', '-k']],
    runAll: ['python', ['-m', 'pytest']],
  },
  go: {
    dir: 'sdks/go',
    verified: false,
    runOne: ['go', ['test', '-run']],
    runAll: ['go', ['test', './...']],
  },
  rust: {
    dir: 'sdks/rust',
    verified: false,
    runOne: ['cargo', ['test', '--', '--exact']],
    runAll: ['cargo', ['test']],
  },
  swift: {
    dir: 'sdks/swift',
    verified: false,
    runOne: ['swift', ['test', '--filter']],
    runAll: ['swift', ['test']],
  },
  kotlin: {
    dir: 'sdks/kotlin',
    verified: false,
    runOne: ['./gradlew', ['test', '--tests']],
    runAll: ['./gradlew', ['test']],
  },
  java: {
    dir: 'sdks/java',
    verified: false,
    // Not Gradle: neither mvn nor gradle is installed on the machines this SDK
    // is developed on, so sdks/java/build.sh is plain javac/java plus curl and
    // a JDK is the only requirement. `./gradlew test` named a command that does
    // not exist in this SDK, so the entry could only ever have failed if a CI
    // job ever started running it.
    runOne: ['./build.sh', ['test', '-t']],
    runAll: ['./build.sh', ['test']],
  },
  csharp: {
    dir: 'sdks/csharp',
    verified: false,
    runOne: ['dotnet', ['test', '--filter']],
    runAll: ['dotnet', ['test']],
  },
  dart: {
    dir: 'sdks/dart',
    verified: false,
    runOne: ['dart', ['test', '--name']],
    runAll: ['dart', ['test']],
  },
  ruby: {
    dir: 'sdks/ruby',
    verified: false,
    runOne: ['bundle', ['exec', 'rake', 'test:run', 'TESTOPTS=--name']],
    runAll: ['bundle', ['exec', 'rake', 'test']],
  },
  php: {
    dir: 'sdks/php',
    verified: false,
    runOne: ['vendor', ['bin', 'phpunit', '--filter']],
    runAll: ['vendor', ['bin', 'phpunit']],
  },
  cpp: {
    dir: 'sdks/cpp',
    sources: 'sdks/cpp',
    verified: true,
    runOne: ['ctest', ['--test-dir', 'build', '-R']],
    runAll: ['ctest', ['--test-dir', 'build', '--output-on-failure']],
  },
  objc: {
    dir: 'sdks/objc',
    verified: false,
    runOne: ['swift', ['test', '--filter']],
    runAll: ['swift', ['test']],
  },
};

/**
 * The six tests every language's suite has to have, in the order the plan names
 * them (SDK2 Task 0 and every task after it).
 *
 * Kept as data so `run-all.sh` can report "three of six present" instead of
 * "the runner gave up", and so a language that renames a test is caught by name
 * rather than by a suite that silently runs nothing.
 */
export const REQUIRED_TESTS = [
  'conformance_all_required_fixtures',
  'retry_reuses_idempotency_key',
  'error_reason_mapping',
  'stream_resume_with_cursor',
  'token_source_refresh',
  'pagination_iterator',
];

/** The full name of one of a language's six tests. */
export function testName(language, short) {
  return `${language}_${short}`;
}

/** Reads `manifest.json`, the authority on the 100% bar. */
export async function loadManifest(dir = FIXTURES_DIR) {
  const manifest = JSON.parse(await readFile(join(dir, 'manifest.json'), 'utf8'));
  const byName = new Map(manifest.fixtures.map((fixture) => [fixture.name, fixture]));
  return { manifest, byName };
}

/** Reads `faults.json`, the injected-fault catalogue. */
export async function loadFaults(dir = FIXTURES_DIR) {
  return JSON.parse(await readFile(join(dir, 'faults.json'), 'utf8'));
}

/**
 * Whether a skip is allowed, and why not when it is not.
 *
 * @param {{name: string, transport?: string, grpcOnly?: boolean}} fixture
 * @param {{transport?: string}} context The transport the language ran on.
 * @returns {{ok: true, clause: string} | {ok: false, why: string}}
 */
export function maySkip(fixture, context = {}) {
  const onFallback = context.transport === 'connect-unary';
  if (!fixture.grpcOnly && fixture.transport !== 'grpc-only') {
    return {
      ok: false,
      why:
        `${fixture.name} is required and is not marked transport:grpc-only, so it ` +
        'cannot be skipped on any transport (design §44 §10.4, D617)',
    };
  }
  if (!onFallback) {
    return {
      ok: false,
      why:
        `${fixture.name} is transport:grpc-only, so it may only be skipped on the ` +
        `Connect-unary fallback; this run used ${context.transport ?? 'an unnamed transport'}`,
    };
  }
  return { ok: true, clause: 'D613' };
}

/**
 * Checks one language's report against the manifest.
 *
 * Returns every problem rather than the first, because a suite that skipped four
 * required fixtures should learn about all four in one run, not one per CI
 * cycle.
 *
 * @param {string} language
 * @param {{tests?: string[], ran?: string[], skipped?: {fixture: string, reason: string}[], transport?: string}} report
 * @param {object} [options]
 * @param {boolean} [options.requireTests] Also check the six test names are present.
 * @returns {Promise<{ok: boolean, problems: string[], ran: string[], missing: string[]}>}
 */
export async function checkLanguage(language, report, options = {}) {
  const { manifest, byName } = await loadManifest();
  const problems = [];
  const ran = new Set(report.ran ?? []);
  const skipped = new Map((report.skipped ?? []).map((entry) => [entry.fixture, entry.reason]));

  if (options.requireTests !== false) {
    for (const short of REQUIRED_TESTS) {
      const wanted = testName(language, short);
      if (!(report.tests ?? []).includes(wanted)) {
        problems.push(
          `${language} has no test named ${wanted}; the six names are part of the contract ` +
            'so two languages can be compared',
        );
      }
    }
  }

  // Anything the report claims that is not in the corpus is a failure. A typo in
  // a fixture name would otherwise read as "that one is done".
  for (const name of ran) {
    if (!byName.has(name)) {
      problems.push(`${language} reports having run ${name}, which is not in the corpus`);
    }
  }
  for (const name of skipped.keys()) {
    if (!byName.has(name)) {
      problems.push(`${language} reports having skipped ${name}, which is not in the corpus`);
    }
  }

  const missing = [];
  for (const fixture of manifest.fixtures) {
    if (!fixture.required) {
      continue;
    }
    if (ran.has(fixture.name)) {
      continue;
    }
    const verdict = maySkip(fixture, { transport: report.transport });
    if (verdict.ok) {
      continue;
    }
    const said = skipped.get(fixture.name);
    missing.push(fixture.name);
    problems.push(
      said === undefined
        ? `${language} did not run required fixture ${fixture.name}` +
          (fixture.reason ? ` (${fixture.kind}: ${fixture.reason})` : '')
        : `${language} skipped required fixture ${fixture.name}: ${said}`,
    );
  }

  // A fixture the language says it skipped **and** ran is a contradiction, and it
  // is the shape a runner takes when a skip was papered over rather than fixed.
  for (const name of skipped.keys()) {
    if (ran.has(name)) {
      problems.push(`${language} both ran and skipped ${name}`);
    }
  }

  return { ok: problems.length === 0, problems, ran: [...ran].sort(), missing };
}

/**
 * Reads a language's report, or returns `null` when there is none.
 *
 * A missing report is not an error here: most languages have not been written
 * yet, and a runner that failed on every absent report would be red on `dev`
 * forever. `run-all.sh` reports it as absent and only a language that claims a
 * result is checked.
 */
export async function loadReport(language, dir = FIXTURES_DIR) {
  try {
    return JSON.parse(await readFile(join(dir, 'results', `${language}.json`), 'utf8'));
  } catch {
    return null;
  }
}

/** Writes a language's report. Used by a suite, and by the runner's own checks. */
export async function saveReport(language, report, dir = FIXTURES_DIR) {
  const { mkdir, writeFile } = await import('node:fs/promises');
  const results = join(dir, 'results');
  await mkdir(results, { recursive: true });
  await writeFile(join(results, `${language}.json`), `${JSON.stringify(report, null, 2)}\n`, 'utf8');
}
