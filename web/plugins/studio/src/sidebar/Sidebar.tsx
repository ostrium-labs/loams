// Studio's sidebar (SD1 Task 17; §53 §5.3; inventory (a): T3 Code's components/Sidebar.tsx
// rewritten, using it as the reference). Projects with their threads and run groups:
//
// - each project's pinned block (agentd's order; drag to reorder, or Move up and Move down
//   in the thread's menu), its run groups with their members, then the rest, newest first;
// - status badges (pending approval, awaiting input, working with its time, errored, an
//   unread completion), the worktree a thread runs in, and its run group;
// - search by title or branch; archived threads are in Studio's settings (design 59, D1079);
// - in the shell's sidebar (design 59, D1078): 28px rows, and each project shows its first
//   `THREADS_SHOWN` unpinned threads until "Show n more" (Review Focus 2);
// - the thread menu (right-click, or the row's "More" button): pin, move, rename, fork,
//   copy, archive, and delete, which offers to remove the thread's worktree only when no
//   other thread uses it (`getOrphanedWorktreePathForThread`); and "Clean up worktrees";
// - the keyboard: ↑/↓ (Home, End) move between rows, Enter opens one, F2 renames,
//   Mod+N starts a thread (the view's `chat.new`), Mod+Shift+[ and ] go to the previous
//   and next thread, and Mod+1…9 jump to the first nine (desktop; hints show while the
//   modifier is held, upstream's shortcutModifierState).
//
// Data comes from sidebar/data.ts (agentd's watches) and actions from useThreadActions.ts.

import {
  closestCenter,
  DndContext,
  type DragEndEvent,
  PointerSensor,
  useSensor,
  useSensors,
} from '@dnd-kit/core';
import { SortableContext, useSortable, verticalListSortingStrategy } from '@dnd-kit/sortable';
import { CSS } from '@dnd-kit/utilities';
import {
  buildSidebarModel,
  buildThreadActionMenuItems,
  formatWorkingDurationLabel,
  formatWorktreePathForDisplay,
  getOrphanedWorktreePathForThread,
  isTrailingDoubleClick,
  type ProjectWorktree,
  planPinnedMove,
  planSidebarThreadDrop,
  RunGroupBadge,
  resolveAdjacentThreadId,
  resolveSidebarDropTarget,
  resolveSidebarRowAccessibility,
  resolveThreadStatusPill,
  type SidebarGroupRows,
  type SidebarListItem,
  type SidebarProjectRows,
  type SidebarRunGroup,
  type SidebarThread,
  shortcutLabelForCommand,
  shouldShowThreadJumpHintsForModifiers,
  sidebarListItemId,
  sidebarMarkerId,
  THREAD_JUMP_KEYBINDING_COMMANDS,
  type ThreadActionMenuId,
  ThreadStatusLabel,
  ThreadWorktreeIndicator,
  threadJumpCommandForIndex,
  useShortcutModifierState,
  useThreadJumpHintVisibility,
} from '@loams/studio-ui';
import {
  AlertDialog,
  Button,
  Checkbox,
  ConfirmDialog,
  cx,
  Dialog,
  Field,
  Input,
  InputGroup,
  InputGroupAddon,
  InputGroupInput,
  Menu,
  MenuItem,
  MenuSeparator,
} from '@loams/ui';
import {
  ChevronDown,
  ChevronRight,
  FolderGit2,
  Layers,
  MoreHorizontal,
  Pin,
  Plus,
  Search,
  Trash2,
} from 'lucide-react';
import {
  type CSSProperties,
  type FormEvent,
  type KeyboardEvent,
  type ReactNode,
  type Ref,
  useEffect,
  useMemo,
  useRef,
  useState,
} from 'react';
import type { StudioAgentd } from '../agentd.js';
import { StudioCommands, type StudioKeys, useStudioCommand } from '../palette/commands.js';
import { CloneDialog } from '../projects/ProjectsSidebar.js';
import { useAddProject, useProjects, useReach } from '../projects/queries.js';
import { type StudioRoute, studioHref } from '../routes.js';
import { useSidebarData } from './data.js';
import { type ThreadActions, useThreadActions, type WorktreeTarget } from './useThreadActions.js';

/** One sidebar row (design 59, D1076): 28px, one line, the selected row a chip. */
const ROW =
  'flex h-7 min-w-0 flex-1 items-center gap-2 rounded-md px-2 text-small text-muted no-underline ' +
  'outline-none hover:bg-row-hover hover:text-ink focus-visible:outline-2 focus-visible:outline-offset-2 ' +
  'focus-visible:outline-accent aria-[current=page]:bg-row-selected aria-[current=page]:text-ink ' +
  'aria-[current=page]:shadow-chip';

/** A project's unpinned threads shown before "Show n more" (Review Focus 2). */
export const THREADS_SHOWN = 10;

