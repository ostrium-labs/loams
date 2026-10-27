#!/usr/bin/env node
// Checks the Cargo projects of tools/nx-cargo against the source:
//
// 1. The crate graph read from the manifests equals `cargo metadata`'s (members and path
//    dependencies of every kind).
// 2. Every file a crate's code names outside the crate and its path dependencies (a relative
//    "../" literal, or "/../" after CARGO_MANIFEST_DIR) is a declared input in inputs.json, and a
//    crate that reaches the workspace root itself is declared `repoWide`. A false cache hit is
//    worse than a miss, so an undeclared read fails the check.
//
//   node tools/nx-cargo/check.cjs [--no-metadata]
// Exit 0: consistent. Exit 1: each problem is printed.
'use strict';

const { execFileSync } = require('node:child_process');
const fs = require('node:fs');
const path = require('node:path');

const { readWorkspace } = require('./workspace.cjs');
const config = require('./inputs.json');

const repoRoot = path.resolve(__dirname, '..', '..');
const noMetadata = process.argv.includes('--no-metadata');
const problems = [];

function globToRegExp(glob) {
  let re = '';
  for (let i = 0; i < glob.length; i++) {
    const c = glob[i];
    if (c === '*' && glob[i + 1] === '*') {
      re += '.*';
      i++;
      if (glob[i + 1] === '/') i++;
    } else if (c === '*') re += '[^/]*';
    else re += c.replace(/[.+?^${}()|[\]\\]/g, '\\$&');
  }
  return new RegExp(`^${re}$`);
}

function walk(dir, out = []) {
  for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
    if (e.name === 'target' || e.name === 'node_modules' || e.name.startsWith('.')) continue;
    const p = path.join(dir, e.name);
    if (e.isDirectory()) walk(p, out);
    else if (e.name.endsWith('.rs')) out.push(p);
  }
  return out;
}

for (const [manifest, ws] of Object.entries(config.workspaces)) {
  const { root, crates } = readWorkspace(repoRoot, manifest);
  const byName = new Map(crates.map((c) => [c.name, c]));

  // 1. The graph against cargo metadata.
  if (!noMetadata) {
    const meta = JSON.parse(
      execFileSync('cargo', ['metadata', '--no-deps', '--format-version', '1', '--locked', '--manifest-path', manifest], {
        cwd: repoRoot,
        encoding: 'utf8',
        maxBuffer: 256 * 1024 * 1024,
      }),
    );
    const names = new Set(meta.packages.map((p) => p.name));
    for (const p of meta.packages) {
      const c = byName.get(p.name);
      if (!c) {
        problems.push(`${manifest}: cargo metadata has ${p.name}, the manifest reader does not`);
        continue;
      }
      const dir = path.relative(repoRoot, path.dirname(p.manifest_path)).split(path.sep).join('/');
      if (dir !== c.dir) problems.push(`${p.name}: directory ${c.dir}, cargo says ${dir}`);
      const want = [...new Set(p.dependencies.filter((d) => d.path && names.has(d.name) && d.name !== p.name).map((d) => d.name))].sort();
      if (JSON.stringify(want) !== JSON.stringify(c.deps)) {
        problems.push(`${p.name}: path dependencies ${JSON.stringify(c.deps)}, cargo says ${JSON.stringify(want)}`);
      }
    }
    for (const c of crates) if (!names.has(c.name)) problems.push(`${manifest}: ${c.name} is not a cargo member`);
  }

  // 2. Reads outside the crate.
  const closure = (name, seen = new Set()) => {
    for (const d of byName.get(name)?.deps ?? []) if (!seen.has(d)) (seen.add(d), closure(d, seen));
    return seen;
  };
  const repoWide = new Set(ws.repoWide ?? []);
  const wsAbs = path.join(repoRoot, root);
  for (const c of crates) {
    const crateAbs = path.join(repoRoot, c.dir);
    const allowed = [c.dir, ...[...closure(c.name)].map((d) => byName.get(d).dir)];
    const declared = (ws.extra?.[c.name] ?? []).map((g) => globToRegExp(g));
    const covered = (rel) =>
      repoWide.has(c.name) ||
      allowed.some((a) => rel === a || rel.startsWith(`${a}/`)) ||
      declared.some((re) => re.test(rel));
    const ignore = new Set((ws.scanIgnore ?? []).map((f) => path.join(repoRoot, f)));
    for (const file of walk(crateAbs)) {
      if (ignore.has(file)) continue;
      const src = fs.readFileSync(file, 'utf8');
      const literals = [];
      for (const m of src.matchAll(/"((?:\.\.\/)+[^"\s{}]*)"/g)) literals.push({ rel: m[1], bases: [path.dirname(file), crateAbs] });
      for (const m of src.matchAll(/"(\/(?:\.\.\/?)+[^"\s{}]*)"/g)) literals.push({ rel: m[1].slice(1), bases: [crateAbs] });
      for (const m of src.matchAll(/join\("(\.\.(?:\/\.\.)*)"\)/g)) literals.push({ rel: m[1], bases: [crateAbs] });
      for (const { rel, bases } of literals) {
        for (const base of bases) {
          const abs = path.normalize(path.join(base, rel));
          if (!abs.startsWith(repoRoot) || !fs.existsSync(abs)) continue;
          const r = path.relative(repoRoot, abs).split(path.sep).join('/');
          const where = path.relative(repoRoot, file);
          // The repository, the workspace or the crates directory: the code walks it.
          if (abs === repoRoot || abs === wsAbs || abs === path.dirname(crateAbs)) {
            if (!repoWide.has(c.name)) problems.push(`${c.name}: ${where} reaches ${r || '.'}/ ("${rel}"); declare the crate repoWide in tools/nx-cargo/inputs.json`);
            continue;
          }
          const isDir = fs.statSync(abs).isDirectory();
          if (!covered(r) && !covered(`${r}/x`)) {
            problems.push(`${c.name}: ${where} reads ${r}${isDir ? '/' : ''}; add it to extra in tools/nx-cargo/inputs.json`);
          }
        }
      }
    }
  }
}

if (problems.length) {
  for (const p of [...new Set(problems)]) console.error(p);
  process.exit(1);
}
console.log('nx-cargo: the crate graph and the declared inputs match the source');
