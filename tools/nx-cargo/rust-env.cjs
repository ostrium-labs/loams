#!/usr/bin/env node
// A runtime input of the Cargo targets (tools/nx-cargo): prints what changes how Rust code
// compiles and behaves, so a cached test or lint result is reused only under the same settings.
//
// - rustc's exact version (from the workspace's rust-toolchain.toml, through rustup);
// - the compile flags that change code: `--cfg`, `-C opt-level`, `debug-assertions`,
//   `overflow-checks`, `panic`, `target-cpu` and `target-feature`, from RUSTFLAGS,
//   CARGO_ENCODED_RUSTFLAGS, CARGO_BUILD_RUSTFLAGS and every Cargo config file Cargo would read;
// - the same profile settings, from config files and CARGO_PROFILE_* variables;
// - the build target (CARGO_BUILD_TARGET) and the host platform.
//
// Linker choice, link arguments, job counts, target directories and debug-info levels are left
// out: they do not change a test's result, and including them would stop the owner's machine
// and CI from sharing entries.
//
//   node tools/nx-cargo/rust-env.cjs <workspace dir, relative to the repository root>
'use strict';

const { execFileSync } = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');

const { readToml } = require('./workspace.cjs');

const repoRoot = path.resolve(__dirname, '..', '..');
const wsDir = path.resolve(repoRoot, process.argv[2] ?? '.');

const SEMANTIC_C = /^(opt-level|debug-assertions|overflow-checks|panic|target-cpu|target-feature|codegen-units)=/;
function semanticFlags(flags) {
  const out = [];
  for (let i = 0; i < flags.length; i++) {
    const f = flags[i];
    if (f === '--cfg' && i + 1 < flags.length) out.push(`--cfg ${flags[++i]}`);
    else if (f.startsWith('--cfg=')) out.push(`--cfg ${f.slice(6)}`);
    else if (f === '-C' && i + 1 < flags.length) {
      const v = flags[++i];
      if (SEMANTIC_C.test(v)) out.push(`-C ${v}`);
    } else if (f.startsWith('-C') && SEMANTIC_C.test(f.slice(2))) out.push(`-C ${f.slice(2)}`);
  }
  return out;
}
const split = (s) => (s ?? '').split(/\s+/).filter(Boolean);

// Cargo's config files: every .cargo/config(.toml) from the workspace up to /, then CARGO_HOME.
function configFiles() {
  const files = [];
  let dir = wsDir;
  for (;;) {
    for (const name of ['config.toml', 'config']) {
      const f = path.join(dir, '.cargo', name);
      if (fs.existsSync(f)) files.push(f);
    }
    const up = path.dirname(dir);
    if (up === dir) break;
    dir = up;
  }
  const home = process.env.CARGO_HOME ?? path.join(os.homedir(), '.cargo');
  for (const name of ['config.toml', 'config']) {
    const f = path.join(home, name);
    if (fs.existsSync(f) && !files.includes(f)) files.push(f);
  }
  return files;
}

const lines = [];
let rustc = 'unknown';
try {
  rustc = execFileSync('rustc', ['-vV'], { cwd: wsDir, encoding: 'utf8' }).trim();
} catch {
  rustc = 'rustc unavailable';
}
lines.push(rustc);
lines.push(`host ${process.platform}-${process.arch}`);
lines.push(`target ${process.env.CARGO_BUILD_TARGET ?? ''}`);

lines.push(`RUSTFLAGS ${semanticFlags(split(process.env.RUSTFLAGS)).join(' ')}`);
lines.push(`CARGO_BUILD_RUSTFLAGS ${semanticFlags(split(process.env.CARGO_BUILD_RUSTFLAGS)).join(' ')}`);
lines.push(
  `CARGO_ENCODED_RUSTFLAGS ${semanticFlags((process.env.CARGO_ENCODED_RUSTFLAGS ?? '').split('\x1f').filter(Boolean)).join(' ')}`,
);

for (const f of configFiles()) {
  let cfg;
  try {
    cfg = readToml(f);
  } catch {
    lines.push(`config unreadable`);
    continue;
  }
  const flags = [...(cfg.build?.rustflags ?? [])];
  for (const t of Object.values(cfg.target ?? {})) flags.push(...(t?.rustflags ?? []));
  const sem = semanticFlags(flags.map(String));
  if (sem.length) lines.push(`config rustflags ${sem.join(' ')}`);
  for (const [profile, settings] of Object.entries(cfg.profile ?? {})) {
    for (const key of ['opt-level', 'debug-assertions', 'overflow-checks', 'panic', 'codegen-units']) {
      if (settings?.[key] !== undefined) lines.push(`config profile.${profile}.${key}=${settings[key]}`);
    }
  }
}
for (const [k, v] of Object.entries(process.env).sort()) {
  if (/^CARGO_PROFILE_[A-Z_]+_(OPT_LEVEL|DEBUG_ASSERTIONS|OVERFLOW_CHECKS|PANIC|CODEGEN_UNITS)$/.test(k)) lines.push(`${k}=${v}`);
}
process.stdout.write(`${lines.join('\n')}\n`);