const ICON_BUTTON =
  'inline-grid size-6 shrink-0 cursor-pointer place-items-center rounded-sm border-0 bg-transparent ' +
  'text-faint hover:bg-raised hover:text-ink focus-visible:outline-2 focus-visible:outline-accent';

/** The project a thread's path is relative to, for "Copy path" and worktree clean-up. */
interface ProjectRef {
  id: string;
  name: string;
  path: string;
  branch: string | null;
}

/** Self-ticking, so only this span re-renders each second (upstream's WorkingDuration). */
function WorkingDuration({ since }: { since: number }) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, []);
  return (
    <span className="shrink-0 tabular-nums text-micro text-faint">
      {formatWorkingDurationLabel(now - since)}
    </span>
  );
}

/** The jump hint over a row while the jump modifier is held; it never takes clicks. */
function JumpHint({ label }: { label: string }) {
  return (
    <span
      aria-hidden="true"
      className={
        /* mono-ok: code */ 'pointer-events-none absolute top-1/2 right-8 z-10 inline-flex h-5 -translate-y-1/2 items-center rounded-pill border border-rule bg-bg px-1.5 font-mono text-[0.66rem] text-ink shadow-pop'
      }
    >
      {label}
    </span>
  );
}

interface RowProps {
  thread: SidebarThread;
  project: ProjectRef | null;
  active: boolean;
  pinned: boolean;
  jumpLabel: string | null;
  group: SidebarRunGroup | undefined;
  menu: (thread: SidebarThread) => ReactNode;
  onRename: (thread: SidebarThread) => void;
  navigate: (to: string) => void;
  /** Sortable rows: dnd-kit's node, transform and listeners. */
  sortable?: {
    setNodeRef: Ref<HTMLLIElement>;
    style: CSSProperties;
    listeners: Record<string, unknown> | undefined;
    dragging: boolean;
  };
}

function ThreadRow({
  thread,
  project,
  active,
  pinned,
  jumpLabel,
  group,
  menu,
  onRename,
  navigate,
  sortable,
}: RowProps) {
  const status = resolveThreadStatusPill({ thread });
  const unread = status?.label === 'Completed';
  const a11y = resolveSidebarRowAccessibility({
    title: thread.title,
    statusLabel: status?.label ?? null,
    projectDisplayName: null,
    isActive: active,
  });
  const href = studioHref({ kind: 'thread', threadId: thread.id });
  const onKeyDown = (e: KeyboardEvent<HTMLAnchorElement>) => {
    if (e.key === 'Enter') {
      e.preventDefault();
      navigate(href);
    } else if (e.key === 'F2') {
      e.preventDefault();
      onRename(thread);
    }
  };
  return (
    <li
      ref={sortable?.setNodeRef}
      style={sortable?.style}
      {...(sortable?.listeners ?? {})}
      data-thread-item={thread.id}
      data-dragging={sortable?.dragging || undefined}
      className={cx(
        'group/thread relative flex min-w-0 items-center',
        sortable?.dragging && 'z-20 bg-surface shadow-pop',
        thread.archived && 'opacity-60',
      )}
      onContextMenu={(e) => {
        e.preventDefault();
        e.currentTarget.querySelector<HTMLButtonElement>('[data-thread-more]')?.click();
      }}
    >
      <a
        href={`#${href}`}
        data-row-link=""
        aria-label={a11y.label}
        aria-current={a11y.current}
        title={project ? `${thread.title} · ${project.name}` : thread.title}
        className={cx(ROW, 'pr-7', unread && 'text-ink')}
        onKeyDown={onKeyDown}
        onClick={(e) => {
          if (isTrailingDoubleClick(e.detail)) e.preventDefault();
        }}
        onDoubleClick={(e) => {
          e.preventDefault();
          onRename(thread);
        }}
      >
        {status ? (
          <ThreadStatusLabel status={status} compact />
        ) : pinned ? (
          <Pin aria-hidden="true" className="size-3 shrink-0 text-faint" />
        ) : (
          <span aria-hidden="true" className="size-3.5 shrink-0" />
        )}
        <span className={cx('min-w-0 flex-1 truncate', unread && 'font-[560]')}>
          {thread.title}
        </span>
        <ThreadWorktreeIndicator thread={thread} />
        {group && <RunGroupBadge title={group.title} />}
        {thread.status === 'working' && thread.statusSince !== null && (
          <WorkingDuration since={thread.statusSince} />
        )}
      </a>
      {jumpLabel && <JumpHint label={jumpLabel} />}
      <div className="absolute top-1/2 right-1 -translate-y-1/2 opacity-0 group-hover/thread:opacity-100 group-focus-within/thread:opacity-100 has-[[aria-expanded=true]]:opacity-100">
        {menu(thread)}
      </div>
    </li>
  );
}

