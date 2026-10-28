// The shell's one listener (D1081): each command reaches its owner, Studio's commands travel
// as SHELL_COMMAND_EVENT, ⌘⇧T reopens the last thread left (W12), and a key the focused
// editor keeps is left alone (Review Focus 1). jsdom is not a Mac, so Mod is Ctrl here.

import { SHELL_COMMAND_EVENT } from '@loams/console-host';
import { SlotRegistry } from '@loams/slots';
import { act, cleanup, fireEvent, render } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vite-plus/test';
import { contextOf, type ShellKeysOptions, useShellKeys } from '../src/keyboard.js';
import { DRAWER, INSPECTOR, PanelController } from '../src/panel.js';
import { HashRouter, type HashSource } from '../src/router.js';

function memory(start: string): HashSource {
  let path = start;
  return {
    get: () => path,
    set: (p) => {
      path = p;
    },
    listen: () => () => {},
  };
}

/** Storage of its own, so one test's open panel does not open the next one's. */
function blank(): Pick<Storage, 'getItem' | 'setItem'> {
  const m = new Map<string, string>();
  return { getItem: (k) => m.get(k) ?? null, setItem: (k, v) => void m.set(k, v) };
}

function Keys(props: ShellKeysOptions) {
  useShellKeys(props);
  return null;
}

function setup(start = '/data') {
  const router = new HashRouter(new SlotRegistry(), memory(start));
  const o = {
    router,
    inspector: new PanelController(INSPECTOR, blank()),
    drawer: new PanelController(DRAWER, blank()),
    toggleSidebar: vi.fn(),
    togglePalette: vi.fn(),
    openSettings: vi.fn(),
    closeOverlay: vi.fn(() => false),
  } satisfies ShellKeysOptions;
  render(<Keys {...o} />);
  const sent: unknown[] = [];
  const onCommand = (e: Event) => sent.push((e as CustomEvent).detail);
  window.addEventListener(SHELL_COMMAND_EVENT, onCommand);
  cleanups.push(() => window.removeEventListener(SHELL_COMMAND_EVENT, onCommand));
  return { ...o, sent };
}

const cleanups: (() => void)[] = [];
afterEach(() => {
  cleanup();
  for (const c of cleanups.splice(0)) c();
  document.body.innerHTML = '';
});

