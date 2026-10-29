// Studio's terminals (SD1 Task 22; inventory (f); D920): T3 Code's drawer, tabs and
// links on xterm.js, over loams-agentd's TerminalService. The xterm surface is a
// stand-in here (jsdom has no canvas); the backend is a fake daemon whose streams
// the tests drive.

import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vite-plus/test';
import {
  type FileLinkTarget,
  fileLinkTarget,
  type TerminalContextSelection,
  TerminalDrawer,
  type TerminalDrawerProps,
  TerminalSessions,
  useTerminalUiStateStore,
} from '../src';
import { extractTerminalLinks } from '../src/terminal/links';
import {
  applyTerminalStreamEvent,
  EMPTY_TERMINAL_SESSION_STATE,
  encodeTerminalInput,
  terminalOutputText,
} from '../src/terminal/session';
import { isMultiLinePaste, sanitizePaste } from '../src/terminal/TerminalDrawer';
import { fakeBackend, fakeSurfaces } from './terminal-fakes';

const LINUX = 'Linux x86_64';
const NO_RETRY = { retryDelays: [] } as const;

function setup(
  props: Partial<TerminalDrawerProps> = {},
  o: { retryDelays?: readonly number[] } = NO_RETRY,
) {
  const daemon = fakeBackend();
  const sessions = new TerminalSessions(daemon.backend, o);
  const surfaces = fakeSurfaces();
  const onOpenUrl = vi.fn();
  const onOpenFile = vi.fn<(t: FileLinkTarget) => void>();
  const onSendSelection = vi.fn<(s: TerminalContextSelection) => void>();
  const all: TerminalDrawerProps = {
    threadId: 'chat-1',
    sessions,
    onOpenUrl,
    onOpenFile,
    onSendSelection,
    platform: LINUX,
    createSurface: surfaces.create,
    ...props,
  };
  const view = render(<TerminalDrawer {...all} />);
  return { daemon, sessions, surfaces, onOpenUrl, onOpenFile, onSendSelection, view, props: all };
}

/** Waits for the drawer's first terminal and its stream. */
async function firstTerminal(t: ReturnType<typeof setup>) {
  await waitFor(() => expect(t.daemon.stream('pty-1')).toBeDefined());
  const surface = t.surfaces.last();
  if (!surface) throw new Error('no surface');
  return {
    stream: t.daemon.stream('pty-1') as NonNullable<ReturnType<typeof t.daemon.stream>>,
    surface,
  };
}

beforeEach(() => {
  useTerminalUiStateStore.setState({ terminalUiStateByThreadId: {} });
  try {
    sessionStorage.clear();
  } catch {
    // jsdom always has it.
  }
});
afterEach(cleanup);

describe('terminal_writes_stream_and_sends_input', () => {
  it('opens a terminal for the thread, writes its output and sends typed input as base64', async () => {
    const t = setup();
    const { stream, surface } = await firstTerminal(t);
    expect(t.daemon.opened).toEqual([{ chatId: 'chat-1' }]);
    expect(stream.afterSeq).toBeUndefined();

    act(() => stream.data(1, 'hello '));
    act(() => stream.data(2, 'wörld\r\n$ '));
    await waitFor(() => expect(surface.written.join('')).toBe('hello wörld\r\n$ '));

    act(() => surface.type('ls -la\r'));
    await waitFor(() => expect(t.daemon.writes).toHaveLength(1));
    expect(t.daemon.writes[0]).toMatchObject({ id: 'pty-1', text: 'ls -la\r' });
    // Always base64, never the daemon's plain-text fallback.
    expect(t.daemon.writes[0]?.raw).toBe(btoa('ls -la\r'));
  });

  it('decodes a character split across two chunks', () => {
    const [first, second] = new TextEncoder().encode('é');
    const decoder = new TextDecoder();
    const half = (b: number | undefined) => btoa(String.fromCharCode(b ?? 0));
    let state = applyTerminalStreamEvent(
      EMPTY_TERMINAL_SESSION_STATE,
      { type: 'data', seq: 1, data: half(first) },
      decoder,
    );
    state = applyTerminalStreamEvent(state, { type: 'data', seq: 2, data: half(second) }, decoder);
    expect(terminalOutputText(state.output)).toBe('é');
    // A replayed seq is skipped.
    expect(
      applyTerminalStreamEvent(state, { type: 'data', seq: 2, data: btoa('x') }, decoder),
    ).toBe(state);
  });

  it('splits large input into chunks the daemon accepts', () => {
    const chunks = encodeTerminalInput('x'.repeat(70 * 1024));
    expect(chunks.length).toBeGreaterThan(1);
    const total = chunks.reduce((n, c) => n + atob(c).length, 0);
    expect(total).toBe(70 * 1024);
    for (const c of chunks) expect(atob(c).length).toBeLessThanOrEqual(32 * 1024);
  });
});

