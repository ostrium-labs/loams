// Portions of this file are from T3 Code (https://github.com/pingdotgg/t3code),
// apps/web/src/components/ThreadTerminalDrawer.tsx at commit 6524a8d4, Copyright (c) 2026 T3 Tools Inc., MIT licence
// (see web/packages/studio-ui/LICENSE.t3code). Modified by The Loams Authors:
// rebuilt on xterm.js and the fit addon in place of the libghostty-vt surface; data through a TerminalBackend over loams-agentd; a token theme; multi-line paste and running-close confirmations in @loams/ui dialogs; Effect, atoms, auth scopes, the preview and editor hand-offs and the native context menus dropped. Loams' changes are Apache-2.0.

// Studio's terminal drawer (§53 §5.7, D920; SD1 Task 22). The tabs, splits, group list
// and toolbar are T3 Code's; the terminal is xterm.js (Q818), coloured from tokens.css
// for both themes. Output is text written into xterm and nothing else (§53 §10).
// - A tab is a PTY loams-agentd opened for the thread; its session (session.ts)
//   replays from the last seq when the drawer comes back.
// - Pasting more than one line asks first; closing a terminal whose shell still runs
//   asks first (§50 §14.5).
// - Ctrl/Cmd-click on an http(s) link opens it outside Studio; on a file path, in the
//   thread's file viewer. A selection can be sent to the composer.
// - Key presses Studio or the shell bind (Ctrl+` and the like) pass through xterm.

