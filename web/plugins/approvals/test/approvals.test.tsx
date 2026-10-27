import { create, fromJson } from '@bufbuild/protobuf';
import { ValueSchema } from '@bufbuild/protobuf/wkt';
import { createClient, createRouterTransport } from '@connectrpc/connect';
import type { AgentdService } from '@loams/console-host';
import { createMockControl } from '@loams/console-host/testing';
import { approvals } from '@loams/proto';
import {
  ApprovalSchema as AgentApprovalSchema,
  ApprovalService as AgentApprovalService,
  ApprovalDecision as AgentDecision,
  ApprovalRisk,
  type ResolveApprovalRequest,
} from '@loams/proto/agentd';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vite-plus/test';
import { AgentInbox, followAgentApprovals, InboxPage, pendingCount } from '../src/index.js';
import { Inbox, reasonOf } from '../src/store.js';

afterEach(cleanup);

async function live(options?: Parameters<typeof createMockControl>[0]) {
  const mock = createMockControl(options);
  const client = createClient(approvals.ApprovalService, mock.transport);
  const inbox = new Inbox();
  const controller = new AbortController();
  (async () => {
    try {
      for await (const m of client.watchApprovals({}, { signal: controller.signal }))
        inbox.apply(m);
    } catch {
      // aborted
    }
  })();
  await waitFor(() => expect(inbox.getSnapshot().connected).toBe(true));
  return { mock, client, inbox, stop: () => controller.abort() };
}

describe('Inbox', () => {
  it('applies snapshot, upsert and remove, and announces only new arrivals', () => {
    const inbox = new Inbox();
    const announced: string[] = [];
    inbox.onNew = (a) => announced.push(a.id);
    const a = (id: string) =>
      ({ ...new Inbox(), id, state: approvals.ApprovalState.PENDING }) as never;
    inbox.apply({
      event: { case: 'snapshot', value: { approvals: [a('one')] } },
      cursor: 'c1',
    } as never);
    inbox.apply({ event: { case: 'upsert', value: a('two') }, cursor: 'c2' } as never);
    inbox.apply({ event: { case: 'heartbeat', value: {} }, cursor: '' } as never);
    expect(inbox.getSnapshot().approvals.map((x) => x.id)).toEqual(['one', 'two']);
    expect(inbox.getSnapshot().cursor).toBe('c2');
    inbox.apply({ event: { case: 'remove', value: 'one' }, cursor: 'c3' } as never);
    expect(inbox.getSnapshot().approvals.map((x) => x.id)).toEqual(['two']);
    expect(announced).toEqual(['two']);
  });

  it('reads reasons from ErrorInfo details or the mock message', () => {
    expect(reasonOf({ rawMessage: 'sign in again [step_up_required]' })).toBe('step_up_required');
    expect(reasonOf(new Error('plain'))).toBeUndefined();
  });
});

describe('InboxPage', () => {
  it('approval_appears_live_and_decides_once', async () => {
    const { mock, client, inbox, stop } = await live();
    render(<InboxPage inbox={inbox} client={client} />);
    expect(screen.getByText('Create an API key for search-dev')).toBeTruthy();
    act(() =>
      mock.addApproval({
        id: 'apr_live',
        revision: 1n,
        kind: 'agent.action',
        summary: 'Let the agent run a migration',
        state: approvals.ApprovalState.PENDING,
        policy: { stepUp: approvals.StepUp.NONE },
      }),
    );
    await screen.findByText('Let the agent run a migration');

    const card = screen
      .getByText('Let the agent run a migration')
      .closest('section') as HTMLElement;
    fireEvent.click(card.querySelector('button.loams-btn-primary') as HTMLButtonElement);
    await waitFor(() => expect(screen.queryByText('Let the agent run a migration')).toBeNull());
    expect(mock.approvals().find((a) => a.id === 'apr_live')?.state).toBe(
      approvals.ApprovalState.APPROVED,
    );
    stop();
  });

  it('destructive approvals need a reason and the typed target name', async () => {
    const { client, inbox, stop } = await live();
    render(<InboxPage inbox={inbox} client={client} />);
    const card = screen
      .getByText('Drop the collection docs in production')
      .closest('section') as HTMLElement;
    const approve = card.querySelector('button.loams-btn-primary') as HTMLButtonElement;
    expect(approve.disabled).toBe(true);
    const [reason, confirm] = card.querySelectorAll('input');
    fireEvent.change(reason as HTMLInputElement, { target: { value: 'cleanup' } });
    expect(approve.disabled).toBe(true);
    fireEvent.change(confirm as HTMLInputElement, { target: { value: 'docs' } });
    expect(approve.disabled).toBe(false);
    stop();
  });

  it('stale_session_requires_step_up', async () => {
    const { client, inbox, stop } = await live({
      authenticatedAt: new Date(Date.now() - 10 * 60_000),
    });
    render(<InboxPage inbox={inbox} client={client} />);
    const card = screen
      .getByText('Drop the collection docs in production')
      .closest('section') as HTMLElement;
    const [reason, confirm] = card.querySelectorAll('input');
    fireEvent.change(reason as HTMLInputElement, { target: { value: 'cleanup' } });
    fireEvent.change(confirm as HTMLInputElement, { target: { value: 'docs' } });
    fireEvent.click(card.querySelector('button.loams-btn-primary') as HTMLButtonElement);
    await screen.findByText(/Sign in again/);
    stop();
  });

  it('the requester cannot approve their own request', async () => {
    const { client, inbox, stop } = await live({ principal: 'usr_dana' });
    render(<InboxPage inbox={inbox} client={client} />);
    const card = screen
      .getByText('Create an API key for search-dev')
      .closest('section') as HTMLElement;
    fireEvent.click(card.querySelector('button.loams-btn-primary') as HTMLButtonElement);
    await screen.findByText(/someone else decides/);
    stop();
  });
});

