// The shell's one keydown listener (D1081): it resolves the event against SHELL_KEYMAP and
// runs the command. Studio's commands travel as SHELL_COMMAND_EVENT (thread.new → chat.new,
// thread.jump.N, thread.next); the shell keeps ⌘⇧T's history itself (W12). ⌘⇧M stays
// `useModeShortcut`'s (D900). The desktop's menu items arrive as SHELL_COMMAND_EVENT and go
// through the same `run` (CF1 Task 14, D1084).

import { MODE_HOME, type PanelService, SHELL_COMMAND_EVENT } from '@loams/console-host';
import { useEffect, useRef } from 'react';
import { runShellCommand } from './frame.js';
import {
  isMacPlatform,
  matchShellKey,
  type ShellKeyCommand,
  type ShellKeyContext,
} from './keys.js';
import type { HashRouter } from './router.js';

export interface ShellKeysOptions {
  router: HashRouter;
  inspector: PanelService;
  drawer: PanelService;
  toggleSidebar(): void;
  togglePalette(): void;
  openSettings(): void;
  /** Closes the shell's own topmost overlay (the narrow sidebar sheet); false if none was open. */
  closeOverlay(): boolean;
}

const EDITABLE =
  'input, textarea, select, [contenteditable]:not([contenteditable="false"]), .xterm';
const RICH_TEXT = '[contenteditable]:not([contenteditable="false"])';

/** Where a key press happened: a terminal, rich text or another editable target. */
export function contextOf(target: EventTarget | null): Omit<ShellKeyContext, 'mac'> {
  const el = target instanceof Element ? target : null;
  const composer = el?.closest('[data-composer-rich-text]');
  return {
    terminal: Boolean(el?.closest('[data-terminal], .xterm')),
    // Studio's composer says whether it is rich text; any other contenteditable is.
    richText: composer
      ? composer.getAttribute('data-composer-rich-text') === 'true'
      : Boolean(el?.closest(RICH_TEXT)),
    editable: Boolean(el?.closest(EDITABLE)),
  };
}

function inPopup(target: EventTarget | null): boolean {
  return target instanceof Element && target.closest('[role="menu"], [role="listbox"]') !== null;
}

const THREAD = /^\/studio\/t\/[^/]+/;

/** How many left threads ⌘⇧T remembers. */
const REOPEN_DEPTH = 20;

export function useShellKeys(o: ShellKeysOptions): void {
  const latest = useRef(o);
  latest.current = o;
  const left = useRef<string[]>([]);

  // ⌘⇧T: remember thread routes as they are left (W12).
  useEffect(() => {
    let previous = o.router.current().path;
    return o.router.subscribe(() => {
      const now = o.router.current().path;
      if (THREAD.test(previous) && now !== previous) {
        left.current = [...left.current.filter((p) => p !== previous), previous].slice(
          -REOPEN_DEPTH,
        );
      }
      previous = now;
    });
  }, [o.router]);

  /** Runs a command; false leaves it to the page. Stable: it reads `latest` and `left`. */
  const run = useRef((command: ShellKeyCommand): boolean => {
    const x = latest.current;
    switch (command) {
      case 'palette.toggle':
        x.togglePalette();
        return true;
      case 'thread.new':
        runShellCommand('chat.new', () => x.router.navigate(MODE_HOME.studio));
        return true;
      case 'sidebar.toggle':
        x.toggleSidebar();
        return true;
      case 'inspector.toggle':
        x.inspector.toggle();
        return true;
      case 'drawer.toggle':
        x.drawer.toggle();
        return true;
      case 'navigation.back':
        globalThis.history.back();
        return true;
      case 'navigation.forward':
        globalThis.history.forward();
        return true;
      case 'thread.reopen': {
        const here = x.router.current().path;
        let last = left.current.pop();
        while (last === here) last = left.current.pop();
        if (last) x.router.navigate(last);
        return true;
      }
      case 'settings.open':
        x.openSettings();
        return true;
      case 'overlay.close':
        return x.closeOverlay();
      default:
        // thread.next and thread.jump.N are Studio's; nothing happens where it is not loaded.
        runShellCommand(command, () => {});
        return true;
    }
  }).current;

  useEffect(() => {
    const mac = isMacPlatform();
    const onKey = (e: KeyboardEvent) => {
      if (e.defaultPrevented || e.isComposing) return;
      const command = matchShellKey(e, { mac, ...contextOf(e.target) });
      if (!command) return;
      // A menu or list box closes itself on Esc; the sheet under it stays open.
      if (command === 'overlay.close' && inPopup(e.target)) return;
      if (run(command)) e.preventDefault();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [run]);

  // The desktop menu (D1084): its View and Window items name the same commands. Studio's
  // (`chat.new`, `thread.next`, `thread.jump.N`) are Studio's listener's, and `run` would
  // send them back out as SHELL_COMMAND_EVENT, so only the shell's own are taken here.
  useEffect(() => {
    const onCommand = (event: Event) => {
      if (event.defaultPrevented) return;
      const detail = (event as CustomEvent<unknown>).detail;
      if (typeof detail !== 'string' || !MENU_COMMANDS.has(detail)) return;
      if (run(detail as ShellKeyCommand)) event.preventDefault();
    };
    window.addEventListener(SHELL_COMMAND_EVENT, onCommand);
    return () => window.removeEventListener(SHELL_COMMAND_EVENT, onCommand);
  }, [run]);
}

/** The menu commands the shell runs itself (`ShellCommand` in the desktop's contracts). */
const MENU_COMMANDS = new Set<string>([
  'palette.toggle',
  'sidebar.toggle',
  'inspector.toggle',
  'drawer.toggle',
  'navigation.back',
  'navigation.forward',
  'settings.open',
  'thread.reopen',
] satisfies ShellKeyCommand[]);
