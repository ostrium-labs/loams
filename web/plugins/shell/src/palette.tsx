// The console's command palette (design §53 §4.5, §5.10; SD1 Task 24, ruling T3-6):
// Mod+K (and Mod+Shift+P) in both modes, through the shell's keymap (keys.ts), on
// @loams/ui's CommandPalette, ranked as T3 Code ranks (@loams/ui `rankCommands`).
//
// Plugins contribute through the `shell.palette.source` slot (design 59, W9; its older
// name `palette.command` stays an alias): an entry renders nothing and,
// while the palette is open, publishes its commands for the current query (so a plugin
// can read server state with its own hooks, and only while someone is looking). The
// shell adds "Go to" rows from the nav and its own general commands.
//
// The mode scope: an entry's mode is its `meta.mode`, else its plugin's (the current
// mode when the plugin is in both). An entry whose `meta.mode` its plugin does not
// declare is refused (§53 §4.1). The current mode's commands come first under their
// own headings; the other mode's follow under one heading, "Console" or "Studio". The
// shell's "Go to" rows reach every page in either mode (D1080), so they are the current
// mode's. A plugin of a mode not yet entered is not loaded, so its commands appear once
// that mode has been visited.

import type { Mode } from '@loams/console-host';
import { type PaletteCommand, SlotBoundary, type SlotEntry, useSlot } from '@loams/slots';
import { type CommandItem, CommandPalette, rankCommands, useToast } from '@loams/ui';
import { useCallback, useEffect, useRef, useState } from 'react';
import { isMacPlatform, matchShellKey } from './keys.js';
import { MODE_LABEL } from './mode-switcher.js';

/** A command with the mode it belongs to. */
export interface ScopedCommand extends PaletteCommand {
  mode: Mode;
}

/** Which modes a plugin declares; the shell's `ModeService.modesOf`. */
export interface PaletteModes {
  current: Mode;
  available: readonly Mode[];
  modesOf(pluginId: string): readonly Mode[];
}

/**
 * The mode a `palette.command` entry's commands belong to, or undefined when its
 * plugin may not show them in the mode it names (§53 §4.1).
 */
export function paletteModeOf(
  entry: Pick<SlotEntry, 'plugin' | 'meta'>,
  modes: PaletteModes | undefined,
): Mode | undefined {
  if (!modes) return entry.meta?.mode ?? 'console';
  const declared = modes.modesOf(entry.plugin);
  if (entry.meta?.mode) return declared.includes(entry.meta.mode) ? entry.meta.mode : undefined;
  return declared.includes(modes.current) ? modes.current : declared[0];
}

/** A palette row with the command it runs. */
export interface PaletteRow extends CommandItem {
  recency?: number;
  secondary?: boolean;
  command: ScopedCommand;
}

/**
 * The palette's rows for a query: the current mode's commands first, then each other
 * available mode's under its heading, ranked within each heading.
 */
export function scopePalette(
  commands: readonly ScopedCommand[],
  current: Mode,
  available: readonly Mode[],
  query: string,
): PaletteRow[] {
  const row = (command: ScopedCommand, key: string, group: string | undefined, detail?: string) =>
    ({
      id: `${command.mode}:${key}`,
      text: command.title,
      group,
      detail,
      keywords: command.keywords,
      icon: command.icon,
      shortcut: command.shortcut,
      disabled: command.disabled,
      recency: command.recency,
      secondary: command.secondary,
      command,
    }) satisfies PaletteRow;
  const rows: PaletteRow[] = [];
  commands.forEach((c, i) => {
    if (c.mode === current) rows.push(row(c, `${i}:${c.id}`, c.group, c.detail));
  });
  for (const other of available) {
    if (other === current) continue;
    commands.forEach((c, i) => {
      if (c.mode === other) {
        rows.push(row(c, `${i}:${c.id}`, MODE_LABEL[other], c.detail));
      }
    });
  }
  return rankCommands(rows, query);
}

/** A cheap fingerprint of what a published list shows, so a re-publish of the same rows is free. */
function signature(commands: readonly PaletteCommand[]): string {
  return commands
    .map((c) =>
      [
        c.id,
        c.title,
        c.group ?? '',
        c.detail ?? '',
        (c.keywords ?? []).join(' '),
        (c.shortcut ?? []).join('+'),
        c.recency ?? '',
        c.disabled ? 1 : 0,
        c.secondary ? 1 : 0,
      ].join('\u0000'),
    )
    .join('\u0001');
}

