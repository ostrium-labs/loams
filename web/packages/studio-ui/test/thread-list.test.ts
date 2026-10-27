// Studio's thread list (SD1 Task 17; inventory (a)). `logic_matches_upstream_cases` ports
// T3 Code's unit cases (Sidebar.logic.test.ts, threadSort.test.ts, worktreeCleanup.test.ts,
// shortcutModifierState.test.ts at 6524a8d4) for the rules Studio kept, restated over
// agentd's shapes (millisecond times, session statuses, pins placed by neighbour). The
// rest covers Studio's own model: placing threads in projects and ordering their rows.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vite-plus/test';
import {
  buildSidebarModel,
  buildThreadActionMenuItems,
  compileResolvedKeybindingsConfig,
  createThreadJumpHintVisibilityController,
  DEFAULT_RESOLVED_KEYBINDINGS,
  formatWorkingDurationLabel,
  formatWorktreePathForDisplay,
  getFallbackThreadIdAfterDelete,
  getOrphanedWorktreePathForThread,
  hasUnseenCompletion,
  isContextMenuPointerDown,
  isTrailingDoubleClick,
  orderItemsByPreferredIds,
  type ProjectWorktree,
  placeThread,
  planPinnedMove,
  planSidebarThreadDrop,
  resolveAdjacentThreadId,
  resolveProjectStatusIndicator,
  resolveShortcutCommand,
  resolveSidebarDropTarget,
  resolveSidebarDropVerb,
  resolveSidebarRowAccessibility,
  resolveSidebarThreadStatus,
  resolveThreadStatusPill,
  SHELL_RUN_RESOLVED_KEYBINDINGS,
  type SidebarListItem,
  type SidebarListMarker,
  type SidebarSection,
  type SidebarThread,
  searchSidebarThreads,
  shortcutModifierStateAfterKeyboardEvent,
  shouldShowThreadJumpHintsForModifiers,
  sortProjectsForSidebar,
  sortThreadsByRecency,
  THREAD_JUMP_HINT_SHOW_DELAY_MS,
  type ThreadStatusPill,
  threadJumpCommandForIndex,
  threadJumpIndexFromCommand,
  threadTraversalDirectionFromCommand,
} from '../src';

const at = (iso: string) => Date.parse(iso);

function thread(over: Partial<SidebarThread> & { id: string }): SidebarThread {
  return {
    title: over.id,
    projectId: 'p1',
    branch: null,
    cwd: null,
    worktreePath: null,
    archived: false,
    createdAt: at('2026-03-09T10:00:00.000Z'),
    updatedAt: at('2026-03-09T10:00:00.000Z'),
    lastMessageAt: null,
    lastSeenAt: null,
    status: 'idle',
    statusSince: null,
    pendingApprovals: 0,
    groupId: null,
    parentId: null,
    ...over,
  };
}

const idle = {
  status: 'idle' as const,
  pendingApprovals: 0,
  lastMessageAt: null,
  lastSeenAt: null,
};

