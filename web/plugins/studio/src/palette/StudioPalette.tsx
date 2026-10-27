// Studio's contribution to the shell's palette (SD1 Task 24; §53 §5.10; inventory (h),
// T3 Code's CommandPalette.tsx rewritten): a `palette.command` entry, mounted only while
// the palette is open, that publishes Studio's rows for the query:
//
// - Actions: the Studio commands something handles right now (new thread, the panels,
//   the terminal, settings), each with its keybinding;
// - Git: the open thread's actions (commit, push, pull request, merge);
// - Threads: the 12 most recent for an empty query, all of them for a query;
// - Files: quick open in the open thread's worktree, for a query;
// - Projects and Run groups;
// - Harness: this device's default harness and model for new threads.
//
// "Switch to Console" is the shell's own row, as it is in both modes. Agentd data comes
// through `StudioPaletteSource` (Task 6 fills it); projects through `useProjects`.

import type { RouterService } from '@loams/console-host';
import { AgentdScope } from '@loams/console-host/query';
import type { PaletteCommand, SlotProps } from '@loams/slots';
import {
  buildFileItems,
  buildProjectActionItems,
  buildRunGroupItems,
  buildThreadActionItems,
  commandLabel,
  type KeybindingCommand,
  RECENT_THREAD_LIMIT,
  STUDIO_KEYBINDING_COMMANDS,
  type StudioPaletteItem,
  shortcutKeysForCommand,
} from '@loams/studio-ui';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useEffect, useSyncExternalStore } from 'react';
import type { StudioAgentd } from '../agentd.js';
import { useProjects, useReach } from '../projects/queries.js';
import { studioHref } from '../routes.js';
import {
  HARNESSES,
  readSettings,
  type StudioSettings,
  writeSettings,
} from '../settings/Settings.js';
import type { StudioKeys } from './commands.js';
import type { StudioPaletteSource } from './source.js';

/** The Studio commands the palette offers when something handles them. */
const PALETTE_COMMANDS: readonly KeybindingCommand[] = STUDIO_KEYBINDING_COMMANDS.filter(
  (c) => c !== 'navigation.back' && c !== 'navigation.forward' && !c.startsWith('thread.jump.'),
);

const COMMAND_TITLES: Partial<Record<KeybindingCommand, string>> = {
  'chat.new': 'New thread',
  'settings.open': 'Studio settings',
  'sidebar.toggle': 'Show or hide the sidebar',
  'rightPanel.toggle': 'Show or hide the panel',
  'terminal.toggle': 'Show or hide the terminal',
  'diff.toggle': 'Show the diff',
  'files.toggle': 'Show the files',
  'preview.toggle': 'Show the preview',
  'thread.previous': 'Previous thread',
  'thread.next': 'Next thread',
};

function toCommand(item: StudioPaletteItem, keys: StudioKeys, threadOpen: boolean): PaletteCommand {
  const shortcut = item.shortcutCommand
    ? shortcutKeysForCommand(keys.keybindings.shown(), item.shortcutCommand, {
        platform: keys.platform,
        context: {
          threadOpen,
          isDesktop: keys.edition === 'desktop',
          isWeb: keys.edition === 'web',
        },
      })
    : null;
  return {
    id: item.id,
    title: item.title,
    group: item.group,
    ...(item.detail ? { detail: item.detail } : {}),
    ...(item.keywords ? { keywords: item.keywords.filter(Boolean) } : {}),
    ...(item.recency !== undefined ? { recency: item.recency } : {}),
    ...(item.secondary ? { secondary: true } : {}),
    ...(item.disabled ? { disabled: true } : {}),
    ...(shortcut ? { shortcut } : {}),
    run: item.run,
  };
}

