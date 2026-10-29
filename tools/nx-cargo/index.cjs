// Nx plugin: one Nx project per crate of the Cargo workspaces named in the plugin options, with
// cacheable `test`, `clippy` and `fmt` targets (docs/guides/nx-cloud.md, Rust).
//
// - The targets have no outputs: Nx caches the result and the log only. Compilation stays in
//   Cargo's target directory and the CI Rust caches.
// - `test` runs `cargo nextest run --workspace -E 'package(=<crate>)'`: the workspace build, with
//   the workspace's feature unification, so per-crate runs share one set of compiled artefacts,
//   and only the crate's tests run. A workspace whose CI runs `cargo test` (inputs.json
//   `"test": "cargo-test"`, fabric/) gets `cargo test -p <crate>` instead.
// - `clippy` runs `cargo clippy -p <crate> --all-targets`; `fmt` runs `cargo fmt -p <crate> --check`.
// - The inputs: the crate's files, its path dependencies' files (transitively, through the Nx
//   project graph), the workspace's manifests, lockfile and toolchain files, proto/, the compile
//   settings (rust-env.cjs), and what inputs.json declares (repoWide, extra, tools, noCache).
//
// nx.json: { "plugin": "./tools/nx-cargo/index.cjs", "options": { "workspaces": ["Cargo.toml"] } }
'use strict';

const path = require('node:path');
const { DependencyType } = require('nx/src/devkit-exports');

const { readWorkspace } = require('./workspace.cjs');

const config = require('./inputs.json');

function workspaceGlobals(root) {
  const r = root === '.' ? '' : `${root}/`;
  return [
    `{workspaceRoot}/${r}Cargo.toml`,
    `{workspaceRoot}/${r}Cargo.lock`,
    `{workspaceRoot}/${r}rust-toolchain.toml`,
    `{workspaceRoot}/${r}proto/**/*`,
    // Cargo and clippy read these from the crate's directory up to the repository root.
    '{workspaceRoot}/clippy.toml',
    '{workspaceRoot}/rustfmt.toml',
    '{workspaceRoot}/tools/nx-cargo/rust-env.cjs',
    { runtime: `node tools/nx-cargo/rust-env.cjs ${root}` },
  ];
}

function projectsFor(manifest, workspaceRoot) {
  const ws = config.workspaces[manifest] ?? {};
  const { root, crates } = readWorkspace(workspaceRoot, manifest);
  const repoWide = new Set(ws.repoWide ?? []);
  const globals = workspaceGlobals(root);
  // `nextest` (the default) or `cargo-test`, as the workspace's CI runs its tests.
  const nextest = (ws.test ?? 'nextest') === 'nextest';
  const projects = {};
  for (const c of crates) {
    const extra = (ws.extra?.[c.name] ?? []).map((g) => `{workspaceRoot}/${g.endsWith('**') ? `${g}/*` : g}`);
    const tools = (ws.tools?.[c.name] ?? []).map((cmd) => ({ runtime: cmd }));
    const wide = repoWide.has(c.name) ? ['{workspaceRoot}/**/*'] : [];
    const cache = !(c.name in (ws.noCache ?? {}));
    const compileInputs = ['cargoCrate', '^cargoCrate', ...globals, ...extra, ...wide];
    const run = (command) => ({ cwd: root, command });
    projects[c.dir] = {
      name: c.name,
      root: c.dir,
      projectType: 'library',
      tags: ['language:rust', 'cargo-crate', `cargo-workspace:${root === '.' ? 'root' : root}`],
      namedInputs: { cargoCrate: ['{projectRoot}/**/*', '!{projectRoot}/target/**/*'] },
      targets: {
        test: nextest
          ? {
              executor: 'nx:run-commands',
              cache,
              inputs: [
                ...compileInputs,
                `{workspaceRoot}/${root === '.' ? '' : `${root}/`}.config/nextest.toml`,
                { runtime: 'cargo nextest --version' },
                ...tools,
              ],
              outputs: [],
              options: run(`cargo nextest run --workspace --locked --no-tests=pass -E 'package(=${c.name})'`),
              metadata: { description: `${c.name}'s tests (nextest), in the workspace build` },
            }
          : {
              executor: 'nx:run-commands',
              cache,
              inputs: [...compileInputs, ...tools],
              outputs: [],
              options: run(`cargo test -p ${c.name} --locked --no-fail-fast`),
              metadata: { description: `${c.name}'s tests (cargo test)` },
            },
        clippy: {
          executor: 'nx:run-commands',
          cache: true,
          inputs: compileInputs,
          outputs: [],
          options: run(`cargo clippy -p ${c.name} --all-targets --locked -- -D warnings`),
          metadata: { description: `clippy on ${c.name}, all targets, warnings denied` },
        },
        fmt: {
          executor: 'nx:run-commands',
          cache: true,
          inputs: [
            '{projectRoot}/**/*.rs',
            '{workspaceRoot}/rustfmt.toml',
            `{workspaceRoot}/${root === '.' ? '' : `${root}/`}rust-toolchain.toml`,
            { runtime: `node tools/nx-cargo/rust-env.cjs ${root}` },
          ],
          outputs: [],
          options: run(`cargo fmt -p ${c.name} --check`),
          metadata: { description: `rustfmt check of ${c.name}` },
        },
      },
    };
  }
  return { projects, crates };
}

const workspacesOf = (options) => options?.workspaces ?? ['Cargo.toml'];

exports.name = 'loams-nx-cargo';

exports.createNodes = [
  '**/Cargo.toml',
  (files, options, context) => {
    const wanted = new Set(workspacesOf(options));
    return files
      .filter((f) => wanted.has(f))
      .map((f) => [f, { projects: projectsFor(f, context.workspaceRoot).projects }]);
  },
];

exports.createDependencies = (options, context) => {
  const deps = [];
  for (const manifest of workspacesOf(options)) {
    const { crates } = readWorkspace(context.workspaceRoot, manifest);
    for (const c of crates) {
      for (const d of c.deps) {
        if (!context.projects[c.name] || !context.projects[d]) continue;
        deps.push({
          source: c.name,
          target: d,
          type: DependencyType.static,
          sourceFile: path.posix.join(c.dir, 'Cargo.toml'),
        });
      }
    }
  }
  return deps;
};
