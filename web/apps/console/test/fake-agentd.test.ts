// The preview's fake agentd (fake-agentd.ts): every demo thread opens with a transcript
// (a reset, then deltas that apply), the Diff panel gets its comments, and one thread
// waits on an approval for its last tool call.

import { createClient, createRouterTransport } from '@connectrpc/connect';
import {
  ApprovalService,
  DiffService,
  MessageRole,
  ReviewService,
  type SessionMessageEntry,
  ThreadService,
} from '@loams/proto/agentd';
import { describe, expect, it } from 'vite-plus/test';
import { fakeAgentdRoutes } from '../src/cordis/fake-agentd.js';
import { DEMO_SCRIPTS } from '../src/cordis/fake-agentd-transcripts.js';

const transport = createRouterTransport((router) =>
  fakeAgentdRoutes(router, { transcriptGapMs: 1 }),
);
const threads = createClient(ThreadService, transport);

/** The daemon's fold (crates/loams-agentd-proto transcript.rs), for upserts only. */
function fold(
  entries: SessionMessageEntry[],
  frame: SessionMessageEntry[] | { after?: string; entry?: SessionMessageEntry }[],
  count?: bigint,
) {
  if (count === undefined) return [...(frame as SessionMessageEntry[])];
  const next = [...entries];
  for (const { after, entry } of frame as { after?: string; entry?: SessionMessageEntry }[]) {
    if (!entry) throw new Error('upsert without an entry');
    const at = next.findIndex((e) => e.id === entry.id);
    if (at >= 0) next.splice(at, 1);
    const anchor = after === undefined ? -1 : next.findIndex((e) => e.id === after);
    if (after !== undefined && anchor < 0) throw new Error(`missing anchor ${after}`);
    next.splice(anchor + 1, 0, entry);
  }
  expect(next).toHaveLength(Number(count));
  return next;
}

async function transcript(chatId: string, frames: number) {
  const abort = new AbortController();
  let entries: SessionMessageEntry[] = [];
  let seen = 0;
  for await (const { update } of threads.watchTranscript({ chatId }, { signal: abort.signal })) {
    const frame = update?.frame;
    if (frame?.case === 'reset') entries = fold(entries, frame.value.entries);
    else if (frame?.case === 'delta')
      entries = fold(entries, frame.value.upsert, frame.value.count);
    else throw new Error('a frame without a reset or a delta');
    seen += 1;
    if (seen === frames) break;
  }
  abort.abort();
  return entries;
}

const toolParts = (entry: SessionMessageEntry | undefined) =>
  (entry?.parts ?? []).flatMap((p) => (p.part.case === 'tool' ? [p.part.value] : []));

describe('the preview fake agentd', () => {
  it('every demo thread opens with a transcript, then a follow-up and a tool call', async () => {
    const { chats } = await (async () => {
      for await (const r of threads.watchThreads({})) return r;
      throw new Error('no threads');
    })();
    expect(chats.length).toBeGreaterThan(0);
    for (const chat of chats) {
      expect(DEMO_SCRIPTS[chat.id], chat.id).toBeDefined();
      const pending = DEMO_SCRIPTS[chat.id]?.pending !== undefined;
      const entries = await transcript(chat.id, pending ? 3 : 4);
      expect(entries.map((e) => e.role)).toEqual([
        MessageRole.USER,
        MessageRole.ASSISTANT,
        MessageRole.USER,
        MessageRole.ASSISTANT,
      ]);
      expect(toolParts(entries[1])[0]?.resolved).toBe(true);
      const last = toolParts(entries[3])[0];
      expect(last?.call?.call.case).toBeDefined();
      // A call waiting on an approval is not resolved; the others carry their output.
      expect(last?.resolved).toBe(!pending);
    }
  });

  it('the watch stays open after its last frame, as the daemon does', async () => {
    const abort = new AbortController();
    const stream = threads.watchTranscript({ chatId: 'demo-build' }, { signal: abort.signal });
    const it = stream[Symbol.asyncIterator]();
    for (let i = 0; i < 4; i += 1) expect((await it.next()).done).toBe(false);
    // A stream that ended here would make the thread page reopen it and start over.
    const next = it.next().catch(() => ({ done: true }));
    const outcome = await Promise.race([
      next.then(() => 'ended'),
      new Promise((resolve) => setTimeout(() => resolve('open'), 100)),
    ]);
    expect(outcome).toBe('open');
    abort.abort();
    await next;
  });

  it('a pending approval waits in the demo-login thread', async () => {
    for await (const { pending } of createClient(ApprovalService, transport).watchApprovals({})) {
      const login = pending.find((a) => a.chatId === 'demo-login');
      expect(login).toMatchObject({ tool: 'Bash', turnId: 'demo-login-u2' });
      break;
    }
  });

  it('writes are refused, since the demo data does not change', async () => {
    const review = createClient(ReviewService, transport);
    await expect(
      review.deleteReviewComment({ checkoutId: 'demo:~/src/loams', id: 'x' }),
    ).rejects.toThrow(/read-only/);
    await expect(
      createClient(ApprovalService, transport).resolveApproval({ callId: 'demo-ap-0' }),
    ).rejects.toThrow(/read-only/);
  });

  it('the Diff panel has a patch and a review comment', async () => {
    const { diff } = await createClient(DiffService, transport).getCheckoutDiff({
      cwd: '~/src/loams',
      mode: '',
    });
    expect(diff?.patch).toContain('toBeEditable');
    for await (const { frame } of createClient(ReviewService, transport).watchReviewComments({
      checkoutId: diff?.checkoutId ?? '',
    })) {
      expect(frame.case).toBe('reset');
      if (frame.case === 'reset') expect(frame.value.comments).toHaveLength(1);
      break;
    }
  });
});
