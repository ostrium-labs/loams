// Studio's keybindings (SD1 Task 24; inventory (h)): T3 Code's model with `when`
// scopes, rebound to Studio's commands, with conflicts flagged in the editor's rows
// and the per-device store under `loams.studio.keybindings`.

import { describe, expect, it } from 'vite-plus/test';
import {
  buildKeybindingRows,
  commandLabel,
  compileResolvedKeybindingsConfig,
  DEFAULT_KEYBINDINGS,
  DEFAULT_RESOLVED_KEYBINDINGS,
  KEYBINDINGS_KEY,
  KeybindingStore,
  keybindingFromKeyboardEvent,
  mergeWithDefaultKeybindings,
  parseKeybindingWhenExpression,
  readCustomKeybindings,
  resolveShortcutCommand,
  type ShortcutEventLike,
  shortcutKeys,
  shortcutLabelForCommand,
} from '../src';

const LINUX = 'Linux x86_64';
const MAC = 'MacIntel';

function key(k: string, mods: Partial<ShortcutEventLike> = {}): ShortcutEventLike {
  return {
    key: k,
    code: /^[a-z]$/i.test(k) ? `Key${k.toUpperCase()}` : undefined,
    metaKey: false,
    ctrlKey: false,
    shiftKey: false,
    altKey: false,
    ...mods,
  };
}

describe('keybinding_when_scopes', () => {
  const defaults = DEFAULT_RESOLVED_KEYBINDINGS;

  it('runs a binding only where its when clause holds', () => {
    const ctrlD = key('d', { ctrlKey: true });
    expect(
      resolveShortcutCommand(ctrlD, defaults, { platform: LINUX, context: { threadOpen: true } }),
    ).toBe('diff.toggle');
    expect(
      resolveShortcutCommand(ctrlD, defaults, {
        platform: LINUX,
        context: { threadOpen: true, terminalFocus: true },
      }),
    ).toBeNull();
    expect(
      resolveShortcutCommand(ctrlD, defaults, { platform: LINUX, context: { threadOpen: false } }),
    ).toBeNull();
  });

  it('maps mod to Cmd on macOS and to Ctrl elsewhere', () => {
    const opts = { platform: MAC, context: { threadOpen: true } };
    expect(resolveShortcutCommand(key('d', { metaKey: true }), defaults, opts)).toBe('diff.toggle');
    expect(resolveShortcutCommand(key('d', { ctrlKey: true }), defaults, opts)).toBeNull();
    expect(shortcutLabelForCommand(defaults, 'diff.toggle', opts)).toBe('⌘D');
    expect(
      shortcutLabelForCommand(defaults, 'diff.toggle', {
        platform: LINUX,
        context: { threadOpen: true },
      }),
    ).toBe('Ctrl+D');
  });

  it('parses when expressions with !, &&, || and parentheses, and refuses broken ones', () => {
    expect(parseKeybindingWhenExpression('a && (b || !c)')).toEqual({
      type: 'and',
      left: { type: 'identifier', name: 'a' },
      right: {
        type: 'or',
        left: { type: 'identifier', name: 'b' },
        right: { type: 'not', node: { type: 'identifier', name: 'c' } },
      },
    });
    expect(parseKeybindingWhenExpression('a &&')).toBeNull();
    expect(parseKeybindingWhenExpression('(a')).toBeNull();
    // A rule whose key or when clause does not parse is dropped, not half-applied.
    expect(
      compileResolvedKeybindingsConfig([
        { key: 'mod+d', command: 'diff.toggle', when: 'threadOpen &&' },
        { key: 'mod+d+e', command: 'chat.new' },
        { key: 'mod+', command: 'chat.new' },
      ]).map((r) => r.shortcut.key),
    ).toEqual(['+']);
  });

  it('lets a later rule win, and a custom rule replace the defaults of its command', () => {
    const merged = mergeWithDefaultKeybindings(
      compileResolvedKeybindingsConfig([{ key: 'mod+d', command: 'chat.new' }]),
    );
    const opts = { platform: LINUX, context: { threadOpen: true } };
    expect(resolveShortcutCommand(key('d', { ctrlKey: true }), merged, opts)).toBe('chat.new');
    // chat.new's default Mod+Shift+O is gone with the override.
    expect(
      resolveShortcutCommand(key('o', { ctrlKey: true, shiftKey: true }), merged, opts),
    ).toBeNull();
  });

  it("stays clear of the shell's own keys (D1081)", () => {
    for (const reserved of ['k', 'j', 'b', 'i', 'n', ',', '[', ']', '1', '9']) {
      expect(
        resolveShortcutCommand(key(reserved, { ctrlKey: true }), DEFAULT_RESOLVED_KEYBINDINGS, {
          platform: LINUX,
          context: { threadOpen: true },
        }),
      ).toBeNull();
    }
    expect(
      resolveShortcutCommand(
        key('m', { ctrlKey: true, shiftKey: true }),
        DEFAULT_RESOLVED_KEYBINDINGS,
        { platform: LINUX, context: { threadOpen: true } },
      ),
    ).toBeNull();
  });

  it("labels Studio's commands with the shell's chords, a custom rule first", () => {
    const store = new KeybindingStore(null);
    const opts = { platform: LINUX, context: { isDesktop: true } };
    expect(shortcutLabelForCommand(store.shown(), 'chat.new', opts)).toBe('Ctrl+N');
    expect(shortcutLabelForCommand(store.shown(), 'thread.jump.3', opts)).toBe('Ctrl+3');
    expect(shortcutLabelForCommand(store.shown(), 'thread.next', opts)).toBe('Ctrl+Tab');
    // A key press never resolves against the labels: the shell runs those chords.
    expect(resolveShortcutCommand(key('n', { ctrlKey: true }), store.resolved(), opts)).toBeNull();
    store.setCommandRules('chat.new', [{ key: 'mod+shift+y' }]);
    expect(shortcutLabelForCommand(store.shown(), 'chat.new', opts)).toBe('Ctrl+Shift+Y');
  });

  it('gives the palette its keys as a chord', () => {
    const [shortcut] = DEFAULT_RESOLVED_KEYBINDINGS.filter((r) => r.command === 'diff.toggle');
    expect(shortcut).toBeDefined();
    if (!shortcut) return;
    expect(shortcutKeys(shortcut.shortcut, LINUX)).toEqual(['Ctrl', 'D']);
    expect(shortcutKeys(shortcut.shortcut, MAC)).toEqual(['⌘', 'D']);
  });
});

