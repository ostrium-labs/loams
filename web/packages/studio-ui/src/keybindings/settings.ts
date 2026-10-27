// Portions of this file are from T3 Code (https://github.com/pingdotgg/t3code),
// apps/web/src/components/settings/KeybindingsSettings.logic.ts at commit 6524a8d4, Copyright (c) 2026 T3 Tools Inc., MIT licence
// (see web/packages/studio-ui/LICENSE.t3code). Modified by The Loams Authors:
// the rows, conflicts, labels and key capture kept over Studio's commands; usage and project-script commands dropped; the shell's reserved keys join the conflict check. Loams' changes are Apache-2.0.

// The keybinding editor's logic (Settings › Studio, SD1 Task 24): one row per binding,
// its source (default or custom), and the commands it conflicts with. Two bindings
// conflict when they share a chord and their `when` clauses are the same or one is
// empty (upstream's rule). A binding on one of the shell's own chords always conflicts,
// because the shell answers those in both modes.

import {
  DEFAULT_RESOLVED_KEYBINDINGS,
  isMacPlatform,
  type KeybindingCommand,
  type KeybindingShortcut,
  type KeybindingWhenNode,
  parseKeybindingShortcut,
  parseKeybindingWhenExpression,
  type ResolvedKeybindingRule,
  type ResolvedKeybindingsConfig,
  STUDIO_KEYBINDING_COMMANDS,
  shortcutKeyFromEvent,
} from "./shared";

/** A chord the shell owns (D1081), with its editor label. */
export interface ShellKeybinding {
  readonly key: string;
  readonly label: string;
  /**
   * The shell takes it even from a terminal on Linux and Windows, where Ctrl+letter is the
   * shell program's (Review Focus 1). On macOS every ⌘ chord passes a terminal.
   */
  readonly terminal?: true;
}

/**
 * The chords the shell owns in both modes (design 59, D1081; @loams/plugin-shell
 * `SHELL_KEYMAP`, whose test keeps the two in step), with their editor labels.
 */
export const SHELL_KEYBINDINGS: ReadonlyArray<ShellKeybinding> = [
  { key: "mod+k", label: "Shell: Command Palette" },
  { key: "mod+shift+p", label: "Shell: Command Palette", terminal: true },
  { key: "mod+n", label: "Shell: New Thread" },
  { key: "mod+b", label: "Shell: Toggle Sidebar" },
  { key: "mod+i", label: "Shell: Toggle Inspector" },
  { key: "mod+j", label: "Shell: Toggle Drawer" },
  ...[1, 2, 3, 4, 5, 6, 7, 8, 9].map((n) => ({ key: `mod+${n}`, label: `Shell: Thread ${n}` })),
  { key: "ctrl+tab", label: "Shell: Next Thread", terminal: true },
  { key: "mod+[", label: "Shell: Back" },
  { key: "mod+]", label: "Shell: Forward" },
  { key: "mod+shift+t", label: "Shell: Reopen Closed Thread" },
  { key: "mod+,", label: "Shell: Settings", terminal: true },
  { key: "mod+shift+m", label: "Shell: Switch Mode", terminal: true },
];

export type KeybindingSource = "Default" | "Custom";

export interface KeybindingRow {
  readonly id: string;
  readonly command: KeybindingCommand;
  readonly key: string;
  readonly when: string;
  readonly source: KeybindingSource;
  readonly defaultKey: string | null;
  readonly defaultWhen: string;
  readonly binding: ResolvedKeybindingRule;
  readonly conflicts: ReadonlyArray<string>;
}

const CORE_WHEN_VARIABLES = [
  "terminalFocus",
  "editableFocus",
  "composerFocus",
  "threadOpen",
  "isWeb",
  "isDesktop",
  "true",
  "false",
] as const;

const KNOWN_WHEN_VARIABLES = new Set<string>(CORE_WHEN_VARIABLES);
for (const binding of DEFAULT_RESOLVED_KEYBINDINGS) {
  collectWhenIdentifiersFromNode(binding.whenAst, KNOWN_WHEN_VARIABLES);
}

export function shortcutToKeybindingInput(shortcut: KeybindingShortcut): string {
  const parts: string[] = [];
  if (shortcut.modKey) parts.push("mod");
  if (shortcut.metaKey) parts.push("meta");
  if (shortcut.ctrlKey) parts.push("ctrl");
  if (shortcut.altKey) parts.push("alt");
  if (shortcut.shiftKey) parts.push("shift");
  parts.push(shortcut.key === " " ? "space" : shortcut.key === "escape" ? "esc" : shortcut.key);
  return parts.join("+");
}