describe('terminal_reattach_replays', () => {
  it('resubscribes after the last seq when the stream drops, without repeating output', async () => {
    const t = setup({}, { retryDelays: [0, 0, 0] });
    const { stream, surface } = await firstTerminal(t);
    act(() => stream.data(1, 'one\r\n'));
    act(() => stream.data(2, 'two\r\n'));
    await waitFor(() => expect(surface.written.join('')).toBe('one\r\ntwo\r\n'));

    act(() => stream.end());
    await waitFor(() => expect(t.daemon.streams.get('pty-1')).toHaveLength(2));
    const again = t.daemon.stream('pty-1');
    expect(again?.afterSeq).toBe(2);
    // A replay that overlaps what the client holds is skipped by seq.
    act(() => again?.data(2, 'two\r\n'));
    act(() => again?.data(3, 'three\r\n'));
    await waitFor(() => expect(surface.written.join('')).toBe('one\r\ntwo\r\nthree\r\n'));
  });

  it('shows the buffered output again and asks only for what followed when the drawer comes back', async () => {
    const t = setup();
    const { stream } = await firstTerminal(t);
    act(() => stream.data(1, 'build ok\r\n'));
    await waitFor(() => expect(t.surfaces.last()?.written.join('')).toBe('build ok\r\n'));

    t.view.unmount();
    expect(stream.aborted).toBe(true);

    render(<TerminalDrawer {...t.props} />);
    await waitFor(() => expect(t.daemon.streams.get('pty-1')).toHaveLength(2));
    expect(t.daemon.stream('pty-1')?.afterSeq).toBe(1);
    const surface = t.surfaces.last();
    expect(surface?.written.join('')).toBe('build ok\r\n');
    // The same tab came back: no second terminal was opened.
    expect(t.daemon.opened).toHaveLength(1);
  });

  it('a terminal the daemon no longer has says so', async () => {
    const t = setup();
    const { stream, surface } = await firstTerminal(t);
    act(() => stream.end(Object.assign(new Error('Terminal not found'), { code: 5 })));
    await waitFor(() =>
      expect(surface.written.join('')).toContain('no longer running on the daemon'),
    );
  });
});

describe('terminal_disposed_on_unmount', () => {
  it('disposes xterm and stops the stream, leaving the shell running', async () => {
    const t = setup();
    const { stream, surface } = await firstTerminal(t);
    t.view.unmount();
    expect(surface.disposed).toBe(true);
    expect(stream.aborted).toBe(true);
    expect(t.daemon.closed).toEqual([]);
  });
});

