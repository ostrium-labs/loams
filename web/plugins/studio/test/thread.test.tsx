// The thread page (SD1 Task 18) against a fake daemon: the transcript over
// `ThreadService.WatchTranscript`, inline questions and approvals, restore with the
// confirmed checksum, and one watch shared by the page and the dock.

import { create, fromBinary, fromJson } from '@bufbuild/protobuf';
import { ValueSchema } from '@bufbuild/protobuf/wkt';
import { createRouterTransport, type HandlerContext, type Transport } from '@connectrpc/connect';
import { AgentdTransportProvider } from '@loams/console-host/query';
import {
  ApprovalDecision,
  ApprovalRisk,
  ApprovalSchema,
  ApprovalService,
  ChatSchema,
  DiffService,
  HarnessId,
  SandboxLevel,
  ThreadService,
  type TranscriptUpdate,
  TranscriptUpdateSchema,
} from '@loams/proto/agentd';
import { Timeline, useThreadTranscript } from '@loams/studio-ui/chat';
import { act, cleanup, fireEvent, render, waitFor, within } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vite-plus/test';
import connectFixtures from '../../../packages/agentd-client/test/transcript.connect.gen.json';
import { useTranscriptStore } from '../src/thread/streams.js';
import { ThreadPage } from '../src/thread/ThreadPage.js';
import { wrap } from './fakes.js';

afterEach(cleanup);

function hex(text: string): Uint8Array {
  const bytes = new Uint8Array(text.length / 2);
  for (let i = 0; i < bytes.length; i += 1)
    bytes[i] = Number.parseInt(text.slice(i * 2, i * 2 + 2), 16);
  return bytes;
}

/** The reset fixture as the daemon's own Connect mapping encodes it (ruling T6-5). */
const RESET: TranscriptUpdate = fromBinary(
  TranscriptUpdateSchema,
  hex((connectFixtures as Record<string, string>)['TranscriptUpdate.reset'] ?? ''),
);

function untilAborted(ctx: HandlerContext): Promise<void> {
  return new Promise((resolve) =>
    ctx.signal.addEventListener('abort', () => resolve(), { once: true }),
  );
}

/** A daemon with one thread, `chat-1`, whose transcript is the reset fixture. */
function fakeDaemon(o: { approvals?: boolean } = {}) {
  const calls: { method: string; request: unknown }[] = [];
  const watches: string[] = [];
  const transport: Transport = createRouterTransport(({ service }) => {
    service(ThreadService, {
      async *watchTranscript(request, ctx) {
        watches.push(request.chatId);
        yield { update: RESET };
        await untilAborted(ctx);
      },
      async *watchThreads(_request, ctx) {
        yield {
          chats: [
            create(ChatSchema, {
              id: 'chat-1',
              title: 'Fix the build',
              cwd: '/home/dev/src/loams',
              branch: 'loams-desktop/fix-build',
              config: {
                harness: HarnessId.MOCK,
                model: 'mock-1',
                sandbox: SandboxLevel.WORKSPACE_WRITE,
              },
            }),
          ],
        };
        await untilAborted(ctx);
      },
      listCheckpoints: () => ({
        checkpoints: [
          {
            turnId: 'msg-u1',
            ref: 'refs/loams/checkpoints/chat-1/turn/1/pre',
            createdAtMs: 1n,
            filesChanged: 1,
          },
        ],
      }),
      getThreadUsage: () => ({
        turns: [],
        totals: { inputTokens: 1200n, outputTokens: 300n, durationMs: 4200n },
      }),
      answerQuestion: (request) => {
        calls.push({ method: 'AnswerQuestion', request });
        return { commandId: 'cmd-1' };
      },
      interrupt: (request) => {
        calls.push({ method: 'Interrupt', request });
        return { commandId: 'cmd-2' };
      },
      restoreCheckpoint: (request) => {
        calls.push({ method: 'RestoreCheckpoint', request });
        return { restoredTo: '0123456789abcdef0123' };
      },
    });
    service(DiffService, {
      getCheckoutDiff: (request) => {
        calls.push({ method: 'GetCheckoutDiff', request });
        return { diff: { cwd: request.cwd, checksum: 'sum-42' } };
      },
    });
    if (o.approvals !== false) {
      service(ApprovalService, {
        async *watchApprovals(_request, ctx) {
          yield {
            pending: [
              create(ApprovalSchema, {
                id: 'call-9',
                chatId: 'chat-1',
                turnId: 'msg-u1',
                tool: 'Bash',
                args: fromJson(ValueSchema, { command: 'cargo build' }),
                risk: ApprovalRisk.WRITE,
                createdAtMs: 5n,
              }),
              create(ApprovalSchema, {
                id: 'call-x',
                chatId: 'chat-2',
                tool: 'Bash',
                createdAtMs: 6n,
              }),
            ],
          };
          await untilAborted(ctx);
        },
        resolveApproval: (request) => {
          calls.push({ method: 'ResolveApproval', request });
          return {};
        },
      });
    }
  });
  return { transport, calls, watches };
}

