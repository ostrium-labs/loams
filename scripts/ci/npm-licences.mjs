#!/usr/bin/env node
// The npm licence gate (design §53 §9.3, D914; SD1 Task 2).
//
//   npm-licences.mjs [--project <nx project>…] [--input <pnpm licenses json>…] [--exceptions <file>]
//
// Runs `pnpm --filter <project>... licenses list --prod --json` for the console, the desktop,
// @loams/studio-ui and @loams/listing (and the workspace packages they depend on), and checks every production
// dependency against CONTRIBUTING rule 3: the allow list in deny.toml, and nothing else. It
// parses SPDX expressions, accepts an `OR` expression when one branch is allowed (and records
// the branch taken), and fails on anything
// else, on an unknown licence and on any Tiptap Pro or Cloud package. Exceptions are exact
// {package, version, licence, reason, approvedBy} entries in npm-licences.exceptions.json; they
// can never admit a copyleft or source-available licence or a Tiptap Pro package.
import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = dirname(fileURLToPath(import.meta.url));
export const REPO_ROOT = resolve(HERE, '..', '..');
export const DEFAULT_PROJECTS = [
  '@loams/console',
  '@loams/desktop',
  '@loams/studio-ui',
  '@loams/factory',
  '@loams/listing',
];
const DEFAULT_EXCEPTIONS = join(HERE, 'npm-licences.exceptions.json');

