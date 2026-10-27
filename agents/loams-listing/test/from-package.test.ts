import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, describe, expect, it } from 'vite-plus/test';
import { fromPackageDir, fromPackageJson, integrityOf, normaliseRepository, PackageError, toPackageJson } from '../src/from-package.js';
import { lint } from '../src/lint.js';
import { fixture } from './helpers.js';

const COMMIT = '0123456789abcdef0123456789abcdef01234567';

let dirs: string[] = [];
afterEach(() => {
  for (const dir of dirs) rmSync(dir, { recursive: true, force: true });
  dirs = [];
});

function tempDir(): string {
  const dir = mkdtempSync(join(tmpdir(), 'loams-listing-'));
  dirs.push(dir);
  return dir;
}

describe('from-package', () => {
  it('from_package_round_trips', () => {
    const listing = fixture('valid/agent-mastra-acp.json');
    // A walrus location exists only once the tarball is stored (D1812, MS1
    // Task 9's publish step), so package.json never carries one.
    delete listing.artifact!.locations;
    const pkg = toPackageJson(listing);
    const again = fromPackageJson(pkg, { commit: listing.source.commit, integrity: listing.artifact!.integrity! });
    expect(again).toEqual(listing);
    expect(lint(again)).toEqual([]);
  });

  it('reads a package directory and pins the tarball bytes', () => {
    const dir = tempDir();
    const tarball = join(dir, 'agent.tgz');
    writeFileSync(tarball, 'not really a tarball, but bytes all the same');
    writeFileSync(
      join(dir, 'package.json'),
      JSON.stringify({
        name: '@loams/hello-agent',
        version: '0.2.0',
        description: 'Says hello from Loams.',
        license: 'Apache-2.0',
        main: 'dist/index.js',
        repository: { type: 'git', url: 'git+https://github.com/ostrium-labs/loams-mastra.git', directory: 'examples/hello' },
        engines: { node: '>=22.13' },
        dependencies: { '@mastra/core': '^1.4.0' },
        loams: {
          agent: {
            id: 'ostrium/hello-agent',
            publisher: { name: 'Ostrium Labs' },
            framework: { name: 'mastra' },
            language: 'typescript',
            surfaces: { acp: {} },
            tools: [],
            models: { api: ['chat-completions'] },
          },
        },
      }),
    );
    const listing = fromPackageDir(dir, { commit: COMMIT, tarball });
    expect(lint(listing)).toEqual([]);
    expect(listing).toMatchObject({
      id: 'ostrium/hello-agent',
      kind: 'agent',
      version: '0.2.0',
      title: '@loams/hello-agent',
      summary: 'Says hello from Loams.',
      licence: 'Apache-2.0',
      source: { repository: 'https://github.com/ostrium-labs/loams-mastra', commit: COMMIT, path: 'examples/hello' },
      framework: { name: 'mastra', version: '1.4.0' },
      artifact: { type: 'npm', name: '@loams/hello-agent', version: '0.2.0', integrity: integrityOf(Buffer.from('not really a tarball, but bytes all the same')) },
      provenance: { type: 'npm-slsa', ref: 'https://registry.npmjs.org/-/npm/v1/attestations/@loams/hello-agent@0.2.0' },
      runtime: { kind: 'node', version: '>=22.13' },
      entrypoint: 'dist/index.js',
      targets: ['agentd'],
      pricing: 'free',
    });
  });

  it('from_package_round_trips_on_bun', () => {
    const listing = fixture('valid/agent-mastra-bun.json');
    const pkg = toPackageJson(listing);
    expect(pkg.engines).toEqual({ bun: '>=1.4.4' });
    expect(pkg.loams!.agent!.engine).toBe('bun');
    const again = fromPackageJson(pkg, { commit: listing.source.commit, integrity: listing.artifact!.integrity! });
    expect(again).toEqual(listing);
    expect(lint(again)).toEqual([]);
  });

  it('reads engine from the block and the range from engines.bun', () => {
    const pkg = toPackageJson(fixture('valid/agent-mastra-acp.json'));
    pkg.engines = { node: '>=22.13', bun: '>=1.4.4, <2.0.0' };
    // engines.bun alone does not choose Bun.
    expect(fromPackageJson(pkg, { commit: COMMIT, integrity: 'sha512-x' }).runtime).toEqual({ kind: 'node', version: '>=22.13' });
    (pkg.loams!.agent as Record<string, unknown>).engine = 'bun';
    expect(fromPackageJson(pkg, { commit: COMMIT, integrity: 'sha512-x' }).runtime).toEqual({ kind: 'node', engine: 'bun', version: '>=1.4.4, <2.0.0' });
    delete pkg.engines;
    expect(fromPackageJson(pkg, { commit: COMMIT, integrity: 'sha512-x' }).runtime).toEqual({ kind: 'node', engine: 'bun', version: '>=1.4.4' });
    (pkg.loams!.agent as Record<string, unknown>).engine = 'deno';
    expect(() => fromPackageJson(pkg, { commit: COMMIT, integrity: 'sha512-x' })).toThrow(/node or bun/);
  });

  it('refuses a block that sets a field package.json owns', () => {
    const pkg = toPackageJson(fixture('valid/agent-mastra-acp.json'));
    (pkg.loams!.agent as Record<string, unknown>).version = '9.9.9';
    expect(() => fromPackageJson(pkg, { commit: COMMIT, integrity: 'sha512-x' })).toThrow(PackageError);
  });

  it('refuses a package without integrity', () => {
    const dir = tempDir();
    writeFileSync(join(dir, 'package.json'), JSON.stringify({ name: 'x', version: '1.0.0', license: 'MIT', repository: 'github:o/r', loams: { agent: {} } }));
    expect(() => fromPackageDir(dir, { commit: COMMIT })).toThrow(/--tarball/);
  });

  it('normalises repository URLs', () => {
    expect(normaliseRepository('git+https://github.com/o/r.git')).toBe('https://github.com/o/r');
    expect(normaliseRepository('github:o/r')).toBe('https://github.com/o/r');
    expect(normaliseRepository('git@github.com:o/r.git')).toBe('https://github.com/o/r');
  });
});
