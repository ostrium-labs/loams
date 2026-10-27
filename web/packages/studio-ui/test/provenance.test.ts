// provenance_hashes_match (SD1 Task 1; design §53 §9.1, D912).
import { createHash } from 'node:crypto';
import { existsSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vite-plus/test';
import { loadProvenance, PACKAGE_DIR, readPackageFile } from './provenance-data';

const sha256 = (text: string) => createHash('sha256').update(text).digest('hex');

describe('provenance_hashes_match', () => {
  const prov = loadProvenance();

  it('names T3 Code, its licence and its copyright', () => {
    expect(prov.upstream).toBe('https://github.com/pingdotgg/t3code');
    expect(prov.license).toBe('MIT');
    expect(prov.copyright).toBe('Copyright (c) 2026 T3 Tools Inc.');
  });

  it('records each source archive by commit, URL and SHA-256', () => {
    expect(prov.archives.length).toBeGreaterThan(0);
    for (const archive of prov.archives) {
      expect(archive.commit).toMatch(/^[0-9a-f]{40}$/);
      expect(archive.sha256).toMatch(/^[0-9a-f]{64}$/);
      expect(archive.url).toBe(`https://github.com/pingdotgg/t3code/archive/${archive.commit}.zip`);
    }
  });

  it('every row is complete and points at an existing file and a recorded archive', () => {
    const commits = new Set(prov.archives.map((a) => a.commit));
    for (const row of prov.files) {
      expect(row.path.startsWith('src/')).toBe(true);
      expect(existsSync(join(PACKAGE_DIR, row.path))).toBe(true);
      expect(commits.has(row.commit)).toBe(true);
      expect(row.upstreamSha256).toMatch(/^[0-9a-f]{64}$/);
      expect(row.localSha256).toMatch(/^[0-9a-f]{64}$/);
      expect(['as-is', 'adapt', 'rewrite']).toContain(row.verdict);
      expect(row.group).not.toBe('');
    }
    expect(new Set(prov.files.map((r) => r.path)).size).toBe(prov.files.length);
  });

  it.each(prov.files.filter((r) => r.verdict === 'as-is').map((r) => [r.path, r] as const))(
    'as-is %s still hashes to its localSha256',
    (_path, row) => {
      expect(sha256(readPackageFile(row.path))).toBe(row.localSha256);
    },
  );
});
