// The planner (design §72 §4.2, D1438): turns an intent into `plan.md`, one to five
// thread-sized tasks, each with acceptance checks. It reads the repository only through
// a read-only Studio thread's output (the workflow passes that in); it never edits files.
import { Agent } from '@mastra/core/agent';
import type { MastraModelConfig } from '@mastra/core/llm';
import { z } from 'zod';

import { UNTRUSTED_RULE, untrusted } from '../untrusted.js';

export const PLANNER_ID = 'planner';

export const planSchema = z.object({
  summary: z.string().min(1),
  tasks: z
    .array(
      z.object({
        title: z.string().min(1),
        description: z.string().min(1),
        checks: z.array(z.string().min(1)).min(1),
      }),
    )
    .min(1)
    .max(5),
});

export type Plan = z.infer<typeof planSchema>;

export const PLANNER_INSTRUCTIONS = [
  'You are the planner of the Loams Software Factory.',
  'Turn the intent into a plan of one to five tasks, in order, each small enough for one',
  'coding-agent thread, each with acceptance checks a reviewer can verify (a test that',
  'passes, a command that succeeds, a behaviour to see).',
  'You never write code and never edit files.',
  UNTRUSTED_RULE,
].join(' ');

export function createPlanner(model: MastraModelConfig): Agent {
  return new Agent({
    id: PLANNER_ID,
    name: 'Planner',
    description: 'Writes the run plan (plan.md): one to five tasks with acceptance checks.',
    instructions: PLANNER_INSTRUCTIONS,
    model,
  });
}

/** The planner's prompt: the intent, and what a read-only thread found, as data. */
export function planPrompt(intent: string, context?: { source: string; text: string }[]): string {
  const parts = [`Intent: ${intent.trim()}`];
  for (const c of context ?? []) parts.push(untrusted(c.source, c.text));
  return parts.join('\n\n');
}

/** Ask the planner; the answer is validated against `planSchema`. */
export async function makePlan(
  planner: Agent,
  intent: string,
  context?: { source: string; text: string }[],
): Promise<Plan> {
  const result = await planner.generate(planPrompt(intent, context), {
    structuredOutput: { schema: planSchema },
  });
  return planSchema.parse(result.object);
}

/** `plan.md`, as the person reads and edits it before approving. */
export function renderPlan(plan: Plan): string {
  const lines = ['# Plan', '', plan.summary.trim(), ''];
  plan.tasks.forEach((task, i) => {
    lines.push(
      `## ${i + 1}. ${task.title.trim()}`,
      '',
      task.description.trim(),
      '',
      'Acceptance checks:',
      '',
    );
    for (const check of task.checks) lines.push(`- [ ] ${check.trim()}`);
    lines.push('');
  });
  return lines.join('\n');
}