function SortableThreadRow(props: Omit<RowProps, 'sortable'>) {
  const { setNodeRef, transform, transition, listeners, isDragging } = useSortable({
    id: props.thread.id,
  });
  return (
    <ThreadRow
      {...props}
      sortable={{
        setNodeRef,
        style: { transform: CSS.Translate.toString(transform), transition },
        listeners: listeners as Record<string, unknown> | undefined,
        dragging: isDragging,
      }}
    />
  );
}

/** A marker row in the sortable list: the pinned block's top and its divider. */
function Marker({ id, children }: { id: string; children?: ReactNode }) {
  const { setNodeRef } = useSortable({ id, disabled: true });
  return (
    <li ref={setNodeRef} aria-hidden={children ? undefined : true} className="list-none">
      {children}
    </li>
  );
}

function GroupRow({
  rows,
  rowProps,
}: {
  rows: SidebarGroupRows;
  rowProps: (thread: SidebarThread) => Omit<RowProps, 'thread' | 'sortable'>;
}) {
  const { group, members } = rows;
  return (
    <li className="list-none">
      <a
        href={`#${studioHref({ kind: 'group', groupId: group.id })}`}
        data-row-link=""
        className={ROW}
        title={group.title}
      >
        <Layers aria-hidden="true" className="size-3.5 shrink-0 text-info" />
        <span className="min-w-0 flex-1 truncate">{group.title}</span>
        <span className="shrink-0 tabular-nums text-micro text-faint">
          {group.memberIds.length} runs
        </span>
      </a>
      <ul className="m-0 ms-3.5 flex list-none flex-col gap-px border-l border-rule-soft p-0 ps-1.5">
        {members.map((t) => (
          <ThreadRow key={t.id} thread={t} {...rowProps(t)} />
        ))}
      </ul>
    </li>
  );
}