describe('keybinding_conflict_detected', () => {
  it('has no conflicts among the defaults', () => {
    const rows = buildKeybindingRows(DEFAULT_RESOLVED_KEYBINDINGS, '');
    expect(rows.length).toBe(DEFAULT_KEYBINDINGS.length);
    expect(rows.filter((r) => r.conflicts.length > 0)).toEqual([]);
  });

  it('flags two commands on one key whose when clauses can both hold', () => {
    const merged = mergeWithDefaultKeybindings(
      compileResolvedKeybindingsConfig([{ key: 'mod+d', command: 'chat.new' }]),
    );
    const rows = buildKeybindingRows(merged, '');
    const chat = rows.find((r) => r.command === 'chat.new');
    const diff = rows.find((r) => r.command === 'diff.toggle');
    expect(chat?.source).toBe('Custom');
    expect(chat?.conflicts).toEqual([commandLabel('diff.toggle')]);
    expect(diff?.conflicts).toEqual([commandLabel('chat.new')]);
  });

  it('does not flag one key under two when clauses that are spelled apart', () => {
    const merged = mergeWithDefaultKeybindings(
      compileResolvedKeybindingsConfig([
        { key: 'mod+d', command: 'chat.new', when: 'terminalFocus' },
      ]),
    );
    expect(
      buildKeybindingRows(merged, '').find((r) => r.command === 'chat.new')?.conflicts,
    ).toEqual([]);
  });

  it("flags a binding that takes one of the shell's keys", () => {
    for (const [k, label] of [
      ['mod+k', 'Shell: Command Palette'],
      ['mod+shift+m', 'Shell: Switch Mode'],
      ['mod+j', 'Shell: Toggle Drawer'],
      ['mod+b', 'Shell: Toggle Sidebar'],
      ['ctrl+tab', 'Shell: Next Thread'],
    ] as const) {
      const merged = mergeWithDefaultKeybindings(
        compileResolvedKeybindingsConfig([{ key: k, command: 'settings.open' }]),
      );
      const row = buildKeybindingRows(merged, '').find((r) => r.command === 'settings.open');
      expect(row?.conflicts).toEqual([label]);
    }
  });

  it('captures a pressed chord as a binding', () => {
    expect(
      keybindingFromKeyboardEvent(
        { key: 'K', code: 'KeyK', ctrlKey: true, shiftKey: true, metaKey: false, altKey: false },
        LINUX,
      ),
    ).toBe('mod+shift+k');
    expect(
      keybindingFromKeyboardEvent(
        { key: 'k', code: 'KeyK', metaKey: true, ctrlKey: false, shiftKey: false, altKey: false },
        MAC,
      ),
    ).toBe('mod+k');
    expect(
      keybindingFromKeyboardEvent(
        {
          key: 'Shift',
          code: 'ShiftLeft',
          shiftKey: true,
          ctrlKey: false,
          metaKey: false,
          altKey: false,
        },
        LINUX,
      ),
    ).toBeNull();
  });
});

