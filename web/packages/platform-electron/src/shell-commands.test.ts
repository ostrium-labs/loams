import { ModeController, SHELL_COMMAND_EVENT } from '@loams/console-host';
import { describe, expect, it, vi } from 'vite-plus/test';
import type { ShellCommand } from './index.js';
import { wireShellCommands } from './index.js';

describe('menu commands reach the shell (D1084)', () => {
  it('forwards each shell command as SHELL_COMMAND_EVENT', () => {
    let send: (c: ShellCommand) => void = () => {};
    const api = {
      shell: {
        onCommand: (cb: (c: ShellCommand) => void) => {
          send = cb;
          return () => {};
        },
      },
    } as never;
    const navigate = vi.fn();
    const mode = new ModeController({
      current: 'console',
      available: ['console'],
      modes: new Map(),
      navigate,
      enter: vi.fn(),
    } as never);
    const target = new EventTarget();
    const seen: unknown[] = [];
    target.addEventListener(SHELL_COMMAND_EVENT, (e) => seen.push((e as CustomEvent).detail));
    wireShellCommands(api, mode, target);
    const commands: ShellCommand[] = [
      'palette.toggle',
      'sidebar.toggle',
      'inspector.toggle',
      'drawer.toggle',
      'navigation.back',
      'navigation.forward',
      'settings.open',
      'thread.next',
      'thread.reopen',
    ];
    for (const c of commands) send(c);
    expect(seen).toEqual(commands);
    // They never switch the mode or navigate on their own.
    expect(mode.current).toBe('console');
    expect(navigate).not.toHaveBeenCalled();
  });
});
