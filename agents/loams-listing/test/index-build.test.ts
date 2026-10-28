import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, describe, expect, it } from 'vite-plus/test';
import { buildIndex, canonical, IndexError, writeIndex } from '../src/index-build.js';
import { validateAgainst } from '../src/schema.js';
import type { Listing } from '../src/types.js';
import { fixture, validFixtures } from './helpers.js';

let dirs: string[] = [];
afterEach(() => {
  for (const dir of dirs) rmSync(dir, { recursive: true, force: true });
  dirs = [];
});

function tempDir(): string {
  const dir = mkdtempSync(join(tmpdir(), 'loams-index-'));
  dirs.push(dir);
  return dir;
}

function put(root: string, listing: Listing, text = `${JSON.stringify(listing, null, 2)}\n`, yanked?: string): void {
  const dir = join(root, 'listings', listing.id, 'versions', listing.version);
  mkdirSync(dir, { recursive: true });
  writeFileSync(join(dir, 'listing.json'), text);
  if (yanked) writeFileSync(join(dir, 'yanked.json'), JSON.stringify({ reason: yanked }));
}

/** Reverses an object's key order at every level: same data, different bytes. */
function reversed(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(reversed);
  if (value && typeof value === 'object') {
    const out: Record<string, unknown> = {};
    for (const key of Object.keys(value).reverse()) out[key] = reversed((value as Record<string, unknown>)[key]);
    return out;
  }
  return value;
}

function withVersion(listing: Listing, version: string): Listing {
  const out = structuredClone(listing);
  out.version = version;
  if (out.artifact?.type === 'npm') out.artifact.version = version;
  if (out.mcp_server) out.mcp_server.server = { ...out.mcp_server.server, version };
  return out;
}

const REVOCATIONS = {
  revocations: [
    { id: 'ostrium/loams-starter', versions: '>=0.1.0, <0.1.1', severity: 'disable', reason: 'Leaked a token.', date: '2026-10-22' },
    { id: 'ostrium/github-mcp-server', versions: '=2.0.3', severity: 'warn', reason: 'Upstream advisory.', date: '2026-10-21' },
  ],
};

function seed(root: string, order: 'forward' | 'backward'): void {
  const listings = validFixtures().map(fixture);
  const all = [...listings, withVersion(listings[0]!, '0.1.1'), withVersion(listings[0]!, '0.2.0-rc.1')];
  for (const listing of order === 'forward' ? all : [...all].reverse()) {
    const text = order === 'forward' ? `${JSON.stringify(listing, null, 2)}\n` : JSON.stringify(reversed(listing));
    put(root, listing, text);
  }
  const revocations = order === 'forward' ? REVOCATIONS : { revocations: [...REVOCATIONS.revocations].reverse().map(reversed) };
  writeFileSync(join(root, 'revocations.json'), JSON.stringify(revocations));
}

describe('index build', () => {
  it('index_build_is_deterministic', () => {
    const a = tempDir();
    const b = tempDir();
    seed(a, 'forward');
    seed(b, 'backward');
    const outA = writeIndex(buildIndex(a), join(a, 'out'));
    const outB = writeIndex(buildIndex(b), join(b, 'out'));
    const again = writeIndex(buildIndex(a), join(a, 'again'));
    for (let i = 0; i < outA.length; i++) {
      const bytes = readFileSync(outA[i]!, 'utf8');
      expect(readFileSync(outB[i]!, 'utf8')).toBe(bytes);
      expect(readFileSync(again[i]!, 'utf8')).toBe(bytes);
    }
  });

  it('writes an index that matches index.v1.json, sorted by id and version', () => {
    const root = tempDir();
    seed(root, 'forward');
    const { index, revocations } = buildIndex(root, { sourceCommit: 'a'.repeat(40) });
    expect(validateAgainst('index.v1.json', index)).toEqual([]);
    expect(validateAgainst('revocations.v1.json', revocations)).toEqual([]);
    const ids = index.listings.map((l) => l.id);
    expect(ids).toEqual([...ids].sort());
    const starter = index.listings.find((l) => l.id === 'ostrium/loams-starter')!;
    expect(starter.versions.map((v) => v.version)).toEqual(['0.1.0', '0.1.1', '0.2.0-rc.1']);
    expect(starter.latest).toBe('0.1.1');
    expect(starter.tier).toBe('first-party');
    expect(index.listings.find((l) => l.id === 'example-org/research-assistant')!.tier).toBe('community');
    expect(index.source_commit).toBe('a'.repeat(40));
  });

  it('pins each version by the sha256 of its canonical listing', async () => {
    const root = tempDir();
    seed(root, 'forward');
    const { index } = buildIndex(root);
    const { createHash } = await import('node:crypto');
    for (const entry of index.listings) {
      for (const version of entry.versions) {
        expect(version.listing_sha256).toBe(createHash('sha256').update(canonical(version.listing)).digest('hex'));
      }
    }
  });

  it('skips yanked versions for latest', () => {
    const root = tempDir();
    const base = fixture('valid/agent-mastra-acp.json');
    put(root, base);
    const next = withVersion(base, '0.1.1');
    put(root, next, undefined, 'Broke the notes database.');
    const [entry] = buildIndex(root).index.listings;
    expect(entry!.latest).toBe('0.1.0');
    expect(entry!.versions[1]!.yanked).toBe('Broke the notes database.');
  });

  it('refuses a listing whose id or version is not its directory', () => {
    const root = tempDir();
    const listing = fixture('valid/agent-mastra-acp.json');
    const dir = join(root, 'listings', 'ostrium', 'other', 'versions', '0.1.0');
    mkdirSync(dir, { recursive: true });
    writeFileSync(join(dir, 'listing.json'), JSON.stringify(listing));
    expect(() => buildIndex(root)).toThrow(IndexError);
    try {
      buildIndex(root);
    } catch (error) {
      expect((error as IndexError).problems.join('\n')).toMatch(/is not ostrium\/other/);
    }
  });

  it('refuses invalid listings and, with --first-party-only, other namespaces', () => {
    const root = tempDir();
    put(root, fixture('valid/agent-pydantic-ai-a2a.json'));
    expect(() => buildIndex(root)).not.toThrow();
    expect(() => buildIndex(root, { firstPartyOnly: true })).toThrow(/namespace-first-party/);
    const bad = fixture('valid/agent-mastra-acp.json');
    (bad as unknown as Record<string, unknown>).pricing = 'paid';
    put(root, bad);
    expect(() => buildIndex(root)).toThrow(/\[schema\]/);
  });

  it('refuses a malformed revocation list', () => {
    const root = tempDir();
    put(root, fixture('valid/agent-mastra-acp.json'));
    writeFileSync(join(root, 'revocations.json'), JSON.stringify({ revocations: [{ id: 'ostrium/x', versions: 'soon', severity: 'disable', reason: 'r', date: '2026-10-22' }] }));
    expect(() => buildIndex(root)).toThrow(/not a version range/);
    writeFileSync(join(root, 'revocations.json'), JSON.stringify({ revocations: [{ id: 'ostrium/x', versions: '*', severity: 'delete', reason: 'r', date: '2026-10-22' }] }));
    expect(() => buildIndex(root)).toThrow(IndexError);
  });

  it('writes an empty revocation list when there is none', () => {
    const root = tempDir();
    put(root, fixture('valid/harness-native.json'));
    expect(buildIndex(root).revocations.revocations).toEqual([]);
  });
});
