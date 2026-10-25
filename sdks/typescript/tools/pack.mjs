#!/usr/bin/env node
// Builds the npm packages `@loams/live` and `@loams/client` the registry gets.
//
//   node sdks/typescript/tools/pack.mjs [--version <semver>] [--smoke]
//
// In the workspace both packages export their TypeScript sources directly
// (`./src/index.ts`), which is right for the console and the conformance suite
// and wrong for npm: Node does not strip types inside node_modules, and a
// consumer's bundler should not have to compile someone else's package. So the
// release stages each package into `packages/<name>/build/npm/` (a `build`
// directory, so neither git nor the monorepo check sees the staged package.json):
//
//   * `src/` copied, then compiled by the package's own pinned `tsc` into
//     `dist/` (ES2022 JavaScript plus `.d.ts`), with NodeNext resolution so the
//     output runs in Node, Bun and any bundler as-is;
//   * `@loams/proto`, the console's private workspace package, **vendored** into
//     `@loams/client`: the files the client imports (and the files those import)
//     are copied under `src/vendor/proto/` and the imports rewritten to relative
//     paths. `@loams/proto` is not published, so a dependency on it would be a
//     dependency nobody can install;
//   * a package.json with the release version, `dist/` exports, `workspace:*`
//     replaced by the exact release version, and no dev-only fields;
//   * README.md with repository-relative links made absolute, and the LICENSE.
//
// `--smoke` then packs both, installs the tarballs into a scratch project next
// to the stage, imports both entry points in Node, and type-checks a consumer
// under NodeNext. Nothing here publishes; sdk-release.yml runs `npm publish` on
// the staged directories.
import { execFileSync } from 'node:child_process';
import { cpSync, existsSync, mkdirSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { dirname, join, posix, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const sdk = resolve(here, '..');
const repo = resolve(sdk, '../..');
const protoPkg = join(repo, 'web/packages/proto');
const GITHUB = 'https://github.com/ostrium-labs/loams';
const BRANCH = 'dev';

function args() {
  const out = { version: null, smoke: false };
  const argv = process.argv.slice(2);
  for (let i = 0; i < argv.length; i++) {
    if (argv[i] === '--version') out.version = argv[++i];
    else if (argv[i] === '--smoke') out.smoke = true;
    else throw new Error(`unknown argument ${argv[i]}`);
  }
  return out;
}

const readJson = (path) => JSON.parse(readFileSync(path, 'utf8'));
const writeJson = (path, value) => writeFileSync(path, `${JSON.stringify(value, null, 2)}\n`);

function walk(dir, files = []) {
  for (const name of readdirSync(dir)) {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) walk(path, files);
    else files.push(path);
  }
  return files;
}

// `./src/x/y.ts` -> `./dist/x/y.js` (+ `.d.ts` for types).
const toDist = (path, ext) => path.replace(/^\.\/src\//, './dist/').replace(/\.ts$/, ext);

function distExports(exportsMap) {
  const out = {};
  for (const [key, target] of Object.entries(exportsMap)) {
    const source = typeof target === 'string' ? target : target.default;
    if (!source.startsWith('./src/') || !source.endsWith('.ts')) {
      throw new Error(`export ${key} -> ${source} is not a ./src/*.ts entry; teach pack.mjs about it`);
    }
    out[key] = { types: toDist(source, '.d.ts'), default: toDist(source, '.js') };
  }
  return out;
}

// Links in a README that are relative to its place in the repository point
// nowhere on npmjs.com, so they are made absolute: images to raw content, the
// rest to the blob view.
function absoluteLinks(markdown, readmeDir) {
  return markdown.replace(/(!?)\[([^\]]*)\]\(([^)\s]+)\)/g, (whole, bang, text, target) => {
    if (/^(https?:|mailto:|#)/.test(target)) return whole;
    const path = posix.normalize(posix.join(readmeDir, target));
    const base = bang ? `https://raw.githubusercontent.com/ostrium-labs/loams/${BRANCH}` : `${GITHUB}/blob/${BRANCH}`;
    return `${bang}[${text}](${base}/${path})`;
  });
}

const PREVIEW_NOTE = (version) =>
  `> **Preview.** \`${version}\` tracks the Loams preview server and may change between previews; ` +
  'pin an exact version. Installed with `npm install <name>@preview`.\n\n';

function readme(stage, sourceReadme, version, name) {
  const dir = posix.normalize(relative(repo, dirname(sourceReadme)).split('\\').join('/'));
  const body = absoluteLinks(readFileSync(sourceReadme, 'utf8'), dir);
  // After the banner and the title, so npm's card shows the title first.
  const lines = body.split('\n');
  const title = lines.findIndex((line) => line.startsWith('# '));
  lines.splice(title + 1, 0, '', PREVIEW_NOTE(version).replace('<name>', name).trimEnd());
  writeFileSync(join(stage, 'README.md'), lines.join('\n'));
}

// Every `@loams/proto/<sub>` import in the staged sources, resolved through the
// proto package's exports, plus the relative imports of those files, copied
// under src/vendor/proto with the same layout as web/packages/proto/src.
function vendorProto(stageSrc) {
  const protoExports = readJson(join(protoPkg, 'package.json')).exports;
  const subpath = (spec) => {
    const key = spec === '@loams/proto' ? '.' : `./${spec.slice('@loams/proto/'.length)}`;
    const target = protoExports[key];
    if (!target) throw new Error(`${spec} is not exported by @loams/proto`);
    return typeof target === 'string' ? target : target.default;
  };
  const pending = [];
  const copied = new Set();
  const importRe = /(from\s+|import\s*\(\s*)(['"])([^'"]+)\2/g;
  const sources = walk(stageSrc).filter((file) => file.endsWith('.ts'));
  for (const file of sources) {
    let text = readFileSync(file, 'utf8');
    text = text.replace(importRe, (whole, lead, quote, spec) => {
      if (spec !== '@loams/proto' && !spec.startsWith('@loams/proto/')) return whole;
      const target = subpath(spec).replace(/^\.\/src\//, '');
      pending.push(target);
      const vendored = join(stageSrc, 'vendor/proto', target);
      let rel = relative(dirname(file), vendored).split('\\').join('/').replace(/\.ts$/, '.js');
      if (!rel.startsWith('.')) rel = `./${rel}`;
      return `${lead}${quote}${rel}${quote}`;
    });
    writeFileSync(file, text);
  }
  while (pending.length) {
    const target = pending.pop();
    if (copied.has(target)) continue;
    copied.add(target);
    const from = join(protoPkg, 'src', target);
    const to = join(stageSrc, 'vendor/proto', target);
    mkdirSync(dirname(to), { recursive: true });
    cpSync(from, to);
    for (const match of readFileSync(from, 'utf8').matchAll(importRe)) {
      const spec = match[3];
      if (spec.startsWith('.')) {
        pending.push(posix.normalize(posix.join(posix.dirname(target), spec.replace(/\.js$/, '.ts'))));
      } else if (spec.startsWith('@loams/')) {
        throw new Error(`${target} imports ${spec}; vendoring only follows relative imports`);
      }
    }
  }
  return [...copied].sort();
}

function compile(pkgDir, stage, extraPaths) {
  const tsconfig = {
    compilerOptions: {
      target: 'ES2022',
      lib: ['ES2023', 'DOM', 'DOM.Iterable'],
      module: 'NodeNext',
      moduleResolution: 'NodeNext',
      strict: true,
      skipLibCheck: true,
      isolatedModules: true,
      verbatimModuleSyntax: true,
      declaration: true,
      rootDir: 'src',
      outDir: 'dist',
      types: [],
      ...(extraPaths ? { paths: extraPaths } : {}),
    },
    include: ['src'],
  };
  writeJson(join(stage, 'tsconfig.json'), tsconfig);
  execFileSync(join(pkgDir, 'node_modules/.bin/tsc'), ['-p', join(stage, 'tsconfig.json')], { stdio: 'inherit' });
  // The build input is not part of the package.
  rmSync(join(stage, 'tsconfig.json'));
}

function stagePackage(name, version, { vendor = false, paths = null } = {}) {
  const pkgDir = join(sdk, 'packages', name);
  const stage = join(pkgDir, 'build/npm');
  rmSync(stage, { recursive: true, force: true });
  mkdirSync(stage, { recursive: true });
  cpSync(join(pkgDir, 'src'), join(stage, 'src'), { recursive: true });
  const vendored = vendor ? vendorProto(join(stage, 'src')) : [];
  compile(pkgDir, stage, paths);
  rmSync(join(stage, 'src'), { recursive: true });

  const source = readJson(join(pkgDir, 'package.json'));
  if (source.version !== version) {
    throw new Error(`${source.name} is ${source.version} in package.json, the release is ${version}: run scripts/sdk/sdk-version.py set`);
  }
  const dependencies = {};
  for (const [dep, spec] of Object.entries(source.dependencies ?? {})) {
    if (dep === '@loams/proto') continue; // vendored above
    if (String(spec).startsWith('workspace:')) dependencies[dep] = version;
    else dependencies[dep] = spec;
  }
  const manifest = {
    name: source.name,
    version,
    description: source.description,
    license: source.license,
    homepage: `${GITHUB}/tree/${BRANCH}/sdks/typescript`,
    bugs: `${GITHUB}/issues`,
    // npm checks a provenance statement against this URL.
    repository: { ...source.repository, url: `git+${GITHUB}.git` },
    keywords: ['loams', 'connect', 'grpc', 'sdk', 'database'],
    type: 'module',
    sideEffects: false,
    exports: distExports(source.exports),
    types: './dist/index.d.ts',
    files: ['dist', 'README.md', 'LICENSE'],
    ...(source.engines ? { engines: source.engines } : {}),
    dependencies,
    publishConfig: { access: 'public' },
  };
  writeJson(join(stage, 'package.json'), manifest);
  const readmePath = existsSync(join(pkgDir, 'README.md')) ? join(pkgDir, 'README.md') : join(sdk, 'README.md');
  readme(stage, readmePath, version, source.name);
  cpSync(join(repo, 'LICENSE'), join(stage, 'LICENSE'));
  console.log(`staged ${source.name}@${version} in ${relative(repo, stage)}${vendored.length ? ` (vendored ${vendored.join(', ')})` : ''}`);
  return stage;
}

function smoke(stages, version) {
  const out = join(sdk, 'packages/client/build/smoke');
  rmSync(out, { recursive: true, force: true });
  mkdirSync(out, { recursive: true });
  const tarballs = stages.map((stage) => {
    // npm <= 11 prints an array, npm 12 an object keyed by package name.
    const parsed = JSON.parse(execFileSync('npm', ['pack', '--json', '--pack-destination', out], { cwd: stage, encoding: 'utf8' }));
    const [result] = Array.isArray(parsed) ? parsed : Object.values(parsed);
    return join(out, result.filename);
  });
  writeJson(join(out, 'package.json'), { name: 'loams-pack-smoke', private: true, type: 'module' });
  execFileSync('npm', ['install', '--no-audit', '--no-fund', '--ignore-scripts', ...tarballs], { cwd: out, stdio: 'inherit' });
  // Both entry points load in plain Node, and the version is the release's.
  writeFileSync(
    join(out, 'load.mjs'),
    [
      "import * as client from '@loams/client';",
      "import * as node from '@loams/client/node';",
      "import * as live from '@loams/live';",
      "import { createRequire } from 'node:module';",
      "const require = createRequire(import.meta.url);",
      "if (typeof client.Loams !== 'function') throw new Error('@loams/client has no Loams export');",
      "if (typeof node.createNodeTransport !== 'function') throw new Error('@loams/client/node has no createNodeTransport');",
      "if (Object.keys(live).length === 0) throw new Error('@loams/live exports nothing');",
      `const want = ${JSON.stringify(version)};`,
      "for (const name of ['@loams/client', '@loams/live']) {",
      "  const { version } = JSON.parse(require('node:fs').readFileSync(`node_modules/${name}/package.json`, 'utf8'));",
      "  if (version !== want) throw new Error(`${name} installed as ${version}, want ${want}`);",
      "}",
      "console.log('loaded @loams/client and @loams/live in node', process.version);",
      '',
    ].join('\n'),
  );
  execFileSync('node', ['load.mjs'], { cwd: out, stdio: 'inherit' });
  // A consumer that type-checks against the published .d.ts under NodeNext.
  writeFileSync(
    join(out, 'consumer.ts'),
    [
      "import { Loams, apiKey } from '@loams/client';",
      "import { createNodeTransport } from '@loams/client/node';",
      "const loams = new Loams({ endpoint: 'http://127.0.0.1:8080', auth: apiKey('k'), transport: createNodeTransport() });",
      'export const call = () => loams.instance.getInstance({});',
      '',
    ].join('\n'),
  );
  writeJson(join(out, 'tsconfig.json'), {
    compilerOptions: { target: 'ES2022', module: 'NodeNext', moduleResolution: 'NodeNext', strict: true, noEmit: true, types: [], lib: ['ES2023', 'DOM'] },
    files: ['consumer.ts'],
  });
  execFileSync(join(sdk, 'packages/client/node_modules/.bin/tsc'), ['-p', join(out, 'tsconfig.json')], { stdio: 'inherit' });
  console.log(`smoke: installed ${tarballs.map((t) => relative(repo, t)).join(', ')}, loaded in Node, type-checked a NodeNext consumer`);
}

const { version: requested, smoke: runSmoke } = args();
const version = requested?.replace(/^v/, '') ?? readJson(join(sdk, 'packages/client/package.json')).version;
const live = stagePackage('live', version);
const client = stagePackage('client', version, {
  vendor: true,
  // Type-check against the staged @loams/live, which is what the client will
  // install; the emitted import stays `@loams/live/live`.
  paths: {
    '@loams/live': ['../../../live/build/npm/dist/index.d.ts'],
    '@loams/live/live': ['../../../live/build/npm/dist/live.d.ts'],
    '@loams/live/options': ['../../../live/build/npm/dist/gen/loams/options/v1/options_pb.d.ts'],
    '@loams/live/value': ['../../../live/build/npm/dist/gen/loams/live/v1/value_pb.d.ts'],
  },
});
if (runSmoke) smoke([live, client], version);