describe('logic_matches_upstream_cases', () => {
  describe('resolveSidebarRowAccessibility', () => {
    it.each([
      {
        title: 'Can you audit the UI?',
        statusLabel: 'Working',
        projectDisplayName: 'T3 Code',
        isActive: true,
        expected: { label: 'Can you audit the UI?, Working, T3 Code', current: 'page' },
      },
      {
        title: 'The audit is done',
        statusLabel: null,
        projectDisplayName: 'T3 Code',
        isActive: false,
        expected: { label: 'The audit is done, T3 Code', current: undefined },
      },
      {
        title: 'Untitled task',
        statusLabel: null,
        projectDisplayName: null,
        isActive: false,
        expected: { label: 'Untitled task', current: undefined },
      },
    ])('leads with the title without folding row actions into its name: %j', (input) => {
      const { expected, ...state } = input;
      expect(resolveSidebarRowAccessibility(state)).toEqual(expected);
    });
  });

  describe('hasUnseenCompletion', () => {
    it('returns true when a thread completed after its last visit', () => {
      expect(
        hasUnseenCompletion({
          ...idle,
          lastMessageAt: at('2026-03-09T10:05:00.000Z'),
          lastSeenAt: at('2026-03-09T10:04:00.000Z'),
        }),
      ).toBe(true);
    });

    it('treats a missing client visit marker as read', () => {
      expect(hasUnseenCompletion({ ...idle, lastMessageAt: at('2026-03-09T10:05:00.000Z') })).toBe(
        false,
      );
    });

    it('is not a completion while the thread still runs', () => {
      expect(
        hasUnseenCompletion({
          ...idle,
          status: 'working',
          lastMessageAt: at('2026-03-09T10:05:00.000Z'),
          lastSeenAt: at('2026-03-09T10:04:00.000Z'),
        }),
      ).toBe(false);
    });
  });

  describe('createThreadJumpHintVisibilityController', () => {
    beforeEach(() => {
      vi.useFakeTimers();
    });
    afterEach(() => {
      vi.useRealTimers();
    });

    it('delays showing jump hints until the configured delay elapses', () => {
      const changes: boolean[] = [];
      const controller = createThreadJumpHintVisibilityController({
        delayMs: THREAD_JUMP_HINT_SHOW_DELAY_MS,
        onVisibilityChange: (visible) => changes.push(visible),
      });
      controller.sync(true);
      vi.advanceTimersByTime(THREAD_JUMP_HINT_SHOW_DELAY_MS - 1);
      expect(changes).toEqual([]);
      vi.advanceTimersByTime(1);
      expect(changes).toEqual([true]);
    });

    it('hides immediately when the modifiers are released', () => {
      const changes: boolean[] = [];
      const controller = createThreadJumpHintVisibilityController({
        delayMs: THREAD_JUMP_HINT_SHOW_DELAY_MS,
        onVisibilityChange: (visible) => changes.push(visible),
      });
      controller.sync(true);
      vi.advanceTimersByTime(THREAD_JUMP_HINT_SHOW_DELAY_MS);
      controller.sync(false);
      expect(changes).toEqual([true, false]);
    });

    it('cancels a pending reveal when the modifier is released early', () => {
      const changes: boolean[] = [];
      const controller = createThreadJumpHintVisibilityController({
        delayMs: THREAD_JUMP_HINT_SHOW_DELAY_MS,
        onVisibilityChange: (visible) => changes.push(visible),
      });
      controller.sync(true);
      vi.advanceTimersByTime(Math.floor(THREAD_JUMP_HINT_SHOW_DELAY_MS / 2));
      controller.sync(false);
      vi.advanceTimersByTime(THREAD_JUMP_HINT_SHOW_DELAY_MS);
      expect(changes).toEqual([]);
    });
  });

  describe('isTrailingDoubleClick', () => {
    it('treats a single click as a normal activation', () => {
      expect(isTrailingDoubleClick(1)).toBe(false);
    });
    it('treats synthetic/keyboard activations (detail 0) as a normal activation', () => {
      expect(isTrailingDoubleClick(0)).toBe(false);
    });
    it('ignores the second click of a double-click so it does not navigate', () => {
      expect(isTrailingDoubleClick(2)).toBe(true);
    });
    it('ignores further clicks of a triple-click', () => {
      expect(isTrailingDoubleClick(3)).toBe(true);
    });
  });

  describe('orderItemsByPreferredIds', () => {
    it('keeps preferred ids first, skips stale ids, and preserves the relative order of remaining items', () => {
      const ordered = orderItemsByPreferredIds({
        items: [
          { id: 'project-1', name: 'One' },
          { id: 'project-2', name: 'Two' },
          { id: 'project-3', name: 'Three' },
        ],
        preferredIds: ['project-3', 'project-missing', 'project-1'],
        getId: (project) => project.id,
      });
      expect(ordered.map((p) => p.id)).toEqual(['project-3', 'project-1', 'project-2']);
    });

    it('does not duplicate items when preferred ids repeat', () => {
      const ordered = orderItemsByPreferredIds({
        items: [
          { id: 'project-1', name: 'One' },
          { id: 'project-2', name: 'Two' },
        ],
        preferredIds: ['project-2', 'project-1', 'project-2'],
        getId: (project) => project.id,
      });
      expect(ordered.map((p) => p.id)).toEqual(['project-2', 'project-1']);
    });

    it('resolves legacy preference aliases without materializing project state', () => {
      const ordered = orderItemsByPreferredIds({
        items: [
          { id: 'physical-a', cwd: '/work/a' },
          { id: 'physical-b', cwd: '/work/b' },
          { id: 'physical-c', cwd: '/work/c' },
        ],
        preferredIds: ['legacy:/work/c', 'legacy:/work/a'],
        getId: (project) => project.id,
        getPreferenceIds: (project) => [project.id, `legacy:${project.cwd}`],
      });
      expect(ordered.map((p) => p.id)).toEqual(['physical-c', 'physical-a', 'physical-b']);
    });
  });

  describe('resolveAdjacentThreadId', () => {
    it('resolves adjacent thread ids in ordered sidebar traversal', () => {
      const threads = ['thread-1', 'thread-2', 'thread-3'];
      expect(
        resolveAdjacentThreadId({
          threadIds: threads,
          currentThreadId: 'thread-2',
          direction: 'previous',
        }),
      ).toBe('thread-1');
      expect(
        resolveAdjacentThreadId({
          threadIds: threads,
          currentThreadId: 'thread-2',
          direction: 'next',
        }),
      ).toBe('thread-3');
      expect(
        resolveAdjacentThreadId({ threadIds: threads, currentThreadId: null, direction: 'next' }),
      ).toBe('thread-1');
      expect(
        resolveAdjacentThreadId({
          threadIds: threads,
          currentThreadId: null,
          direction: 'previous',
        }),
      ).toBe('thread-3');
      expect(
        resolveAdjacentThreadId({
          threadIds: threads,
          currentThreadId: 'thread-1',
          direction: 'previous',
        }),
      ).toBeNull();
    });
  });

  describe('isContextMenuPointerDown', () => {
    it('treats secondary-button presses as context menu gestures on all platforms', () => {
      expect(isContextMenuPointerDown({ button: 2, ctrlKey: false, isMac: false })).toBe(true);
    });
    it('treats ctrl+primary-click as a context menu gesture on macOS', () => {
      expect(isContextMenuPointerDown({ button: 0, ctrlKey: true, isMac: true })).toBe(true);
    });
    it('does not treat ctrl+primary-click as a context menu gesture off macOS', () => {
      expect(isContextMenuPointerDown({ button: 0, ctrlKey: true, isMac: false })).toBe(false);
    });
  });

  describe('resolveSidebarThreadStatus', () => {
    it('prioritizes approval over a running session', () => {
      expect(resolveSidebarThreadStatus({ pendingApprovals: 1, status: 'working' })).toBe(
        'approval',
      );
    });
    it('prioritizes awaiting input over a running session, below approval', () => {
      expect(resolveSidebarThreadStatus({ pendingApprovals: 0, status: 'awaiting-input' })).toBe(
        'input',
      );
      expect(resolveSidebarThreadStatus({ pendingApprovals: 2, status: 'awaiting-input' })).toBe(
        'approval',
      );
    });
    it('reports working for a working session', () => {
      expect(resolveSidebarThreadStatus({ pendingApprovals: 0, status: 'working' })).toBe(
        'working',
      );
    });
    it('reports failed only while the session errored', () => {
      expect(resolveSidebarThreadStatus({ pendingApprovals: 0, status: 'errored' })).toBe('failed');
    });
    it('defaults to ready with an idle session', () => {
      expect(resolveSidebarThreadStatus({ pendingApprovals: 0, status: 'idle' })).toBe('ready');
    });
  });

  describe('searchSidebarThreads', () => {
    const threads = [
      { id: 'thread-1', title: 'Fix workspace search', branch: 'alpha', project: 'Alpha' },
      { id: 'thread-2', title: 'Review providers', branch: null, project: 'Workspace' },
      { id: 'thread-3', title: 'WORKTREE cleanup', branch: 'beta', project: 'Beta' },
      { id: 'thread-4', title: 'Docs', branch: 'work/docs', project: 'Beta' },
    ];
    it('matches thread titles case-insensitively and preserves their order', () => {
      expect(searchSidebarThreads(threads, 'work')).toEqual([threads[0], threads[2], threads[3]]);
    });
    it('does not match project metadata', () => {
      expect(searchSidebarThreads(threads, 'workspace')).toEqual([threads[0]]);
    });
    it('returns no results for an empty query', () => {
      expect(searchSidebarThreads(threads, '   ')).toEqual([]);
    });
    it('appends branch-only matches after every title match', () => {
      expect(searchSidebarThreads(threads, 'work').at(-1)).toBe(threads[3]);
    });
  });

  describe('formatWorkingDurationLabel', () => {
    it('formats seconds, minutes, and hours', () => {
      expect(formatWorkingDurationLabel(0)).toBe('0s');
      expect(formatWorkingDurationLabel(42_000)).toBe('42s');
      expect(formatWorkingDurationLabel(5 * 60_000)).toBe('5m');
      expect(formatWorkingDurationLabel(90 * 60_000)).toBe('1h 30m');
    });
    it('clamps negative and non-finite elapsed values to zero', () => {
      expect(formatWorkingDurationLabel(-5_000)).toBe('0s');
      expect(formatWorkingDurationLabel(Number.NaN)).toBe('0s');
    });
  });

  describe('resolveThreadStatusPill', () => {
    it('shows pending approval before all other statuses', () => {
      expect(
        resolveThreadStatusPill({
          thread: { ...idle, pendingApprovals: 1, status: 'awaiting-input' },
        }),
      ).toMatchObject({ label: 'Pending Approval', pulse: false });
    });
    it('shows awaiting input when the agent asked a question', () => {
      expect(
        resolveThreadStatusPill({ thread: { ...idle, status: 'awaiting-input' } }),
      ).toMatchObject({ label: 'Awaiting Input', pulse: false });
    });
    it('falls back to working when the thread is actively running without blockers', () => {
      expect(resolveThreadStatusPill({ thread: { ...idle, status: 'working' } })).toMatchObject({
        label: 'Working',
        pulse: true,
      });
    });
    it('shows a completion the person has not seen', () => {
      expect(
        resolveThreadStatusPill({ thread: { ...idle, lastMessageAt: 2, lastSeenAt: 1 } }),
      ).toMatchObject({ label: 'Completed' });
      expect(resolveThreadStatusPill({ thread: idle })).toBeNull();
    });
    it('uses token classes only', () => {
      for (const thread of [
        { ...idle, pendingApprovals: 1 },
        { ...idle, status: 'working' as const },
        { ...idle, status: 'errored' as const },
        { ...idle, lastMessageAt: 2, lastSeenAt: 1 },
      ]) {
        const pill = resolveThreadStatusPill({ thread });
        expect(pill?.colorClass).toMatch(/^text-(accent-fg|info|danger|grow)$/);
        expect(pill?.dotClass).toMatch(/^bg-(accent|info|danger|grow)$/);
      }
    });
  });

  describe('resolveProjectStatusIndicator', () => {
    const pill = (label: ThreadStatusPill['label']): ThreadStatusPill => ({
      label,
      colorClass: 'text-info',
      dotClass: 'bg-info',
      pulse: false,
    });
    it('returns null when no threads have a notable status', () => {
      expect(resolveProjectStatusIndicator([null, null])).toBeNull();
    });
    it('surfaces the highest-priority actionable state across project threads', () => {
      expect(
        resolveProjectStatusIndicator([
          pill('Completed'),
          pill('Pending Approval'),
          pill('Working'),
        ]),
      ).toMatchObject({ label: 'Pending Approval' });
    });
  });

  describe('getFallbackThreadIdAfterDelete', () => {
    const t = (id: string, projectId: string, iso: string) =>
      thread({ id, projectId, createdAt: at(iso), updatedAt: at(iso) });
    it("returns the top remaining thread in the deleted thread's project sidebar order", () => {
      expect(
        getFallbackThreadIdAfterDelete({
          threads: [
            t('thread-oldest', 'project-1', '2026-03-09T10:00:00.000Z'),
            t('thread-active', 'project-1', '2026-03-09T10:05:00.000Z'),
            t('thread-newest', 'project-1', '2026-03-09T10:10:00.000Z'),
            t('thread-other-project', 'project-2', '2026-03-09T10:20:00.000Z'),
          ],
          deletedThreadId: 'thread-active',
          sortOrder: 'created_at',
        }),
      ).toBe('thread-newest');
    });
    it('skips other threads being deleted in the same action', () => {
      expect(
        getFallbackThreadIdAfterDelete({
          threads: [
            t('thread-active', 'project-1', '2026-03-09T10:05:00.000Z'),
            t('thread-newest', 'project-1', '2026-03-09T10:10:00.000Z'),
            t('thread-next', 'project-1', '2026-03-09T10:07:00.000Z'),
          ],
          deletedThreadId: 'thread-active',
          deletedThreadIds: new Set(['thread-active', 'thread-newest']),
          sortOrder: 'created_at',
        }),
      ).toBe('thread-next');
    });
  });

  describe('sortProjectsForSidebar', () => {
    it('sorts projects by the most recent message across their threads', () => {
      const sorted = sortProjectsForSidebar(
        [
          { id: 'project-1', title: 'Older project' },
          { id: 'project-2', title: 'Newer project' },
        ],
        [
          thread({
            id: 'a',
            projectId: 'project-1',
            updatedAt: at('2026-03-09T10:01:00.000Z'),
          }),
          thread({
            id: 'b',
            projectId: 'project-2',
            updatedAt: at('2026-03-09T10:05:00.000Z'),
          }),
        ],
        'updated_at',
      );
      expect(sorted.map((p) => p.id)).toEqual(['project-2', 'project-1']);
    });
    it('falls back to project timestamps when a project has no threads', () => {
      const sorted = sortProjectsForSidebar(
        [
          { id: 'project-1', title: 'Older project', updatedAt: at('2026-03-09T10:01:00.000Z') },
          { id: 'project-2', title: 'Newer project', updatedAt: at('2026-03-09T10:05:00.000Z') },
        ],
        [],
        'updated_at',
      );
      expect(sorted.map((p) => p.id)).toEqual(['project-2', 'project-1']);
    });
    it('keeps the given order when manual', () => {
      const projects = [
        { id: 'b', title: 'B' },
        { id: 'a', title: 'A' },
      ];
      expect(sortProjectsForSidebar(projects, [], 'manual')).toEqual(projects);
    });
  });

  describe('sortThreads (threadSort.test.ts)', () => {
    it('sorts newest first with a descending id tiebreak', () => {
      const sorted = sortThreadsByRecency(
        [
          { id: 'a', createdAt: 1, updatedAt: 5 },
          { id: 'b', createdAt: 2, updatedAt: 5 },
          { id: 'c', createdAt: 3, updatedAt: 9 },
        ],
        'updated_at',
      );
      expect(sorted.map((t) => t.id)).toEqual(['c', 'b', 'a']);
    });
    it('sorts by creation time when asked', () => {
      const sorted = sortThreadsByRecency(
        [
          { id: 'a', createdAt: 3, updatedAt: 1 },
          { id: 'b', createdAt: 1, updatedAt: 9 },
        ],
        'created_at',
      );
      expect(sorted.map((t) => t.id)).toEqual(['a', 'b']);
    });
  });

  describe('dragging', () => {
    const marker = (name: SidebarListMarker): SidebarListItem => ({ kind: 'marker', marker: name });
    const row = (key: string, section: SidebarSection): SidebarListItem => ({
      kind: 'thread',
      key,
      section,
    });
    // Pinned p1 p2 | Active a1 a2
    const items: readonly SidebarListItem[] = [
      marker('pinned-header'),
      row('p1', 'pinned'),
      row('p2', 'pinned'),
      marker('pinned-divider'),
      row('a1', 'active'),
      row('a2', 'active'),
    ];

    it('resolves the drop section and orders from the divider', () => {
      expect(resolveSidebarDropTarget(items, 'p1', 'a2')).toEqual({
        section: 'active',
        pinnedOrder: ['p2'],
        activeOrder: ['a1', 'a2', 'p1'],
      });
      expect(resolveSidebarDropTarget(items, 'a1', 'p1')).toEqual({
        section: 'pinned',
        pinnedOrder: ['a1', 'p1', 'p2'],
        activeOrder: ['a2'],
      });
      expect(resolveSidebarDropTarget(items, 'missing', 'p1')).toBeNull();
      expect(resolveSidebarDropVerb('active', 'pinned')).toBe('pin');
      expect(resolveSidebarDropVerb('pinned', 'active')).toBe('unpin');
      expect(resolveSidebarDropVerb('pinned', 'pinned')).toBeNull();
    });

    it('only changes lifecycle when the inbox is time-ordered', () => {
      expect(
        planSidebarThreadDrop({
          activeKey: 'a1',
          activeSection: 'active',
          target: { section: 'active', pinnedOrder: ['p1', 'p2'], activeOrder: ['a2', 'a1'] },
          pinnedOrder: ['p1', 'p2'],
        }),
      ).toEqual({ kind: 'none' });
      expect(
        planSidebarThreadDrop({
          activeKey: 'p1',
          activeSection: 'pinned',
          target: { section: 'active', pinnedOrder: ['p2'], activeOrder: ['a1', 'p1', 'a2'] },
          pinnedOrder: ['p1', 'p2'],
        }),
      ).toEqual({ kind: 'unpin' });
    });

    it('places a pin after the row above it, or before the row below it', () => {
      expect(
        planSidebarThreadDrop({
          activeKey: 'p1',
          activeSection: 'pinned',
          target: { section: 'pinned', pinnedOrder: ['p2', 'p1'], activeOrder: ['a1', 'a2'] },
          pinnedOrder: ['p1', 'p2'],
        }),
      ).toEqual({ kind: 'reorder-pinned', order: ['p2', 'p1'], position: { after: 'p2' } });
      expect(
        planSidebarThreadDrop({
          activeKey: 'a1',
          activeSection: 'active',
          target: { section: 'pinned', pinnedOrder: ['a1', 'p1', 'p2'], activeOrder: ['a2'] },
          pinnedOrder: ['p1', 'p2'],
        }),
      ).toEqual({ kind: 'pin', order: ['a1', 'p1', 'p2'], position: { before: 'p1' } });
    });

    it('writes nothing when dropped back where it started', () => {
      expect(
        planSidebarThreadDrop({
          activeKey: 'p1',
          activeSection: 'pinned',
          target: { section: 'pinned', pinnedOrder: ['p1', 'p2'], activeOrder: ['a1', 'a2'] },
          pinnedOrder: ['p1', 'p2'],
        }),
      ).toEqual({ kind: 'none' });
    });

    it('moves a pinned thread up or down by one (planPinnedMove)', () => {
      expect(
        planPinnedMove({ orderedIds: ['a', 'b', 'c'], movedId: 'c', direction: 'up' }),
      ).toEqual({ order: ['a', 'c', 'b'], position: { after: 'a' } });
      expect(
        planPinnedMove({ orderedIds: ['a', 'b', 'c'], movedId: 'b', direction: 'up' }),
      ).toEqual({ order: ['b', 'a', 'c'], position: { before: 'a' } });
      expect(planPinnedMove({ orderedIds: ['a', 'b'], movedId: 'a', direction: 'up' })).toBeNull();
      expect(
        planPinnedMove({ orderedIds: ['a', 'b'], movedId: 'b', direction: 'down' }),
      ).toBeNull();
    });
  });

  describe('worktreeCleanup (worktreeCleanup.test.ts)', () => {
    it('returns the worktree path when no other thread shares it', () => {
      expect(
        getOrphanedWorktreePathForThread(
          [
            { id: 't1', worktreePath: '/wt/a' },
            { id: 't2', worktreePath: '/wt/b' },
          ],
          't1',
        ),
      ).toBe('/wt/a');
    });
    it('returns null when another thread shares the worktree', () => {
      expect(
        getOrphanedWorktreePathForThread(
          [
            { id: 't1', worktreePath: '/wt/a' },
            { id: 't2', worktreePath: ' /wt/a ' },
          ],
          't1',
        ),
      ).toBeNull();
    });
    it('returns null for a thread in the main checkout or unknown', () => {
      expect(getOrphanedWorktreePathForThread([{ id: 't1', worktreePath: null }], 't1')).toBeNull();
      expect(getOrphanedWorktreePathForThread([], 'missing')).toBeNull();
    });
    it('formats a worktree path for display by its last segment', () => {
      expect(formatWorktreePathForDisplay('/home/me/.loams/worktrees/repo-feature/')).toBe(
        'repo-feature',
      );
      expect(formatWorktreePathForDisplay('C:\\work\\wt')).toBe('wt');
    });
  });

  describe('shortcutModifierState (shortcutModifierState.test.ts)', () => {
    const none = { metaKey: false, ctrlKey: false, altKey: false, shiftKey: false };
    const ev = (type: string, key: string, mods: Partial<typeof none> = {}) =>
      ({ type, key, ...none, ...mods }) as KeyboardEvent;
    it('sets a modifier on its keydown and clears it on keyup', () => {
      const held = shortcutModifierStateAfterKeyboardEvent(
        none,
        ev('keydown', 'Meta', { metaKey: true }),
      );
      expect(held.metaKey).toBe(true);
      expect(shortcutModifierStateAfterKeyboardEvent(held, ev('keyup', 'Meta')).metaKey).toBe(
        false,
      );
    });
    it('never sets a bit from a non-modifier key event', () => {
      expect(
        shortcutModifierStateAfterKeyboardEvent(none, ev('keydown', 'Enter', { metaKey: true })),
      ).toBe(none);
    });
    it('clears a bit when a non-modifier key reports it released', () => {
      const held = { ...none, ctrlKey: true };
      expect(shortcutModifierStateAfterKeyboardEvent(held, ev('keydown', 'a')).ctrlKey).toBe(false);
    });
  });

  describe('thread jumps (keybindings.ts)', () => {
    const LINUX = 'Linux x86_64';
    it('maps indexes to the nine jump commands and back', () => {
      expect(threadJumpCommandForIndex(0)).toBe('thread.jump.1');
      expect(threadJumpCommandForIndex(8)).toBe('thread.jump.9');
      expect(threadJumpCommandForIndex(9)).toBeNull();
      expect(threadJumpIndexFromCommand('thread.jump.3')).toBe(2);
      expect(threadJumpIndexFromCommand('chat.new')).toBeNull();
      expect(threadTraversalDirectionFromCommand('thread.next')).toBe('next');
      expect(threadTraversalDirectionFromCommand('thread.previous')).toBe('previous');
      expect(threadTraversalDirectionFromCommand(null)).toBeNull();
    });
    it('labels Mod+1…9 on the desktop only, and leaves the keys to the shell (D1081)', () => {
      const key = {
        key: '2',
        code: 'Digit2',
        ctrlKey: true,
        metaKey: false,
        shiftKey: false,
        altKey: false,
      };
      // The shell's keymap runs the jumps; Studio's own defaults no longer bind them.
      expect(
        resolveShortcutCommand(key, DEFAULT_RESOLVED_KEYBINDINGS, {
          platform: LINUX,
          context: { isDesktop: true, isWeb: false },
        }),
      ).toBeNull();
      expect(
        resolveShortcutCommand(key, SHELL_RUN_RESOLVED_KEYBINDINGS, {
          platform: LINUX,
          context: { isDesktop: true, isWeb: false },
        }),
      ).toBe('thread.jump.2');
      expect(
        resolveShortcutCommand(key, SHELL_RUN_RESOLVED_KEYBINDINGS, {
          platform: LINUX,
          context: { isDesktop: false, isWeb: true },
        }),
      ).toBeNull();
    });
    it('shows hints only while the held modifiers match a jump binding, never in a terminal', () => {
      const ctrl = { metaKey: false, ctrlKey: true, altKey: false, shiftKey: false };
      const opts = { platform: LINUX, context: { isDesktop: true } };
      expect(
        shouldShowThreadJumpHintsForModifiers(ctrl, SHELL_RUN_RESOLVED_KEYBINDINGS, opts),
      ).toBe(true);
      expect(
        shouldShowThreadJumpHintsForModifiers(
          { ...ctrl, shiftKey: true },
          SHELL_RUN_RESOLVED_KEYBINDINGS,
          opts,
        ),
      ).toBe(false);
      expect(
        shouldShowThreadJumpHintsForModifiers(ctrl, SHELL_RUN_RESOLVED_KEYBINDINGS, {
          platform: LINUX,
          context: { isDesktop: true, terminalFocus: true },
        }),
      ).toBe(false);
      expect(
        shouldShowThreadJumpHintsForModifiers(ctrl, compileResolvedKeybindingsConfig([]), opts),
      ).toBe(false);
    });
  });
});

