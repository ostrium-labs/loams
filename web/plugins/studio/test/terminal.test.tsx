// Studio's terminal drawer over a daemon (SD1 Task 22; §53 §5.7): the container wires
// @loams/studio-ui's drawer to `loams.agentd.v1.TerminalService` through the agentd
// transport, and the layout's drawer to `terminal.toggle` (Ctrl+`, T24-4).

import { create } from '@bufbuild/protobuf';
import { createRouterTransport, type Transport } from '@connectrpc/connect';
import type { AgentdService } from '@loams/console-host';
import { AgentdTransportProvider } from '@loams/console-host/query';
import {
  ProjectService,
  type SubscribeTerminalRequest,
  TerminalService,
  TerminalSessionSchema,
} from '@loams/proto/agentd';
import { useTerminalUiStateStore } from '@loams/studio-ui';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vite-plus/test';
import { fakeSurfaces } from '../../../packages/studio-ui/test/terminal-fakes.js';
import { studioAgentd } from '../src/agentd.js';
import { createStudioKeys } from '../src/palette/commands.js';
import {
  fileViewerHref,
  isStudioShortcut,
  ThreadTerminalDrawer,
} from '../src/thread/TerminalDrawer.js';
import { takeTerminalContexts, terminalContexts } from '../src/thread/terminalContexts.js';
import { wrap } from './fakes.js';
import { StudioInShell } from './shell.js';

const LINUX = 'Linux x86_64';
const READY = { state: 'ready' } as const;
const b64 = (text: string) => btoa(String.fromCharCode(...new TextEncoder().encode(text)));
const unb64 = (data: string) =>
  new TextDecoder().decode(Uint8Array.from(atob(data), (c) => c.charCodeAt(0)));

/** A daemon with Studio's projects and a TerminalService whose output the test sends. */
function terminalDaemon() {
  const calls: string[] = [];
  const written: string[] = [];
  const subscriptions: SubscribeTerminalRequest[] = [];
  let push: ((text: string) => void) | null = null;
  let seq = 0n;
  const transport: Transport = createRouterTransport(({ service }) => {
    service(ProjectService, {
      listRepos: () => ({ repos: [] }),
    });
    service(TerminalService, {
      openTerminal: (req) => {
        calls.push(`OpenTerminal:${req.chatId}:${req.cols}x${req.rows}`);
        return {
          terminal: create(TerminalSessionSchema, { id: 'pty-7', cwd: '/srv/site', shell: 'bash' }),
        };
      },
      async *subscribeTerminal(req, ctx) {
        subscriptions.push(req);
        const queue: string[] = [];
        let wake: (() => void) | null = null;
        push = (text) => {
          queue.push(text);
          wake?.();
        };
        while (!ctx.signal.aborted) {
          const text = queue.shift();
          if (text === undefined) {
            await new Promise<void>((resolve) => {
              wake = resolve;
              ctx.signal.addEventListener('abort', () => resolve(), { once: true });
            });
            continue;
          }
          seq += 1n;
          yield { event: { event: { case: 'data', value: { seq, data: b64(text) } } } };
        }
      },
      writeTerminal: (req) => {
        written.push(unb64(req.data));
        return { ok: true };
      },
      resizeTerminal: (req) => {
        calls.push(`ResizeTerminal:${req.cols}x${req.rows}`);
        return { ok: true };
      },
      closeTerminal: (req) => {
        calls.push(`CloseTerminal:${req.terminalId}`);
        return { ok: true };
      },
    });
  });
  return {
    transport,
    calls,
    written,
    subscriptions,
    send: (text: string) => push?.(text),
  };
}

function viewport(wide: boolean) {
  window.matchMedia = ((query: string) => ({
    matches: query.includes('min-width') ? wide : !wide,
    media: query,
    addEventListener: () => {},
    removeEventListener: () => {},
    // xterm.js still uses the old listener API for the device pixel ratio.
    addListener: () => {},
    removeListener: () => {},
  })) as unknown as typeof window.matchMedia;
}