function ProjectSection({
  rows,
  collapsed,
  onToggle,
  activeProjectId,
  rowProps,
  actions,
  threadsKnown,
  onNewThread,
}: {
  rows: SidebarProjectRows;
  collapsed: boolean;
  onToggle: () => void;
  activeProjectId: string | undefined;
  rowProps: (thread: SidebarThread) => Omit<RowProps, 'thread' | 'sortable'>;
  actions: ThreadActions;
  /** The threads have arrived, so an empty project really has none. */
  threadsKnown: boolean;
  onNewThread?: () => void;
}) {
  const { project, pinned, groups, threads: all, status } = rows;
  const [expanded, setExpanded] = useState(false);
  // The first THREADS_SHOWN, and the open thread wherever it is in the list.
  const threads = expanded ? all : all.filter((t, i) => i < THREADS_SHOWN || rowProps(t).active);
  const more = all.length - threads.length;
  const sensors = useSensors(useSensor(PointerSensor, { activationConstraint: { distance: 4 } }));
  const pinnedOrder = pinned.map((t) => t.id);
  const items: SidebarListItem[] = [
    { kind: 'marker', marker: 'pinned-header' },
    ...pinned.map((t) => ({ kind: 'thread' as const, key: t.id, section: 'pinned' as const })),
    { kind: 'marker', marker: 'pinned-divider' },
    ...threads.map((t) => ({ kind: 'thread' as const, key: t.id, section: 'active' as const })),
  ];
  const onDragEnd = (e: DragEndEvent) => {
    if (!e.over) return;
    const activeKey = String(e.active.id);
    const target = resolveSidebarDropTarget(items, activeKey, String(e.over.id));
    if (!target) return;
    const plan = planSidebarThreadDrop({
      activeKey,
      activeSection: pinnedOrder.includes(activeKey) ? 'pinned' : 'active',
      target,
      pinnedOrder,
    });
    if (plan.kind === 'pin') void actions.pin(activeKey, plan.position);
    else if (plan.kind === 'reorder-pinned') void actions.reorder(activeKey, plan.position);
    else if (plan.kind === 'unpin') void actions.unpin(activeKey);
  };
  const empty = pinned.length + groups.length + threads.length === 0;
  const name = project?.name ?? 'Other threads';
  const listId = `studio-threads-${project?.id ?? 'other'}`;

  return (
    <li className="list-none" data-project={project?.id ?? ''}>
      <div className="group/project flex min-w-0 items-center gap-0.5 pr-1">
        <button
          type="button"
          className={ICON_BUTTON}
          aria-expanded={!collapsed}
          aria-controls={listId}
          aria-label={collapsed ? `Show ${name}'s threads` : `Hide ${name}'s threads`}
          onClick={onToggle}
        >
          {collapsed ? (
            <ChevronRight aria-hidden="true" className="size-3.5" />
          ) : (
            <ChevronDown aria-hidden="true" className="size-3.5" />
          )}
        </button>
        {project ? (
          <a
            href={`#${studioHref({ kind: 'project', projectId: project.id })}`}
            data-row-link=""
            aria-current={project.id === activeProjectId ? 'page' : undefined}
            title={project.path}
            className={cx(ROW, 'px-1.5 font-[540] text-ink')}
          >
            <FolderGit2 size={15} strokeWidth={1.75} aria-hidden="true" className="shrink-0" />
            <span className="min-w-0 flex-1 truncate">{project.name}</span>
            {status && <ThreadStatusLabel status={status} compact />}
            {project.branch && (
              <span
                className={
                  /* mono-ok: id */ 'shrink-0 font-mono text-[0.68rem] font-normal text-faint'
                }
              >
                {project.branch}
              </span>
            )}
          </a>
        ) : (
          <span className={cx(ROW, 'px-1.5 font-[540] hover:bg-transparent')}>{name}</span>
        )}
        {onNewThread && (
          <button
            type="button"
            className={ICON_BUTTON}
            aria-label={`New thread in ${name}`}
            title="New thread"
            onClick={onNewThread}
          >
            <Plus aria-hidden="true" className="size-3.5" />
          </button>
        )}
      </div>
      {!collapsed && (
        <ul
          id={listId}
          aria-label={`${name} threads`}
          className="m-0 flex list-none flex-col gap-px p-0 ps-5"
        >
          {empty && threadsKnown && <li className="px-2 py-1 text-small text-faint">No threads</li>}
          <DndContext sensors={sensors} collisionDetection={closestCenter} onDragEnd={onDragEnd}>
            <SortableContext
              items={items.map(sidebarListItemId)}
              strategy={verticalListSortingStrategy}
            >
              <Marker id={sidebarMarkerId('pinned-header')}>
                {pinned.length > 0 && (
                  <span className="block px-2 pt-1 text-micro font-medium text-faint">Pinned</span>
                )}
              </Marker>
              {pinned.map((t) => (
                <SortableThreadRow key={t.id} thread={t} {...rowProps(t)} />
              ))}
              <Marker id={sidebarMarkerId('pinned-divider')}>
                {pinned.length > 0 && <span className="mx-2 my-1 block h-px bg-rule-soft" />}
              </Marker>
              {groups.map((g) => (
                <GroupRow key={g.group.id} rows={g} rowProps={rowProps} />
              ))}
              {threads.map((t) => (
                <SortableThreadRow key={t.id} thread={t} {...rowProps(t)} />
              ))}
            </SortableContext>
          </DndContext>
          {more > 0 && (
            <li className="flex list-none">
              <button
                type="button"
                className={cx(
                  ROW,
                  'cursor-pointer border-0 bg-transparent text-left font-[inherit] text-faint',
                )}
                onClick={() => setExpanded(true)}
              >
                <span aria-hidden="true" className="size-3.5 shrink-0" />
                Show {more} more
              </button>
            </li>
          )}
        </ul>
      )}
    </li>
  );
}

function RenameDialog({
  thread,
  onClose,
  onRename,
}: {
  thread: SidebarThread | null;
  onClose: () => void;
  onRename: (title: string) => void;
}) {
  // Keyed by the thread (the parent's `key`), so each opens with its own title.
  const [title, setTitle] = useState(thread?.title ?? '');
  const submit = (e: FormEvent) => {
    e.preventDefault();
    const value = title.trim();
    if (value) onRename(value.slice(0, 200));
  };
  return (
    <Dialog
      open={thread !== null}
      onClose={onClose}
      title="Rename thread"
      footer={
        <>
          <Button variant="quiet" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" type="submit" form="studio-rename-thread">
            Rename
          </Button>
        </>
      }
    >
      <form id="studio-rename-thread" onSubmit={submit}>
        <Field label="Title">
          {(props) => (
            <Input
              {...props}
              value={title}
              onChange={(e) => setTitle(e.target.value)}
              autoComplete="off"
              maxLength={200}
            />
          )}
        </Field>
      </form>
    </Dialog>
  );
}

function DeleteDialog({
  thread,
  worktree,
  onClose,
  onDelete,
  pending,
}: {
  thread: SidebarThread | null;
  /** The thread's worktree when no other thread uses it: the only case to offer removing it. */
  worktree: WorktreeTarget | null;
  onClose: () => void;
  onDelete: (removeWorktree: boolean) => void;
  pending: boolean;
}) {
  // Keyed by the thread (the parent's `key`), so each prompt starts unchecked.
  const [removeWorktree, setRemoveWorktree] = useState(false);
  return (
    <ConfirmDialog
      open={thread !== null}
      onClose={onClose}
      object={`"${thread?.title ?? ''}"`}
      confirm="Delete thread"
      pending={pending}
      onConfirm={() => onDelete(removeWorktree)}
      impact={
        <>
          Its conversation is deleted for good. Archive keeps it instead.
          {worktree && (
            <span className="mt-3 block">
              <Checkbox
                label={`Also remove its worktree, ${formatWorktreePathForDisplay(worktree.worktreePath)}`}
                checked={removeWorktree}
                onChange={(e) => setRemoveWorktree(e.target.checked)}
              />
            </span>
          )}
        </>
      }
    />
  );
}