// Never allowed, not even by an exception (CONTRIBUTING rule 3: no AGPL, GPL, LGPL, BSL (Business
// Source), SSPL, ELv2 or proprietary code). Non-commercial Creative Commons is proprietary in effect.
const HARD_DENY = [
  /^A?GPL/i,
  /^LGPL/i,
  /^SSPL/i,
  /^BUSL/i,
  /^Elastic-/i,
  /^Commons-Clause/i,
  /^CC-BY-NC/i,
  /^CC-BY-ND/i,
];
const PROPRIETARY_SCOPES = [/^@tiptap-pro\//, /^@tiptap-cloud\//];

class GateError extends Error {}

/** The `allow = [...]` list of deny.toml's [licenses] table. */
export function loadAllowList(denyToml = readFileSync(join(REPO_ROOT, 'deny.toml'), 'utf8')) {
  const section = denyToml.split(/^\[licenses\]\s*$/m)[1]?.split(/^\[/m)[0] ?? '';
  const list = section.match(/^allow\s*=\s*\[([\s\S]*?)\]/m)?.[1];
  if (!list) throw new GateError('deny.toml has no [licenses] allow list');
  const ids = [...list.replace(/#.*$/gm, '').matchAll(/"([^"]+)"/g)].map((m) => m[1]);
  return new Set(ids);
}

// ---------- SPDX ----------

function tokenize(expr) {
  const tokens = [];
  const re = /\s*(\(|\)|[A-Za-z0-9.+:-]+)\s*/y;
  let at = 0;
  while (at < expr.length) {
    re.lastIndex = at;
    const m = re.exec(expr);
    if (!m) throw new GateError(`cannot parse licence ${JSON.stringify(expr)}`);
    tokens.push(m[1]);
    at = re.lastIndex;
  }
  return tokens;
}

/** Parses an SPDX expression into {id, exception?} | {op: 'AND'|'OR', left, right}. */
export function parseSpdx(expr) {
  const tokens = tokenize(expr.trim());
  if (tokens.length === 0) throw new GateError('empty licence');
  let i = 0;
  const peek = () => tokens[i];
  const keyword = (t) => ['AND', 'OR', 'WITH'].includes(t?.toUpperCase());
  function primary() {
    const t = tokens[i++];
    if (t === undefined) throw new GateError(`unexpected end of ${JSON.stringify(expr)}`);
    if (t === '(') {
      const node = orExpr();
      if (tokens[i++] !== ')')
        throw new GateError(`unbalanced parentheses in ${JSON.stringify(expr)}`);
      return node;
    }
    if (t === ')' || keyword(t)) throw new GateError(`unexpected ${t} in ${JSON.stringify(expr)}`);
    if (peek()?.toUpperCase() === 'WITH') {
      i++;
      const exception = tokens[i++];
      if (!exception || exception === '(' || exception === ')' || keyword(exception)) {
        throw new GateError(`WITH needs an exception in ${JSON.stringify(expr)}`);
      }
      return { id: t, exception };
    }
    return { id: t };
  }
  function andExpr() {
    let left = primary();
    while (peek()?.toUpperCase() === 'AND') {
      i++;
      left = { op: 'AND', left, right: primary() };
    }
    return left;
  }
  function orExpr() {
    let left = andExpr();
    while (peek()?.toUpperCase() === 'OR') {
      i++;
      left = { op: 'OR', left, right: andExpr() };
    }
    return left;
  }
  const node = orExpr();
  if (i !== tokens.length) throw new GateError(`trailing ${tokens[i]} in ${JSON.stringify(expr)}`);
  return node;
}

const show = (node) =>
  node.op
    ? `(${show(node.left)} ${node.op} ${show(node.right)})`
    : node.exception
      ? `${node.id} WITH ${node.exception}`
      : node.id;

function leafAllowed(node, allow) {
  const lower = new Map([...allow].map((id) => [id.toLowerCase(), id]));
  const id = node.id.replace(/\+$/, '');
  if (HARD_DENY.some((re) => re.test(id))) return null;
  const key = (node.exception ? `${id} WITH ${node.exception}` : id).toLowerCase();
  return lower.get(key) ?? null;
}

function evalNode(node, allow) {
  if (!node.op) {
    const canonical = leafAllowed(node, allow);
    return canonical ? { ok: true, chosen: canonical } : { ok: false };
  }
  const left = evalNode(node.left, allow);
  if (node.op === 'OR') {
    // Prefer a permissive branch over MPL's file-level copyleft (dompurify is taken under Apache-2.0).
    const right = evalNode(node.right, allow);
    if (left.ok && right.ok)
      return /MPL/.test(left.chosen) && !/MPL/.test(right.chosen) ? right : left;
    return left.ok ? left : right;
  }
  const right = evalNode(node.right, allow);
  return left.ok && right.ok
    ? { ok: true, chosen: `${left.chosen} AND ${right.chosen}` }
    : { ok: false };
}

/** {ok, chosen?, reason?}. `chosen` is the licence (or OR branch) the package is taken under. */
export function evaluateLicence(licence, allow) {
  const text = (licence ?? '').trim();
  if (
    text === '' ||
    /^(unknown|unlicensed|none)$/i.test(text) ||
    /^see licen[cs]e in/i.test(text)
  ) {
    return { ok: false, reason: 'unknown or unlicensed' };
  }
  let node;
  try {
    node = parseSpdx(text);
  } catch (err) {
    return { ok: false, reason: `not an SPDX expression (${err.message})` };
  }
  const res = evalNode(node, allow);
  if (res.ok) return { ok: true, chosen: res.chosen, expression: show(node) };
  return {
    ok: false,
    reason: isHardDenied(node) ? 'copyleft or source-available' : 'not on the allow list',
  };
}

function isHardDenied(node) {
  if (!node.op) return HARD_DENY.some((re) => re.test(node.id));
  // An OR is only hard-denied if every branch is; an AND if any part is.
  return node.op === 'OR'
    ? isHardDenied(node.left) && isHardDenied(node.right)
    : isHardDenied(node.left) || isHardDenied(node.right);
}

// ---------- packages ----------

/** Flattens `pnpm licenses list --json` output into [{name, version, licence}]. */
export function flattenReport(report) {
  const out = [];
  for (const [group, pkgs] of Object.entries(report ?? {})) {
    for (const pkg of pkgs) {
      for (const version of pkg.versions ?? [pkg.version]) {
        out.push({ name: pkg.name, version, licence: pkg.license ?? group });
      }
    }
  }
  return out;
}

function validateExceptions(exceptions, allow) {
  for (const e of exceptions) {
    for (const field of ['package', 'version', 'licence', 'reason', 'approvedBy']) {
      if (typeof e[field] !== 'string' || e[field].trim() === '') {
        throw new GateError(`exception ${JSON.stringify(e)} needs a non-empty ${field}`);
      }
    }
    if (PROPRIETARY_SCOPES.some((re) => re.test(e.package))) {
      throw new GateError(
        `exception for ${e.package}: Tiptap Pro and Cloud packages are never allowed`,
      );
    }
    if (evaluateLicence(e.licence, allow).reason === 'copyleft or source-available') {
      throw new GateError(`exception for ${e.package}: ${e.licence} can never be excepted`);
    }
  }
}

/** Returns {failures, accepted}. Throws on a malformed or forbidden exception. */
export function checkPackages(pkgs, { allow, exceptions }) {
  validateExceptions(exceptions, allow);
  const failures = [];
  const accepted = [];
  for (const pkg of pkgs) {
    if (PROPRIETARY_SCOPES.some((re) => re.test(pkg.name))) {
      failures.push({ ...pkg, problem: 'Tiptap Pro/Cloud package (proprietary, never allowed)' });
      continue;
    }
    const verdict = evaluateLicence(pkg.licence, allow);
    if (verdict.ok) {
      accepted.push({ ...pkg, chosen: verdict.chosen, expression: verdict.expression });
      continue;
    }
    const exception = exceptions.find(
      (e) => e.package === pkg.name && e.version === pkg.version && e.licence === pkg.licence,
    );
    if (exception && verdict.reason !== 'copyleft or source-available') {
      accepted.push({ ...pkg, chosen: pkg.licence, exception });
      continue;
    }
    failures.push({ ...pkg, problem: verdict.reason });
  }
  return { failures, accepted };
}

function table(rows) {
  const head = ['Package', 'Version', 'Licence', 'Problem', 'Projects'];
  const body = rows.map((r) => [
    r.name,
    r.version,
    r.licence || '(none)',
    r.problem,
    (r.projects ?? []).join(', '),
  ]);
  const widths = head.map((h, c) =>
    Math.max(h.length, ...body.map((row) => String(row[c]).length)),
  );
  const line = (cells) =>
    `| ${cells.map((cell, c) => String(cell).padEnd(widths[c])).join(' | ')} |`;
  return [line(head), line(widths.map((w) => '-'.repeat(w))), ...body.map(line)].join('\n');
}

function pnpmReport(project) {
  const out = execFileSync(
    'pnpm',
    ['--filter', `${project}...`, 'licenses', 'list', '--prod', '--json'],
    {
      cwd: REPO_ROOT,
      encoding: 'utf8',
      maxBuffer: 256 * 1024 * 1024,
    },
  );
  // pnpm prints a plain sentence instead of JSON when a project has no production dependencies.
  return out.trim().startsWith('{') ? JSON.parse(out) : {};
}

function parseArgs(argv) {
  const opts = { project: [], input: [] };
  for (let i = 0; i < argv.length; i++) {
    const flag = argv[i];
    const value = argv[i + 1];
    if (!['--project', '--input', '--exceptions'].includes(flag) || value === undefined) {
      throw new GateError(
        `usage: npm-licences.mjs [--project <p>…] [--input <json>…] [--exceptions <file>] (bad ${flag})`,
      );
    }
    i++;
    if (flag === '--exceptions') opts.exceptions = value;
    else opts[flag.slice(2)].push(value);
  }
  return opts;
}

function main(argv) {
  const opts = parseArgs(argv);
  const allow = loadAllowList();
  const exceptions = JSON.parse(readFileSync(opts.exceptions ?? DEFAULT_EXCEPTIONS, 'utf8'));
  const sources = opts.input.length
    ? opts.input.map((path) => [path, JSON.parse(readFileSync(path, 'utf8'))])
    : (opts.project.length ? opts.project : DEFAULT_PROJECTS).map((p) => [p, pnpmReport(p)]);
  const byKey = new Map();
  for (const [source, report] of sources) {
    for (const pkg of flattenReport(report)) {
      const key = `${pkg.name}@${pkg.version}`;
      const seen = byKey.get(key) ?? { ...pkg, projects: [] };
      seen.projects.push(source);
      byKey.set(key, seen);
    }
  }
  const pkgs = [...byKey.values()].sort((a, b) => a.name.localeCompare(b.name));
  const { failures, accepted } = checkPackages(pkgs, { allow, exceptions });
  for (const a of accepted) {
    if (a.exception) {
      process.stdout.write(
        `exception: ${a.name}@${a.version} ${a.licence} [approved by: ${a.exception.approvedBy}] ${a.exception.reason}\n`,
      );
    } else if (a.expression?.includes(' OR ')) {
      process.stdout.write(`choice: ${a.name}@${a.version} ${a.licence} taken under ${a.chosen}\n`);
    }
  }
  if (failures.length) {
    process.stdout.write(
      `\nnpm licence gate failed: ${failures.length} package(s)\n\n${table(failures)}\n`,
    );
    process.stdout.write(
      '\nSee CONTRIBUTING.md rule 3. Record an approved exception in scripts/ci/npm-licences.exceptions.json only for a licence that is not copyleft.\n',
    );
    return 1;
  }
  process.stdout.write(
    `npm licence gate passed: ${pkgs.length} production package(s) from ${sources.map(([s]) => s).join(', ')}\n`,
  );
  return 0;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    process.exitCode = main(process.argv.slice(2));
  } catch (err) {
    process.stderr.write(`npm-licences: ${err instanceof GateError ? err.message : err.stack}\n`);
    process.exitCode = 2;
  }
}
