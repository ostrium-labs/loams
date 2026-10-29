// The kill switch (design §72 §7.4, D1432; plan FT1, FT0 Task 10).
//
// `factory.kill(scope)` stops runs: a run, every run using an agent, or every run of a
// project. For each run it cancels the workflow, interrupts the run's Studio threads
// (`interrupt_chat`), closes its open pull request (`close_change_request`) and records
// the run as `killed`. It never deletes a branch or a worktree: the work stays where a
// person can pick it up. A killed agent is also blocked, so no new run may use it until
// a person lifts the block.
//
// Loams Bot offers it as the tool `factory_kill`, which always asks the person first
// (Mastra's `requireApproval`, surfaced as an approval in Studio), and as `/kill` in the
// thread, which goes through the same tool.
import { createTool } from '@mastra/core/tools';
import { z } from 'zod';

import type { AgentdCall } from './deploy/live.js';

/**
 * Where the `killed` transition is written: the run recorder of Task 9
 * (`createRunRecorder`) has this shape.
 */
export interface KillRecorder {
  transition(t: {
    run: string;
    seq: number;
    kind: 'killed';
    patch: { state: 'killed'; error?: string };
  }): Promise<unknown>;
}

export type KillScope =
  | { kind: 'run'; run: string }
  | { kind: 'agent'; agent: string }
  | { kind: 'project'; project: string };

/** What the kill switch needs to know about a live run. */
export interface LiveRun {
  run: string;
  project: string;
  /** The agents holding a role in it (planner, reviewer, a harness, …). */
  agents: string[];
  /** Its Studio threads (build threads and the run's own). */
  threads: string[];
  /** Its open pull request, if any, and the thread whose remote holds it. */
  pr?: { provider: 'forgejo' | 'github'; number: number; chat: string };
  /** The run's next transition number (for the `killed` record). */
  nextSeq: number;
}

/** The runs that are not finished. */
export interface RunIndex {
  live(): Promise<LiveRun[]>;
}

/** Agents a person has killed. Kept by the host (Task 7 stores it with the workflow state). */
export interface KillList {
  block(agent: string): Promise<void>;
  isBlocked(agent: string): Promise<boolean>;
  unblock(agent: string): Promise<void>;
}

export function memoryKillList(): KillList {
  const blocked = new Set<string>();
  return {
    block: async (agent) => void blocked.add(agent),
    isBlocked: async (agent) => blocked.has(agent),
    unblock: async (agent) => void blocked.delete(agent),
  };
}

export interface KillDeps {
  call: AgentdCall;
  runs: RunIndex;
  /** Cancels the run's `factory.run` workflow (Task 7's engine). */
  cancelRun: (run: string) => Promise<void>;
  killed: KillList;
  record?: KillRecorder;
}

export interface KillReport {
  scope: KillScope;
  runs: string[];
  interrupted: string[];
  closedPrs: number[];
  /** What could not be done; the rest still ran. */
  errors: string[];
}

export class KilledAgentError extends Error {
  override name = 'KilledAgentError';
}

const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

function matches(scope: KillScope, run: LiveRun): boolean {
  switch (scope.kind) {
    case 'run':
      return run.run === scope.run;
    case 'agent':
      return run.agents.includes(scope.agent);
    case 'project':
      return run.project === scope.project;
  }
}

/**
 * Kill every live run in `scope`. Each step is tried for every run even when another
 * fails (a kill must not stop half way); the failures are in `errors`.
 */
export async function kill(scope: KillScope, deps: KillDeps): Promise<KillReport> {
  const report: KillReport = { scope, runs: [], interrupted: [], closedPrs: [], errors: [] };
  if (scope.kind === 'agent') await deps.killed.block(scope.agent);
  const targets = (await deps.runs.live()).filter((r) => matches(scope, r));
  for (const run of targets) {
    report.runs.push(run.run);
    // The workflow first, so no step starts a new thread or call after this.
    try {
      await deps.cancelRun(run.run);
    } catch (e) {
      report.errors.push(`${run.run}: cancel: ${message(e)}`);
    }
    for (const chat of run.threads) {
      try {
        await deps.call('interrupt_chat', { chat });
        report.interrupted.push(chat);
      } catch (e) {
        report.errors.push(`${run.run}: interrupt ${chat}: ${message(e)}`);
      }
    }
    if (run.pr) {
      try {
        await deps.call('close_change_request', {
          chat: run.pr.chat,
          provider: run.pr.provider,
          number: run.pr.number,
        });
        report.closedPrs.push(run.pr.number);
      } catch (e) {
        report.errors.push(`${run.run}: close #${run.pr.number}: ${message(e)}`);
      }
    }
    try {
      await deps.record?.transition({
        run: run.run,
        seq: run.nextSeq,
        kind: 'killed',
        patch: {
          state: 'killed',
          ...(scope.kind === 'run' ? {} : { error: `killed: ${scope.kind}` }),
        },
      });
    } catch (e) {
      report.errors.push(`${run.run}: record: ${message(e)}`);
    }
  }
  return report;
}

/** Refuse a new run that would use a killed agent (checked before the run starts). */
export async function assertAgentsUsable(agents: string[], killed: KillList): Promise<void> {
  for (const agent of agents) {
    if (await killed.isBlocked(agent)) {
      throw new KilledAgentError(
        `agent ${agent} was killed; a person must lift the block before a run uses it`,
      );
    }
  }
}

/** `/kill`, `/kill run <id>`, `/kill agent <id>`, `/kill project <id>`; `run` is the default. */
export function parseKillCommand(text: string, currentRun?: string): KillScope | undefined {
  const words = text.trim().split(/\s+/);
  if (words[0] !== '/kill') return undefined;
  const [kind, id] = words.length === 2 ? ['run', words[1]] : [words[1], words[2]];
  if (words.length === 1) return currentRun ? { kind: 'run', run: currentRun } : undefined;
  if (!id || words.length > 3) return undefined;
  switch (kind) {
    case 'run':
      return { kind: 'run', run: id };
    case 'agent':
      return { kind: 'agent', agent: id };
    case 'project':
      return { kind: 'project', project: id };
    default:
      return undefined;
  }
}

export const KILL_TOOL = 'factory_kill';

/** Loams Bot's kill tool: always asks the person first. */
export function createKillTool(deps: KillDeps) {
  return createTool({
    id: KILL_TOOL,
    description:
      'Stop factory runs: one run, every run using an agent (and block that agent), or every run of a project. Cancels the workflow, interrupts the threads, closes the open pull request and keeps the branches. The person confirms it first.',
    inputSchema: z.object({
      kind: z.enum(['run', 'agent', 'project']),
      id: z.string().min(1),
    }),
    requireApproval: true,
    execute: async (input: { kind: 'run' | 'agent' | 'project'; id: string }) => {
      const scope: KillScope =
        input.kind === 'run'
          ? { kind: 'run', run: input.id }
          : input.kind === 'agent'
            ? { kind: 'agent', agent: input.id }
            : { kind: 'project', project: input.id };
      return await kill(scope, deps);
    },
  });
}
