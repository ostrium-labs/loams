// `loams-listing from-package <dir>`: an npm package's `loams.agent` block
// (design 73 section 3.6, Q1125) plus what package.json already says becomes
// listing.json. Authors write only what npm metadata cannot express.
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { LISTING_SCHEMA_ID, type Listing } from './types.js';

/** Fields package.json supplies; the block may not set them. */
const DERIVED = ['version', 'licence', 'source', 'artifact', 'provenance', 'runtime', 'pricing'] as const;

/** The engines of the node runtime (D1808). `engine` is the one runtime field the block sets. */
const ENGINES = ['node', 'bun'] as const;
type EngineName = (typeof ENGINES)[number];
const DEFAULT_RANGE: Record<EngineName, string> = { node: '>=22.13', bun: '>=1.4.4' };

/** The listing's field order, so generated files diff cleanly. */
const ORDER: (keyof Listing)[] = [
  '$schema', 'id', 'kind', 'version', 'title', 'summary', 'description', 'icon', 'licence', 'source',
  'publisher', 'support', 'security_contact', 'framework', 'language', 'artifact', 'provenance', 'runtime',
  'entrypoint', 'targets', 'surfaces', 'loams', 'connectors', 'mcp_servers', 'tools', 'place', 'models',
  'cost', 'config', 'conformance', 'harness', 'mcp_server', 'pricing',
];

/** Frameworks whose version is read from the package's dependencies when the block omits it. */
const FRAMEWORK_PACKAGES: Record<string, string> = {
  mastra: '@mastra/core',
  'openai-agents': '@openai/agents',
  langgraph: '@langchain/langgraph',
  eve: 'eve',
};

export class PackageError extends Error {}

interface PackageJson {
  name?: string;
  version?: string;
  description?: string;
  license?: string;
  main?: string;
  repository?: string | { type?: string; url?: string; directory?: string };
  engines?: { node?: string; bun?: string };
  dependencies?: Record<string, string>;
  devDependencies?: Record<string, string>;
  loams?: { agent?: Record<string, unknown> };
}

export interface FromPackageOptions {
  /** The source commit; default `git rev-parse HEAD` in the package directory. */
  commit?: string;
  /** The tarball's npm integrity (`sha512-…`). */
  integrity?: string;
  /** A packed tarball whose sha512 becomes the integrity. */
  tarball?: string;
}

export function integrityOf(bytes: Uint8Array): string {
  return `sha512-${createHash('sha512').update(bytes).digest('base64')}`;
}