beforeEach(() => {
  useTerminalUiStateStore.setState({ terminalUiStateByThreadId: {} });
  takeTerminalContexts('c_1');
  try {
    localStorage.clear();
    sessionStorage.clear();
  } catch {
    // jsdom has both.
  }
  viewport(true);
  // Empty's grain canvas observes its size; jsdom has no ResizeObserver.
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver;
});
afterEach(cleanup);

function renderDrawer(
  daemon = terminalDaemon(),
  extra: { openExternal?: (u: string) => Promise<void> } = {},
) {
  const surfaces = fakeSurfaces();
  const navigate = vi.fn();
  const keys = createStudioKeys({ edition: 'desktop', platform: LINUX });
  render(
    wrap(
      <AgentdTransportProvider transport={daemon.transport}>
        <ThreadTerminalDrawer
          threadId="c_1"
          keys={keys}
          navigate={navigate}
          createSurface={surfaces.create}
          {...extra}
        />
      </AgentdTransportProvider>,
    ),
  );
  return { daemon, surfaces, navigate, keys };
}

describe('terminal_writes_stream_and_sends_input (over loams.agentd.v1)', () => {
  it('opens a PTY for the thread, shows its output and writes input as base64', async () => {
    const t = renderDrawer();
    await waitFor(() => expect(t.daemon.subscriptions).toHaveLength(1));
    expect(t.daemon.calls[0]).toBe('OpenTerminal:c_1:80x24');
    expect(t.daemon.subscriptions[0]?.terminalId).toBe('pty-7');
    expect(t.daemon.subscriptions[0]?.afterSeq).toBeUndefined();

    act(() => t.daemon.send('bash-5.2$ '));
    const surface = t.surfaces.last();
    await waitFor(() => expect(surface?.written.join('')).toBe('bash-5.2$ '));

    act(() => surface?.type('git status\r'));
    await waitFor(() => expect(t.daemon.written).toEqual(['git status\r']));
  });
});

describe('file_link_opens_viewer (the route)', () => {
  it('opens the Files panel at the path and line', () => {
    expect(fileViewerHref('c_1', { path: 'src/main.rs', line: 12, column: 5 })).toBe(
      '/studio/t/c_1?panel=files&path=src%2Fmain.rs&line=12&column=5',
    );
  });

  it('a Ctrl+click on a path in the terminal navigates there', async () => {
    const t = renderDrawer();
    await waitFor(() => expect(t.daemon.subscriptions).toHaveLength(1));
    act(() => t.daemon.send('error at ./app/page.tsx:3\r\n'));
    const surface = t.surfaces.last();
    await waitFor(() => expect(surface?.lines.join('')).toContain('page.tsx'));
    const links = await new Promise<
      { text: string; activate: (e: MouseEvent, t: string) => void }[]
    >((resolve) => surface?.links?.provideLinks(1, (l) => resolve(l ?? [])));
    links[0]?.activate(new MouseEvent('click', { ctrlKey: true }), links[0].text);
    expect(t.navigate).toHaveBeenCalledWith('/studio/t/c_1?panel=files&path=app%2Fpage.tsx&line=3');
  });

  it('an http(s) link goes to the platform', async () => {
    const openExternal = vi.fn(async () => {});
    const t = renderDrawer(undefined, { openExternal });
    await waitFor(() => expect(t.daemon.subscriptions).toHaveLength(1));
    act(() => t.daemon.send('ready on https://localhost:3000\r\n'));
    const surface = t.surfaces.last();
    await waitFor(() => expect(surface?.lines.join('')).toContain('localhost'));
    const links = await new Promise<
      { text: string; activate: (e: MouseEvent, t: string) => void }[]
    >((resolve) => surface?.links?.provideLinks(1, (l) => resolve(l ?? [])));
    links[0]?.activate(new MouseEvent('click', { ctrlKey: true }), links[0].text);
    expect(openExternal).toHaveBeenCalledWith('https://localhost:3000');
  });
});

