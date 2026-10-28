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