export function whenAstToExpression(node: KeybindingWhenNode | undefined): string {
  if (!node) return "";
  switch (node.type) {
    case "identifier":
      return node.name;
    case "not":
      return `!${wrapWhenExpression(node.node)}`;
    case "and":
      return `${wrapWhenExpression(node.left)} && ${wrapWhenExpression(node.right)}`;
    case "or":
      return `${wrapWhenExpression(node.left)} || ${wrapWhenExpression(node.right)}`;
  }
}

function wrapWhenExpression(node: KeybindingWhenNode): string {
  if (node.type === "identifier" || node.type === "not") return whenAstToExpression(node);
  return `(${whenAstToExpression(node)})`;
}

export function parseWhenExpressionDraft(
  expression: string,
): { ok: true; value: KeybindingWhenNode | undefined } | { ok: false; message: string } {
  const trimmed = expression.trim();
  if (trimmed.length === 0) return { ok: true, value: undefined };

  const ast = parseKeybindingWhenExpression(trimmed);
  if (!ast) {
    return {
      ok: false,
      message: "Use variables with !, &&, ||, and parentheses.",
    };
  }

  return { ok: true, value: ast };
}

function isDefaultBinding(binding: ResolvedKeybindingRule): boolean {
  const bindingKey = shortcutToKeybindingInput(binding.shortcut);
  const bindingWhen = whenAstToExpression(binding.whenAst);
  return DEFAULT_RESOLVED_KEYBINDINGS.some(
    (entry) =>
      entry.command === binding.command &&
      shortcutToKeybindingInput(entry.shortcut) === bindingKey &&
      whenAstToExpression(entry.whenAst) === bindingWhen,
  );
}

function defaultBindingForBinding(
  binding: ResolvedKeybindingRule,
): ResolvedKeybindingRule | undefined {
  const bindingKey = shortcutToKeybindingInput(binding.shortcut);
  const bindingWhen = whenAstToExpression(binding.whenAst);

  return (
    DEFAULT_RESOLVED_KEYBINDINGS.find(
      (entry) =>
        entry.command === binding.command &&
        shortcutToKeybindingInput(entry.shortcut) === bindingKey &&
        whenAstToExpression(entry.whenAst) === bindingWhen,
    ) ??
    DEFAULT_RESOLVED_KEYBINDINGS.find(
      (entry) =>
        entry.command === binding.command && whenAstToExpression(entry.whenAst) === bindingWhen,
    ) ??
    DEFAULT_RESOLVED_KEYBINDINGS.find((entry) => entry.command === binding.command)
  );
}

function keybindingRowId(command: KeybindingCommand, key: string, when: string): string {
  return `${command}\u0000${key}\u0000${when}`;
}

function conflictsWithWhen(leftWhen: string, rightWhen: string): boolean {
  return leftWhen.length === 0 || rightWhen.length === 0 || leftWhen === rightWhen;
}

/** A chord in its canonical spelling (`ctrl+shift+K` and `shift+ctrl+k` are one chord). */
function canonicalKey(key: string): string {
  const shortcut = parseKeybindingShortcut(key.trim());
  return shortcut ? shortcutToKeybindingInput(shortcut) : key.trim();
}

const SHELL_KEYS = new Map(
  SHELL_KEYBINDINGS.map((entry) => [canonicalKey(entry.key), entry.label]),
);

export function keybindingConflictLabels(
  rows: ReadonlyArray<Pick<KeybindingRow, "id" | "key" | "when" | "command">>,
  input: { readonly rowId: string; readonly key: string; readonly when: string },
): ReadonlyArray<string> {
  if (input.key.trim().length === 0) return [];
  const key = canonicalKey(input.key);
  const conflicts: Array<string> = [];
  const shell = SHELL_KEYS.get(key);
  if (shell) conflicts.push(shell);
  for (const candidate of rows) {
    if (
      candidate.id !== input.rowId &&
      canonicalKey(candidate.key) === key &&
      conflictsWithWhen(candidate.when, input.when)
    ) {
      conflicts.push(commandLabel(candidate.command));
    }
  }
  return [...new Set(conflicts)].toSorted();
}

export interface KeybindingGroup {
  readonly id: string;
  readonly title: string;
  readonly rows: ReadonlyArray<KeybindingRow>;
}

