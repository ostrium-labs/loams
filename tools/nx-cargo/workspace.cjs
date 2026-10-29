// Reads a Cargo workspace from its manifests, without running Cargo: the members, and each
// member's path dependencies (normal, dev and build, including target-specific ones and
// `workspace = true` entries). Nx computes the project graph in every job, including JS-only
// ones, so the graph must not need a Rust toolchain. `check.cjs` compares the result with
// `cargo metadata`.
'use strict';

const fs = require('node:fs');
const path = require('node:path');

const toml = require(require.resolve('smol-toml', { paths: [require.resolve('nx/package.json')] }));

function readToml(file) {
  return toml.parse(fs.readFileSync(file, 'utf8'));
}

// Cargo's member globs: `*` within one path segment (all this repository uses).
function expandMember(root, pattern) {
  const parts = pattern.split('/');
  let dirs = [''];
  for (const part of parts) {
    const next = [];
    for (const d of dirs) {
      if (part.includes('*')) {
        const re = new RegExp(`^${part.replace(/[.+?^${}()|[\]\\]/g, '\\$&').replace(/\*/g, '.*')}$`);
        let entries = [];
        try {
          entries = fs.readdirSync(path.join(root, d), { withFileTypes: true });
        } catch {
          entries = [];
        }
        for (const e of entries) if (e.isDirectory() && re.test(e.name)) next.push(path.posix.join(d, e.name));
      } else {
        next.push(path.posix.join(d, part));
      }
    }
    dirs = next;
  }
  return dirs.filter((d) => fs.existsSync(path.join(root, d, 'Cargo.toml'))).sort();
}

function dependencyTables(manifest) {
  const tables = [];
  for (const key of ['dependencies', 'dev-dependencies', 'build-dependencies']) {
    if (manifest[key]) tables.push(manifest[key]);
  }
  for (const target of Object.values(manifest.target ?? {})) {
    for (const key of ['dependencies', 'dev-dependencies', 'build-dependencies']) {
      if (target?.[key]) tables.push(target[key]);
    }
  }
  return tables;
}

/**
 * @param {string} repoRoot absolute path of the repository
 * @param {string} manifestRel the workspace's Cargo.toml, relative to repoRoot
 * @returns {{ root: string, crates: Array<{ name: string, dir: string, deps: string[] }> }}
 *   dir and root are relative to repoRoot (POSIX); deps are member crate names.
 */
function readWorkspace(repoRoot, manifestRel) {
  const wsDir = path.posix.dirname(manifestRel) === '.' ? '' : path.posix.dirname(manifestRel);
  const wsAbs = path.join(repoRoot, wsDir);
  const ws = readToml(path.join(repoRoot, manifestRel));
  const members = ws.workspace?.members ?? [];
  const excluded = new Set((ws.workspace?.exclude ?? []).map((e) => path.posix.normalize(e)));
  const wsDeps = ws.workspace?.dependencies ?? {};

  const dirs = [...new Set(members.flatMap((m) => expandMember(wsAbs, m)))].filter((d) => !excluded.has(d));
  const crates = dirs.map((d) => {
    const manifest = readToml(path.join(wsAbs, d, 'Cargo.toml'));
    return { name: manifest.package?.name, dir: d, manifest };
  });
  const byDir = new Map(crates.map((c) => [c.dir, c.name]));

  const out = crates.map(({ name, dir, manifest }) => {
    const deps = new Set();
    for (const table of dependencyTables(manifest)) {
      for (const [key, spec] of Object.entries(table)) {
        let target;
        if (spec && typeof spec === 'object' && typeof spec.path === 'string') {
          target = path.posix.normalize(path.posix.join(dir, spec.path));
        } else if (spec && typeof spec === 'object' && spec.workspace === true) {
          const w = wsDeps[key];
          if (w && typeof w === 'object' && typeof w.path === 'string') target = path.posix.normalize(w.path);
        }
        if (target !== undefined && byDir.has(target) && byDir.get(target) !== name) deps.add(byDir.get(target));
      }
    }
    return { name, dir: path.posix.join(wsDir, dir), deps: [...deps].sort() };
  });
  return { root: wsDir || '.', crates: out };
}

module.exports = { readWorkspace, readToml };