describe('multiline_paste_confirms', () => {
  it('pastes one line straight away and asks before more than one', async () => {
    const t = setup();
    const { stream, surface } = await firstTerminal(t);
    act(() => stream.data(1, '$ '));
    const host = t.view.container.querySelector('[data-terminal] > div') as HTMLElement;

    const paste = (text: string) => {
      const event = new Event('paste', { bubbles: true, cancelable: true }) as ClipboardEvent;
      Object.defineProperty(event, 'clipboardData', {
        value: { getData: () => text },
      });
      host.dispatchEvent(event);
      return event;
    };

    // One line (even with its newline) is pasted at once, through xterm's own paste.
    expect(paste('echo hi\n').defaultPrevented).toBe(true);
    expect(screen.queryByRole('alertdialog')).toBeNull();
    expect(surface.pasted).toEqual(['echo hi\n']);

    const event = paste('rm -rf build\nmake\n');
    expect(event.defaultPrevented).toBe(true);
    const dialog = await screen.findByRole('alertdialog');
    expect(dialog.textContent).toContain('Paste 2 lines?');
    expect(surface.pasted).toEqual(['echo hi\n']);
    fireEvent.click(screen.getByRole('button', { name: 'Paste' }));
    await waitFor(() => expect(surface.pasted).toEqual(['echo hi\n', 'rm -rf build\nmake\n']));
    await waitFor(() =>
      expect(t.daemon.writes.map((w) => w.text)).toEqual(['echo hi\n', 'rm -rf build\nmake\n']),
    );
  });

  it('cancel pastes nothing', async () => {
    const t = setup();
    const { surface } = await firstTerminal(t);
    const host = t.view.container.querySelector('[data-terminal] > div') as HTMLElement;
    const event = new Event('paste', { bubbles: true, cancelable: true }) as ClipboardEvent;
    Object.defineProperty(event, 'clipboardData', { value: { getData: () => 'a\nb' } });
    host.dispatchEvent(event);
    await screen.findByRole('alertdialog');
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    await waitFor(() => expect(screen.queryByRole('alertdialog')).toBeNull());
    expect(surface.pasted).toEqual([]);
    expect(t.daemon.writes).toEqual([]);
  });

  // SD1 Task 30: a hidden ESC [201~ would end a bracketed paste early and run the rest.
  it('strips control characters from a paste before it reaches the shell', async () => {
    const t = setup();
    const { stream, surface } = await firstTerminal(t);
    act(() => stream.data(1, '$ '));
    const host = t.view.container.querySelector('[data-terminal] > div') as HTMLElement;
    const event = new Event('paste', { bubbles: true, cancelable: true }) as ClipboardEvent;
    Object.defineProperty(event, 'clipboardData', {
      value: { getData: () => 'ls\u001b[201~ rm -rf ~\u0003\u0004\u009b' },
    });
    host.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(true);
    await waitFor(() => expect(surface.pasted).toEqual(['ls[201~ rm -rf ~']));
    expect(sanitizePaste('a\tb\r\nc\u007f\u0085')).toBe('a\tb\r\nc');
  });

  it('counts lines as a shell would run them', () => {
    expect(isMultiLinePaste('one')).toBe(false);
    expect(isMultiLinePaste('one\n')).toBe(false);
    expect(isMultiLinePaste('one\r\ntwo')).toBe(true);
    expect(isMultiLinePaste('one\rtwo')).toBe(true);
  });
});

describe('close_running_confirms', () => {
  it('asks before closing a running terminal, and closes it on the daemon only on yes', async () => {
    const t = setup();
    const { stream } = await firstTerminal(t);
    act(() => stream.data(1, '$ '));
    await waitFor(() => expect(t.sessions.get('pty-1').status).toBe('running'));

    fireEvent.click(screen.getByRole('button', { name: 'Close terminal' }));
    const dialog = await screen.findByRole('alertdialog');
    expect(dialog.textContent).toContain('Close terminal "Terminal 1"?');
    expect(dialog.textContent).toContain('stops the running process');
    expect(t.daemon.closed).toEqual([]);

    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    await waitFor(() => expect(screen.queryByRole('alertdialog')).toBeNull());
    expect(t.daemon.closed).toEqual([]);

    fireEvent.click(screen.getByRole('button', { name: 'Close terminal' }));
    await screen.findByRole('alertdialog');
    fireEvent.click(within('alertdialog').getByRole('button', { name: 'Close terminal' }));
    await waitFor(() => expect(t.daemon.closed).toEqual(['pty-1']));
    await screen.findByText('No terminals for this thread yet.');
  });

  it('closes a terminal whose process exited without asking', async () => {
    const t = setup();
    const { stream, surface } = await firstTerminal(t);
    act(() => stream.exit(1, 0));
    await waitFor(() => expect(surface.written.join('')).toContain('Process exited with code 0'));
    fireEvent.click(screen.getByRole('button', { name: 'Close terminal' }));
    await waitFor(() => expect(t.daemon.closed).toEqual(['pty-1']));
    expect(screen.queryByRole('alertdialog')).toBeNull();
  });
});