import "@xterm/xterm/css/xterm.css";
import { AlertDialog, cx, Menu, MenuItem, Tooltip } from "@loams/ui";
import { FitAddon } from "@xterm/addon-fit";
import {
  type IBufferRange,
  type IDisposable,
  type ILink,
  type ILinkProvider,
  type ITheme,
  Terminal,
} from "@xterm/xterm";
import {
  MessageSquarePlus,
  Play,
  Plus,
  Square,
  SquareSplitHorizontal,
  SquareSplitVertical,
  TerminalSquare,
  Trash2,
  X,
} from "lucide-react";
import {
  type ReactNode,
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import { currentPlatform } from "../keybindings/keybindings";
import { needsCloseConfirm, terminalCloseQuestion } from "./closeConfirm";
import { normalizeTerminalContextSelection, type TerminalContextSelection } from "./context";
import { nextTerminalLabel } from "./labels";
import {
  collectWrappedTerminalLinkLine,
  extractTerminalLinks,
  isTerminalLinkActivation,
  isTerminalUrl,
  type TerminalBufferLineLike,
  type WrappedTerminalLinkLineSegment,
} from "./links";
import {
  INITIAL_TERMINAL_OUTPUT_CURSOR,
  readTerminalOutputUpdate,
  type TerminalInfo,
  type TerminalOutputCursor,
  type TerminalSessionState,
  type TerminalSessions,
} from "./session";
import { type FileLinkTarget, fileLinkTarget } from "./targets";
import {
  canSplit,
  selectThreadTerminalUiState,
  type TerminalTab,
  type ThreadTerminalGroup,
  useTerminalUiStateStore,
} from "./uiStore";

const DEFAULT_COLS = 80;
const DEFAULT_ROWS = 24;
// A new PTY starts at this size; the viewport fits it as soon as it is laid out.
const INITIAL_SIZE = { cols: DEFAULT_COLS, rows: DEFAULT_ROWS } as const;

/* ---------- the surface ---------- */

/** The part of xterm.js the drawer uses, so tests can stand in for it. */
export interface TerminalSurface {
  readonly cols: number;
  readonly rows: number;
  open(element: HTMLElement): void;
  write(data: string): void;
  reset(): void;
  paste(text: string): void;
  focus(): void;
  fit(): void;
  setTheme(theme: ITheme): void;
  hasSelection(): boolean;
  getSelection(): string;
  getSelectionPosition(): IBufferRange | undefined;
  clearSelection(): void;
  getLine(index: number): TerminalBufferLineLike | undefined;
  onData(listener: (data: string) => void): IDisposable;
  onResize(listener: (size: { cols: number; rows: number }) => void): IDisposable;
  onSelectionChange(listener: () => void): IDisposable;
  attachCustomKeyEventHandler(handler: (event: KeyboardEvent) => boolean): void;
  registerLinkProvider(provider: ILinkProvider): IDisposable;
  dispose(): void;
}

export interface TerminalSurfaceOptions {
  theme: ITheme;
  fontFamily: string;
  fontSize: number;
}

export type CreateTerminalSurface = (options: TerminalSurfaceOptions) => TerminalSurface;

/** xterm.js with the fit addon. */
export const createXtermSurface: CreateTerminalSurface = (options) => {
  const term = new Terminal({
    theme: options.theme,
    fontFamily: options.fontFamily,
    fontSize: options.fontSize,
    cursorBlink: true,
    scrollback: 5000,
    allowProposedApi: false,
    // Links are ours (links.ts), activated with Ctrl/Cmd only.
    linkHandler: null,
  });
  const fit = new FitAddon();
  term.loadAddon(fit);
  return {
    get cols() {
      return term.cols;
    },
    get rows() {
      return term.rows;
    },
    open: (element) => term.open(element),
    write: (data) => term.write(data),
    reset: () => term.reset(),
    paste: (text) => term.paste(text),
    focus: () => term.focus(),
    fit: () => {
      try {
        fit.fit();
      } catch {
        // Not laid out yet (a hidden drawer); the next resize fits.
      }
    },
    setTheme: (theme) => {
      term.options.theme = theme;
    },
    hasSelection: () => term.hasSelection(),
    getSelection: () => term.getSelection(),
    getSelectionPosition: () => term.getSelectionPosition(),
    clearSelection: () => term.clearSelection(),
    getLine: (index) => term.buffer.active.getLine(index),
    onData: (listener) => term.onData(listener),
    onResize: (listener) => term.onResize(listener),
    onSelectionChange: (listener) => term.onSelectionChange(listener),
    attachCustomKeyEventHandler: (handler) => term.attachCustomKeyEventHandler(handler),
    registerLinkProvider: (provider) => term.registerLinkProvider(provider),
    dispose: () => term.dispose(),
  };
};

/* ---------- the theme ---------- */

const THEME_TOKENS: ReadonlyArray<readonly [keyof ITheme, string]> = [
  ["background", "--op-term-bg"],
  ["foreground", "--op-term-fg"],
  ["cursor", "--op-term-cursor"],
  ["cursorAccent", "--op-term-bg"],
  ["selectionBackground", "--op-term-selection"],
  ["black", "--op-term-black"],
  ["red", "--op-term-red"],
  ["green", "--op-term-green"],
  ["yellow", "--op-term-yellow"],
  ["blue", "--op-term-blue"],
  ["magenta", "--op-term-magenta"],
  ["cyan", "--op-term-cyan"],
  ["white", "--op-term-white"],
  ["brightBlack", "--op-term-bright-black"],
  ["brightRed", "--op-term-bright-red"],
  ["brightGreen", "--op-term-bright-green"],
  ["brightYellow", "--op-term-bright-yellow"],
  ["brightBlue", "--op-term-bright-blue"],
  ["brightMagenta", "--op-term-bright-magenta"],
  ["brightCyan", "--op-term-bright-cyan"],
  ["brightWhite", "--op-term-bright-white"],
];

/**
 * The xterm theme from tokens.css (`--op-term-*`), as the element sees them: the light
 * or the dark values, whichever `.dark` on <html> selects. A token the page lacks
 * (tests, an early paint) is left to xterm's default.
 */
export function terminalThemeFromTokens(element?: Element | null): ITheme {
  const styles = getComputedStyle(element ?? document.documentElement);
  const theme: ITheme = {};
  for (const [key, token] of THEME_TOKENS) {
    const value = styles.getPropertyValue(token).trim();
    if (value) (theme as Record<string, string>)[key] = value;
  }
  return theme;
}

function monoFont(): string {
  try {
    const value = getComputedStyle(document.documentElement)
      .getPropertyValue("--loams-font-mono") // mono-ok: code
      .trim();
    return value || "ui-monospace, monospace";
  } catch {
    return "ui-monospace, monospace";
  }
}

/** Calls `fn` when <html>'s class or style changes (the theme switch). */
function onThemeChange(fn: () => void): () => void {
  const observer = new MutationObserver(fn);
  observer.observe(document.documentElement, {
    attributes: true,
    attributeFilter: ["class", "style"],
  });
  return () => observer.disconnect();
}

/* ---------- helpers kept from upstream ---------- */

export function terminalSelectionLineRange(position: {
  start: { y: number };
  end: { y: number };
}): { lineStart: number; lineEnd: number } {
  const lineStart = position.start.y + 1;
  return {
    lineStart,
    lineEnd: Math.max(lineStart, position.end.y + 1),
  };
}

function systemMessage(message: string): string {
  return `\r\n\x1b[2m[terminal] ${message}\x1b[0m\r\n`;
}

/** The buffer range of `[start, end)` in a wrapped line's text. */
export function linkRange(
  segments: ReadonlyArray<WrappedTerminalLinkLineSegment>,
  start: number,
  end: number,
): IBufferRange | null {
  const at = (offset: number) => {
    const segment = segments.find((s) => offset >= s.startIndex && offset < s.endIndex);
    return segment ? { x: offset - segment.startIndex + 1, y: segment.bufferLineNumber } : null;
  };
  const from = at(start);
  const to = at(end - 1);
  return from && to ? { start: from, end: to } : null;
}

/**
 * Pasted text without the control characters a shell or the terminal would act on
 * (SD1 Task 30): ESC and the other C0 controls but tab and line breaks, DEL and the C1
 * controls. A copied snippet can hide `ESC [201~` (the end of a bracketed paste), after
 * which the rest of the paste runs as typed commands, or an ETX or EOT that interrupts
 * or ends the shell. The paste confirmation shows the cleaned text, and that is what is
 * sent.
 */
export function sanitizePaste(text: string): string {
  // oxlint-disable-next-line no-control-regex -- control characters are what is removed
  return text.replace(/[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f-\u009f]/g, "");
}

/** Whether a paste needs confirmation: more than one line once a final newline is set aside. */
export function isMultiLinePaste(text: string): boolean {
  return /\r\n|\r|\n/.test(text.replace(/(?:\r\n|\r|\n)$/, ""));
}

function lineCount(text: string): number {
  return text.replace(/(?:\r\n|\r|\n)$/, "").split(/\r\n|\r|\n/).length;
}

/* ---------- one terminal ---------- */

function useTerminalSession(sessions: TerminalSessions, terminalId: string): TerminalSessionState {
  const subscribe = useCallback(
    (listener: () => void) => sessions.subscribe(terminalId, listener),
    [sessions, terminalId],
  );
  return useSyncExternalStore(
    subscribe,
    () => sessions.get(terminalId),
    () => sessions.get(terminalId),
  );
}

interface TerminalViewportProps {
  tab: TerminalTab;
  sessions: TerminalSessions;
  createSurface: CreateTerminalSurface;
  platform: string;
  visible: boolean;
  autoFocus: boolean;
  focusRequestId: number;
  onOpenUrl: (url: string) => void;
  onOpenFile: (target: FileLinkTarget) => void;
  onSendSelection?: (selection: TerminalContextSelection) => void;
  /** Asks the user before a multi-line paste; `paste` runs it. */
  onConfirmPaste: (text: string, paste: () => void) => void;
  /** Key presses that belong to Studio or the shell, not the terminal. */
  isAppShortcut?: (event: KeyboardEvent) => boolean;
}

export function TerminalViewport({
  tab,
  sessions,
  createSurface,
  platform,
  visible,
  autoFocus,
  focusRequestId,
  onOpenUrl,
  onOpenFile,
  onSendSelection,
  onConfirmPaste,
  isAppShortcut,
}: TerminalViewportProps) {
  const terminalId = tab.id;
  const containerRef = useRef<HTMLDivElement>(null);
  const surfaceRef = useRef<TerminalSurface | null>(null);
  const cursorRef = useRef<TerminalOutputCursor>(INITIAL_TERMINAL_OUTPUT_CURSOR);
  const shownStatusRef = useRef<string | null>(null);
  const writeQueueRef = useRef<Promise<void>>(Promise.resolve());
  const [selected, setSelected] = useState(false);
  const session = useTerminalSession(sessions, terminalId);
  const sessionRef = useRef(session);
  sessionRef.current = session;
  const latest = useRef({
    onOpenUrl,
    onOpenFile,
    onConfirmPaste,
    isAppShortcut,
    cwd: tab.cwd,
    platform,
  });
  latest.current = { onOpenUrl, onOpenFile, onConfirmPaste, isAppShortcut, cwd: tab.cwd, platform };

  const writable = session.status === "running" || session.status === "connecting";
  const writableRef = useRef(writable);
  writableRef.current = writable;

  // Writes what the session holds that this surface has not shown, and the status line.
  const sync = useCallback((surface: TerminalSurface, state: TerminalSessionState) => {
    const update = readTerminalOutputUpdate(state.output, cursorRef.current);
    if (update.type === "reset") {
      surface.reset();
      if (update.data.length > 0) surface.write(update.data);
      shownStatusRef.current = null;
    } else if (update.type === "append") {
      surface.write(update.data);
    }
    cursorRef.current = update.cursor;
    const status =
      state.status === "exited"
        ? `exited:${state.exitCode ?? ""}`
        : state.error
          ? `error:${state.error}`
          : null;
    if (status && status !== shownStatusRef.current) {
      shownStatusRef.current = status;
      surface.write(
        systemMessage(
          state.status === "exited"
            ? state.signal
              ? `Process exited (${state.signal})`
              : `Process exited with code ${state.exitCode ?? 0}`
            : (state.error ?? ""),
        ),
      );
    }
  }, []);

  useEffect(() => {
    const mount = containerRef.current;
    if (!mount) return;
    const surface = createSurface({
      theme: terminalThemeFromTokens(mount),
      fontFamily: monoFont(),
      fontSize: 12,
    });
    surfaceRef.current = surface;
    cursorRef.current = INITIAL_TERMINAL_OUTPUT_CURSOR;
    shownStatusRef.current = null;
    surface.open(mount);
    surface.fit();
    const disposables: IDisposable[] = [];
    const send = (data: string) => {
      if (!writableRef.current) return;
      writeQueueRef.current = writeQueueRef.current
        .then(() => sessions.write(terminalId, data))
        .catch((error: unknown) => {
          surface.write(
            systemMessage(error instanceof Error ? error.message : "Terminal write failed"),
          );
        });
    };
    disposables.push(surface.onData(send));
    disposables.push(
      surface.onResize(({ cols, rows }) => {
        if (!writableRef.current) return;
        sessions.resize(terminalId, cols, rows).catch(() => {
          // The next resize, or the next attach, sends the size again.
        });
      }),
    );
    disposables.push(surface.onSelectionChange(() => setSelected(surface.hasSelection())));
    surface.attachCustomKeyEventHandler((event) => {
      // `false`: xterm leaves the key alone and it reaches Studio's and the shell's keys.
      if (event.type === "keydown" && latest.current.isAppShortcut?.(event)) return false;
      return true;
    });
    const provider: ILinkProvider = {
      provideLinks(bufferLineNumber, callback) {
        const line = collectWrappedTerminalLinkLine(bufferLineNumber, (index) =>
          surface.getLine(index),
        );
        if (!line) return callback(undefined);
        const links: ILink[] = [];
        for (const match of extractTerminalLinks(line.text)) {
          const range = linkRange(line.segments, match.start, match.end);
          if (!range) continue;
          links.push({
            range,
            text: match.text,
            decorations: { pointerCursor: true, underline: true },
            activate(event, text) {
              const now = latest.current;
              if (!isTerminalLinkActivation(event, now.platform)) return;
              if (isTerminalUrl(text)) now.onOpenUrl(text);
              else now.onOpenFile(fileLinkTarget(text, now.cwd));
            },
          });
        }
        callback(links.length > 0 ? links : undefined);
      },
    };
    disposables.push(surface.registerLinkProvider(provider));

    // Every paste is ours: xterm never sees the event. Control characters are removed
    // (sanitizePaste), and a paste of more than one line waits for a yes.
    const onPaste = (event: ClipboardEvent) => {
      const raw = event.clipboardData?.getData("text/plain") ?? "";
      event.preventDefault();
      event.stopImmediatePropagation();
      if (!writableRef.current) return;
      const text = sanitizePaste(raw);
      if (text === "") return;
      if (isMultiLinePaste(text)) latest.current.onConfirmPaste(text, () => surface.paste(text));
      else surface.paste(text);
    };
    mount.addEventListener("paste", onPaste, true);

    const stopTheme = onThemeChange(() => surface.setTheme(terminalThemeFromTokens(mount)));
    const resizeObserver =
      typeof ResizeObserver === "undefined" ? null : new ResizeObserver(() => surface.fit());
    resizeObserver?.observe(mount);

    sync(surface, sessionRef.current);
    if (writableRef.current && surface.cols > 0 && surface.rows > 0) {
      sessions.resize(terminalId, surface.cols, surface.rows).catch(() => {});
    }
    return () => {
      resizeObserver?.disconnect();
      stopTheme();
      mount.removeEventListener("paste", onPaste, true);
      for (const disposable of disposables) disposable.dispose();
      surfaceRef.current = null;
      surface.dispose();
    };
  }, [createSurface, sessions, sync, terminalId]);

  useEffect(() => {
    const surface = surfaceRef.current;
    if (surface) sync(surface, session);
  }, [session, sync]);

  useLayoutEffect(() => {
    if (visible) surfaceRef.current?.fit();
  }, [visible]);

  // oxlint-disable-next-line react/exhaustive-deps -- focusRequestId asks again.
  useEffect(() => {
    if (!autoFocus || !visible) return;
    surfaceRef.current?.focus();
  }, [autoFocus, focusRequestId, visible]);

  const sendSelection = () => {
    const surface = surfaceRef.current;
    if (!surface || !onSendSelection || !surface.hasSelection()) return;
    const position = surface.getSelectionPosition();
    if (!position) return;
    const selection = normalizeTerminalContextSelection({
      terminalId,
      terminalLabel: tab.label,
      ...terminalSelectionLineRange(position),
      text: surface.getSelection(),
    });
    if (!selection) return;
    onSendSelection(selection);
    surface.clearSelection();
    setSelected(false);
    surface.focus();
  };

  return (
    <section
      className="relative h-full w-full"
      aria-label={`${tab.label} (${tab.shell || "shell"})`}
      data-terminal
      data-terminal-id={terminalId}
    >
      <div
        ref={containerRef}
        className="h-full w-full overflow-hidden bg-(--op-term-bg) py-1 pl-2"
      />
      {selected && onSendSelection && (
        <button
          type="button"
          className={cx(
            "absolute top-1 right-2 z-10 inline-flex cursor-pointer items-center gap-1 rounded-pill",
            "border border-rule bg-surface px-2 py-0.5 text-[0.75rem] text-ink shadow-pop hover:bg-raised",
          )}
          onMouseDown={(event) => event.preventDefault()}
          onClick={sendSelection}
        >
          <MessageSquarePlus size={12} aria-hidden="true" />
          Add to composer
        </button>
      )}
    </section>
  );
}

/* ---------- the drawer ---------- */

/** A project action Studio can run as a named terminal (§53 §5.7). */
export interface TerminalProjectAction {
  id: string;
  name: string;
}

export interface TerminalDrawerProps {
  /** The thread (loams-agentd chat id) whose worktree the terminals run in. */
  threadId: string;
  sessions: TerminalSessions;
  /** Where new terminals start; the daemon falls back to the thread's own cwd. */
  cwd?: string;
  /** Whether the drawer is on screen (fits and focus wait for it). */
  visible?: boolean;
  /** Opens a first terminal when the thread has none. */
  autoOpen?: boolean;
  /** Bumped to move focus into the active terminal (the toggle shortcut). */
  focusRequestId?: number;
  onOpenUrl: (url: string) => void;
  onOpenFile: (target: FileLinkTarget) => void;
  onSendSelection?: (selection: TerminalContextSelection) => void;
  isAppShortcut?: (event: KeyboardEvent) => boolean;
  actions?: ReadonlyArray<TerminalProjectAction>;
  /** Runs a project action in a new terminal (ProjectService.RunProjectAction). */
  onRunAction?: (
    actionId: string,
    size: { cols: number; rows: number },
  ) => Promise<{ terminal: TerminalInfo; name: string }>;
  platform?: string;
  createSurface?: CreateTerminalSurface;
}

const BUTTON =
  "inline-grid size-6 shrink-0 cursor-pointer place-items-center rounded-pill border-0 bg-transparent " +
  "text-muted hover:bg-raised hover:text-ink disabled:cursor-not-allowed disabled:opacity-50";

function ActionButton({
  label,
  onClick,
  disabled,
  children,
}: {
  label: string;
  onClick: () => void;
  disabled?: boolean;
  children: ReactNode;
}) {
  return (
    <Tooltip content={label}>
      <button
        type="button"
        className={BUTTON}
        aria-label={label}
        disabled={disabled}
        onClick={onClick}
      >
        {children}
      </button>
    </Tooltip>
  );
}

type Confirm =
  | { kind: "close"; terminalIds: [string, ...string[]]; labels: [string, ...string[]] }
  | { kind: "paste"; text: string; paste: () => void };

export function TerminalDrawer({
  threadId,
  sessions,
  cwd,
  visible = true,
  autoOpen = true,
  focusRequestId = 0,
  onOpenUrl,
  onOpenFile,
  onSendSelection,
  isAppShortcut,
  actions,
  onRunAction,
  platform = currentPlatform(),
  createSurface = createXtermSurface,
}: TerminalDrawerProps) {
  const state = useTerminalUiStateStore((store) =>
    selectThreadTerminalUiState(store.terminalUiStateByThreadId, threadId),
  );
  const addTerminal = useTerminalUiStateStore((store) => store.addTerminal);
  const setActiveTerminal = useTerminalUiStateStore((store) => store.setActiveTerminal);
  const closeTerminal = useTerminalUiStateStore((store) => store.closeTerminal);
  const [opening, setOpening] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [confirm, setConfirm] = useState<Confirm | null>(null);
  const [focusBump, setFocusBump] = useState(0);
  const autoOpened = useRef(new Set<string>());

  const { terminalIds, terminalGroups, tabs } = state;
  const activeTerminalId = state.activeTerminalId;
  const activeGroup: ThreadTerminalGroup | undefined =
    terminalGroups.find((g) => g.id === state.activeTerminalGroupId) ??
    terminalGroups.find((g) => g.terminalIds.includes(activeTerminalId));
  const visibleTerminalIds =
    activeGroup?.terminalIds ?? (activeTerminalId ? [activeTerminalId] : []);
  const splitDirection = activeGroup?.splitDirection ?? "horizontal";
  const hasSidebar = terminalIds.length > 1;
  const showGroupHeaders =
    terminalGroups.length > 1 || terminalGroups.some((g) => g.terminalIds.length > 1);
  const splitAllowed = terminalIds.length > 0 && canSplit(state);

  const open = useCallback(
    async (mode: "new" | "split" | "splitVertical") => {
      setOpening(true);
      setError(null);
      try {
        const info = await sessions.backend.open({
          chatId: threadId,
          ...INITIAL_SIZE,
          ...(cwd ? { cwd } : {}),
        });
        const current = selectThreadTerminalUiState(
          useTerminalUiStateStore.getState().terminalUiStateByThreadId,
          threadId,
        );
        addTerminal(
          threadId,
          {
            id: info.id,
            label: nextTerminalLabel(Object.values(current.tabs).map((t) => t.label)),
            cwd: info.cwd,
            shell: info.shell,
          },
          mode,
        );
        setFocusBump((n) => n + 1);
      } catch (cause) {
        setError(cause instanceof Error ? cause.message : "The terminal did not open.");
      } finally {
        setOpening(false);
      }
    },
    [addTerminal, cwd, sessions, threadId],
  );

  const runAction = async (action: TerminalProjectAction) => {
    if (!onRunAction) return;
    setOpening(true);
    setError(null);
    try {
      const run = await onRunAction(action.id, INITIAL_SIZE);
      addTerminal(
        threadId,
        {
          id: run.terminal.id,
          label: run.name || action.name,
          cwd: run.terminal.cwd,
          shell: run.terminal.shell,
        },
        "new",
      );
      setFocusBump((n) => n + 1);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : `"${action.name}" did not start.`);
    } finally {
      setOpening(false);
    }
  };

  // A thread with no terminal gets one when the drawer first shows it (upstream opens
  // `term-1` the same way).
  useEffect(() => {
    if (!autoOpen || !visible || terminalIds.length > 0 || autoOpened.current.has(threadId)) {
      return;
    }
    autoOpened.current.add(threadId);
    void open("new");
  }, [autoOpen, open, terminalIds.length, threadId, visible]);

  const removeTerminals = async (ids: readonly string[]) => {
    setError(null);
    for (const id of ids) {
      try {
        await sessions.close(id);
      } catch (cause) {
        setError(cause instanceof Error ? cause.message : "The terminal did not close.");
        continue;
      }
      closeTerminal(threadId, id);
    }
  };

  const requestClose = (terminalId: string) => {
    if (!terminalId) return;
    const tab = tabs[terminalId];
    if (needsCloseConfirm(sessions.get(terminalId).status)) {
      setConfirm({ kind: "close", terminalIds: [terminalId], labels: [tab?.label ?? "Terminal"] });
      return;
    }
    void removeTerminals([terminalId]);
  };

  const confirmPaste = useCallback(
    (text: string, paste: () => void) => setConfirm({ kind: "paste", text, paste }),
    [],
  );

  const toolbar = (
    <>
      <ActionButton
        label={
          splitAllowed ? "Split terminal side by side" : "Split terminal (at most 4 per group)"
        }
        disabled={!splitAllowed || opening}
        onClick={() => void open("split")}
      >
        <SquareSplitHorizontal size={13} aria-hidden="true" />
      </ActionButton>
      <ActionButton
        label={splitAllowed ? "Split terminal stacked" : "Split terminal (at most 4 per group)"}
        disabled={!splitAllowed || opening}
        onClick={() => void open("splitVertical")}
      >
        <SquareSplitVertical size={13} aria-hidden="true" />
      </ActionButton>
      <ActionButton label="New terminal" disabled={opening} onClick={() => void open("new")}>
        <Plus size={13} aria-hidden="true" />
      </ActionButton>
      {actions && actions.length > 0 && onRunAction && (
        <Menu
          trigger={
            <button
              type="button"
              className={BUTTON}
              aria-label="Run a project action"
              disabled={opening}
            >
              <Play size={13} aria-hidden="true" />
            </button>
          }
        >
          {actions.map((action) => (
            <MenuItem key={action.id} onSelect={() => void runAction(action)}>
              {action.name}
            </MenuItem>
          ))}
        </Menu>
      )}
      <ActionButton
        label="Close terminal"
        disabled={!activeTerminalId}
        onClick={() => requestClose(activeTerminalId)}
      >
        <Trash2 size={13} aria-hidden="true" />
      </ActionButton>
    </>
  );

  const viewport = (terminalId: string, autoFocus: boolean) => {
    const tab = tabs[terminalId];
    if (!tab) return null;
    return (
      <TerminalViewport
        key={terminalId}
        tab={tab}
        sessions={sessions}
        createSurface={createSurface}
        platform={platform}
        visible={visible}
        autoFocus={autoFocus}
        focusRequestId={focusRequestId + focusBump}
        onOpenUrl={onOpenUrl}
        onOpenFile={onOpenFile}
        {...(onSendSelection ? { onSendSelection } : {})}
        onConfirmPaste={confirmPaste}
        {...(isAppShortcut ? { isAppShortcut } : {})}
      />
    );
  };

  const dialogs = (
    <>
      <AlertDialog
        open={confirm?.kind === "close"}
        onClose={() => setConfirm(null)}
        title={confirm?.kind === "close" ? terminalCloseQuestion(confirm.labels).title : ""}
        confirm="Close terminal"
        tone="danger"
        onConfirm={() => {
          const ids = confirm?.kind === "close" ? confirm.terminalIds : [];
          setConfirm(null);
          void removeTerminals(ids);
        }}
      >
        {confirm?.kind === "close" ? terminalCloseQuestion(confirm.labels).description : null}
      </AlertDialog>
      <AlertDialog
        open={confirm?.kind === "paste"}
        onClose={() => setConfirm(null)}
        title={
          confirm?.kind === "paste" ? `Paste ${lineCount(confirm.text)} lines?` : "Paste lines?"
        }
        confirm="Paste"
        onConfirm={() => {
          const paste = confirm?.kind === "paste" ? confirm.paste : undefined;
          setConfirm(null);
          paste?.();
        }}
      >
        The shell runs each line as if you typed it.
      </AlertDialog>
    </>
  );

  if (terminalIds.length === 0) {
    return (
      <div
        data-thread-terminal-drawer
        className="flex h-full min-h-0 flex-col items-center justify-center gap-3 px-4 py-6 text-center text-[0.86rem] text-muted"
      >
        <p className="m-0">
          {opening ? "Opening a terminal…" : "No terminals for this thread yet."}
        </p>
        {error && (
          <p role="alert" className="m-0 text-danger">
            {error}
          </p>
        )}
        <button
          type="button"
          className={cx(
            "inline-flex cursor-pointer items-center gap-1.5 rounded-pill border border-rule bg-bg px-3 py-1",
            "text-[0.8rem] text-ink hover:bg-raised disabled:cursor-not-allowed disabled:opacity-50",
          )}
          disabled={opening}
          onClick={() => void open("new")}
        >
          <Plus size={13} aria-hidden="true" />
          New terminal
        </button>
        {dialogs}
      </div>
    );
  }

  return (
    <div
      data-thread-terminal-drawer
      className="relative flex h-full min-h-0 min-w-0 flex-col overflow-hidden bg-(--op-term-bg)"
    >
      {!hasSidebar && (
        <div className="pointer-events-none absolute top-1 right-2 z-20">
          <div className="pointer-events-auto inline-flex items-center gap-0.5 rounded-pill border border-rule bg-surface px-1 py-0.5 shadow-pop">
            {toolbar}
          </div>
        </div>
      )}
      {error && (
        <div
          role="alert"
          className="flex shrink-0 items-center gap-2 border-b border-rule-soft bg-danger-soft px-3 py-1 text-[0.8rem] text-danger"
        >
          <span className="min-w-0 flex-1 truncate">{error}</span>
          <button
            type="button"
            className={BUTTON}
            aria-label="Dismiss"
            onClick={() => setError(null)}
          >
            <X size={12} aria-hidden="true" />
          </button>
        </div>
      )}
      <div className={cx("flex min-h-0 flex-1", hasSidebar && "gap-1.5")}>
        <div className="min-w-0 flex-1">
          {visibleTerminalIds.length > 1 ? (
            <div
              className="grid h-full w-full min-w-0 overflow-hidden"
              style={
                splitDirection === "vertical"
                  ? { gridTemplateRows: `repeat(${visibleTerminalIds.length}, minmax(0, 1fr))` }
                  : {
                      gridTemplateColumns: `repeat(${visibleTerminalIds.length}, minmax(0, 1fr))`,
                    }
              }
            >
              {visibleTerminalIds.map((terminalId) => (
                // oxlint-disable-next-line jsx-a11y/no-static-element-interactions -- a pointer shortcut; the tab list is the keyboard path.
                <div
                  key={terminalId}
                  className={cx(
                    "min-h-0 min-w-0",
                    splitDirection === "vertical"
                      ? "border-t first:border-t-0"
                      : "border-l first:border-l-0",
                    terminalId === activeTerminalId ? "border-rule" : "border-rule-soft",
                  )}
                  onMouseDown={() => {
                    if (terminalId !== activeTerminalId) setActiveTerminal(threadId, terminalId);
                  }}
                >
                  {viewport(terminalId, terminalId === activeTerminalId)}
                </div>
              ))}
            </div>
          ) : (
            <div className="h-full">{viewport(activeTerminalId, true)}</div>
          )}
        </div>

        {hasSidebar && (
          <aside
            aria-label="Terminals"
            className="flex w-40 min-w-40 flex-col border-l border-rule-soft bg-surface"
          >
            <div className="flex h-7 shrink-0 items-center justify-end gap-0.5 border-b border-rule-soft px-1">
              {toolbar}
            </div>
            <div className="min-h-0 flex-1 overflow-y-auto p-1">
              {terminalGroups.map((group) => {
                const isGroupActive = group.terminalIds.includes(activeTerminalId);
                const groupActiveTerminalId = isGroupActive
                  ? activeTerminalId
                  : (group.terminalIds[0] ?? activeTerminalId);
                const count = group.terminalIds.length;
                const isSplitGroup = count > 1;
                const groupLabel = !isSplitGroup
                  ? "Single"
                  : group.splitDirection === "vertical"
                    ? "Stacked"
                    : "Side by side";
                const GroupIcon = !isSplitGroup
                  ? Square
                  : group.splitDirection === "vertical"
                    ? SquareSplitVertical
                    : SquareSplitHorizontal;
                return (
                  <div key={group.id} className="pb-0.5">
                    {showGroupHeaders && (
                      <button
                        type="button"
                        className={cx(
                          "flex h-6 w-full cursor-pointer items-center gap-1 rounded-sm border-0 px-1.5 text-[0.72rem]",
                          isGroupActive
                            ? "bg-raised text-ink"
                            : "bg-transparent text-muted hover:bg-raised hover:text-ink",
                        )}
                        onClick={() => setActiveTerminal(threadId, groupActiveTerminalId)}
                      >
                        <GroupIcon size={12} aria-hidden="true" />
                        <span className="min-w-0 flex-1 truncate text-left">{groupLabel}</span>
                        <span className="text-faint tabular-nums">{count}</span>
                      </button>
                    )}
                    <div className="flex flex-col gap-0.5">
                      {group.terminalIds.map((terminalId) => {
                        const isActive = terminalId === activeTerminalId;
                        const label = tabs[terminalId]?.label ?? "Terminal";
                        return (
                          <div
                            key={terminalId}
                            className={cx(
                              "group/tab flex h-6 w-full items-center gap-1 rounded-sm pr-1 pl-1.5 text-[0.78rem]",
                              isActive
                                ? "bg-raised text-ink"
                                : "text-muted hover:bg-raised hover:text-ink",
                            )}
                          >
                            <TerminalSquare size={12} aria-hidden="true" />
                            <button
                              type="button"
                              aria-current={isActive ? "true" : undefined}
                              className="min-w-0 flex-1 cursor-pointer truncate border-0 bg-transparent p-0 text-left text-inherit"
                              onClick={() => setActiveTerminal(threadId, terminalId)}
                            >
                              {label}
                            </button>
                            <button
                              type="button"
                              className={cx(BUTTON, "size-5")}
                              aria-label={`Close ${label}`}
                              onClick={() => requestClose(terminalId)}
                            >
                              <X size={11} aria-hidden="true" />
                            </button>
                          </div>
                        );
                      })}
                    </div>
                  </div>
                );
              })}
            </div>
          </aside>
        )}
      </div>
      {dialogs}
    </div>
  );
}
