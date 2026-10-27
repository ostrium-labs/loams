// Studio's keybindings, stored per device (SD1 Task 24; §53 §5.10; Shared contracts
// `loams.studio.keybindings`). Only the custom rules are stored, as upstream's
// `keybindings.json` holds them: a command with custom rules loses its defaults, and
// resetting the command brings them back. Every storage access is wrapped: a blocked
// or full storage leaves the defaults on read and keeps a change for this page on
// write. Stored rows are untrusted, so each is checked (a known command, a chord and a
// `when` clause that parse) and a bad one is dropped.

import {
  compileResolvedKeybindingsConfig,
  DEFAULT_RESOLVED_KEYBINDINGS,
  isKeybindingCommand,
  type KeybindingCommand,
  type KeybindingRule,
  MAX_KEYBINDINGS_COUNT,
  mergeWithDefaultKeybindings,
  parseKeybindingShortcut,
  parseKeybindingWhenExpression,
  type ResolvedKeybindingsConfig,
  SHELL_RUN_RESOLVED_KEYBINDINGS,
} from "./shared";

export const KEYBINDINGS_KEY = "loams.studio.keybindings";

/** The slice of `Storage` the store uses. */
export interface KeybindingStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

function ruleOf(value: unknown): KeybindingRule | null {
  if (!value || typeof value !== "object") return null;
  const { key, command, when } = value as Record<string, unknown>;
  if (typeof key !== "string" || !isKeybindingCommand(command)) return null;
  if (!parseKeybindingShortcut(key)) return null;
  if (when === undefined) return { key, command };
  if (typeof when !== "string" || !parseKeybindingWhenExpression(when)) return null;
  return { key, command, when };
}

/** The custom rules stored on this device; none when storage is blocked or the value is bad. */
export function readCustomKeybindings(
  storage: KeybindingStorage | null | undefined,
): KeybindingRule[] {
  try {
    const raw = storage?.getItem(KEYBINDINGS_KEY);
    if (!raw) return [];
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed
      .map(ruleOf)
      .filter((rule): rule is KeybindingRule => rule !== null)
      .slice(-MAX_KEYBINDINGS_COUNT);
  } catch {
    return [];
  }
}

/** Writes the custom rules; false when storage refused them. */
export function writeCustomKeybindings(
  storage: KeybindingStorage | null | undefined,
  rules: readonly KeybindingRule[],
): boolean {
  try {
    if (!storage) return false;
    storage.setItem(KEYBINDINGS_KEY, JSON.stringify(rules));
    return true;
  } catch {
    return false;
  }
}

/** `localStorage`, or null where reading it throws (some private windows) or it is absent. */
export function browserStorage(): KeybindingStorage | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

/** A command's new bindings: a chord and an optional `when` clause each. */
export type CommandBinding = { key: string; when?: string };

/**
 * The keybindings in memory, with their storage behind them. `resolved()` is a stable
 * snapshot between changes, for `useSyncExternalStore`.
 */
export class KeybindingStore {
  #rules: KeybindingRule[];
  #resolved: ResolvedKeybindingsConfig;
  #shown: ResolvedKeybindingsConfig;
  readonly #listeners = new Set<() => void>();

  constructor(private readonly storage: KeybindingStorage | null | undefined = browserStorage()) {
    this.#rules = readCustomKeybindings(storage);
    const [resolved, shown] = this.#merge();
    this.#resolved = resolved;
    this.#shown = shown;
  }

  /** The custom rules, in the order they apply. */
  rules(): readonly KeybindingRule[] {
    return this.#rules;
  }

  /** The defaults merged with the custom rules: what key presses resolve against. */
  readonly resolved = (): ResolvedKeybindingsConfig => this.#resolved;

  /**
   * What a label shows: `resolved()` with the shell's chords for Studio's commands
   * (`SHELL_RUN_KEYBINDINGS`) between the defaults and the custom rules, so a custom rule
   * is still the one shown. Never resolve a key press against it: the shell runs those.
   */
  readonly shown = (): ResolvedKeybindingsConfig => this.#shown;

  readonly subscribe = (listener: () => void): (() => void) => {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  };

  /**
   * Replaces a command's bindings with `bindings`; bad rows are dropped. A command left
   * with none falls back to its defaults (to switch one off, give it `when: "false"`).
   */
  setCommandRules(command: KeybindingCommand, bindings: readonly CommandBinding[]): void {
    const next = bindings
      .map((b) => ruleOf({ key: b.key, command, ...(b.when ? { when: b.when } : {}) }))
      .filter((rule): rule is KeybindingRule => rule !== null);
    this.#set([...this.#rules.filter((r) => r.command !== command), ...next]);
  }

  /** Drops a command's custom rules, so its defaults apply again. */
  resetCommand(command: KeybindingCommand): void {
    this.#set(this.#rules.filter((r) => r.command !== command));
  }

  /** Drops every custom rule. */
  resetAll(): void {
    this.#set([]);
  }

  #set(rules: KeybindingRule[]): void {
    this.#rules = rules.slice(-MAX_KEYBINDINGS_COUNT);
    const [resolved, shown] = this.#merge();
    this.#resolved = resolved;
    this.#shown = shown;
    // A refused write keeps the change for this page (keybindings_storage_failure_tolerated).
    writeCustomKeybindings(this.storage, this.#rules);
    for (const listener of [...this.#listeners]) listener();
  }

  #merge(): [ResolvedKeybindingsConfig, ResolvedKeybindingsConfig] {
    const custom = compileResolvedKeybindingsConfig(this.#rules);
    const resolved = mergeWithDefaultKeybindings(custom);
    const overridden = new Set(custom.map((binding) => binding.command));
    const defaults = DEFAULT_RESOLVED_KEYBINDINGS.filter((b) => !overridden.has(b.command));
    return [resolved, [...defaults, ...SHELL_RUN_RESOLVED_KEYBINDINGS, ...custom]];
  }
}