describe('the sidebar model', () => {
  const projects = [
    { id: '/src/loams', name: 'loams', path: '/src/loams', branch: 'dev' },
    { id: '/src/site', name: 'site', path: '/src/site', branch: 'main' },
  ];
  const worktrees = new Map<string, ProjectWorktree[]>([
    [
      '/src/loams',
      [
        { path: '/src/loams', branch: 'dev', chatIds: [], dirty: false, main: true },
        { path: '/wt/loams-fix', branch: 'fix', chatIds: ['listed'], dirty: false, main: false },
      ],
    ],
  ]);

  it('places a thread by its worktree, then by the longest checkout holding its cwd', () => {
    expect(placeThread({ id: 'listed', cwd: null, repoRoot: null }, projects, worktrees)).toEqual({
      projectId: '/src/loams',
      worktreePath: '/wt/loams-fix',
    });
    expect(
      placeThread({ id: 'x', cwd: '/wt/loams-fix/crates', repoRoot: null }, projects, worktrees),
    ).toEqual({ projectId: '/src/loams', worktreePath: '/wt/loams-fix' });
    expect(
      placeThread({ id: 'y', cwd: '/src/site/web', repoRoot: '/src/site' }, projects, worktrees),
    ).toEqual({ projectId: '/src/site', worktreePath: null });
    // `/src/loams-old` is not inside `/src/loams`.
    expect(
      placeThread({ id: 'z', cwd: '/src/loams-old', repoRoot: null }, projects, worktrees),
    ).toEqual({ projectId: null, worktreePath: null });
  });

  it('orders pins as agentd stores them, nests run groups, and puts the rest newest first', () => {
    const model = buildSidebarModel({
      projects,
      threads: [
        thread({ id: 'old', projectId: '/src/loams', updatedAt: 1 }),
        thread({ id: 'new', projectId: '/src/loams', updatedAt: 9 }),
        thread({ id: 'pin-b', projectId: '/src/loams' }),
        thread({ id: 'pin-a', projectId: '/src/loams' }),
        thread({ id: 'm1', projectId: '/src/loams', groupId: 'g1' }),
        thread({ id: 'm2', projectId: '/src/loams', groupId: 'g1' }),
        thread({ id: 'lost', projectId: null }),
        thread({ id: 'gone', projectId: '/src/site', archived: true }),
      ],
      groups: [
        {
          id: 'g1',
          title: 'Try three fixes',
          projectId: null,
          memberIds: ['m1', 'm2'],
          createdAt: 5,
          state: 'running',
        },
      ],
      pinnedIds: ['pin-a', 'pin-b'],
    });
    const [loams, site, other] = model.projects;
    expect(loams?.pinned.map((t) => t.id)).toEqual(['pin-a', 'pin-b']);
    expect(loams?.groups.map((g) => g.members.map((t) => t.id))).toEqual([['m1', 'm2']]);
    expect(loams?.threads.map((t) => t.id)).toEqual(['new', 'old']);
    expect(site?.threads).toEqual([]);
    expect(other?.project).toBeNull();
    expect(model.orderedThreadIds).toEqual(['pin-a', 'pin-b', 'm1', 'm2', 'new', 'old', 'lost']);
    expect(model.pinnedOrder).toEqual(['pin-a', 'pin-b']);
  });

  it('hides archived threads unless asked, and projects without a match while searching', () => {
    const threads = [
      thread({ id: 'a', title: 'Fix login', projectId: '/src/loams' }),
      thread({ id: 'b', title: 'Old idea', projectId: '/src/site', archived: true }),
    ];
    expect(
      buildSidebarModel({ projects, threads, pinnedIds: [], showArchived: true }).orderedThreadIds,
    ).toEqual(['a', 'b']);
    const searched = buildSidebarModel({ projects, threads, pinnedIds: [], query: 'login' });
    expect(searched.projects.map((p) => p.project?.id)).toEqual(['/src/loams']);
  });

  it('builds the thread menu with Move only for pins, and disables archive while running', () => {
    const items = buildThreadActionMenuItems({
      canOperate: true,
      branch: 'fix',
      path: '/wt/a',
      isPinned: true,
      canMoveUp: false,
      canMoveDown: true,
      isArchived: false,
      isRunning: true,
    });
    expect(items.map((i) => i.id)).toEqual([
      'unpin',
      'move-up',
      'move-down',
      'rename',
      'fork',
      'copy-path',
      'copy-branch',
      'copy-thread-id',
      'archive',
      'delete',
    ]);
    expect(items.find((i) => i.id === 'move-up')?.disabled).toBe(true);
    expect(items.find((i) => i.id === 'archive')?.disabled).toBe(true);
    const readOnly = buildThreadActionMenuItems({
      canOperate: false,
      branch: null,
      path: null,
      isPinned: false,
      canMoveUp: false,
      canMoveDown: false,
      isArchived: true,
      isRunning: false,
    });
    expect(readOnly.filter((i) => !i.disabled).map((i) => i.id)).toEqual(['copy-thread-id']);
  });
});
