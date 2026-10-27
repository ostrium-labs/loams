// D1081's table, and Review Focus 1: shortcuts typed into editable targets. Studio's rich-text
// composer keeps ⌘B/⌘I (bold, italic); a terminal keeps Ctrl+K/J/B/N/I on Linux and Windows
// (kill-line, newline, tmux's prefix, next history, Tab); Esc in an input is the input's.

import { SHELL_KEYBINDINGS } from '@loams/studio-ui';
import { describe, expect, it } from 'vite-plus/test';
import { keyLabel, matchShellKey, SHELL_KEYMAP, type ShellKeyContext } from '../src/keys.js';

const mac: ShellKeyContext = { mac: true, terminal: false, richText: false, editable: false };
const linux: ShellKeyContext = { ...mac, mac: false };
const key = (k: string, mods: Partial<KeyboardEvent> = {}) =>
  ({
    key: k,
    code: '',
    metaKey: false,
    ctrlKey: false,
    shiftKey: false,
    altKey: false,
    ...mods,
  }) as KeyboardEvent;

describe('the keymap (D1081)', () => {
  it.each([
    [key('k', { metaKey: true }), 'palette.toggle'],
    [key('P', { metaKey: true, shiftKey: true }), 'palette.toggle'],
    [key('n', { metaKey: true }), 'thread.new'],
    [key('b', { metaKey: true }), 'sidebar.toggle'],
    [key('i', { metaKey: true }), 'inspector.toggle'],
    [key('j', { metaKey: true }), 'drawer.toggle'],
    [key('3', { metaKey: true }), 'thread.jump.3'],
    [key('Tab', { ctrlKey: true }), 'thread.next'],
    [key('[', { metaKey: true }), 'navigation.back'],
    [key(']', { metaKey: true }), 'navigation.forward'],
    [key('T', { metaKey: true, shiftKey: true }), 'thread.reopen'],
    [key(',', { metaKey: true }), 'settings.open'],
    [key('Escape'), 'overlay.close'],
  ] as const)('on macOS %o is %s', (e, command) => {
    expect(matchShellKey(e, mac)).toBe(command);
  });

  it('is Ctrl on Windows and Linux, and ⌘ does nothing there', () => {
    expect(matchShellKey(key('k', { ctrlKey: true }), linux)).toBe('palette.toggle');
    expect(matchShellKey(key('k', { metaKey: true }), linux)).toBeUndefined();
    expect(matchShellKey(key('k', { ctrlKey: true }), mac)).toBeUndefined();
    expect(keyLabel('mod+shift+p', true)).toEqual(['⌘', '⇧', 'P']);
    expect(keyLabel('mod+shift+p', false)).toEqual(['Ctrl', 'Shift', 'P']);
  });

  it('reads the physical key when the layout types another character', () => {
    // A Cyrillic layout types "л" on the K key; the code still names it.
    expect(matchShellKey(key('л', { code: 'KeyK', ctrlKey: true }), linux)).toBe('palette.toggle');
    // AZERTY types "è" on the 7 key without Shift; the digit row's code is the key.
    expect(matchShellKey(key('è', { code: 'Digit7', metaKey: true }), mac)).toBe('thread.jump.7');
    // Dvorak types "t" on the physical K key: the layout's letter wins, so it is not the palette.
    expect(matchShellKey(key('t', { code: 'KeyK', ctrlKey: true }), linux)).toBeUndefined();
  });

  it('every binding is in one table, and Studio lists them for its terminal and settings', () => {
    expect(SHELL_KEYMAP.map((b) => b.key)).toEqual(
      expect.arrayContaining([
        'mod+k',
        'mod+shift+p',
        'mod+n',
        'mod+b',
        'mod+i',
        'mod+j',
        'ctrl+tab',
        'mod+[',
        'mod+]',
        'mod+shift+t',
        'mod+,',
        'escape',
      ]),
    );
    const listed = new Map(SHELL_KEYBINDINGS.map((b) => [b.key, b]));
    for (const b of SHELL_KEYMAP)
      if (b.key !== 'escape') expect(listed.has(b.key), b.key).toBe(true);
  });

  it('Studio’s terminal passes a chord through on Linux exactly when the shell takes it there', () => {
    const term = { ...linux, terminal: true, editable: true };
    for (const b of SHELL_KEYMAP) {
      if (b.key === 'escape') continue;
      const shellTakes = b.when ? b.when(term) : true;
      expect(SHELL_KEYBINDINGS.find((l) => l.key === b.key)?.terminal ?? false, b.key).toBe(
        shellTakes,
      );
    }
  });
});

describe('editable targets keep their keys (Review Focus 1)', () => {
  it('the rich-text composer keeps ⌘B and ⌘I; the palette still opens', () => {
    const composer = { ...mac, richText: true, editable: true };
    expect(matchShellKey(key('b', { metaKey: true }), composer)).toBeUndefined();
    expect(matchShellKey(key('i', { metaKey: true }), composer)).toBeUndefined();
    expect(matchShellKey(key('k', { metaKey: true }), composer)).toBe('palette.toggle');
  });

  it('a terminal on Linux keeps Ctrl+K, J, B, N and I; Ctrl+Shift+P still opens the palette', () => {
    const term = { ...linux, terminal: true, editable: true };
    for (const k of ['k', 'j', 'b', 'n', 'i'])
      expect(matchShellKey(key(k, { ctrlKey: true }), term), k).toBeUndefined();
    expect(matchShellKey(key('P', { ctrlKey: true, shiftKey: true }), term)).toBe('palette.toggle');
  });

  it('a macOS terminal still gets ⌘ shortcuts (⌘ never reaches the shell program)', () => {
    expect(
      matchShellKey(key('j', { metaKey: true }), { ...mac, terminal: true, editable: true }),
    ).toBe('drawer.toggle');
  });

  it('Escape in an input is the input’s, and ⌘[ in a text field is not Back', () => {
    const field = { ...mac, editable: true };
    expect(matchShellKey(key('Escape'), field)).toBeUndefined();
    expect(matchShellKey(key('[', { metaKey: true }), field)).toBeUndefined();
  });

  it('a key with an extra modifier is not the binding', () => {
    expect(matchShellKey(key('k', { metaKey: true, altKey: true }), mac)).toBeUndefined();
    expect(matchShellKey(key('n', { metaKey: true, shiftKey: true }), mac)).toBeUndefined();
  });
});
