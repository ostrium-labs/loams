// The console's keyboard (design 59, D1081): one table, here. ⌘ is Ctrl on Windows and Linux;
// ⌃Tab is Control everywhere. Part 2 and later parts add rows to SHELL_KEYMAP; a user-editable
// keymap is Q930. `when` keeps a key with the focused editor or terminal (Review Focus 1).
// Studio's `SHELL_KEYBINDINGS` (@loams/studio-ui) lists the same chords for its terminal and
// its keybinding editor; `test/keymap.test.ts` keeps the two in step.

export type ShellKeyCommand =
  | 'palette.toggle'
  | 'thread.new'
  | 'sidebar.toggle'
  | 'inspector.toggle'
  | 'drawer.toggle'
  | `thread.jump.${1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9}`
  | 'thread.next'
  | 'navigation.back'
  | 'navigation.forward'
  | 'thread.reopen'
  | 'settings.open'
  | 'overlay.close';

export interface ShellKeyContext {
  mac: boolean;
  /** Focus is in a terminal (`[data-terminal]`). */
  terminal: boolean;
  /** Focus is in a rich-text editor (`[data-composer-rich-text="true"]`, contenteditable). */
  richText: boolean;
  /** Focus is in any text input, textarea, select or contenteditable. */
  editable: boolean;
}

export interface ShellKeyBinding {
  key: string;
  command: ShellKeyCommand;
  label: string;
  /** False keeps the key with the focused element. */
  when?: (c: ShellKeyContext) => boolean;
}

/** On Linux and Windows a terminal owns Ctrl+letter; on macOS ⌘ never reaches it. */
const notTerminal = (c: ShellKeyContext) => c.mac || !c.terminal;
const notRichText = (c: ShellKeyContext) => !c.richText;
const notEditable = (c: ShellKeyContext) => !c.editable;

const JUMPS = [1, 2, 3, 4, 5, 6, 7, 8, 9] as const;

export const SHELL_KEYMAP: readonly ShellKeyBinding[] = [
  { key: 'mod+k', command: 'palette.toggle', label: 'Command palette', when: notTerminal },
  { key: 'mod+shift+p', command: 'palette.toggle', label: 'Command palette' },
  { key: 'mod+n', command: 'thread.new', label: 'New thread', when: notTerminal },
  {
    key: 'mod+b',
    command: 'sidebar.toggle',
    label: 'Toggle sidebar',
    when: (c) => notTerminal(c) && notRichText(c),
  },
  {
    key: 'mod+i',
    command: 'inspector.toggle',
    label: 'Toggle inspector',
    when: (c) => notTerminal(c) && notRichText(c),
  },
  { key: 'mod+j', command: 'drawer.toggle', label: 'Toggle drawer', when: notTerminal },
  ...JUMPS.map((n): ShellKeyBinding => ({
    key: `mod+${n}`,
    command: `thread.jump.${n}`,
    label: `Thread ${n}`,
    when: notTerminal,
  })),
  { key: 'ctrl+tab', command: 'thread.next', label: 'Next thread' },
  {
    key: 'mod+[',
    command: 'navigation.back',
    label: 'Back',
    when: (c) => notTerminal(c) && notEditable(c),
  },
  {
    key: 'mod+]',
    command: 'navigation.forward',
    label: 'Forward',
    when: (c) => notTerminal(c) && notEditable(c),
  },
  {
    key: 'mod+shift+t',
    command: 'thread.reopen',
    label: 'Reopen closed thread',
    when: notTerminal,
  },
  { key: 'mod+,', command: 'settings.open', label: 'Settings' },
  {
    key: 'escape',
    command: 'overlay.close',
    label: 'Close the topmost overlay',
    when: notEditable,
  },
];

interface Chord {
  key: string;
  mod: boolean;
  ctrl: boolean;
  shift: boolean;
  alt: boolean;
}

function parse(spec: string): Chord {
  const parts = spec.split('+');
  const key = parts.at(-1) ?? '';
  return {
    key,
    mod: parts.includes('mod'),
    ctrl: parts.includes('ctrl'),
    shift: parts.includes('shift'),
    alt: parts.includes('alt'),
  };
}

const CHORDS = SHELL_KEYMAP.map((b) => [b, parse(b.key)] as const);

/**
 * The keys a press may stand for: the layout's character, and, where the layout types no
 * Latin letter or digit there (Cyrillic, AZERTY's digit row), the physical key's. A Latin
 * layout's own letter wins, so Dvorak's ⌘K is the key marked K (as T3 Code resolves it).
 */
function pressedKeys(e: Pick<KeyboardEvent, 'key' | 'code'>): Set<string> {
  const key = e.key.toLowerCase();
  const keys = new Set([key]);
  const letter = e.code?.match(/^Key([A-Z])$/)?.[1];
  if (letter && !/^[a-z]$/.test(key)) keys.add(letter.toLowerCase());
  const digit = e.code?.match(/^Digit([0-9])$/)?.[1];
  if (digit && !/^[0-9]$/.test(key)) keys.add(digit);
  return keys;
}

export function matchShellKey(
  e: Pick<KeyboardEvent, 'key' | 'code' | 'metaKey' | 'ctrlKey' | 'shiftKey' | 'altKey'>,
  ctx: ShellKeyContext,
): ShellKeyCommand | undefined {
  const pressed = pressedKeys(e);
  for (const [binding, c] of CHORDS) {
    const wantMeta = c.mod && ctx.mac;
    const wantCtrl = c.ctrl || (c.mod && !ctx.mac);
    if (
      e.metaKey !== wantMeta ||
      e.ctrlKey !== wantCtrl ||
      e.shiftKey !== c.shift ||
      e.altKey !== c.alt
    )
      continue;
    if (!pressed.has(c.key)) continue;
    // The table has no duplicate chords, so a failed `when` leaves the key to the target.
    if (binding.when && !binding.when(ctx)) return undefined;
    return binding.command;
  }
  return undefined;
}

/** A chord as key caps: `mod+shift+p` is ⌘ ⇧ P on macOS and Ctrl Shift P elsewhere. */
export function keyLabel(spec: string, mac: boolean): string[] {
  return spec.split('+').map((p) => {
    if (p === 'mod') return mac ? '⌘' : 'Ctrl';
    if (p === 'ctrl') return mac ? '⌃' : 'Ctrl';
    if (p === 'shift') return mac ? '⇧' : 'Shift';
    if (p === 'alt') return mac ? '⌥' : 'Alt';
    if (p === 'escape') return 'Esc';
    if (p === 'tab') return 'Tab';
    return p.length === 1 ? p.toUpperCase() : p;
  });
}

/** Whether this device spells Mod as ⌘. */
export function isMacPlatform(): boolean {
  try {
    return /mac|iphone|ipad|ipod/i.test(globalThis.navigator?.platform ?? '');
  } catch {
    // No navigator (tests, workers): the Ctrl spelling.
    return false;
  }
}
