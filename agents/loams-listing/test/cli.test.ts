import { mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vite-plus/test';
import { main } from '../src/cli.js';
import { TESTDATA } from './helpers.js';

async function run(...args: string[]) {
  const out: string[] = [];
  const err: string[] = [];
  const code = await main(args, { stdout: (l) => out.push(l), stderr: (l) => err.push(l), cwd: fileURLToPath(TESTDATA) });
  return { code, out, err };
}

describe('cli', () => {
  it('lint exits 0 on valid listings', async () => {
    const result = await run('lint', 'valid/agent-mastra-acp.json', 'valid/mcp-server-remote.json');
    expect(result.code).toBe(0);
    expect(result.out).toEqual(['valid/agent-mastra-acp.json: ok', 'valid/mcp-server-remote.json: ok']);
  });

  it('lint exits 1 and names the rule', async () => {
    const result = await run('lint', '--first-party-only', 'valid/agent-pydantic-ai-a2a.json');
    expect(result.code).toBe(1);
    expect(result.err.join('\n')).toMatch(/error \[namespace-first-party\]/);
  });

  it('lint --index resolves depends_on against index.v2.json, else index.json', async () => {
    const dir = mkdtempSync(join(tmpdir(), 'listing-index-'));
    const write = (name: string, version: string) =>
      writeFileSync(join(dir, name), JSON.stringify({ index_version: 1, listings: [{ id: 'acme/pg', versions: [{ version }] }] }));
    write('index.json', '16.4.0');
    expect((await run('lint', '--index', dir, 'v2/valid/app-oci.json')).code).toBe(0);
    write('index.v2.json', '17.0.0');
    const result = await run('lint', '--index', dir, 'v2/valid/app-oci.json');
    expect(result.code).toBe(1);
    expect(result.err.join('\n')).toMatch(/error \[depends-on-resolves\]/);
    expect((await run('lint', '--index', join(dir, 'nope'), 'v2/valid/app-oci.json')).code).toBe(2);
  });

  it('lint --index counts the listings being linted as part of the index', async () => {
    const dir = mkdtempSync(join(tmpdir(), 'listing-index-'));
    writeFileSync(join(dir, 'index.json'), JSON.stringify({ index_version: 1, listings: [] }));
    const pg = JSON.parse(readFileSync(new URL('v2/valid/app-oci.json', TESTDATA), 'utf8')) as Record<string, unknown>;
    writeFileSync(join(dir, 'pg.json'), JSON.stringify({ ...pg, id: 'acme/pg', version: '16.4.0', depends_on: [] }));
    const alone = await run('lint', '--index', dir, 'v2/valid/app-oci.json');
    expect(alone.err.join('\n')).toMatch(/\[depends-on-resolves\]/);
    const together = await run('lint', '--index', dir, join(dir, 'pg.json'), 'v2/valid/app-oci.json');
    expect(together.err.join('\n')).not.toMatch(/depends-on-resolves/);
    expect(together.code).toBe(0);
  });

  it('lint notes the rules it skipped without --submitter and --index', async () => {
    const result = await run('lint', 'valid/agent-mastra-acp.json');
    expect(result.code).toBe(0);
    expect(result.err).toEqual([
      'note: no --submitter given; reserved-namespace was skipped',
      'note: no --index given; depends-on-resolves was skipped',
    ]);
  });

  it('lint --submitter gates reserved namespaces', async () => {
    expect((await run('lint', '--submitter', 'acme', 'valid/agent-mastra-acp.json')).err.join('\n')).toMatch(/\[reserved-namespace\]/);
    expect((await run('lint', '--submitter', 'ostrium-labs', 'valid/agent-mastra-acp.json')).code).toBe(0);
  });

  it('lint exits 1 on a file that is not JSON', async () => {
    const result = await run('lint', 'invalid.json.missing');
    expect(result.code).toBe(1);
  });

  it('exits 2 with usage on a bad command', async () => {
    expect((await run('publish')).code).toBe(2);
    expect((await run('index', 'build', 'x')).code).toBe(2);
  });

  it('lists its rules', async () => {
    const result = await run('rules');
    expect(result.out.some((l) => l.startsWith('tool-poisoning\t'))).toBe(true);
  });
});