/** Worktrees no thread uses any more, per project: the "Clean up worktrees" candidates. */
export function orphanedWorktrees(
  projects: readonly ProjectRef[],
  worktrees: ReadonlyMap<string, readonly ProjectWorktree[]>,
  threads: readonly SidebarThread[],
): Array<WorktreeTarget & { branch: string; dirty: boolean; project: string }> {
  const ids = new Set(threads.map((t) => t.id));
  const used = new Set(threads.map((t) => t.worktreePath).filter((p): p is string => !!p));
  const out: Array<WorktreeTarget & { branch: string; dirty: boolean; project: string }> = [];
  for (const project of projects) {
    for (const w of worktrees.get(project.id) ?? []) {
      if (w.main || used.has(w.path) || w.chatIds.some((id) => ids.has(id))) continue;
      out.push({
        repoPath: project.path,
        worktreePath: w.path,
        branch: w.branch,
        dirty: w.dirty,
        project: project.name,
      });
    }
  }
  return out;
}

function CleanupDialog({
  open,
  candidates,
  onClose,
  onRemove,
  pending,
}: {
  open: boolean;
  candidates: ReturnType<typeof orphanedWorktrees>;
  onClose: () => void;
  onRemove: (targets: WorktreeTarget[]) => void;
  pending: boolean;
}) {
  const removable = candidates.filter((c) => !c.dirty);
  return (
    <AlertDialog
      open={open}
      onClose={onClose}
      title="Clean up worktrees"
      confirm={
        removable.length
          ? `Remove ${removable.length} worktree${removable.length === 1 ? '' : 's'}`
          : 'Close'
      }
      tone={removable.length ? 'danger' : undefined}
      pending={pending}
      onConfirm={() => (removable.length ? onRemove(removable) : onClose())}
    >
      {candidates.length === 0 ? (
        'Every worktree is used by a thread.'
      ) : (
        <>
          No thread uses these worktrees. Ones with uncommitted changes are kept.
          <span className="mt-2 grid gap-1">
            {candidates.map((c) => (
              <span key={c.worktreePath} className="flex items-center gap-2 text-[0.84rem]">
                <FolderGit2 aria-hidden="true" className="size-3.5 shrink-0" />
                <span
                  className={/* mono-ok: path */ 'min-w-0 flex-1 truncate font-mono'}
                  title={c.worktreePath}
                >
                  {c.project} / {formatWorktreePathForDisplay(c.worktreePath)}
                  {c.branch ? ` (${c.branch})` : ''}
                </span>
                {c.dirty && <span className="shrink-0 text-accent-fg">has changes</span>}
              </span>
            ))}
          </span>
        </>
      )}
    </AlertDialog>
  );
}

/** Moves focus between the sidebar's rows (↑/↓, Home, End), as a tree's rows move. */
function onRowsKeyDown(e: KeyboardEvent<HTMLElement>) {
  if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(e.key)) return;
  const target = e.target as HTMLElement;
  if (target.closest('input, textarea, [role="menu"]')) return;
  const rows = [...e.currentTarget.querySelectorAll<HTMLElement>('[data-row-link]')];
  if (rows.length === 0) return;
  const at = rows.indexOf(target);
  const next =
    e.key === 'Home'
      ? 0
      : e.key === 'End'
        ? rows.length - 1
        : e.key === 'ArrowDown'
          ? Math.min(rows.length - 1, at + 1)
          : Math.max(0, at === -1 ? rows.length - 1 : at - 1);
  e.preventDefault();
  rows[next]?.focus();
}

