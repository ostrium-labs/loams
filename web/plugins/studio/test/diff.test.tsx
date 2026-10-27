// The thread's Diff tab (SD1 Task 20) over a daemon's `DiffService`, `ReviewService`,
// `GitService` and `ThreadService` (an in-memory Connect router): the review actions and
// the guarded discards reach agentd with the thread, the comments and the checksum.

import { create } from '@bufbuild/protobuf';
import { Code, ConnectError, createRouterTransport } from '@connectrpc/connect';
import { AgentdTransportProvider } from '@loams/console-host/query';
import {
  ChangeRequestState,
  CheckoutDiffSchema,
  DiffService,
  DiffSide,
  GitService,
  type PostReviewToChangeRequestRequest,
  ReviewCommentSchema,
  ReviewService,
  type SendReviewToThreadRequest,
  ThreadService,
} from '@loams/proto/agentd';
import { ErrorInfoSchema } from '@loams/proto/errors';
import { useDiffPanelStore } from '@loams/studio-ui/diff';
import {
  act,
  cleanup,
  configure,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from '@testing-library/react';
import { afterEach, beforeAll, beforeEach, describe, expect, it } from 'vite-plus/test';
import { DiffTab } from '../src/thread/DiffTab.js';
import { diffRequest, isStaleDiff } from '../src/thread/diff-source.js';
import { wrap } from './fakes.js';

// The first mounts highlight the diff with a cold syntax highlighter, which takes over the
// default 1 s on a loaded CI runner (the first two tests failed there, never locally). 4 s
// stays under Vitest's 5 s test timeout.
configure({ asyncUtilTimeout: 4000 });

beforeAll(() => {
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver;
});
beforeEach(() => {
  useDiffPanelStore.setState({ byThread: {}, branchBaseRefByThread: {}, viewedByThread: {} });
});
afterEach(cleanup);

const PATCH = `diff --git a/src/app.ts b/src/app.ts
index 1111111..2222222 100644
--- a/src/app.ts
+++ b/src/app.ts
@@ -1,2 +1,2 @@
-export const b = 1;
+export const b = 2;
 export const c = 3;
`;

/** Waits until the stream is cancelled, so a watch stays open. */
const hold = (signal: AbortSignal) =>
  new Promise<void>((resolve) => signal.addEventListener('abort', () => resolve()));

function fakeAgentd(
  opts: {
    changeRequest?: boolean;
    stale?: boolean;
    /** When it settles, the thread's branch is renamed and the diff sent again, unchanged. */
    later?: Promise<void>;
  } = {},
) {
  const calls: string[] = [];
  const sent: SendReviewToThreadRequest[] = [];
  const posted: PostReviewToChangeRequestRequest[] = [];
  const discards: { paths: string[]; expectedChecksum: string }[] = [];
  const staleError = () =>
    new ConnectError('stale_diff: the checkout moved', Code.FailedPrecondition, undefined, [
      { desc: ErrorInfoSchema, value: { reason: 'stale_diff' } },
    ]);
  const diff = create(CheckoutDiffSchema, {
    checkoutId: 'co-7',
    cwd: '/repo/wt',
    patch: PATCH,
    files: [{ path: 'src/app.ts', status: 'modified', additions: 1, deletions: 1 }],
    additions: 1,
    deletions: 1,
    checksum: 'sum-7',
  });
  const transport = createRouterTransport(({ service }) => {
    service(ThreadService, {
      async *watchThreads(_req, ctx) {
        yield {
          chats: [
            { id: 'thread-1', deviceId: 'd', cwd: '/repo/wt', checkoutId: 'co-7', branch: 'feat' },
          ],
        };
        if (opts.later) {
          await opts.later;
          // The daemon names the thread after its first turn and renames its branch.
          yield {
            chats: [
              {
                id: 'thread-1',
                deviceId: 'd',
                cwd: '/repo/wt',
                checkoutId: 'co-7',
                branch: 'feat-renamed',
                title: 'Feature',
              },
            ],
          };
        }
        await hold(ctx.signal);
      },
    });
    service(DiffService, {
      getCheckoutDiff: (req) => {
        calls.push(`GetCheckoutDiff:${req.cwd}:${req.mode}`);
        return { diff };
      },
      async *watchCheckoutDiffs(_req, ctx) {
        yield { diffs: [diff] };
        if (opts.later) {
          await opts.later;
          yield { diffs: [{ ...diff, updatedAtMs: 2n }] };
        }
        await hold(ctx.signal);
      },
      discard: (req) => {
        calls.push(`Discard:${req.chatId}:${req.checkoutId}:${req.expectedChecksum}`);
        if (opts.stale) throw staleError();
        return { ok: true, checksum: 'sum-8' };
      },
      discardPaths: (req) => {
        discards.push({ paths: req.paths, expectedChecksum: req.expectedChecksum });
        if (opts.stale) throw staleError();
        return {};
      },
    });
    service(ReviewService, {
      async *watchReviewComments(req, ctx) {
        calls.push(`WatchReviewComments:${req.checkoutId}`);
        yield {
          frame: {
            case: 'reset',
            value: {
              comments: [
                create(ReviewCommentSchema, {
                  id: 'r1',
                  checkoutId: 'co-7',
                  path: 'src/app.ts',
                  side: DiffSide.NEW,
                  line: 1,
                  body: 'Why 2?',
                  diffChecksum: 'sum-7',
                }),
                create(ReviewCommentSchema, {
                  id: 'r2',
                  checkoutId: 'co-7',
                  path: 'src/app.ts',
                  side: DiffSide.OLD,
                  line: 1,
                  body: 'Old line',
                  diffChecksum: 'sum-6',
                  outdated: true,
                }),
              ],
            },
          },
        };
        await hold(ctx.signal);
      },
      sendReviewToThread: (req) => {
        sent.push(req);
        return { commandId: 'cmd-1' };
      },
      postReviewToChangeRequest: (req) => {
        posted.push(req);
        return { url: 'https://forge.example/o/r/pulls/3' };
      },
    });
    service(GitService, {
      async *watchChangeRequest(req, ctx) {
        calls.push(`WatchChangeRequest:${req.cwd}:${req.branch ?? ''}`);
        if (opts.changeRequest) {
          yield {
            status: {
              checkoutId: 'co-7',
              cwd: '/repo/wt',
              branch: 'feat',
              changeRequest: {
                provider: 'forgejo',
                number: 3n,
                title: 'Feature',
                url: 'https://forge.example/o/r/pulls/3',
                state: ChangeRequestState.OPEN,
              },
            },
          };
        }
        await hold(ctx.signal);
      },
      listBranches: () => ({ branches: ['main', 'feat'] }),
    });
  });
  return { transport, calls, sent, posted, discards };
}

function mount(daemon: ReturnType<typeof fakeAgentd>) {
  return render(
    wrap(
      <AgentdTransportProvider transport={daemon.transport}>
        <DiffTab threadId="thread-1" />
      </AgentdTransportProvider>,
    ),
  );
}

describe('the Diff tab over agentd', () => {
  it('shows the thread checkout’s diff and its comments', async () => {
    const daemon = fakeAgentd();
    mount(daemon);
    await screen.findByText('Why 2?');
    expect(daemon.calls).toContain('GetCheckoutDiff:/repo/wt:');
    expect(daemon.calls).toContain('WatchReviewComments:co-7');
    expect(daemon.calls).toContain('WatchChangeRequest:/repo/wt:feat');
    const card = screen.getByText('Old line').closest('[data-diff-comment-annotation]');
    expect(within(card as HTMLElement).getByText('outdated')).toBeTruthy();
  });
});

describe('send_review_to_thread_calls_rpc', () => {
  it('calls SendReviewToThread with the thread and its open comments', async () => {
    const daemon = fakeAgentd();
    mount(daemon);
    fireEvent.click(await screen.findByRole('button', { name: /Send 2 comments to the agent/ }));
    await waitFor(() => expect(daemon.sent).toHaveLength(1));
    expect(daemon.sent[0]?.chatId).toBe('thread-1');
    expect(daemon.sent[0]?.commentIds).toEqual(['r1', 'r2']);
  });
});

describe('post_review_only_with_change_request', () => {
  it('has no "Post to PR" without an open pull request', async () => {
    mount(fakeAgentd());
    await screen.findByRole('button', { name: /Send 2 comments/ });
    expect(screen.queryByRole('button', { name: /Post to PR/ })).toBeNull();
  });

  it('posts the current comments to the open pull request', async () => {
    const daemon = fakeAgentd({ changeRequest: true });
    mount(daemon);
    fireEvent.click(await screen.findByRole('button', { name: 'Post to PR #3' }));
    await waitFor(() => expect(daemon.posted).toHaveLength(1));
    expect(daemon.posted[0]?.checkoutId).toBe('co-7');
    // The outdated comment is not posted: its anchor would land on the wrong line (T11-4).
    expect(daemon.posted[0]?.commentIds).toEqual(['r1']);
    await screen.findByText(/Posted 1 to pull request #3/);
  });
});

describe('discard_requires_confirmation_and_checksum', () => {
  it('sends Discard with the thread, the checkout and the confirmed checksum', async () => {
    const daemon = fakeAgentd();
    mount(daemon);
    fireEvent.click(await screen.findByRole('button', { name: 'Discard all changes' }));
    expect(daemon.calls.some((c) => c.startsWith('Discard:'))).toBe(false);
    fireEvent.click(
      within(await screen.findByRole('alertdialog')).getByRole('button', { name: 'Discard all' }),
    );
    await waitFor(() => expect(daemon.calls).toContain('Discard:thread-1:co-7:sum-7'));
  });

  it('turns stale_diff into a message', async () => {
    const daemon = fakeAgentd({ stale: true });
    mount(daemon);
    fireEvent.click(await screen.findByRole('button', { name: 'Discard changes to src/app.ts' }));
    fireEvent.click(
      within(await screen.findByRole('alertdialog')).getByRole('button', { name: 'Discard file' }),
    );
    await waitFor(() =>
      expect(daemon.discards).toEqual([{ paths: ['src/app.ts'], expectedChecksum: 'sum-7' }]),
    );
    await screen.findByText(/The diff changed since you last saw it/);
  });

  it('recognises stale_diff by its ErrorInfo reason only', () => {
    expect(
      isStaleDiff(
        new ConnectError('moved', Code.FailedPrecondition, undefined, [
          { desc: ErrorInfoSchema, value: { reason: 'stale_diff' } },
        ]),
      ),
    ).toBe(true);
    expect(isStaleDiff(new ConnectError('nothing_to_commit', Code.FailedPrecondition))).toBe(false);
    expect(isStaleDiff(new Error('stale_diff'))).toBe(false);
  });
});

describe('diff_sources_switch', () => {
  it('maps each source to GetCheckoutDiff’s mode', () => {
    const thread = { chatId: 't', cwd: '/w' };
    expect(diffRequest(thread, { kind: 'working' })).toEqual({ cwd: '/w', mode: '' });
    expect(diffRequest(thread, { kind: 'branch', baseRef: 'main' })).toEqual({
      cwd: '/w',
      mode: 'branch',
      baseRef: 'main',
    });
    expect(diffRequest(thread, { kind: 'commit', commitSha: 'abc' })).toEqual({
      cwd: '/w',
      mode: 'commit',
      commitSha: 'abc',
    });
    expect(diffRequest(thread, { kind: 'turn' })).toEqual({ cwd: '/w', mode: 'turn', chatId: 't' });
  });
});

describe('review_draft_survives_thread_updates', () => {
  it('keeps a half-written comment when the thread’s branch is renamed', async () => {
    let release = () => {};
    const later = new Promise<void>((resolve) => {
      release = resolve;
    });
    const daemon = fakeAgentd({ later });
    mount(daemon);
    const card = (await screen.findByText('Why 2?')).closest(
      '[data-diff-comment-annotation]',
    ) as HTMLElement;
    fireEvent.click(within(card).getByRole('button', { name: 'Edit comment' }));
    const box = (await screen.findByRole('textbox', {
      name: /^Comment on line/,
    })) as HTMLTextAreaElement;
    fireEvent.change(box, { target: { value: 'Half-written' } });

    // The thread's row changes under the open draft, and the diff is sent again unchanged.
    await act(async () => release());
    // The open pull request follows the new branch...
    await waitFor(() => expect(daemon.calls).toContain('WatchChangeRequest:/repo/wt:feat-renamed'));
    // ...while the diff, its comments and the draft stay as they were.
    expect(screen.queryByText('Loading changes')).toBeNull();
    expect(box.isConnected).toBe(true);
    expect(box.value).toBe('Half-written');
    expect(daemon.calls.filter((c) => c === 'WatchReviewComments:co-7')).toHaveLength(1);
  });
});
