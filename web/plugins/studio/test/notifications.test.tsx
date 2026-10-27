// The notification surface (SD1 Task 14; §53 §6.5 G19): the bell lists the daemon's
// notifications newest first, counts the ones that arrived since the page opened, leads to
// their thread or run group, and hides itself while the daemon does not serve
// notifications. (OS notifications are Electron main's, SD1 Task 28; the thread header's
// usage is Task 18's `ThreadHeader` over `GetThreadUsage`.)

import type { MessageInitShape } from '@bufbuild/protobuf';
import { createRouterTransport } from '@connectrpc/connect';
import { AgentdScope } from '@loams/console-host/query';
import { desktopAgentdEndpoint } from '@loams/platform-electron';
import { createMockAgentdApi } from '@loams/plugin-desktop-agentd';
import {
  NotificationKind,
  type NotificationSchema,
  NotificationService,
} from '@loams/proto/agentd';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vite-plus/test';
import { studioAgentd } from '../src/agentd.js';
import {
  addNotification,
  type StudioNotification,
  toStudioNotification,
  webNotifier,
} from '../src/notifications/data.js';
import { NotificationsBell } from '../src/notifications/NotificationsBell.js';
import { wrap } from './fakes.js';
import { live } from './sidebar-fakes.js';

type NotificationInit = MessageInitShape<typeof NotificationSchema>;

/** A daemon serving notifications, a feed one can post to. */
function daemon(o: { notifications?: false } = {}) {
  const feed = live<NotificationInit[]>([]);
  const transport = createRouterTransport(({ service }) => {
    if (o.notifications !== false) {
      service(NotificationService, {
        async *watchNotifications(_req, ctx) {
          let sent = 0;
          for await (const list of feed.stream(ctx.signal)) {
            for (const n of list.slice(sent)) yield { notification: n };
            sent = list.length;
          }
        },
      });
    }
  });
  const mock = createMockAgentdApi({ running: true, background: 'off' });
  const agentd = studioAgentd(desktopAgentdEndpoint({ agentd: mock.api }, transport), {
    pickFolder: async () => null,
  });
  const post = (n: {
    id: string;
    kind: NotificationKind;
    title: string;
    chatId?: string;
    groupId?: string;
    createdAtMs?: bigint;
  }) =>
    act(() => {
      feed.set([...feed.get(), { createdAtMs: BigInt(Date.now()), ...n }]);
    });
  return { agentd, post };
}

function showBell(d: ReturnType<typeof daemon>) {
  const navigate = vi.fn();
  render(
    wrap(
      <AgentdScope agentd={d.agentd}>
        <NotificationsBell navigate={navigate} />
      </AgentdScope>,
    ),
  );
  return { navigate };
}

beforeEach(() => {
  window.matchMedia = ((query: string) => ({
    matches: query.includes('min-width'),
    media: query,
    addEventListener: () => {},
    removeEventListener: () => {},
  })) as unknown as typeof window.matchMedia;
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe('the notification surface', () => {
  it('notifications_list_and_count_new_ones', async () => {
    const d = daemon();
    // One from before the page opened: listed, not counted.
    d.post({
      id: 'old',
      kind: NotificationKind.TURN_DONE,
      chatId: 't-1',
      title: 'Turn finished: Old work',
      createdAtMs: BigInt(Date.now() - 60_000),
    });
    const { navigate } = showBell(d);
    await screen.findByRole('button', { name: 'Notifications' });
    d.post({ id: 'n-1', kind: NotificationKind.ERROR, chatId: 't-2', title: 'Turn failed: Build' });
    d.post({
      id: 'n-2',
      kind: NotificationKind.GROUP_READY,
      groupId: 'g-1',
      title: 'Run group ready',
    });
    const bell = await screen.findByRole('button', { name: 'Notifications, 2 unread' });
    fireEvent.click(bell);
    const list = await screen.findByRole('list', { name: 'Notifications' });
    const links = [...list.querySelectorAll('a')].map((a) => a.getAttribute('href'));
    expect(links).toEqual(['#/studio/g/g-1', '#/studio/t/t-2', '#/studio/t/t-1']);
    expect(list.textContent).toContain('Turn failed: Build');
    fireEvent.click(screen.getByRole('link', { name: /Turn failed: Build/ }));
    expect(navigate).toHaveBeenCalledWith('/studio/t/t-2');
    // Opened, so read.
    await screen.findByRole('button', { name: 'Notifications' });
  });

  it('notifications_hidden_when_unimplemented', async () => {
    const d = daemon({ notifications: false });
    showBell(d);
    // The stream fails `unimplemented`: no bell, and no error either.
    await waitFor(() => expect(screen.queryByRole('button', { name: /Notifications/ })).toBeNull());
    await new Promise((r) => setTimeout(r, 50));
    expect(screen.queryByRole('button', { name: /Notifications/ })).toBeNull();
  });

  it('notifications_kept_by_id_newest_first', () => {
    const at = (id: string, ms: number) =>
      toStudioNotification({
        $typeName: 'loams.agentd.v1.Notification',
        id,
        kind: NotificationKind.APPROVAL,
        title: id,
        createdAtMs: BigInt(ms),
      });
    let list = addNotification([], at('a', 1));
    list = addNotification(list, at('b', 3));
    list = addNotification(list, at('a', 1));
    list = addNotification(list, at('c', 2));
    expect(list.map((n) => n.id)).toEqual(['b', 'c', 'a']);
    expect(list[0]?.href).toBeUndefined();
  });

  it('calls onArrive for notifications that arrive after the page opened (W15)', async () => {
    const d = daemon();
    d.post({
      id: 'old',
      kind: NotificationKind.TURN_DONE,
      title: 'Earlier',
      createdAtMs: BigInt(Date.now() - 60_000),
    });
    const arrived: string[] = [];
    render(
      wrap(
        <AgentdScope agentd={d.agentd}>
          <NotificationsBell navigate={vi.fn()} onArrive={(n) => arrived.push(n.id)} />
        </AgentdScope>,
      ),
    );
    d.post({
      id: 'n-new',
      kind: NotificationKind.APPROVAL,
      title: 'Deploy needs approval',
      createdAtMs: BigInt(Date.now() + 1000),
    });
    await waitFor(() => expect(arrived).toEqual(['n-new']));
  });

  it('the web edition posts OS notifications only while unfocused, and a click leads to the thread (W15)', () => {
    const notify = vi.fn(async (_n: { title: string; body: string; route?: string }) => {});
    let focused = false;
    const post = webNotifier(notify, () => focused);
    const n = (kind: NotificationKind, id = 'n1'): StudioNotification => ({
      id,
      kind,
      title: 'Deploy needs approval',
      createdAtMs: Date.now(),
      chatId: 'c1',
      href: '/studio/t/c1',
    });
    post(n(NotificationKind.APPROVAL));
    expect(notify).toHaveBeenCalledWith({
      title: 'Approval waiting',
      body: 'Deploy needs approval',
      route: '/studio/t/c1',
    });
    post(n(NotificationKind.CHECK_FAILED, 'n2'));
    expect(notify).toHaveBeenCalledTimes(1);
    focused = true;
    post(n(NotificationKind.TURN_DONE, 'n3'));
    expect(notify).toHaveBeenCalledTimes(1);
  });
});