describe('useShellKeys (D1081)', () => {
  it('runs the shell’s own commands', () => {
    const o = setup();
    fireEvent.keyDown(window, { key: 'k', ctrlKey: true });
    fireEvent.keyDown(window, { key: 'P', ctrlKey: true, shiftKey: true });
    expect(o.togglePalette).toHaveBeenCalledTimes(2);
    fireEvent.keyDown(window, { key: 'b', ctrlKey: true });
    expect(o.toggleSidebar).toHaveBeenCalledOnce();
    fireEvent.keyDown(window, { key: 'i', ctrlKey: true });
    fireEvent.keyDown(window, { key: 'j', ctrlKey: true });
    expect(o.inspector.state().open).toBe(true);
    expect(o.drawer.state().open).toBe(true);
    fireEvent.keyDown(window, { key: ',', ctrlKey: true });
    expect(o.openSettings).toHaveBeenCalledOnce();
  });

  it('sends Studio its commands, and opens Studio’s home when nothing takes a new thread', () => {
    const o = setup();
    fireEvent.keyDown(window, { key: '4', ctrlKey: true });
    fireEvent.keyDown(window, { key: 'Tab', ctrlKey: true });
    fireEvent.keyDown(window, { key: 'n', ctrlKey: true });
    expect(o.sent).toEqual(['thread.jump.4', 'thread.next', 'chat.new']);
    expect(o.router.current().path).toBe('/studio');
  });

  it('a new thread stays where Studio took it', () => {
    const o = setup();
    const take = (e: Event) => e.preventDefault();
    window.addEventListener(SHELL_COMMAND_EVENT, take);
    cleanups.push(() => window.removeEventListener(SHELL_COMMAND_EVENT, take));
    fireEvent.keyDown(window, { key: 'n', ctrlKey: true });
    expect(o.router.current().path).toBe('/data');
  });

  it('⌘⇧T reopens the threads left, most recent first (W12)', () => {
    const o = setup('/studio/t/a');
    act(() => o.router.navigate('/studio/t/b'));
    act(() => o.router.navigate('/data'));
    fireEvent.keyDown(window, { key: 'T', ctrlKey: true, shiftKey: true });
    expect(o.router.current().path).toBe('/studio/t/b');
    fireEvent.keyDown(window, { key: 'T', ctrlKey: true, shiftKey: true });
    expect(o.router.current().path).toBe('/studio/t/a');
  });

  it('⌘⇧T does nothing before a thread was left', () => {
    const o = setup('/data');
    act(() => o.router.navigate('/git'));
    fireEvent.keyDown(window, { key: 'T', ctrlKey: true, shiftKey: true });
    expect(o.router.current().path).toBe('/git');
  });

  it('Esc closes the shell’s overlay only when one was open, and not from a menu', () => {
    const o = setup();
    const quiet = fireEvent.keyDown(window, { key: 'Escape' });
    expect(o.closeOverlay).toHaveBeenCalledOnce();
    expect(quiet).toBe(true); // nothing closed, so the key was not taken
    o.closeOverlay.mockReturnValue(true);
    expect(fireEvent.keyDown(window, { key: 'Escape' })).toBe(false);
    const menu = document.createElement('div');
    menu.setAttribute('role', 'menu');
    const item = document.createElement('button');
    menu.append(item);
    document.body.append(menu);
    fireEvent.keyDown(item, { key: 'Escape' });
    expect(o.closeOverlay).toHaveBeenCalledTimes(2);
  });

  it('leaves a key another handler took, and an IME composition', () => {
    const o = setup();
    const take = (e: KeyboardEvent) => e.preventDefault();
    document.addEventListener('keydown', take);
    fireEvent.keyDown(document.body, { key: 'k', ctrlKey: true });
    document.removeEventListener('keydown', take);
    fireEvent.keyDown(window, { key: 'k', ctrlKey: true, isComposing: true });
    expect(o.togglePalette).not.toHaveBeenCalled();
  });

  it('a terminal and rich text keep their keys (Review Focus 1)', () => {
    const o = setup();
    const term = document.createElement('div');
    term.setAttribute('data-terminal', '');
    const input = document.createElement('textarea');
    term.append(input);
    const rich = document.createElement('div');
    rich.setAttribute('contenteditable', 'true');
    document.body.append(term, rich);
    for (const k of ['k', 'j', 'b', 'n', 'i']) fireEvent.keyDown(input, { key: k, ctrlKey: true });
    fireEvent.keyDown(rich, { key: 'b', ctrlKey: true });
    fireEvent.keyDown(rich, { key: 'i', ctrlKey: true });
    expect(o.togglePalette).not.toHaveBeenCalled();
    expect(o.toggleSidebar).not.toHaveBeenCalled();
    expect(o.inspector.state().open).toBe(false);
    expect(o.drawer.state().open).toBe(false);
    expect(o.sent).toEqual([]);
    // Ctrl+Shift+P still opens the palette from the terminal.
    fireEvent.keyDown(input, { key: 'P', ctrlKey: true, shiftKey: true });
    expect(o.togglePalette).toHaveBeenCalledOnce();
  });
});

describe('contextOf', () => {
  it('reads Studio’s composer: plain text gives up ⌘B, rich text keeps it', () => {
    const composer = document.createElement('div');
    composer.setAttribute('contenteditable', 'true');
    composer.setAttribute('data-composer-rich-text', 'false');
    expect(contextOf(composer)).toEqual({ terminal: false, richText: false, editable: true });
    composer.setAttribute('data-composer-rich-text', 'true');
    expect(contextOf(composer).richText).toBe(true);
    expect(contextOf(null)).toEqual({ terminal: false, richText: false, editable: false });
  });
});
