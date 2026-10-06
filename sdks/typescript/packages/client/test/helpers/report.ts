// The conformance report the TypeScript suite writes (design §44 §10.4, D617).
//
// `sdks/fixtures/results/typescript.json` is what
// `check-languages.mjs --check typescript` reads, and until a suite writes one
// that command has nothing to check — which is how the 100% required-fixture bar
// came to be enforced for no language at all, TypeScript included.
//
// ## Generated, never committed
//
// The report is **not** in git, and that is a decision rather than an oversight
// (D640). A committed report is a claim about a corpus, made by a suite, and it
// starts being wrong the moment either moves: `manifest.json` gains a required
// fixture, or the suite stops driving one. Both of those are real regressions,
// and a committed file turns both into a green diff — the suite fails to run, or
// is run with a filter, and CI reads a report that describes the last run that
// happened to write it. `check-languages.mjs` is only a gate if what it reads
// came out of the run in the same breath.
//
// So the suite **generates** it, CI **generate-then-check**, and the file is
// gitignored like every other build output. What stays in git is the thing that
// makes the report trustworthy: the driver that derives `ran` from the corpus it
// really exercised.
//
// ## What may go in it
//
// - `tests` is the suite's own inventory, discovered by looking for each
//   canonical name in this package's test sources. A renamed test drops out of
//   the report instead of being claimed, and `checkLanguage` says which.
// - `ran` is what `driveRequiredFixtures` returned: fixtures whose steps were
//   driven **and held**. It is never a list written down here.
// - `skipped` is empty because the TypeScript suite skips nothing. The one legal
//   skip is a fixture marked `transport: grpc-only` on the Connect-unary
//   fallback (D613), and no fixture in the corpus carries it — this suite speaks
//   gRPC-Web and Connect, so there is nothing to fall back from.
// - `transport` is what the run actually used, which is the field `maySkip`
//   keys off.

import { readFile, readdir, rm } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { REQUIRED_TESTS, saveReport, testName } from '../../../../../conformance/required.mjs';
import { FIXTURES } from './server.js';

/** Where the report lands: `saveReport` puts it here, and the runner reads it. */
export const REPORT = join(FIXTURES, 'results', 'typescript.json');

/** This package's `test/` directory, where the six canonical names have to appear. */
const SUITE_TESTS = resolve(dirname(fileURLToPath(import.meta.url)), '..');

/**
 * Removes a report left by an earlier run.
 *
 * Called before the suite drives anything, so a run that then crashes or is cut
 * short leaves **no** report rather than the previous run's. A gate reading a
 * report from a run that did not finish is the failure mode this whole file is
 * about.
 */
export async function clearReport(): Promise<void> {
  await rm(REPORT, { force: true });
}

/** Every `.ts` under `test/`, read to find the canonical test names. */
async function suiteSources(directory: string): Promise<string> {
  const entries = await readdir(directory, { withFileTypes: true });
  const texts: string[] = [];
  for (const entry of entries) {
    const full = join(directory, entry.name);
    if (entry.isDirectory()) {
      texts.push(await suiteSources(full));
    } else if (entry.name.endsWith('.ts')) {
      texts.push(await readFile(full, 'utf8'));
    }
  }
  return texts.join('\n');
}

/**
 * The six canonical tests this suite actually has, discovered not declared.
 *
 * The short names come from `required.mjs`, which owns them; the presence check
 * is this package's own sources, so a test that was renamed stops being claimed
 * and `checkLanguage` fails with the name that is missing instead of the suite
 * quietly asserting less than it says.
 */
export async function testsThisSuiteHas(): Promise<string[]> {
  const sources = await suiteSources(SUITE_TESTS);
  return REQUIRED_TESTS.map((short) => testName('typescript', short)).filter((name) =>
    sources.includes(name),
  );
}

/**
 * Writes the report for a run that drove `ran`.
 *
 * The fixtures are sorted so two runs of the same corpus produce the same file:
 * a generated artefact that reorders itself is one nobody can diff.
 */
export async function writeConformanceReport(
  ran: readonly string[],
  transport: string,
): Promise<readonly string[]> {
  const report = {
    tests: await testsThisSuiteHas(),
    ran: [...new Set(ran)].sort(),
    skipped: [] as { fixture: string; reason: string }[],
    transport,
  };
  await saveReport('typescript', report);
  return report.ran;
}