export function StudioSidebar({
  agentd,
  route,
  navigate,
  keys,
  children,
}: {
  agentd: StudioAgentd;
  route: StudioRoute;
  navigate: (to: string) => void;
  /** Studio's commands and keybindings: next/previous and the thread jumps. */
  keys?: StudioKeys;
  /** Groups below the projects: the Factory group (SD1 Task 27). */
  children?: ReactNode;
}) {
  const reach = useReach(agentd);
  const ready = reach.state === 'ready';
  const projects = useProjects(agentd);
  const add = useAddProject();
  const [cloning, setCloning] = useState(false);
  const [data, applyPins] = useSidebarData(agentd, ready ? projects.data : undefined);
  const [query, setQuery] = useState('');
  const [collapsed, setCollapsed] = useState<ReadonlySet<string>>(new Set());
  const [renaming, setRenaming] = useState<SidebarThread | null>(null);
  const [deleting, setDeleting] = useState<SidebarThread | null>(null);
  const [cleaning, setCleaning] = useState(false);

  const activeThreadId = route.kind === 'thread' ? route.threadId : undefined;
  const activeThread = data.threads.find((t) => t.id === activeThreadId);
  // The project row is the current page only on its own route; on a thread, the thread is.
  const activeProjectId = route.kind === 'project' ? route.projectId : undefined;
  const projectRefs: ProjectRef[] = useMemo(
    () =>
      (projects.data ?? []).map((p) => ({
        id: p.id,
        name: p.name,
        path: p.path,
        branch: p.branch,
      })),
    [projects.data],
  );
  const projectById = useMemo(() => new Map(projectRefs.map((p) => [p.id, p])), [projectRefs]);
  const model = useMemo(
    () =>
      buildSidebarModel({
        projects: projectRefs,
        threads: data.threads,
        groups: data.groups,
        pinnedIds: data.pinnedIds,
        query,
        // Archived threads are in Studio's settings (settings/Archived.tsx, D1079).
        showArchived: false,
      }),
    [projectRefs, data.threads, data.groups, data.pinnedIds, query],
  );
  const groupById = useMemo(() => new Map(data.groups.map((g) => [g.id, g])), [data.groups]);
  const actions = useThreadActions({
    navigate,
    activeThreadId,
    threads: data.threads,
    applyPins,
  });

  // Opening a thread marks what it has said so far as seen (unread badges follow).
  const seen = useRef<string>('');
  useEffect(() => {
    const t = activeThread;
    if (!t || t.lastMessageAt === null) return;
    if (t.lastSeenAt !== null && t.lastSeenAt >= t.lastMessageAt) return;
    const mark = `${t.id}@${t.lastMessageAt}`;
    if (seen.current === mark) return;
    seen.current = mark;
    void actions.markSeen(t.id);
  }, [activeThread, actions]);

  // Next/previous and Mod+1…9 follow the visible order.
  const ordered = model.orderedThreadIds;
  const open = (id: string | null | undefined) => {
    if (id) navigate(studioHref({ kind: 'thread', threadId: id }));
  };
  const commands = keys?.commands;
  const bus = commands ?? null;
  const traverse = (direction: 'previous' | 'next') => () =>
    open(
      resolveAdjacentThreadId({
        threadIds: ordered,
        currentThreadId: activeThreadId ?? null,
        direction,
      }),
    );
  useCommand(bus, 'thread.previous', ready ? traverse('previous') : undefined);
  useCommand(bus, 'thread.next', ready ? traverse('next') : undefined);
  for (const [index, command] of THREAD_JUMP_KEYBINDING_COMMANDS.entries()) {
    // A fixed list, so the hooks are called in the same order every render.
    // oxlint-disable-next-line react/rules-of-hooks -- THREAD_JUMP_KEYBINDING_COMMANDS is constant.
    useCommand(bus, command, ready && ordered[index] ? () => open(ordered[index]) : undefined);
  }

  // Jump hints while the jump modifier is held (upstream's shortcutModifierState).
  const modifiers = useShortcutModifierState();
  const matchOptions = keys && {
    platform: keys.platform,
    context: { isDesktop: keys.edition === 'desktop', isWeb: keys.edition === 'web' },
  };
  const wantHints =
    keys !== undefined &&
    matchOptions !== undefined &&
    shouldShowThreadJumpHintsForModifiers(modifiers, keys.keybindings.shown(), matchOptions);
  const { showThreadJumpHints, updateThreadJumpHintsVisibility } = useThreadJumpHintVisibility();
  useEffect(
    () => updateThreadJumpHintsVisibility(wantHints),
    [wantHints, updateThreadJumpHintsVisibility],
  );
  const jumpLabels = useMemo(() => {
    const labels = new Map<string, string>();
    if (!keys || !showThreadJumpHints) return labels;
    const options = {
      platform: keys.platform,
      context: { isDesktop: keys.edition === 'desktop', isWeb: keys.edition === 'web' },
    };
    for (const [index, id] of ordered.entries()) {
      const label = shortcutLabelForCommand(
        keys.keybindings.shown(),
        threadJumpCommandForIndex(index),
        options,
      );
      if (!label) break;
      labels.set(id, label);
    }
    return labels;
  }, [keys, ordered, showThreadJumpHints]);

  const onAdd = async () => {
    if (agentd.pickFolder) {
      const path = await agentd.pickFolder();
      if (path) add.mutate({ path });
      return;
    }
    add.reset();
    setCloning(true);
  };

  const copy = (text: string) => {
    void globalThis.navigator?.clipboard?.writeText(text).catch(() => undefined);
  };

  const onMenu = (thread: SidebarThread, id: ThreadActionMenuId) => {
    const project = thread.projectId ? projectById.get(thread.projectId) : undefined;
    switch (id) {
      case 'pin':
        return void actions.pin(thread.id);
      case 'unpin':
        return void actions.unpin(thread.id);
      case 'move-up':
      case 'move-down': {
        const plan = planPinnedMove({
          orderedIds: model.pinnedOrder.filter(
            (pid) => data.threads.find((t) => t.id === pid)?.projectId === thread.projectId,
          ),
          movedId: thread.id,
          direction: id === 'move-up' ? 'up' : 'down',
        });
        if (plan) void actions.reorder(thread.id, plan.position);
        return;
      }
      case 'rename':
        return setRenaming(thread);
      case 'fork':
        return void actions.fork(thread.id);
      case 'copy-path':
        return copy(thread.worktreePath ?? thread.cwd ?? project?.path ?? '');
      case 'copy-branch':
        return copy(thread.branch ?? '');
      case 'copy-thread-id':
        return copy(thread.id);
      case 'archive':
        return void actions.setArchived(thread.id, true);
      case 'unarchive':
        return void actions.setArchived(thread.id, false);
      case 'delete':
        return setDeleting(thread);
    }
  };

  const menu = (thread: SidebarThread) => {
    const pinnedHere = model.pinnedOrder.filter(
      (pid) => data.threads.find((t) => t.id === pid)?.projectId === thread.projectId,
    );
    const pinIndex = pinnedHere.indexOf(thread.id);
    const items = buildThreadActionMenuItems({
      canOperate: ready,
      branch: thread.branch,
      path: thread.worktreePath ?? thread.cwd ?? null,
      isPinned: pinIndex !== -1,
      canMoveUp: pinIndex > 0,
      canMoveDown: pinIndex !== -1 && pinIndex < pinnedHere.length - 1,
      isArchived: thread.archived,
      isRunning: thread.status === 'working',
    });
    return (
      <Menu
        label={`${thread.title} actions`}
        trigger={
          <button
            type="button"
            className={ICON_BUTTON}
            data-thread-more=""
            aria-label={`More actions for ${thread.title}`}
          >
            <MoreHorizontal aria-hidden="true" className="size-3.5" />
          </button>
        }
      >
        {items.flatMap((item) => [
          ...(item.separatorBefore ? [<MenuSeparator key={`${item.id}-sep`} />] : []),
          <MenuItem
            key={item.id}
            disabled={item.disabled}
            tone={item.destructive ? 'danger' : undefined}
            onSelect={() => onMenu(thread, item.id)}
          >
            {item.label}
          </MenuItem>,
        ])}
      </Menu>
    );
  };

  const rowProps = (thread: SidebarThread) => ({
    project: thread.projectId ? (projectById.get(thread.projectId) ?? null) : null,
    active: thread.id === activeThreadId,
    pinned: model.pinnedOrder.includes(thread.id),
    jumpLabel: jumpLabels.get(thread.id) ?? null,
    group: thread.groupId ? groupById.get(thread.groupId) : undefined,
    menu,
    onRename: setRenaming,
    navigate,
  });

  const deletingWorktree = useMemo((): WorktreeTarget | null => {
    if (!deleting?.projectId) return null;
    const path = getOrphanedWorktreePathForThread(data.threads, deleting.id);
    const project = projectById.get(deleting.projectId);
    return path && project ? { repoPath: project.path, worktreePath: path } : null;
  }, [deleting, data.threads, projectById]);
  const cleanup = useMemo(
    () => orphanedWorktrees(projectRefs, data.worktrees, data.threads),
    [projectRefs, data.worktrees, data.threads],
  );

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex h-7 shrink-0 items-center justify-between gap-2 pr-1 pl-2">
        <h2 className="m-0 text-micro font-medium text-faint">Projects</h2>
        <Button
          variant="quiet"
          size="sm"
          onClick={onAdd}
          disabled={!ready || add.isPending}
          aria-label="Add project"
          title="Add project"
        >
          <Plus size={14} aria-hidden="true" />
          <span>Add</span>
        </Button>
      </div>
      {ready && data.state === 'ready' && (
        <div className="shrink-0 pt-1 pb-1">
          <InputGroup className="h-8">
            <InputGroupAddon>
              <Search aria-hidden="true" className="size-3.5" />
            </InputGroupAddon>
            <InputGroupInput
              type="search"
              aria-label="Search threads"
              placeholder="Search threads"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Escape' && query) {
                  e.preventDefault();
                  setQuery('');
                }
              }}
              className="text-small"
            />
          </InputGroup>
        </div>
      )}
      <nav aria-label="Projects" className="py-1" onKeyDown={onRowsKeyDown}>
        {ready && projects.isSuccess && (projects.data.length > 0 || model.projects.length > 0) && (
          <ul className="m-0 flex list-none flex-col gap-1 p-0">
            {model.projects.map((rows) => {
              const id = rows.project?.id ?? '';
              const project = rows.project;
              return (
                <ProjectSection
                  key={id || 'other'}
                  rows={rows}
                  collapsed={!query && collapsed.has(id)}
                  onToggle={() =>
                    setCollapsed((prev) => {
                      const next = new Set(prev);
                      if (next.has(id)) next.delete(id);
                      else next.add(id);
                      return next;
                    })
                  }
                  activeProjectId={activeProjectId}
                  rowProps={rowProps}
                  actions={actions}
                  threadsKnown={data.state === 'ready'}
                  onNewThread={
                    project && data.state === 'ready'
                      ? () => void actions.create(project)
                      : undefined
                  }
                />
              );
            })}
          </ul>
        )}
        {ready && projects.isSuccess && query && model.projects.length === 0 && (
          <p className="m-0 px-2 py-1.5 text-small text-faint">No threads match "{query}".</p>
        )}
        {ready && projects.isPending && (
          <p className="m-0 px-2 py-1.5 text-small text-faint">Loading…</p>
        )}
        {ready &&
          projects.isSuccess &&
          projects.data.length === 0 &&
          model.projects.length === 0 && (
            <p className="m-0 px-2 py-1.5 text-small text-faint">No projects yet.</p>
          )}
        {ready && projects.isError && (
          <p role="alert" className="m-0 px-2 py-1.5 text-small text-danger">
            Could not list projects: {projects.error.message}
          </p>
        )}
        {ready && data.state === 'error' && (
          <p role="alert" className="m-0 px-2 py-1.5 text-small text-danger">
            Could not list threads: {data.error}
          </p>
        )}
        {ready && data.state === 'loading' && projects.isSuccess && projects.data.length > 0 && (
          <p className="m-0 px-2 py-1 text-small text-faint">Loading threads…</p>
        )}
        {!ready && <p className="m-0 px-2 py-1.5 text-small text-faint">Not connected.</p>}
        {add.isError && !cloning && (
          <p role="alert" className="m-0 px-2 py-1.5 text-small text-danger">
            Could not add the project: {add.error?.message}
          </p>
        )}
        {actions.error && (
          <p role="alert" className="m-0 flex items-start gap-2 px-2 py-1.5 text-small text-danger">
            <span className="min-w-0 flex-1">{actions.error}</span>
            <button
              type="button"
              className={ICON_BUTTON}
              aria-label="Dismiss"
              onClick={actions.clearError}
            >
              ×
            </button>
          </p>
        )}
      </nav>
      {children}
      <div className="flex shrink-0 flex-col gap-px pt-1">
        {ready && data.state === 'ready' && (
          <button
            type="button"
            onClick={() => setCleaning(true)}
            className={cx(ROW, 'cursor-pointer border-0 bg-transparent text-left font-[inherit]')}
          >
            <Trash2 size={15} strokeWidth={1.75} aria-hidden="true" />
            Clean up worktrees
          </button>
        )}
      </div>
      <CloneDialog
        open={cloning}
        onClose={() => setCloning(false)}
        pending={add.isPending}
        failure={add.isError ? add.error?.message : undefined}
        onClone={(url) => add.mutate({ url }, { onSuccess: () => setCloning(false) })}
      />
      <RenameDialog
        key={`rename-${renaming?.id ?? ''}`}
        thread={renaming}
        onClose={() => setRenaming(null)}
        onRename={(title) => {
          const id = renaming?.id;
          setRenaming(null);
          if (id) void actions.rename(id, title);
        }}
      />
      <DeleteDialog
        key={`delete-${deleting?.id ?? ''}`}
        thread={deleting}
        worktree={deletingWorktree}
        pending={actions.pending}
        onClose={() => setDeleting(null)}
        onDelete={(removeWorktree) => {
          const thread = deleting;
          setDeleting(null);
          if (thread)
            void actions.remove(
              thread,
              removeWorktree ? (deletingWorktree ?? undefined) : undefined,
            );
        }}
      />
      <CleanupDialog
        open={cleaning}
        candidates={cleanup}
        pending={actions.pending}
        onClose={() => setCleaning(false)}
        onRemove={(targets) => {
          setCleaning(false);
          void actions.removeWorktrees(targets);
        }}
      />
    </div>
  );
}

/** `useStudioCommand` when Studio's command bus is there; nothing without one. */
function useCommand(
  bus: StudioKeys['commands'] | null,
  command: Parameters<typeof useStudioCommand>[1],
  handler: (() => void) | undefined,
) {
  useStudioCommand(bus ?? NO_BUS, command, bus ? handler : undefined);
}

const NO_BUS = new StudioCommands();
