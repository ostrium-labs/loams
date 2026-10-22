// The thread's terminal drawer (SD1 Task 22; §53 §5.7, D920): @loams/studio-ui's
// drawer over the daemon's `loams.agentd.v1.TerminalService`, reached through the
// console's agentd transport (SD1 Task 6) from the nearest `AgentdScope`. It is the
// layout's bottom drawer, shown and hidden with `terminal.toggle` (Ctrl+`, T24-4).
// - One TerminalSessions registry per transport, so a terminal's buffer and last seq
//   outlive the drawer and reattach with the daemon's replay.
// - http(s) links go to the platform's openExternal; file paths to the thread's Files
//   panel (`?panel=files&path=…&line=…`); selections to the composer's waiting list.
// - Key presses Studio binds (in the terminal's focus) and the shell's own chords
//   (Mod+K, Mod+J, Mod+Shift+M) pass through xterm to their handlers.

import { createClient, type Transport } from '@connectrpc/connect';
import { skipToken } from '@connectrpc/connect-query';
import { useAgentdQuery, useAgentdTransport } from '@loams/console-host/query';
import { ProjectService, TerminalService } from '@loams/proto/agentd';
import {
  type FileLinkTarget,
  isMacPlatform,
  matchesKeybindingShortcut,
  parseKeybindingShortcut,
  resolveShortcutCommand,
  SHELL_KEYBINDINGS,
  type TerminalBackend,
  TerminalDrawer,
  type TerminalDrawerProps,
  type TerminalProjectAction,
  TerminalSessions,
} from '@loams/studio-ui';
import { Empty } from '@loams/ui';
import { useCallback, useMemo } from 'react';
import type { StudioKeys } from '../palette/commands.js';
import { studioHref } from '../routes.js';
import { addTerminalContext } from './terminalContexts.js';

/** `TerminalBackend` over a daemon's TerminalService. */
export function agentdTerminalBackend(transport: Transport): TerminalBackend {
  const client = createClient(TerminalService, transport);
  return {
    async open({ chatId, cols, rows, cwd }) {
      const { terminal } = await client.openTerminal({
        chatId,
        cols,
        rows,
        ...(cwd ? { cwd } : {}),
      });
      if (!terminal) throw new Error('The daemon did not open a terminal.');
      return { id: terminal.id, cwd: terminal.cwd, shell: terminal.shell };
    },
    async *subscribe(terminalId, afterSeq, signal) {
      const stream = client.subscribeTerminal(
        { terminalId, ...(afterSeq !== undefined ? { afterSeq: BigInt(afterSeq) } : {}) },
        { signal },
      );
      for await (const { event } of stream) {
        const item = event?.event;
        if (item?.case === 'data') {
          yield { type: 'data', seq: Number(item.value.seq), data: item.value.data };
        } else if (item?.case === 'exit') {
          yield {
            type: 'exit',
            seq: Number(item.value.seq),
            exitCode: item.value.exitCode,
            ...(item.value.signal ? { signal: item.value.signal } : {}),
          };
        }
      }
    },
    async write(terminalId, data) {
      await client.writeTerminal({ terminalId, data });
    },
    async resize(terminalId, cols, rows) {
      await client.resizeTerminal({ terminalId, cols, rows });
    },
    async close(terminalId) {
      await client.closeTerminal({ terminalId });
    },
  };
}

const registries = new WeakMap<Transport, TerminalSessions>();

/** The terminal sessions of a daemon connection: one registry per transport. */
export function terminalSessionsFor(transport: Transport): TerminalSessions {
  let sessions = registries.get(transport);
  if (!sessions) {
    sessions = new TerminalSessions(agentdTerminalBackend(transport));
    registries.set(transport, sessions);
  }
  return sessions;
}

const SHELL_SHORTCUTS = SHELL_KEYBINDINGS.flatMap((b) => {
  const shortcut = parseKeybindingShortcut(b.key);
  return shortcut ? [{ shortcut, terminal: b.terminal === true }] : [];
});

/**
 * Whether a key press in a terminal belongs to Studio (a binding that applies with the
 * terminal focused and has a handler now) or to the shell (its own chords). On Linux and
 * Windows the shell program keeps Ctrl+letter (Ctrl+K, J, B, N, I…), so only the chords
 * the shell takes from a terminal there pass (D1081, Review Focus 1); on macOS every ⌘
 * chord passes, since ⌘ never reaches the program.
 */
