// Tests for the npm licence gate (SD1 Task 2, design §53 §9.3, D914).
// Run with `node --test scripts/ci/npm-licences.test.mjs`.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import {
  checkPackages,
  DEFAULT_PROJECTS,
  evaluateLicence,
  loadAllowList,
  parseSpdx,
  REPO_ROOT,
} from './npm-licences.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const gate = join(here, 'npm-licences.mjs');
const allow = loadAllowList();

// A `pnpm licenses list --json` report: { licence: [{ name, versions, license }] }.
function report(entries) {
  const out = {};
  for (const [name, version, license] of entries) {
    out[license] = [...(out[license] ?? []), { name, versions: [version], license, paths: [] }];
  }
  return out;
}

function runGate(entries, extra = []) {
  const dir = mkdtempSync(join(tmpdir(), 'loams-npm-licences-'));
  const input = join(dir, 'report.json');
  writeFileSync(input, JSON.stringify(report(entries)));
  const exceptions = join(dir, 'exceptions.json');
  writeFileSync(exceptions, '[]\n');
  return spawnSync(
    process.execPath,
    [gate, '--input', input, '--exceptions', exceptions, ...extra],
    { encoding: 'utf8' },
  );
}

test('npm_licences_allowlist', () => {
  for (const licence of [
    'MIT',
    'ISC',
    'Apache-2.0',
    'BSD-3-Clause',
    '(Apache-2.0 AND BSD-3-Clause)',
  ]) {
    assert.equal(evaluateLicence(licence, allow).ok, true, licence);
  }
  const mpl = evaluateLicence('MPL-2.0 OR Apache-2.0', allow);
  assert.equal(mpl.ok, true);
  assert.equal(mpl.chosen, 'Apache-2.0', 'the permissive branch is preferred');
  // The @pierre/* packages publish the SPDX id in lower case (SD1 T0-5).
  assert.equal(evaluateLicence('apache-2.0', allow).chosen, 'Apache-2.0');
  const jszip = evaluateLicence('(MIT OR GPL-3.0-or-later)', allow);
  assert.equal(jszip.ok, true);
  assert.equal(jszip.chosen, 'MIT', 'the OR branch taken is recorded');
  assert.equal(evaluateLicence('Apache-2.0 WITH LLVM-exception', allow).ok, true);
  const res = runGate([
    ['a', '1.0.0', 'MIT'],
    ['b', '1.0.0', 'ISC'],
    ['c', '1.0.0', 'Apache-2.0'],
    ['d', '1.0.0', 'BSD-3-Clause'],
    ['dompurify', '3.4.16', '(MPL-2.0 OR Apache-2.0)'],
    ['jszip', '3.10.1', '(MIT OR GPL-3.0-or-later)'],
  ]);
  assert.equal(res.status, 0, res.stdout + res.stderr);
  assert.match(res.stdout, /jszip@3\.10\.1.*MIT/);
});

test('npm_licences_rejects_lgpl', () => {
  assert.equal(evaluateLicence('LGPL-3.0', allow).ok, false);
  const res = runGate([
    ['react', '19.3.0', 'MIT'],
    ['heic-to', '1.5.0', 'LGPL-3.0'],
  ]);
  assert.equal(res.status, 1);
  assert.match(res.stdout + res.stderr, /heic-to/);
  assert.match(res.stdout + res.stderr, /LGPL-3\.0/);
  assert.doesNotMatch(res.stdout + res.stderr, /\| react /, 'only failures are tabled');
});

test('npm_licences_rejects_copyleft_and_source_available', () => {
  for (const licence of [
    'GPL-3.0-only',
    'GPL-2.0-or-later',
    'AGPL-3.0',
    'LGPL-2.1-or-later',
    'SSPL-1.0',
    'BUSL-1.1',
    'Elastic-2.0',
    'GPL-2.0 WITH Classpath-exception-2.0',
    'MIT AND GPL-3.0',
    'CC-BY-NC-4.0',
  ]) {
    assert.equal(evaluateLicence(licence, allow).ok, false, licence);
  }
});

test('npm_licences_rejects_unknown', () => {
  for (const licence of [
    'UNKNOWN',
    'Unknown',
    'UNLICENSED',
    'SEE LICENSE IN LICENSE.md',
    '',
    'MIT OR',
    'Custom: https://x',
  ]) {
    assert.equal(evaluateLicence(licence, allow).ok, false, JSON.stringify(licence));
  }
  const res = runGate([['mystery', '0.0.1', 'Unknown']]);
  assert.equal(res.status, 1);
  assert.match(res.stdout + res.stderr, /mystery/);
});