/** Page sections in display order; a command joins the first group listing its prefix. */
const KEYBINDING_GROUPS = [
  {
    id: "navigation",
    title: "Navigation",
    prefixes: ["sidebar", "navigation", "rightPanel", "settings"],
  },
  { id: "threads", title: "Threads", prefixes: ["chat", "thread"] },
  { id: "panels", title: "Panels", prefixes: ["diff", "files", "preview", "terminal"] },
] as const;
const OTHER_KEYBINDING_GROUP = { id: "other", title: "Other" } as const;

function keybindingGroupFor(command: KeybindingCommand): { id: string; title: string } {
  const prefix = String(command).split(".")[0] ?? "";
  return (
    KEYBINDING_GROUPS.find((group) => (group.prefixes as ReadonlyArray<string>).includes(prefix)) ??
    OTHER_KEYBINDING_GROUP
  );
}

/** Splits sorted rows into the page's sections, dropping sections with no rows. */
export function groupKeybindingRows(
  rows: ReadonlyArray<KeybindingRow>,
): ReadonlyArray<KeybindingGroup> {
  const rowsByGroup = new Map<string, Array<KeybindingRow>>();
  for (const row of rows) {
    const group = keybindingGroupFor(row.command);
    const bucket = rowsByGroup.get(group.id);
    if (bucket) bucket.push(row);
    else rowsByGroup.set(group.id, [row]);
  }
  return [...KEYBINDING_GROUPS, OTHER_KEYBINDING_GROUP].flatMap((group) => {
    const groupRows = rowsByGroup.get(group.id);
    return groupRows ? [{ id: group.id, title: group.title, rows: groupRows }] : [];
  });
}

export function buildKeybindingRows(
  keybindings: ResolvedKeybindingsConfig,
  query: string,
): ReadonlyArray<KeybindingRow> {
  const normalizedQuery = query.trim().toLowerCase();
  const rows = keybindings.map((binding, index) => {
    const defaultBinding = defaultBindingForBinding(binding);
    const key = shortcutToKeybindingInput(binding.shortcut);
    const when = whenAstToExpression(binding.whenAst);
    return {
      id: `${keybindingRowId(binding.command, key, when)}\u0000${index}`,
      command: binding.command,
      key,
      when,
      source: isDefaultBinding(binding) ? "Default" : "Custom",
      defaultKey: defaultBinding ? shortcutToKeybindingInput(defaultBinding.shortcut) : null,
      defaultWhen: whenAstToExpression(defaultBinding?.whenAst),
      binding,
      conflicts: [],
    } satisfies KeybindingRow;
  });

  const rowsWithConflicts = rows.map((row) => {
    const conflicts = keybindingConflictLabels(rows, {
      rowId: row.id,
      key: row.key,
      when: row.when,
    });
    return conflicts.length > 0 ? Object.assign({}, row, { conflicts }) : row;
  });

  rowsWithConflicts.sort((left, right) => {
    const commandCompare = left.command.localeCompare(right.command);
    if (commandCompare !== 0) return commandCompare;
    return left.key.localeCompare(right.key);
  });

  if (normalizedQuery.length === 0) {
    return rowsWithConflicts;
  }

  return rowsWithConflicts.filter((row) => {
    return (
      row.command.toLowerCase().includes(normalizedQuery) ||
      commandLabel(row.command).toLowerCase().includes(normalizedQuery) ||
      row.key.toLowerCase().includes(normalizedQuery) ||
      row.when.toLowerCase().includes(normalizedQuery) ||
      row.source.toLowerCase().includes(normalizedQuery)
    );
  });
}

function collectWhenIdentifiersFromNode(
  node: KeybindingWhenNode | undefined,
  identifiers: Set<string>,
): void {
  if (!node) return;
  switch (node.type) {
    case "identifier":
      identifiers.add(node.name);
      return;
    case "not":
      collectWhenIdentifiersFromNode(node.node, identifiers);
      return;
    case "and":
    case "or":
      collectWhenIdentifiersFromNode(node.left, identifiers);
      collectWhenIdentifiersFromNode(node.right, identifiers);
      return;
  }
}

export function isKnownWhenVariable(identifier: string): boolean {
  return KNOWN_WHEN_VARIABLES.has(identifier);
}

export function unknownWhenVariables(node: KeybindingWhenNode | undefined): ReadonlyArray<string> {
  const identifiers = new Set<string>();
  collectWhenIdentifiersFromNode(node, identifiers);
  return [...identifiers].filter((identifier) => !isKnownWhenVariable(identifier)).toSorted();
}