describe('selection_to_composer (the composer list)', () => {
  it('adds the selection to the thread’s waiting contexts', async () => {
    const t = renderDrawer();
    await waitFor(() => expect(t.daemon.subscriptions).toHaveLength(1));
    const surface = t.surfaces.last();
    act(() =>
      surface?.select('npm ERR! missing script', { start: { x: 0, y: 4 }, end: { x: 9, y: 4 } }),
    );
    fireEvent.click(await screen.findByRole('button', { name: 'Add to composer' }));
    expect(terminalContexts('c_1')).toEqual([
      {
        terminalId: 'pty-7',
        terminalLabel: 'Terminal 1',
        lineStart: 5,
        lineEnd: 5,
        text: 'npm ERR! missing script',
      },
    ]);
  });
});

describe('the toggle (Ctrl+`, T24-4)', () => {
  it('Ctrl+` and the shell’s chords pass through the terminal; ordinary keys do not', () => {
    const keys = createStudioKeys({ edition: 'desktop', platform: LINUX });
    const off = keys.commands.register('terminal.toggle', () => {});
    const press = (key: string, mods: KeyboardEventInit = {}) =>
      isStudioShortcut(keys, new KeyboardEvent('keydown', { key, ...mods }));
    expect(press('`', { ctrlKey: true, code: 'Backquote' })).toBe(true);
    // On Linux the shell program keeps Ctrl+K, J, B, N and I (D1081, Review Focus 1); the
    // shell's chords it does not need pass.
    for (const k of ['k', 'j', 'b', 'n', 'i'])
      expect(press(k, { ctrlKey: true, code: `Key${k.toUpperCase()}` }), k).toBe(false);
    expect(press('P', { ctrlKey: true, shiftKey: true, code: 'KeyP' })).toBe(true);
    expect(press('Tab', { ctrlKey: true, code: 'Tab' })).toBe(true);
    expect(press('M', { ctrlKey: true, shiftKey: true, code: 'KeyM' })).toBe(true);
    expect(press('c', { ctrlKey: true, code: 'KeyC' })).toBe(false);
    // Studio's own bindings with `!terminalFocus` stay the terminal's.
    expect(press('d', { ctrlKey: true, code: 'KeyD' })).toBe(false);
    off();
    expect(press('`', { ctrlKey: true, code: 'Backquote' })).toBe(false);
  });

  it('on macOS every ⌘ chord of the shell passes; Ctrl+letter stays the terminal’s', () => {
    const keys = createStudioKeys({ edition: 'desktop', platform: 'MacIntel' });
    const press = (key: string, mods: KeyboardEventInit = {}) =>
      isStudioShortcut(keys, new KeyboardEvent('keydown', { key, ...mods }));
    for (const k of ['k', 'j', 'b', 'n', 'i'])
      expect(press(k, { metaKey: true, code: `Key${k.toUpperCase()}` }), k).toBe(true);
    expect(press('k', { ctrlKey: true, code: 'KeyK' })).toBe(false);
  });

  it('shows and hides the thread’s terminal drawer', async () => {
    const daemon = terminalDaemon();
    const service: AgentdService = {
      kind: 'desktop',
      reach: () => READY,
      subscribe: () => () => {},
      transport: () => daemon.transport,
      host: () => undefined,
    };
    const keys = createStudioKeys({ edition: 'desktop', platform: LINUX });
    render(
      wrap(
        <StudioInShell
          agentd={studioAgentd(service)}
          route={{ kind: 'thread', threadId: 'c_1', panel: 'diff' }}
          navigate={vi.fn()}
          keys={keys}
        />,
      ),
    );
    await screen.findByRole('tab', { name: 'Diff' });
    // The terminal is the shell drawer's tab (design 59).
    expect(screen.queryByRole('region', { name: 'Drawer' })).toBeNull();

    fireEvent.keyDown(document.body, { key: '`', code: 'Backquote', ctrlKey: true });
    await screen.findByRole('region', { name: 'Drawer' });
    await waitFor(() => expect(daemon.calls).toContain('OpenTerminal:c_1:80x24'));

    fireEvent.keyDown(document.body, { key: '`', code: 'Backquote', ctrlKey: true });
    await waitFor(() => expect(screen.queryByRole('region', { name: 'Drawer' })).toBeNull());
    // Hiding the drawer leaves the shell running.
    expect(daemon.calls.some((c) => c.startsWith('CloseTerminal'))).toBe(false);
  });
});
