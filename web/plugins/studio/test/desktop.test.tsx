// Loams Studio's desktop integration in the renderer (SD1 Task 28): the menu's
// "New thread", the editor choice and the notification choices Electron main keeps.

import { SHELL_COMMAND_EVENT } from '@loams/console-host';
import type { StudioEditor } from '@loams/desktop/contracts';
import { cleanup, fireEvent, render, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vite-plus/test';
import { type StudioDesktop, studioAgentd } from '../src/agentd.js';
import { listenShellCommands, StudioCommands } from '../src/palette/commands.js';
import { SETTINGS_KEY, SettingsPage } from '../src/settings/Settings.js';
import { desktopFake, wrap } from './fakes.js';

afterEach(cleanup);
beforeEach(() => localStorage.clear());

function fakeBridge(start: StudioEditor = { editor: 'vscode' }) {
  let editor = start;
  const bridge = {
    openInEditor: vi.fn(async () => ({ ok: true as const, value: undefined })),
    editor: vi.fn(async () => editor),
    setEditor: vi.fn(async (e: 'vscode' | 'zed' | 'cursor') => {
      editor = { editor: e };
      return { ok: true as const, value: undefined };
    }),
    chooseEditorCommand: vi.fn(async () => {
      editor = { editor: 'command', commandName: 'subl' };
      return { ok: true as const, value: editor };
    }),
    setNotifications: vi.fn(async () => ({ ok: true as const, value: undefined })),
  } satisfies StudioDesktop;
  return bridge;
}

describe('the desktop menu', () => {
  it('new_thread_menu_runs_chat_new', () => {
    const commands = new StudioCommands();
    const target = new EventTarget();
    const off = listenShellCommands(commands, target);
    const send = () => {
      const e = new CustomEvent(SHELL_COMMAND_EVENT, { detail: 'chat.new', cancelable: true });
      target.dispatchEvent(e);
      return e.defaultPrevented;
    };
    // No view handles chat.new yet: the sender opens Studio's home.
    expect(send()).toBe(false);
    const handler = vi.fn();
    const unregister = commands.register('chat.new', handler);
    expect(send()).toBe(true);
    expect(handler).toHaveBeenCalledOnce();
    unregister();
    off();
  });
});

describe("the shell's commands (D1081)", () => {
  it('runs the jumps and the next thread, and leaves the shell its own commands', () => {
    const commands = new StudioCommands();
    const target = new EventTarget();
    const off = listenShellCommands(commands, target);
    const send = (detail: string) => {
      const e = new CustomEvent(SHELL_COMMAND_EVENT, { detail, cancelable: true });
      target.dispatchEvent(e);
      return e.defaultPrevented;
    };
    const ran: string[] = [];
    for (const c of ['thread.jump.3', 'thread.next', 'settings.open', 'sidebar.toggle'] as const)
      commands.register(c, () => ran.push(c));
    expect(send('thread.jump.3')).toBe(true);
    expect(send('thread.next')).toBe(true);
    // Studio has commands of these names, but from the shell they are the shell's.
    expect(send('settings.open')).toBe(false);
    expect(send('sidebar.toggle')).toBe(false);
    expect(send('no.such.command')).toBe(false);
    expect(ran).toEqual(['thread.jump.3', 'thread.next']);
    off();
  });
});

describe('Settings › Studio on the desktop', () => {
  it('mirrors the notification choices to Electron main', async () => {
    const bridge = fakeBridge();
    const { agentd } = desktopFake();
    const page = render(
      wrap(<SettingsPage agentd={studioAgentd(agentd, undefined, bridge)} navigate={vi.fn()} />),
    );
    await waitFor(() =>
      expect(bridge.setNotifications).toHaveBeenLastCalledWith({ turnDone: true, approval: true }),
    );
    fireEvent.click(page.getByLabelText('When an approval is waiting'));
    await waitFor(() =>
      expect(bridge.setNotifications).toHaveBeenLastCalledWith({ turnDone: true, approval: false }),
    );
    expect(JSON.parse(localStorage.getItem(SETTINGS_KEY) ?? '{}')).toMatchObject({
      notifyApprovalWaiting: false,
    });
  });

  it('chooses an editor scheme, or a program through the OS picker', async () => {
    const bridge = fakeBridge();
    const { agentd } = desktopFake();
    const page = render(
      wrap(<SettingsPage agentd={studioAgentd(agentd, undefined, bridge)} navigate={vi.fn()} />),
    );
    const select = (await page.findByLabelText('Open threads in')) as HTMLSelectElement;
    expect(select.value).toBe('vscode');
    fireEvent.change(select, { target: { value: 'zed' } });
    await waitFor(() => expect(bridge.setEditor).toHaveBeenCalledWith('zed'));
    fireEvent.change(select, { target: { value: 'command' } });
    await waitFor(() => expect(bridge.chooseEditorCommand).toHaveBeenCalledOnce());
    await page.findByText("Runs subl with the thread's folder.");
  });

  it('has no editor card on the web', () => {
    const { agentd } = desktopFake();
    const page = render(wrap(<SettingsPage agentd={agentd} navigate={vi.fn()} />));
    expect(page.queryByText('Editor')).toBeNull();
  });
});