function renderPage(transport: Transport, extra?: React.ReactNode) {
  const navigate = vi.fn();
  const utils = render(
    wrap(
      <AgentdTransportProvider transport={transport}>
        <ThreadPage threadId="chat-1" navigate={navigate} />
        {extra}
      </AgentdTransportProvider>,
    ),
  );
  return { navigate, ...utils };
}

describe('ThreadPage', () => {
  it('shows the transcript, the header and the turn summary', async () => {
    const daemon = fakeDaemon();
    const { findByText, getByRole, container } = renderPage(daemon.transport);
    await findByText('Building now.');
    await findByText('Fix the build', { selector: 'h2' });
    const header = container.querySelector('[data-thread-header]') as HTMLElement;
    await waitFor(() => expect(header.textContent).toContain('Mock · mock-1'));
    expect(header.textContent).toContain('loams-desktop/fix-build');
    expect(header.textContent).toContain('workspace write');
    await waitFor(() => expect(header.textContent).toContain('1.5K tokens'));
    getByRole('log', { name: 'Thread timeline' });
    expect(daemon.watches).toEqual(['chat-1']);
  });

  it('hands the checkout to the desktop editor (SD1 Task 28, Q810)', async () => {
    const daemon = fakeDaemon();
    const openInEditor = vi.fn(async () => ({
      ok: false as const,
      code: 'no_editor',
      message: 'Choose an editor first',
    }));
    const { findByRole, findByText } = render(
      wrap(
        <AgentdTransportProvider transport={daemon.transport}>
          <ThreadPage threadId="chat-1" navigate={vi.fn()} openInEditor={openInEditor} />
        </AgentdTransportProvider>,
      ),
    );
    fireEvent.click(await findByRole('button', { name: 'Open in editor' }));
    await waitFor(() => expect(openInEditor).toHaveBeenCalledWith('/home/dev/src/loams'));
    await findByText('Open in editor failed: Choose an editor first');
  });

  it('offers no editor without the desktop bridge', async () => {
    const daemon = fakeDaemon();
    const { findByText, queryByRole } = renderPage(daemon.transport);
    await findByText('Building now.');
    expect(queryByRole('button', { name: 'Open in editor' })).toBeNull();
  });

  it('answers a question through AnswerQuestion', async () => {
    const daemon = fakeDaemon();
    const { findByRole } = renderPage(daemon.transport);
    const card = await findByRole('group', { name: 'Question: Branch' });
    fireEvent.click(within(card).getByRole('button', { name: /dev/ }));
    await waitFor(() => expect(daemon.calls.map((c) => c.method)).toContain('AnswerQuestion'));
    expect(daemon.calls.find((c) => c.method === 'AnswerQuestion')?.request).toMatchObject({
      chatId: 'chat-1',
      requestId: 'req-1',
      answers: [{ questionId: 'q-1', labels: ['dev'] }],
    });
  });

  it('opens file links in the file viewer and turn diffs in the diff panel', async () => {
    const daemon = fakeDaemon();
    const { findByRole, navigate } = renderPage(daemon.transport);
    fireEvent.click(await findByRole('button', { name: 'View diff' }));
    expect(navigate).toHaveBeenCalledWith('/studio/t/chat-1?panel=diff&turn=msg-u1');
  });
});