describe('Inbox stream health', () => {
  it('a heartbeat after a failure does not clear the stale warning', () => {
    const inbox = new Inbox();
    inbox.apply({ event: { case: 'snapshot', value: { approvals: [] } }, cursor: 'c1' } as never);
    inbox.failed('reset');
    inbox.apply({ event: { case: 'heartbeat', value: {} }, cursor: 'c2' } as never);
    expect(inbox.getSnapshot()).toMatchObject({ connected: false, error: 'reset', cursor: 'c2' });
    inbox.apply({ event: { case: 'snapshot', value: { approvals: [] } }, cursor: 'c3' } as never);
    expect(inbox.getSnapshot().connected).toBe(true);
    expect(inbox.getSnapshot().error).toBeUndefined();
  });
});

describe('pendingCount', () => {
  it('badge_counts_only_pending_approvals', () => {
    const mk = (state: number) => ({ state }) as never;
    expect(pendingCount({ approvals: [] })).toBe(0);
    expect(
      pendingCount({
        approvals: [
          mk(approvals.ApprovalState.PENDING),
          mk(approvals.ApprovalState.PENDING),
          mk(approvals.ApprovalState.APPROVED),
        ],
      }),
    ).toBe(2);
  });
});

/** A daemon whose `WatchApprovals` holds one supervised tool call (SD1 Task 13). */
function agentDaemon() {
  const resolved: ResolveApprovalRequest[] = [];
  const transport = createRouterTransport(({ service }) => {
    service(AgentApprovalService, {
      async *watchApprovals(_request, ctx) {
        yield {
          pending: [
            create(AgentApprovalSchema, {
              id: 'call-7',
              chatId: 'chat-1',
              turnId: 'msg-1',
              tool: 'Bash',
              args: fromJson(ValueSchema, { command: 'rm -rf build' }),
              risk: ApprovalRisk.DESTRUCTIVE,
              createdAtMs: 5n,
            }),
          ],
        };
        await new Promise<void>((resolve) =>
          ctx.signal.addEventListener('abort', () => resolve(), { once: true }),
        );
      },
      resolveApproval: (request) => {
        resolved.push(request);
        return {};
      },
    });
  });
  const agentd: AgentdService = {
    kind: 'desktop',
    reach: () => ({ state: 'ready' }),
    subscribe: () => () => {},
    transport: () => transport,
    host: () => undefined,
  };
  return { agentd, resolved };
}

describe('agent approvals', () => {
  it('approvals_page_lists_agent_approvals', async () => {
    const { client, inbox, stop } = await live();
    const daemon = agentDaemon();
    const agentInbox = new AgentInbox();
    const unfollow = followAgentApprovals(daemon.agentd, agentInbox);
    render(
      <InboxPage
        inbox={inbox}
        client={client}
        agent={{
          inbox: agentInbox,
          resolve: async (approval, decision) => {
            await createClient(
              AgentApprovalService,
              daemon.agentd.transport() as never,
            ).resolveApproval({ callId: approval.id, chatId: approval.chatId, decision });
          },
        }}
      />,
    );
    // The server's approval and the agent's, side by side, each labelled by source.
    expect(screen.getByText('Create an API key for search-dev')).toBeTruthy();
    const title = await screen.findByText('Bash wants to run');
    const card = title.closest('section') as HTMLElement;
    expect(card.textContent).toContain('Agent');
    expect(card.textContent).toContain('destructive');
    expect(card.textContent).toContain('rm -rf build');
    expect(card.querySelector('a')?.getAttribute('href')).toBe('#/studio/t/chat-1');
    const server = screen.getByText('Create an API key for search-dev').closest('section');
    expect(server?.textContent).toContain('Server');

    // "Always for this thread" sends the thread with the decision.
    fireEvent.click(screen.getByRole('button', { name: 'Always for this thread' }));
    await waitFor(() => expect(daemon.resolved).toHaveLength(1));
    expect(daemon.resolved[0]).toMatchObject({
      callId: 'call-7',
      chatId: 'chat-1',
      decision: AgentDecision.ALWAYS,
    });
    await waitFor(() => expect(screen.queryByText('Bash wants to run')).toBeNull());
    unfollow();
    stop();
  });

  it('no daemon, or one without approvals, lists no agent approvals', () => {
    const agentInbox = new AgentInbox();
    const unfollow = followAgentApprovals(
      {
        kind: 'web',
        reach: () => ({ state: 'no-host' }),
        subscribe: () => () => {},
        transport: () => undefined,
        host: () => undefined,
      },
      agentInbox,
    );
    expect(agentInbox.getSnapshot()).toMatchObject({ approvals: [], available: false });
    unfollow();
  });
});
