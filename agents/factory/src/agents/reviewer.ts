// The reviewer (design §72 §4.2, D1439): reads a thread's diff against the plan's
// acceptance checks and drafts anchored comments, which it posts to Studio's diff review
// with `post_review_comment` (D905). The person accepts or skips them.
import { Agent } from '@mastra/core/agent';
import type { MastraModelConfig } from '@mastra/core/llm';
import { z } from 'zod';

import { UNTRUSTED_RULE, untrusted } from '../untrusted.js';
import type { Plan } from './planner.js';

export const REVIEWER_ID = 'reviewer';

export const reviewSchema = z.object({
  verdict: z.enum(['looks-good', 'changes-requested']),
  comments: z
    .array(
      z.object({
        path: z.string().min(1),
        line: z.number().int().min(1),
        endLine: z.number().int().min(1).optional(),
        side: z.enum(['new', 'old']).default('new'),
        body: z.string().min(1),
      }),
    )
    .max(30),
});

export type Review = z.infer<typeof reviewSchema>;

export const REVIEWER_INSTRUCTIONS = [
  'You are the reviewer of the Loams Software Factory.',
  'Review the diff against the plan task and its acceptance checks. Comment only on',
  'specific lines, with a concrete reason; say nothing about style the checks do not ask',
  'for. Use verdict looks-good when nothing needs to change.',
  UNTRUSTED_RULE,
].join(' ');

export function createReviewer(model: MastraModelConfig): Agent {
  return new Agent({
    id: REVIEWER_ID,
    name: 'Reviewer',
    description: 'Reviews a thread diff against the plan and drafts anchored comments.',
    instructions: REVIEWER_INSTRUCTIONS,
    model,
  });
}

/** What `get_thread_diff` answers, as far as the review needs it. */
export interface ThreadDiff {
  patch: string;
  checksum: string;
}

export function reviewPrompt(plan: Plan, task: number, diff: ThreadDiff): string {
  const current = plan.tasks[task];
  if (!current) throw new Error(`the plan has no task ${task + 1}`);
  return [
    `Task ${task + 1}: ${current.title}`,
    current.description,
    'Acceptance checks:',
    ...current.checks.map((c) => `- ${c}`),
    untrusted('diff', diff.patch),
  ].join('\n');
}

/** Posts one comment through agentd (`post_review_comment`). */
export type PostComment = (comment: {
  chat: string;
  path: string;
  line: number;
  end_line?: number;
  side: 'new' | 'old';
  body: string;
  diff_checksum: string;
}) => Promise<unknown>;

/** Review a thread and post its comments, each anchored to the reviewed diff. */
export async function reviewThread(
  reviewer: Agent,
  args: { chat: string; plan: Plan; task: number; diff: ThreadDiff; post: PostComment },
): Promise<Review> {
  const result = await reviewer.generate(reviewPrompt(args.plan, args.task, args.diff), {
    structuredOutput: { schema: reviewSchema },
  });
  const review = reviewSchema.parse(result.object);
  for (const c of review.comments) {
    await args.post({
      chat: args.chat,
      path: c.path,
      line: c.line,
      ...(c.endLine ? { end_line: c.endLine } : {}),
      side: c.side,
      body: c.body,
      diff_checksum: args.diff.checksum,
    });
  }
  return review;
}