describe('approval_always_scoped_to_thread', () => {
  it("shows only this thread's approvals, and Always is sent for this thread", async () => {
    const daemon = fakeDaemon();
    const { findByRole, queryAllByRole } = renderPage(daemon.transport);
    const card = await findByRole('region', { name: 'Command approval' });
    expect(card.textContent).toContain('cargo build');
    expect(queryAllByRole('region', { name: /approval/ })).toHaveLength(1);
    await act(async () => {
      fireEvent.click(within(card).getByRole('button', { name: /Always for this thread/ }));
    });
    const resolve = daemon.calls.find((c) => c.method === 'ResolveApproval');
    expect(resolve?.request).toMatchObject({
      callId: 'call-9',
      chatId: 'chat-1',
      decision: ApprovalDecision.ALWAYS,
    });
  });

  it('shows no approvals when the daemon has none yet (unimplemented)', async () => {
    const daemon = fakeDaemon({ approvals: false });
    const { findByText, queryByRole, queryByText } = renderPage(daemon.transport);
    await findByText('Building now.');
    expect(queryByRole('region', { name: /approval/ })).toBeNull();
    expect(queryByText(/Approvals are unavailable/)).toBeNull();
  });
});

describe('restore_turn_requires_confirmation', () => {
  it('restores only after confirming, against the checkout checksum', async () => {
    const daemon = fakeDaemon();
    const { findByRole, findByText } = renderPage(daemon.transport);
    fireEvent.click(await findByRole('button', { name: /Restore to before this turn/ }));
    const dialog = await findByRole('alertdialog');
    expect(daemon.calls.map((c) => c.method)).not.toContain('RestoreCheckpoint');
    await act(async () => {
      fireEvent.click(within(dialog).getByRole('button', { name: 'Restore' }));
    });
    await findByText(/Restored the checkout to 0123456789ab/);
    expect(daemon.calls.find((c) => c.method === 'GetCheckoutDiff')?.request).toMatchObject({
      cwd: '/home/dev/src/loams',
      chatId: 'chat-1',
    });
    expect(daemon.calls.find((c) => c.method === 'RestoreCheckpoint')?.request).toMatchObject({
      chatId: 'chat-1',
      turnId: 'msg-u1',
      expectedChecksum: 'sum-42',
    });
  });
});

/** The dock's view of a thread: the compact timeline over the same store (D917). */
function Dock({ transport }: { transport: Transport }) {
  const store = useTranscriptStore(transport);
  const transcript = useThreadTranscript(store as NonNullable<typeof store>, 'chat-1');
  return (
    <div data-dock="">
      <Timeline entries={transcript.entries} variant="compact" />
    </div>
  );
}

describe('dock_and_page_share_store', () => {
  it('opens one watch for the page and the dock, and both show the same transcript', async () => {
    const daemon = fakeDaemon();
    const { container, findAllByText } = renderPage(
      daemon.transport,
      <Dock transport={daemon.transport} />,
    );
    expect(await findAllByText('Building now.')).toHaveLength(2);
    expect(daemon.watches).toEqual(['chat-1']);
    const dock = container.querySelector('[data-dock] [data-timeline="compact"]');
    const page = container.querySelector('[data-thread-page] [data-timeline="page"]');
    const ids = (root: Element | null) =>
      [...(root?.querySelectorAll('[data-row-id]') ?? [])].map((n) =>
        n.getAttribute('data-row-id'),
      );
    expect(ids(dock)).toEqual(ids(page));
    expect(ids(dock).length).toBeGreaterThan(5);
  });
});