/** `git+https://github.com/o/r.git` and `github:o/r` → `https://github.com/o/r`. */
export function normaliseRepository(url: string): string {
  let out = url.trim();
  const short = /^github:([^/]+\/[^/]+)$/.exec(out);
  if (short) out = `https://github.com/${short[1]}`;
  out = out.replace(/^git\+/, '').replace(/^git:\/\//, 'https://').replace(/^ssh:\/\/git@/, 'https://');
  out = out.replace(/^git@([^:]+):/, 'https://$1/');
  return out.replace(/\.git$/, '').replace(/\/$/, '');
}

export function attestationsUrl(name: string, version: string): string {
  return `https://registry.npmjs.org/-/npm/v1/attestations/${name}@${version}`;
}

function ordered(listing: Record<string, unknown>): Listing {
  const out: Record<string, unknown> = {};
  for (const key of ORDER) if (listing[key] !== undefined) out[key] = listing[key];
  for (const key of Object.keys(listing).sort()) if (!(key in out)) out[key] = listing[key];
  return out as unknown as Listing;
}

/** Builds the listing from parsed package.json (pure; see {@link fromPackageDir} for the files). */
export function fromPackageJson(pkg: PackageJson, options: { commit: string; integrity: string }): Listing {
  const block = pkg.loams?.agent;
  if (!block || typeof block !== 'object') throw new PackageError('package.json has no loams.agent block');
  for (const key of DERIVED) {
    if (key in block) throw new PackageError(`loams.agent.${key} comes from package.json; remove it from the block`);
  }
  if (!pkg.name || !pkg.version) throw new PackageError('package.json needs name and version');
  if (!pkg.license) throw new PackageError('package.json needs a license (SPDX)');
  const repository = typeof pkg.repository === 'string' ? { url: pkg.repository } : (pkg.repository ?? {});
  if (!repository.url) throw new PackageError('package.json needs repository.url, the provenance source');
  const { engine: engineField, ...rest } = block;
  if (engineField !== undefined && !ENGINES.includes(engineField as EngineName)) {
    throw new PackageError(`loams.agent.engine is node or bun, not ${JSON.stringify(engineField)}`);
  }
  // `engines.bun` alone does not choose Bun: many packages declare both engines.
  const engine = (engineField as EngineName | undefined) ?? 'node';
  const kind = (rest.kind as string | undefined) ?? 'agent';
  if (kind === 'mcp-server' && (block.mcp_server as { mode?: string } | undefined)?.mode === 'remote') {
    throw new PackageError('a remote MCP server installs no package; write its listing.json by hand');
  }

  const framework = block.framework as { name?: string; version?: string } | undefined;
  let frameworkOut = framework;
  if (framework?.name && framework.version === undefined) {
    const dep = FRAMEWORK_PACKAGES[framework.name];
    const range = dep ? (pkg.dependencies?.[dep] ?? pkg.devDependencies?.[dep]) : undefined;
    if (range) frameworkOut = { ...framework, version: range.replace(/^[\^~=]/, '') };
  }

  const listing: Record<string, unknown> = {
    $schema: LISTING_SCHEMA_ID,
    ...rest,
    kind,
    version: pkg.version,
    title: block.title ?? pkg.name,
    summary: block.summary ?? pkg.description,
    licence: pkg.license,
    source: {
      repository: normaliseRepository(repository.url),
      commit: options.commit,
      ...(repository.directory ? { path: repository.directory } : {}),
    },
    framework: frameworkOut,
    artifact: { type: 'npm', name: pkg.name, version: pkg.version, integrity: options.integrity },
    provenance: { type: 'npm-slsa', ref: attestationsUrl(pkg.name, pkg.version) },
    runtime: {
      kind: 'node',
      ...(engineField !== undefined ? { engine } : {}),
      version: pkg.engines?.[engine] ?? DEFAULT_RANGE[engine],
    },
    entrypoint: block.entrypoint ?? pkg.main,
    pricing: 'free',
  };
  if (kind === 'agent' && listing.targets === undefined) listing.targets = ['agentd'];
  return ordered(listing);
}

/** The inverse of {@link fromPackageJson}: the package.json fields and block a listing came from. */
export function toPackageJson(listing: Listing): PackageJson {
  const block: Record<string, unknown> = { ...listing };
  for (const key of [...DERIVED, '$schema'] as const) delete block[key];
  const engine = listing.runtime?.engine;
  if (engine) block.engine = engine;
  if (listing.artifact?.type !== 'npm') throw new PackageError('only npm listings come from a package');
  return {
    name: listing.artifact.name,
    version: listing.version,
    license: listing.licence,
    repository: {
      type: 'git',
      url: `git+${listing.source.repository}.git`,
      ...(listing.source.path ? { directory: listing.source.path } : {}),
    },
    ...(listing.runtime?.version ? { engines: { [engine ?? 'node']: listing.runtime.version } } : {}),
    loams: { agent: block },
  };
}

function gitHead(dir: string): string {
  try {
    return execFileSync('git', ['-C', dir, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim();
  } catch {
    throw new PackageError(`${dir} is not in a Git checkout; pass --commit`);
  }
}

export function fromPackageDir(dir: string, options: FromPackageOptions = {}): Listing {
  const pkg = JSON.parse(readFileSync(join(dir, 'package.json'), 'utf8')) as PackageJson;
  let integrity = options.integrity;
  if (options.tarball) integrity = integrityOf(readFileSync(options.tarball));
  if (!integrity) throw new PackageError('pass --tarball <file> or --integrity <sha512-…>: the listing pins the exact bytes');
  return fromPackageJson(pkg, { commit: options.commit ?? gitHead(dir), integrity });
}
