// Checks what the languages' suites report, and whether they have drifted apart.
//
//   node sdks/conformance/check-languages.mjs --list
//   node sdks/conformance/check-languages.mjs --check typescript
//   node sdks/conformance/check-languages.mjs --drift [--strict]
//
// ## Two different questions
//
// **Coverage** (`--check`): did a language run every required fixture? The answer
// comes from `sdks/fixtures/results/<language>.json`, which a suite writes, and
// the rule comes from `required.mjs`. This is the 100% bar of D617 and it is
// enforced, not described.
//
// **Drift** (`--drift`): do the languages agree about what they are testing?
// That is a different question and it is answered statically, by reading the
// suites: a language that names three fixtures where another names nine is not
// necessarily broken, but it is a fact somebody should know before comparing two
// implementations.
//
// Drift is a **warning by default**. A language early in its wave legitimately
// covers less, and a runner that failed would be red on `dev` for a week while
// eleven SDKs are written. `--strict` turns it into a failure, which is what CI
// uses for the waves that are meant to be comparable.

import { readdir } from 'node:fs/promises';
import { existsSync, readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { LANGUAGES, REQUIRED_TESTS, checkLanguage, loadManifest, loadReport } from './required.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, '..', '..');

const has = (name) => process.argv.includes(`--${name}`);
const value = (name) => {
  const at = process.argv.indexOf(`--${name}`);
  return at >= 0 && at + 1 < process.argv.length ? process.argv[at + 1] : null;
};

/** The source files of a language's suite: anything that could name a test. */
async function suiteFiles(dir) {
  const out = [];
  const walk = async (current) => {
    let entries;
    try {
      entries = await readdir(current, { withFileTypes: true });
    } catch {
      return;
    }
    for (const entry of entries) {
      if (entry.name === 'node_modules' || entry.name === 'target' || entry.name.startsWith('.')) {
        continue;
      }
      const full = join(current, entry.name);
      if (entry.isDirectory()) {
        await walk(full);
      } else if (/\.(ts|tsx|js|mjs|py|go|rs|swift|kt|java|cs|dart|rb|php|cpp|h|mm|m)$/.test(entry.name)) {
        out.push(full);
      }
    }
  };
  await walk(dir);
  return out;
}

/**
 * What a language's suite names: the six tests, the fixtures and the clauses.
 *
 * Static on purpose. A suite that ran a fixture but forgot to write a report
 * would look like one that never ran it, and the fix for that is a report; the
 * alternative — trusting the report alone — means a suite whose report was
 * hand-written passes. Both together are stronger than either.
 */
async function references(language) {
  const spec = LANGUAGES[language];
  if (!spec) {
    return null;
  }
  const { manifest } = await loadManifest();
  const files = await suiteFiles(join(root, spec.sources ?? spec.dir));
  const text = files.map((file) => readFileSyncSafe(file)).join('\n');
  const names = manifest.fixtures.map((fixture) => fixture.name);
  return {
    files: files.length,
    tests: REQUIRED_TESTS.filter((short) => text.includes(`${language}_${short}`)),
    fixtures: names.filter((name) => text.includes(name)),
    clauses: Object.keys(manifest.clauses).filter((clause) => text.includes(clause)),
  };
}

function readFileSyncSafe(file) {
  try {
    return readFileSync(file, 'utf8');
  } catch {
    return '';
  }
}

const known = Object.keys(LANGUAGES).filter((language) => existsSync(join(root, LANGUAGES[language].dir)));

if (has('list')) {
  for (const language of known) {
    const spec = LANGUAGES[language];
    const found = await references(language);
    console.log(
      `${language.padEnd(12)} ${(spec.sources ?? spec.dir).padEnd(32)} ${found.tests.length}/6 tests, ` +
        `${found.fixtures.length} fixtures, ${found.clauses.length} clauses, ` +
        `${found.files} source files`,
    );
  }
  process.exit(0);
}

if (value('check')) {
  const language = value('check');
  const report = await loadReport(language);
  if (report === null) {
    console.error(
      `${language} has no report at sdks/fixtures/results/${language}.json, so nothing can be ` +
        'checked. A suite writes one when it runs.',
    );
    process.exit(2);
  }
  const verdict = await checkLanguage(language, report);
  for (const problem of verdict.problems) {
    console.error(`  - ${problem}`);
  }
  if (!verdict.ok) {
    console.error(
      `${language} does not meet the 100% bar: ${verdict.missing.length} required fixture(s) not run.`,
    );
    process.exit(1);
  }
  console.log(`${language}: ${verdict.ran.length} fixture(s) run, all required ones covered.`);
  process.exit(0);
}

if (has('drift')) {
  const { manifest } = await loadManifest();
  const seen = {};
  for (const language of known) {
    seen[language] = await references(language);
  }
  const languages = Object.keys(seen);

  console.log('conformance drift between languages');
  console.log('');
  const header = ['language', 'tests', 'fixtures', 'clauses'].join('  ');
  console.log(header);
  console.log('-'.repeat(header.length));
  for (const language of languages) {
    console.log(
      [language, `${seen[language].tests.length}/6`, String(seen[language].fixtures.length), seen[language].clauses.join(',')].join(
        '  ',
      ),
    );
  }
  console.log('');

  let problems = 0;
  for (const language of languages) {
    const missingTests = REQUIRED_TESTS.filter((short) => !seen[language].tests.includes(short));
    if (missingTests.length > 0) {
      problems += 1;
      console.log(`${language} is missing ${missingTests.length} of the six tests:`);
      for (const short of missingTests) {
        console.log(`    ${language}_${short}`);
      }
    }
  }

  // Coverage against the required set, which is the question that matters while
  // there is only one language to compare: "this suite names 13 of 28 required
  // fixtures" is the fact worth printing, and a cross-language diff would never
  // surface it.
  const required = manifest.fixtures.filter((fixture) => fixture.required).map((f) => f.name);
  console.log('');
  console.log('required-fixture coverage:');
  for (const language of languages) {
    const named = new Set(seen[language].fixtures);
    const hit = required.filter((name) => named.has(name));
    const gap = required.filter((name) => !named.has(name));
    console.log(`  ${language.padEnd(12)} ${hit.length}/${required.length} required fixtures named by the suite`);
    if (gap.length > 0) {
      problems += 1;
      console.log(`    not named: ${gap.join(' ')}`);
    }
  }

  // The union of what any language names, so "two languages cover different
  // things" is a concrete diff rather than a suspicion.
  const union = new Set();
  for (const language of languages) {
    for (const name of seen[language].fixtures) {
      union.add(name);
    }
  }
  if (languages.length > 1 && union.size > 0) {
    console.log('');
    console.log(`${union.size} fixture(s) named by at least one language:`);
    for (const name of [...union].sort()) {
      const naming = languages.filter((language) => seen[language].fixtures.includes(name));
      const row = manifest.fixtures.find((fixture) => fixture.name === name);
      console.log(
        `  ${row?.required ? 'required ' : 'optional '} ${name.padEnd(42)} ${naming.join(' ')}`,
      );
    }
    const coveredByAll = languages.filter((language) => seen[language].fixtures.length === union.size);
    if (coveredByAll.length < languages.length) {
      problems += 1;
      console.log('');
      console.log(
        'languages do not agree on the fixture set: ' +
          languages
            .map((language) => `${language} (${seen[language].fixtures.length})`)
            .join(', '),
      );
    }
  }

  if (problems > 0 && has('strict')) {
    console.error('');
    console.error(`${problems} drift problem(s); --strict was given, so this is a failure.`);
    process.exit(1);
  }
  if (problems > 0) {
    console.log('');
    console.log(`${problems} drift problem(s); a warning without --strict.`);
  }
  process.exit(0);
}

console.error('usage: check-languages.mjs --list | --check <language> | --drift [--strict]');
process.exit(2);