function memoryStorage(seed: Record<string, string> = {}) {
  const data = new Map(Object.entries(seed));
  return {
    data,
    getItem: (k: string) => data.get(k) ?? null,
    setItem: (k: string, v: string) => {
      data.set(k, v);
    },
  };
}

const throwing = {
  getItem: (): string | null => {
    throw new Error('SecurityError');
  },
  setItem: () => {
    throw new Error('QuotaExceededError');
  },
};

describe('keybindings_storage_failure_tolerated', () => {
  it('reads the defaults when storage throws', () => {
    expect(readCustomKeybindings(throwing)).toEqual([]);
    const store = new KeybindingStore(throwing);
    expect(store.resolved()).toEqual(DEFAULT_RESOLVED_KEYBINDINGS);
  });

  it('keeps a change for this page when storage refuses to write it', () => {
    const store = new KeybindingStore(throwing);
    let calls = 0;
    store.subscribe(() => calls++);
    expect(() => store.setCommandRules('chat.new', [{ key: 'mod+shift+y' }])).not.toThrow();
    expect(calls).toBe(1);
    expect(store.rules()).toEqual([{ key: 'mod+shift+y', command: 'chat.new' }]);
    expect(
      resolveShortcutCommand(key('y', { ctrlKey: true, shiftKey: true }), store.resolved(), {
        platform: LINUX,
      }),
    ).toBe('chat.new');
  });

  it('drops malformed, unknown and unparseable rows, and survives bad JSON', () => {
    expect(readCustomKeybindings(memoryStorage({ [KEYBINDINGS_KEY]: '{nope' }))).toEqual([]);
    expect(readCustomKeybindings(memoryStorage({ [KEYBINDINGS_KEY]: '{"a":1}' }))).toEqual([]);
    const stored = JSON.stringify([
      { key: 'mod+shift+y', command: 'chat.new' },
      { key: 'mod+shift+u', command: 'no.such.command' },
      { key: 'mod+d+e', command: 'diff.toggle' },
      { key: 'mod+e', command: 'files.toggle', when: '((' },
      { key: 'mod+e', command: 'files.toggle', when: 42 },
      'mod+x',
      null,
    ]);
    expect(readCustomKeybindings(memoryStorage({ [KEYBINDINGS_KEY]: stored }))).toEqual([
      { key: 'mod+shift+y', command: 'chat.new' },
    ]);
  });

  it('persists to loams.studio.keybindings, and resets a command to its defaults', () => {
    const storage = memoryStorage();
    const store = new KeybindingStore(storage);
    store.setCommandRules('diff.toggle', [{ key: 'mod+shift+d', when: 'threadOpen' }]);
    expect(JSON.parse(storage.data.get(KEYBINDINGS_KEY) ?? '[]')).toEqual([
      { key: 'mod+shift+d', command: 'diff.toggle', when: 'threadOpen' },
    ]);
    expect(new KeybindingStore(storage).rules()).toEqual(store.rules());
    store.resetCommand('diff.toggle');
    expect(store.rules()).toEqual([]);
    expect(store.resolved()).toEqual(DEFAULT_RESOLVED_KEYBINDINGS);
  });
});