function Palette({
  query,
  publish,
  agentd,
  source,
  router,
  keys,
  copy,
}: SlotProps<'palette.command'> & {
  agentd: StudioAgentd;
  source: StudioPaletteSource;
  router: Pick<RouterService, 'current' | 'subscribe' | 'navigate'>;
  keys: StudioKeys;
  /** Writes text to the clipboard (the platform's). */
  copy?: (text: string) => Promise<void>;
}) {
  const location = useSyncExternalStore(router.subscribe, () => router.current());
  useSyncExternalStore(keys.commands.subscribe, keys.commands.getVersion);
  useSyncExternalStore(keys.keybindings.subscribe, keys.keybindings.resolved);
  const ready = useReach(agentd).state === 'ready';
  const client = useQueryClient();
  const host = agentd.cacheKey();
  const threadId = location.path.startsWith('/studio/t/') ? location.params.threadId : undefined;
  const projectId = location.path.startsWith('/studio/p/') ? location.params.projectId : undefined;
  const q = query.trim();

  const projects = useProjects(agentd);
  const threads = useQuery({
    queryKey: ['studio', 'palette', 'threads', host],
    queryFn: () => source.threads(),
    enabled: ready,
  });
  const groups = useQuery({
    queryKey: ['studio', 'palette', 'groups', host],
    queryFn: () => source.runGroups(),
    enabled: ready,
  });
  const files = useQuery({
    queryKey: ['studio', 'palette', 'files', host, threadId, q],
    queryFn: () => source.searchFiles(threadId ?? '', q),
    enabled: ready && threadId !== undefined && q.length > 0,
  });
  const gitKey = ['studio', 'palette', 'git', host, threadId] as const;
  const git = useQuery({
    queryKey: gitKey,
    queryFn: () => source.gitActions(threadId ?? ''),
    enabled: ready && threadId !== undefined,
  });
  const harnesses = useQuery({
    queryKey: ['studio', 'palette', 'harnesses', host],
    queryFn: () => source.harnesses(),
  });

  const navigate = (to: string) => router.navigate(to);
  const projectTitleById = new Map((projects.data ?? []).map((p) => [p.id, p.name]));
  const settings = readSettings();
  const items: StudioPaletteItem[] = [];

  for (const command of PALETTE_COMMANDS) {
    if (!keys.commands.has(command)) continue;
    items.push({
      id: `command:${command}`,
      title: COMMAND_TITLES[command] ?? commandLabel(command),
      group: 'Actions',
      keywords: [commandLabel(command)],
      shortcutCommand: command,
      run: () => {
        keys.commands.run(command);
      },
    });
  }

  const project = projects.data?.find((p) => p.id === projectId);
  if (project && copy) {
    items.push({
      id: 'project:copy-path',
      title: 'Copy project path',
      group: 'Actions',
      detail: project.path,
      keywords: [project.name],
      run: () => copy(project.path),
    });
  }

  if (threadId) {
    for (const row of git.data ?? []) {
      items.push({
        id: `git:${row.action}`,
        title: row.label,
        group: 'Git',
        ...(row.disabledReason ? { detail: row.disabledReason, disabled: true } : {}),
        keywords: ['git', row.action],
        run: async () => {
          await source.runGitAction(threadId, row.action);
          await client.invalidateQueries({ queryKey: gitKey });
        },
      });
    }
  }

  items.push(
    ...buildThreadActionItems({
      threads: threads.data ?? [],
      activeThreadId: threadId,
      projectTitleById,
      // Upstream: an empty query lists recent threads; a query searches them all.
      ...(q ? {} : { limit: RECENT_THREAD_LIMIT }),
      open: (thread) => navigate(studioHref({ kind: 'thread', threadId: thread.id })),
    }),
  );

  if (threadId) {
    items.push(
      ...buildFileItems({
        paths: files.data ?? [],
        open: (path) =>
          navigate(
            `${studioHref({ kind: 'thread', threadId, panel: 'files' })}&path=${encodeURIComponent(path)}`,
          ),
      }),
    );
  }

  items.push(
    ...buildProjectActionItems({
      projects: projects.data ?? [],
      open: (p) => navigate(studioHref({ kind: 'project', projectId: p.id })),
    }),
    ...buildRunGroupItems({
      groups: groups.data ?? [],
      projectTitleById,
      open: (g) => navigate(studioHref({ kind: 'group', groupId: g.id })),
    }),
  );

  const known = (id: string): id is StudioSettings['harness'] =>
    HARNESSES.some((h) => h.value === id);
  for (const harness of harnesses.data ?? []) {
    if (!known(harness.id)) continue;
    const current = settings.harness === harness.id;
    items.push({
      id: `harness:${harness.id}`,
      title: `Use ${harness.label} for new threads`,
      group: 'Harness and model',
      ...(current ? { detail: 'Current' } : {}),
      keywords: ['harness', harness.label],
      secondary: true,
      run: () => {
        writeSettings({ harness: harness.id as StudioSettings['harness'] });
      },
    });
    if (!current) continue;
    for (const model of harness.models) {
      items.push({
        id: `model:${harness.id}:${model}`,
        title: `Use model ${model}`,
        group: 'Harness and model',
        ...(settings.model === model ? { detail: 'Current' } : {}),
        keywords: ['model', harness.label],
        secondary: true,
        run: () => {
          writeSettings({ model });
        },
      });
    }
    if (settings.model) {
      items.push({
        id: `model:${harness.id}:default`,
        title: `Use ${harness.label}'s default model`,
        group: 'Harness and model',
        keywords: ['model'],
        secondary: true,
        run: () => {
          writeSettings({ model: '' });
        },
      });
    }
  }

  const commands = items.map((item) => toCommand(item, keys, threadId !== undefined));
  useEffect(() => publish(commands));
  return null;
}

/** Studio's palette, reading projects over `agentd`'s transport (SD1 Task 6). */
export function StudioPalette(props: Parameters<typeof Palette>[0]) {
  return (
    <AgentdScope agentd={props.agentd}>
      <Palette {...props} />
    </AgentdScope>
  );
}