function within(role: string) {
  const el = screen.getByRole(role);
  return {
    getByRole: (r: string, o: { name: string }) =>
      [...el.querySelectorAll(r === 'button' ? 'button' : `[role="${r}"]`)].find(
        (b) => b.textContent === o.name,
      ) as HTMLElement,
  };
}

describe('file_link_opens_viewer', () => {
  async function linksOn(text: string) {
    const t = setup();
    const { stream, surface } = await firstTerminal(t);
    act(() => stream.data(1, text));
    await waitFor(() => expect(surface.lines.join('')).toContain(text.trim()));
    const provider = surface.links;
    if (!provider) throw new Error('no link provider');
    const row = surface.lines.findIndex((l) => l.length > 0) + 1;
    const links = await new Promise<
      NonNullable<Parameters<Parameters<typeof provider.provideLinks>[1]>[0]>
    >((resolve) => provider.provideLinks(row, (l) => resolve(l ?? [])));
    return { t, links };
  }

  it('opens a compiler path in the viewer with its line, relative to the terminal', async () => {
    const { t, links } = await linksOn('src/main.rs:12:5: error: oops');
    expect(links.map((l) => l.text)).toEqual(['src/main.rs:12:5']);
    const link = links[0];
    // A plain click does nothing; Ctrl+click (Cmd on macOS) opens.
    link?.activate(new MouseEvent('click'), link.text);
    expect(t.onOpenFile).not.toHaveBeenCalled();
    link?.activate(new MouseEvent('click', { ctrlKey: true }), link.text);
    expect(t.onOpenFile).toHaveBeenCalledWith({ path: 'src/main.rs', line: 12, column: 5 });
    expect(t.onOpenUrl).not.toHaveBeenCalled();
  });

  it('opens an http(s) link outside Studio', async () => {
    const { t, links } = await linksOn('Local: http://localhost:5173/ ready');
    expect(links.map((l) => l.text)).toEqual(['http://localhost:5173/']);
    links[0]?.activate(new MouseEvent('click', { ctrlKey: true }), 'http://localhost:5173/');
    expect(t.onOpenUrl).toHaveBeenCalledWith('http://localhost:5173/');
    expect(t.onOpenFile).not.toHaveBeenCalled();
  });

  it('resolves absolute paths inside the worktree to relative ones', () => {
    const cwd = '/home/omar/src/site';
    expect(fileLinkTarget('/home/omar/src/site/app/page.tsx:3', cwd)).toEqual({
      path: 'app/page.tsx',
      line: 3,
    });
    expect(fileLinkTarget('./lib/a.ts', cwd)).toEqual({ path: 'lib/a.ts' });
    expect(fileLinkTarget('/etc/hosts', cwd)).toEqual({ path: '/etc/hosts' });
    expect(fileLinkTarget('../other/x.ts', cwd)).toEqual({ path: '/home/omar/src/other/x.ts' });
  });

  it('finds links as upstream does', () => {
    expect(extractTerminalLinks('see https://example.com/a). and ./b/c.ts:1:2:')).toEqual([
      { kind: 'url', text: 'https://example.com/a', start: 4, end: 25 },
      { kind: 'path', text: './b/c.ts:1:2', start: 32, end: 44 },
    ]);
  });
});