test('npm_licences_rejects_tiptap_pro', () => {
  const { failures } = checkPackages(
    [
      { name: '@tiptap-pro/extension-ai', version: '3.0.0', licence: 'MIT', projects: ['x'] },
      { name: '@tiptap/core', version: '3.31.3', licence: 'MIT', projects: ['x'] },
    ],
    { allow, exceptions: [] },
  );
  assert.deepEqual(
    failures.map((f) => f.name),
    ['@tiptap-pro/extension-ai'],
  );
  const res = runGate([['@tiptap-pro/extension-collaboration', '3.0.0', 'MIT']]);
  assert.equal(res.status, 1);
  assert.match(res.stdout + res.stderr, /Tiptap Pro/);
});

test('npm_licences_exceptions_are_exact_and_cannot_admit_copyleft', () => {
  const pkgs = [{ name: 'font', version: '1.0.0', licence: 'OFL-1.1', projects: ['x'] }];
  const ok = {
    package: 'font',
    version: '1.0.0',
    licence: 'OFL-1.1',
    reason: 'a font',
    approvedBy: 'owner',
  };
  assert.equal(checkPackages(pkgs, { allow, exceptions: [ok] }).failures.length, 0);
  assert.equal(
    checkPackages(pkgs, { allow, exceptions: [{ ...ok, version: '0.9.0' }] }).failures.length,
    1,
  );
  assert.throws(() => checkPackages(pkgs, { allow, exceptions: [{ ...ok, approvedBy: '' }] }));
  const gpl = [{ name: 'g', version: '1.0.0', licence: 'GPL-3.0', projects: ['x'] }];
  assert.throws(() =>
    checkPackages(gpl, {
      allow,
      exceptions: [{ ...ok, package: 'g', licence: 'GPL-3.0' }],
    }),
  );
});

test('npm_licences_parses_spdx', () => {
  assert.deepEqual(parseSpdx('MIT'), { id: 'MIT' });
  assert.deepEqual(parseSpdx('(MIT OR Apache-2.0) AND ISC'), {
    op: 'AND',
    left: { op: 'OR', left: { id: 'MIT' }, right: { id: 'Apache-2.0' } },
    right: { id: 'ISC' },
  });
  assert.deepEqual(parseSpdx('Apache-2.0 WITH LLVM-exception'), {
    id: 'Apache-2.0',
    exception: 'LLVM-exception',
  });
  assert.throws(() => parseSpdx('MIT AND'));
  assert.throws(() => parseSpdx('(MIT'));
});

test('npm_licences_allow_list_matches_deny_toml', () => {
  for (const id of [
    'Apache-2.0',
    'MIT',
    'BSD-2-Clause',
    'BSD-3-Clause',
    'ISC',
    'Zlib',
    'MPL-2.0',
  ]) {
    assert.ok(allow.has(id), id);
  }
  // Owner rulings: no npm-only additions to deny.toml's list (SD1 T2-2), so Python-2.0 stays
  // refused. BlueOak-1.0.0 is allowed (owner, 2026-10-24; FT1 FT0-1-1 superseded).
  for (const id of [
    'GPL-3.0',
    'LGPL-3.0',
    'AGPL-3.0',
    'SSPL-1.0',
    'BUSL-1.1',
    'Python-2.0',
  ]) {
    assert.ok(!allow.has(id), id);
  }
});

test('npm_licences_allows_blueoak', () => {
  // Owner, 2026-10-24 (FT1 FT0-1-1 superseded): BlueOak-1.0.0 is in deny.toml's allow list.
  assert.ok(allow.has('BlueOak-1.0.0'));
  assert.equal(evaluateLicence('BlueOak-1.0.0', allow).ok, true);
  const res = runGate([['lru-cache', '11.5.3', 'BlueOak-1.0.0']]);
  assert.equal(res.status, 0, res.stdout + res.stderr);
});

test('npm_licences_default_projects_include_factory', () => {
  assert.ok(DEFAULT_PROJECTS.includes('@loams/factory'));
});

test('npm_licences_allows_0bsd', () => {
  // Owner, 2026-10-13 (SD1 T2-6): 0BSD is in deny.toml's allow list, for crates and npm.
  assert.ok(allow.has('0BSD'));
  assert.equal(evaluateLicence('0BSD', allow).ok, true);
  const res = runGate([['tslib', '2.3.0', '0BSD']]);
  assert.equal(res.status, 0, res.stdout + res.stderr);
});

const installed = existsSync(join(REPO_ROOT, 'node_modules', '.pnpm'));

test('npm_licences_current_tree_passes', { skip: !installed && 'run pnpm install first' }, () => {
  const res = spawnSync(process.execPath, [gate], { encoding: 'utf8', cwd: REPO_ROOT });
  assert.equal(res.status, 0, res.stdout + res.stderr);
});