export function isStudioShortcut(keys: StudioKeys, event: KeyboardEvent): boolean {
  const command = resolveShortcutCommand(event, keys.keybindings.resolved(), {
    platform: keys.platform,
    context: {
      terminalFocus: true,
      threadOpen: true,
      isDesktop: keys.edition === 'desktop',
      isWeb: keys.edition === 'web',
    },
  });
  if (command && keys.commands.has(command)) return true;
  const mac = isMacPlatform(keys.platform);
  return SHELL_SHORTCUTS.some(
    (s) => (mac || s.terminal) && matchesKeybindingShortcut(event, s.shortcut, keys.platform),
  );
}

/** The Files panel's href for a path a terminal printed. */
export function fileViewerHref(threadId: string, target: FileLinkTarget): string {
  const q = new URLSearchParams({ path: target.path });
  if (target.line !== undefined) q.set('line', String(target.line));
  if (target.column !== undefined) q.set('column', String(target.column));
  return `${studioHref({ kind: 'thread', threadId, panel: 'files' })}&${q}`;
}

function isHttpUrl(url: string): boolean {
  try {
    const { protocol } = new URL(url);
    return protocol === 'http:' || protocol === 'https:';
  } catch {
    return false;
  }
}

function ProjectActionsDrawer(
  props: Omit<TerminalDrawerProps, 'actions' | 'onRunAction'> & {
    transport: Transport;
    spaceId: string | undefined;
  },
) {
  const { transport, spaceId, ...rest } = props;
  const listed = useAgentdQuery(
    ProjectService.method.listProjectActions,
    spaceId ? { spaceId } : skipToken,
  );
  const actions: TerminalProjectAction[] | undefined = listed.data?.snapshot?.actions.map((a) => ({
    id: a.id,
    name: a.name,
  }));
  const projects = useMemo(() => createClient(ProjectService, transport), [transport]);
  const onRunAction = useCallback(
    async (actionId: string, size: { cols: number; rows: number }) => {
      if (!spaceId) throw new Error('This thread has no project.');
      const { run } = await projects.runProjectAction({
        spaceId,
        chatId: rest.threadId,
        actionId,
        ...size,
      });
      if (!run?.terminal) throw new Error('The action did not start a terminal.');
      const { id, cwd, shell } = run.terminal;
      return { terminal: { id, cwd, shell }, name: run.actionName };
    },
    [projects, rest.threadId, spaceId],
  );
  return <TerminalDrawer {...rest} {...(actions && spaceId ? { actions, onRunAction } : {})} />;
}

export function ThreadTerminalDrawer({
  threadId,
  keys,
  navigate,
  openExternal,
  spaceId,
  cwd,
  createSurface,
}: {
  threadId: string;
  keys: StudioKeys;
  navigate: (to: string) => void;
  /** The platform's external opener; a new browser tab without one. */
  openExternal?: (url: string) => Promise<void> | void;
  /** The thread's project (agentd space), for its project actions; none hides them. */
  spaceId?: string;
  cwd?: string;
  /** Tests: a stand-in for xterm.js. */
  createSurface?: TerminalDrawerProps['createSurface'];
}) {
  const transport = useAgentdTransport();
  const sessions = useMemo(() => (transport ? terminalSessionsFor(transport) : null), [transport]);
  const isAppShortcut = useCallback(
    (event: KeyboardEvent) => isStudioShortcut(keys, event),
    [keys],
  );
  const onOpenUrl = useCallback(
    (url: string) => {
      if (!isHttpUrl(url)) return;
      if (openExternal) void Promise.resolve(openExternal(url)).catch(() => {});
      else window.open(url, '_blank', 'noopener,noreferrer');
    },
    [openExternal],
  );
  const onOpenFile = useCallback(
    (target: FileLinkTarget) => navigate(fileViewerHref(threadId, target)),
    [navigate, threadId],
  );
  const onSendSelection = useCallback(
    (selection: Parameters<typeof addTerminalContext>[1]) =>
      addTerminalContext(threadId, selection),
    [threadId],
  );
  if (!transport || !sessions) {
    return (
      <div className="p-3">
        <Empty title="No terminals" seed={5}>
          Terminals open here once Studio reaches loams-agentd.
        </Empty>
      </div>
    );
  }
  return (
    <ProjectActionsDrawer
      transport={transport}
      spaceId={spaceId}
      threadId={threadId}
      sessions={sessions}
      {...(cwd ? { cwd } : {})}
      platform={keys.platform}
      onOpenUrl={onOpenUrl}
      onOpenFile={onOpenFile}
      onSendSelection={onSendSelection}
      isAppShortcut={isAppShortcut}
      {...(createSurface ? { createSurface } : {})}
    />
  );
}