describe('selection_to_composer', () => {
  it('sends the selected lines, labelled by terminal and line range', async () => {
    const t = setup();
    const { stream, surface } = await firstTerminal(t);
    act(() => stream.data(1, 'a\r\nerror: boom\r\n  at x\r\n'));
    expect(screen.queryByRole('button', { name: 'Add to composer' })).toBeNull();
    act(() =>
      surface.select('error: boom\r\n  at x\r\n', {
        start: { x: 0, y: 1 },
        end: { x: 6, y: 2 },
      }),
    );
    fireEvent.click(await screen.findByRole('button', { name: 'Add to composer' }));
    expect(t.onSendSelection).toHaveBeenCalledWith({
      terminalId: 'pty-1',
      terminalLabel: 'Terminal 1',
      lineStart: 2,
      lineEnd: 3,
      text: 'error: boom\n  at x',
    });
    expect(surface.selection).toBeNull();
    expect(screen.queryByRole('button', { name: 'Add to composer' })).toBeNull();
  });
});

describe('the drawer', () => {
  it('splits into a group and lists the terminals', async () => {
    const t = setup();
    await firstTerminal(t);
    fireEvent.click(screen.getByRole('button', { name: 'Split terminal side by side' }));
    await waitFor(() => expect(t.daemon.stream('pty-2')).toBeDefined());
    const list = screen.getByRole('complementary', { name: 'Terminals' });
    expect(list.textContent).toContain('Terminal 1');
    expect(list.textContent).toContain('Terminal 2');
    expect(list.textContent).toContain('Side by side');
    expect(t.view.container.querySelectorAll('[data-terminal]')).toHaveLength(2);
  });

  it('lets Studio and shell shortcuts through xterm', async () => {
    const isAppShortcut = (e: KeyboardEvent) => e.ctrlKey && e.key === '`';
    const t = setup({ isAppShortcut });
    const { surface } = await firstTerminal(t);
    const handler = surface.keyHandler;
    expect(handler?.(new KeyboardEvent('keydown', { key: '`', ctrlKey: true }))).toBe(false);
    expect(handler?.(new KeyboardEvent('keydown', { key: 'c', ctrlKey: true }))).toBe(true);
  });

  it('runs a project action as a named terminal', async () => {
    const onRunAction = vi.fn(async () => ({
      terminal: { id: 'pty-run', cwd: '/home/omar/src/site', shell: 'zsh' },
      name: 'Dev server',
    }));
    const t = setup({ actions: [{ id: 'dev', name: 'Dev server' }], onRunAction });
    await firstTerminal(t);
    fireEvent.click(screen.getByRole('button', { name: 'Run a project action' }));
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Dev server' }));
    await waitFor(() => expect(t.daemon.stream('pty-run')).toBeDefined());
    expect(onRunAction).toHaveBeenCalledWith('dev', { cols: 80, rows: 24 });
    expect(screen.getByRole('complementary', { name: 'Terminals' }).textContent).toContain(
      'Dev server',
    );
  });

  it('shows the error and a retry when the daemon cannot open a terminal', async () => {
    const daemon = fakeBackend();
    daemon.backend.open = async () => {
      throw new Error('Too many open terminals (maximum 32)');
    };
    const sessions = new TerminalSessions(daemon.backend, NO_RETRY);
    render(
      <TerminalDrawer
        threadId="chat-9"
        sessions={sessions}
        onOpenUrl={() => {}}
        onOpenFile={() => {}}
        createSurface={fakeSurfaces().create}
      />,
    );
    expect(await screen.findByRole('alert')).toHaveProperty(
      'textContent',
      'Too many open terminals (maximum 32)',
    );
    expect(screen.getByRole('button', { name: 'New terminal' })).toBeDefined();
  });

  it('keeps tabs per browser tab in sessionStorage', async () => {
    const t = setup();
    await firstTerminal(t);
    const stored = JSON.parse(sessionStorage.getItem('loams.studio.terminals') ?? '{}');
    expect(Object.keys(stored.state.terminalUiStateByThreadId)).toEqual(['chat-1']);
    expect(stored.state.terminalUiStateByThreadId['chat-1'].terminalIds).toEqual(['pty-1']);
  });
});
