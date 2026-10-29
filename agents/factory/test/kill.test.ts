// FT0 Task 10 (plan FT1): the kill switch's host half, against a fake agentd, a fake
// workflow engine and a fake run index. agentd's half (`CloseChangeRequest` keeps the
// branch) is `kill_keeps_branches` in crates/loams-agentd-sessions/tests/factory_loop.rs.
import { describe, expect, it } from 'vitest';

import {
  type AgentdCall,
  assertAgentsUsable,
  createKillTool,
  KilledAgentError,
  kill,
  type LiveRun,
  memoryKillList,
  parseKillCommand,
} from '../src/index.js';

const RUNS: LiveRun[] = [
  {
    run: 'run-1',
    project: 'comet',
    agents: ['planner', 'reviewer', 'claude-code'],
    threads: ['bot-1', 'build-1a', 'build-1b'],
    pr: { provider: 'forgejo', number: 7, chat: 'build-1a' },
    nextSeq: 6,
  },
  {
    run: 'run-2',
    project: 'comet',
    agents: ['planner', 'codex'],
    threads: ['bot-2'],
    nextSeq: 3,
  },
  { run: 'run-3', project: 'other', agents: ['codex'], threads: ['bot-3'], nextSeq: 2 },
];

function world(options: { failInterrupt?: string } = {}) {
  const calls: { tool: string; args: Record<string, unknown> }[] = [];
  const cancelled: string[] = [];
  const recorded: { run: string; seq: number; kind: string; patch: unknown }[] = [];
  const call: AgentdCall = async (tool, args) => {
    calls.push({ tool, args });
    if (tool === 'interrupt_chat' && args.chat === options.failInterrupt) {
      throw new Error('not_found');
    }
    return { ok: true };
  };
  const killed = memoryKillList();
  const deps = {
    call,
    runs: { live: async () => RUNS },
    cancelRun: async (run: string) => void cancelled.push(run),
    killed,
    record: { transition: async (t: (typeof recorded)[number]) => void recorded.push(t) },
  };
  return { deps, calls, cancelled, recorded, killed };
}

describe('the kill switch', () => {
  it('kill_interrupts_threads_and_cancels_workflow', async () => {
    const w = world();
    const report = await kill({ kind: 'run', run: 'run-1' }, w.deps);
    expect(w.cancelled).toEqual(['run-1']);
    expect(report.runs).toEqual(['run-1']);
    expect(report.interrupted).toEqual(['bot-1', 'build-1a', 'build-1b']);
    expect(w.calls.filter((c) => c.tool === 'interrupt_chat').map((c) => c.args.chat)).toEqual([
      'bot-1',
      'build-1a',
      'build-1b',
    ]);
    // The workflow is cancelled before any thread is touched.
    expect(w.calls[0]?.tool).toBe('interrupt_chat');
    expect(report.closedPrs).toEqual([7]);
    expect(w.calls.find((c) => c.tool === 'close_change_request')?.args).toEqual({
      chat: 'build-1a',
      provider: 'forgejo',
      number: 7,
    });
    expect(w.recorded).toEqual([
      { run: 'run-1', seq: 6, kind: 'killed', patch: { state: 'killed' } },
    ]);
    expect(report.errors).toEqual([]);
  });

  it('kill_keeps_branches', async () => {
    const w = world();
    await kill({ kind: 'project', project: 'comet' }, w.deps);
    // Only interrupts and closes: nothing deletes a branch, a worktree or a thread.
    const tools = new Set(w.calls.map((c) => c.tool));
    expect([...tools].sort()).toEqual(['close_change_request', 'interrupt_chat']);
    expect(w.cancelled).toEqual(['run-1', 'run-2']);
  });

  it('agent_kill_blocks_new_runs_using_it', async () => {
    const w = world();
    const report = await kill({ kind: 'agent', agent: 'codex' }, w.deps);
    expect(report.runs).toEqual(['run-2', 'run-3']);
    expect(w.recorded.map((r) => r.patch)).toEqual([
      { state: 'killed', error: 'killed: agent' },
      { state: 'killed', error: 'killed: agent' },
    ]);
    await expect(assertAgentsUsable(['planner', 'codex'], w.killed)).rejects.toThrow(
      KilledAgentError,
    );
    await expect(assertAgentsUsable(['planner', 'claude-code'], w.killed)).resolves.toBe(undefined);
    await w.killed.unblock('codex');
    await expect(assertAgentsUsable(['codex'], w.killed)).resolves.toBe(undefined);
  });

  it('keeps going when one step fails', async () => {
    const w = world({ failInterrupt: 'build-1a' });
    const report = await kill({ kind: 'run', run: 'run-1' }, w.deps);
    expect(report.interrupted).toEqual(['bot-1', 'build-1b']);
    expect(report.closedPrs).toEqual([7]);
    expect(report.errors).toEqual(['run-1: interrupt build-1a: not_found']);
    expect(w.recorded).toHaveLength(1);
  });

  it('parses /kill', () => {
    expect(parseKillCommand('/kill', 'run-1')).toEqual({ kind: 'run', run: 'run-1' });
    expect(parseKillCommand('/kill')).toBeUndefined();
    expect(parseKillCommand('/kill run-9')).toEqual({ kind: 'run', run: 'run-9' });
    expect(parseKillCommand('/kill agent codex')).toEqual({ kind: 'agent', agent: 'codex' });
    expect(parseKillCommand(' /kill project comet ')).toEqual({
      kind: 'project',
      project: 'comet',
    });
    expect(parseKillCommand('/kill thing x')).toBeUndefined();
    expect(parseKillCommand('please /kill')).toBeUndefined();
  });

  it('the Loams Bot tool asks the person first', () => {
    const tool = createKillTool(world().deps);
    expect(tool.requireApproval).toBe(true);
  });
});
