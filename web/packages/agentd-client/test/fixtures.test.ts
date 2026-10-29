// Rust → TypeScript (design §50 §8.3). `tsc` checks every `satisfies` line of
// the generated `fixtures.gen.ts` against its generated type (the
// `typecheck` target); this suite feeds every `AgentEvent` fixture through
// the session reducer.

import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import {
  FIXTURES,
  fixture_AgentEvent_done,
  fixture_AgentEvent_inputRequested,
  fixture_AgentEvent_inputResolved,
  fixture_AgentEvent_sessionStarted,
  fixture_AgentEvent_subagent,
  fixture_AgentEvent_textDelta,
  fixture_AgentEvent_toolCall,
  fixture_AgentEvent_toolResult,
  fixture_AgentEvent_usage,
} from '../src/fixtures.gen';
import type { AgentEvent } from '../src/gen/AgentEvent';
import { METHODS } from '../src/gen/methods';
import { assertNever, EMPTY_SESSION, reduceSession, type SessionState } from '../src/reducer';

const agentEvents = FIXTURES.filter((f) => f.type === 'AgentEvent').map(
  (f) => f.value as AgentEvent,
);

const fold = (events: AgentEvent[], from: SessionState = EMPTY_SESSION) =>
  events.reduce(reduceSession, from);

describe('fixtures', () => {
  it('has a fixture for every AgentEvent variant the reducer handles', () => {
    const types = new Set(agentEvents.map((e) => e.type));
    expect(types.size).toBe(agentEvents.length);
    expect(types.size).toBeGreaterThanOrEqual(17);
  });

  it('every AgentEvent fixture passes through reduceSession', () => {
    for (const event of agentEvents) {
      const before = structuredClone(EMPTY_SESSION);
      const after = reduceSession(before, event);
      expect(after).not.toBe(before);
      expect(before).toEqual(EMPTY_SESSION);
    }
  });

  it('folds a turn the way the daemon folds it into the doc', () => {
    const state = fold([
      fixture_AgentEvent_sessionStarted,
      fixture_AgentEvent_textDelta,
      fixture_AgentEvent_textDelta,
      fixture_AgentEvent_toolCall,
      fixture_AgentEvent_toolResult,
      fixture_AgentEvent_inputRequested,
      fixture_AgentEvent_inputResolved,
      fixture_AgentEvent_usage,
      fixture_AgentEvent_usage,
    ]);
    expect(state.running).toBe(true);
    expect(state.sessionId).toBe('claude-session-1');
    expect(state.parts.map((p) => p.kind)).toEqual(['text', 'tool', 'input']);
    expect(state.parts[0]).toEqual({ kind: 'text', id: 't0', text: 'BuildingBuilding' });
    expect(state.parts[1]).toMatchObject({
      kind: 'tool',
      id: 'call-1',
      resolved: true,
      isError: false,
      // "fn main() {}" became three lines: three added, one removed.
      diffStats: [{ path: 'src/main.rs', additions: 3, deletions: 1 }],
    });
    expect(state.parts[2]).toMatchObject({ kind: 'input', resolved: true });
    expect(state.usage).toEqual({ inputTokens: 2400, outputTokens: 600 });

    const done = reduceSession(state, fixture_AgentEvent_done);
    expect(done.running).toBe(false);
    expect(done.lastStatus).toBe('completed');
  });

  it('marks a spawn chip from its subagent traffic, never an ordinary chip', () => {
    const spawn: AgentEvent = {
      type: 'toolCall',
      id: 'call-1',
      call: { kind: 'unknown', name: 'Agent: review', input: null },
    };
    const live = fold([spawn, fixture_AgentEvent_subagent]);
    expect(live.parts[0]).toMatchObject({ subagentStatus: 'running' });

    const plain = fold([fixture_AgentEvent_toolCall, fixture_AgentEvent_subagent]);
    expect(plain.parts[0]).not.toHaveProperty('subagentStatus');
  });

  it('fails loudly on a variant it does not know', () => {
    const unknown = { type: 'fromTheFuture' } as unknown as never;
    expect(() => assertNever(unknown)).toThrow(/unhandled variant/);
    expect(() => reduceSession(EMPTY_SESSION, unknown)).toThrow(/unhandled variant/);
  });
});

describe('the method table', () => {
  it('names each method once', () => {
    const names = METHODS.map((m) => m.name);
    expect(new Set(names).size).toBe(names.length);
  });

  it('keeps the lifecycle away from the renderer', () => {
    const renderer = new Set<string>(METHODS.filter((m) => m.renderer).map((m) => m.name));
    expect(renderer.has('Hello')).toBe(false);
    expect(renderer.has('Shutdown')).toBe(false);
  });

  it('gives role mcp only the shim methods', () => {
    expect(METHODS.filter((m) => m.role === 'mcp').map((m) => m.name)).toEqual([
      'Hello',
      'ListHarnesses',
      'ListModels',
      'QueueCommand',
      'WatchDocMessages',
      'QueueMessage',
      'WatchChats',
      'WatchSessions',
      'WatchSpaces',
      'Mutate',
      'LocalDevice',
      'EngineInfo',
      'CreateRunWorktree',
      'GetThreadDiff',
      'PostReviewComment',
      'OpenChangeRequest',
      'RequestApproval',
      'GetApproval',
      'MergeChangeRequest',
      'GetChecks',
      'GetCheckLog',
      'DeployLiveApp',
      'CloseChangeRequest',
    ]);
  });
});

describe('the ./types entry', () => {
  it('imports nothing from Node', () => {
    const src = join(import.meta.dirname, '..', 'src');
    const files = readdirSync(src, { recursive: true, encoding: 'utf8' }).filter((f) =>
      f.endsWith('.ts'),
    );
    for (const file of files) {
      const text = readFileSync(join(src, file), 'utf8');
      expect(text, file).not.toMatch(/from ['"]node:|require\(/);
    }
  });
});