export function buildWhenVariableOptions(): ReadonlyArray<string> {
  return [...KNOWN_WHEN_VARIABLES].toSorted((left, right) => {
    const leftCoreIndex = CORE_WHEN_VARIABLES.indexOf(left as (typeof CORE_WHEN_VARIABLES)[number]);
    const rightCoreIndex = CORE_WHEN_VARIABLES.indexOf(
      right as (typeof CORE_WHEN_VARIABLES)[number],
    );
    if (leftCoreIndex !== -1 || rightCoreIndex !== -1) {
      return (
        (leftCoreIndex === -1 ? Number.MAX_SAFE_INTEGER : leftCoreIndex) -
        (rightCoreIndex === -1 ? Number.MAX_SAFE_INTEGER : rightCoreIndex)
      );
    }
    return left.localeCompare(right);
  });
}

export function buildKeybindingCommandOptions(): ReadonlyArray<KeybindingCommand> {
  return [...STUDIO_KEYBINDING_COMMANDS].toSorted((left, right) =>
    commandLabel(left).localeCompare(commandLabel(right)),
  );
}

const COMMAND_LABELS: Partial<Record<KeybindingCommand, string>> = {
  "chat.new": "Thread: New",
  "files.toggle": "Files: Show or Hide",
  "diff.toggle": "Diff: Show or Hide",
  "preview.toggle": "Preview: Show or Hide",
  "terminal.toggle": "Terminal: Show or Hide",
  "rightPanel.toggle": "Panel: Show or Hide",
  "sidebar.toggle": "Sidebar: Show or Hide",
  "thread.previous": "Thread: Previous",
  "thread.next": "Thread: Next",
  ...Object.fromEntries(
    Array.from({ length: 9 }, (_, i) => [`thread.jump.${i + 1}`, `Thread: Open ${i + 1}`]),
  ),
};

export function commandLabel(command: KeybindingCommand): string {
  const known = COMMAND_LABELS[command];
  if (known) return known;
  return String(command).split(".").map(titleCaseCommandSegment).join(": ");
}

function titleCaseCommandSegment(segment: string): string {
  const words: Array<string> = [];
  for (const part of segment.replace(/([a-z0-9])([A-Z])/g, "$1 $2").split(/[-_\s]+/)) {
    if (part.length > 0) {
      words.push(part.slice(0, 1).toUpperCase() + part.slice(1));
    }
  }
  return words.join(" ");
}

function normalizeShortcutKeyToken(key: string): string | null {
  const normalized = key.toLowerCase();
  if (
    normalized === "meta" ||
    normalized === "control" ||
    normalized === "ctrl" ||
    normalized === "shift" ||
    normalized === "alt" ||
    normalized === "option"
  ) {
    return null;
  }
  if (normalized === " ") return "space";
  if (normalized === "escape") return "esc";
  if (normalized === "arrowup") return "arrowup";
  if (normalized === "arrowdown") return "arrowdown";
  if (normalized === "arrowleft") return "arrowleft";
  if (normalized === "arrowright") return "arrowright";
  if (normalized.length === 1) return normalized;
  if (/^f\d{1,2}$/.test(normalized)) return normalized;
  if (normalized === "enter" || normalized === "tab" || normalized === "backspace") {
    return normalized;
  }
  if (normalized === "delete" || normalized === "home" || normalized === "end") {
    return normalized;
  }
  if (normalized === "pageup" || normalized === "pagedown") return normalized;
  return null;
}

/** Turns a keydown into a binding such as `mod+shift+k` or `tab`. Null for modifier-only presses. */
export function keybindingFromKeyboardEvent(
  event: Pick<KeyboardEvent, "key" | "code" | "metaKey" | "ctrlKey" | "altKey" | "shiftKey">,
  platform: string,
): string | null {
  const keyToken = normalizeShortcutKeyToken(shortcutKeyFromEvent(event));
  if (!keyToken) return null;

  const parts: string[] = [];
  if (isMacPlatform(platform)) {
    if (event.metaKey) parts.push("mod");
    if (event.ctrlKey) parts.push("ctrl");
  } else {
    if (event.ctrlKey) parts.push("mod");
    if (event.metaKey) parts.push("meta");
  }
  if (event.altKey) parts.push("alt");
  if (event.shiftKey) parts.push("shift");
  parts.push(keyToken);
  return parts.join("+");
}