function Source({
  entry,
  query,
  onPublish,
}: {
  entry: SlotEntry<'palette.command'>;
  query: string;
  onPublish: (id: number, commands: readonly PaletteCommand[]) => void;
}) {
  const publish = useCallback(
    (commands: readonly PaletteCommand[]) => onPublish(entry.id, commands),
    [entry.id, onPublish],
  );
  const Entry = entry.component;
  return (
    <SlotBoundary plugin={entry.plugin}>
      <Entry query={query} publish={publish} />
    </SlotBoundary>
  );
}

/**
 * True when the key press opens the palette (D1081: Mod+K or Mod+Shift+P; Ctrl on Linux
 * and Windows, ⌘ on macOS). The shell's keymap (`useShellKeys`) is the one listener.
 */
export function isPaletteShortcut(e: KeyboardEvent): boolean {
  return (
    matchShellKey(e, {
      mac: isMacPlatform(),
      terminal: false,
      richText: false,
      editable: false,
    }) === 'palette.toggle'
  );
}

export function ShellPalette({
  modes,
  nav,
  general,
  open: given,
  onOpenChange,
}: {
  /** The current and available modes; absent, everything is Console's. */
  modes?: PaletteModes;
  /** The "Go to" (and pin) commands of each mode for the query, from the pages. */
  nav: (mode: Mode, query: string) => ScopedCommand[];
  /** The shell's own commands (switch mode, theme, agent panel), in the current mode. */
  general: ScopedCommand[];
  /** Open state, when the caller owns it (the top bar's search button opens it too). */
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
}) {
  const [own, setOwn] = useState(false);
  const open = given ?? own;
  const setOpen = useCallback(
    (next: boolean) => {
      if (onOpenChange) onOpenChange(next);
      if (given === undefined) setOwn(next);
    },
    [given, onOpenChange],
  );
  const [query, setQuery] = useState('');
  // `shell.palette.source` (design 59, W9) and its alias `palette.command` have the same
  // props, so their entries mount the same way.
  const legacy = useSlot('palette.command');
  const sources = useSlot('shell.palette.source');
  const entries = [...legacy, ...(sources as unknown as SlotEntry<'palette.command'>[])];
  const toast = useToast();
  const published = useRef(new Map<number, { sig: string; commands: readonly PaletteCommand[] }>());
  const [, bump] = useState(0);

  useEffect(() => {
    if (open) return;
    // Each opening starts from an empty query and fresh contributions.
    setQuery('');
    published.current.clear();
  }, [open]);

  const onPublish = useCallback((id: number, commands: readonly PaletteCommand[]) => {
    const sig = signature(commands);
    const before = published.current.get(id);
    published.current.set(id, { sig, commands });
    if (before?.sig !== sig) bump((n) => n + 1);
  }, []);

  const current = modes?.current ?? 'console';
  const available = modes?.available ?? ['console'];
  const scoped: ScopedCommand[] = [];
  const contributed = entries.flatMap((entry) => {
    const mode = paletteModeOf(entry, modes);
    if (!mode || !available.includes(mode)) return [];
    return (published.current.get(entry.id)?.commands ?? []).map((c) => ({ ...c, mode }));
  });
  const byMode = (m: Mode) => contributed.filter((c) => c.mode === m);
  for (const m of [current, ...available.filter((a) => a !== current)]) {
    scoped.push(...byMode(m), ...nav(m, query));
    if (m === current) scoped.push(...general);
  }
  const rows = scopePalette(scoped, current, available, query);
  const byId = new Map(rows.map((r) => [r.id, r]));

  const onSelect = (item: CommandItem) => {
    const command = byId.get(item.id)?.command;
    if (!command || command.disabled) return;
    if (!command.keepOpen) setOpen(false);
    const fail = (error: unknown) =>
      toast.add({
        title: `${command.title} failed`,
        description: error instanceof Error ? error.message : String(error),
        type: 'danger',
      });
    try {
      const result = command.run();
      if (result && typeof (result as Promise<void>).then === 'function') {
        (result as Promise<void>).catch(fail);
      }
    } catch (error) {
      fail(error);
    }
  };

  return (
    <>
      {open && (
        <div hidden data-palette-sources>
          {entries.map((entry) => (
            <Source key={entry.id} entry={entry} query={query} onPublish={onPublish} />
          ))}
        </div>
      )}
      <CommandPalette
        open={open}
        onOpenChange={setOpen}
        items={rows}
        onSelect={onSelect}
        query={query}
        onQueryChange={setQuery}
        filter={null}
        placeholder={
          current === 'studio'
            ? 'Search threads, projects, files and commands…'
            : 'Search pages and commands…'
        }
        footer={
          <>
            <span>↑↓ to move · ↵ to run · Esc to close</span>
            <span>{MODE_LABEL[current]} first</span>
          </>
        }
      />
    </>
  );
}
