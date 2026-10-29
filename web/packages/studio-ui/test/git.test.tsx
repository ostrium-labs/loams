// Studio's branch and git toolbar (SD1 Task 23; §53 §5.7): the upstream cases of
// GitActionsControl.logic and BranchToolbar.logic, ported (the merge entry Studio adds is
// set aside where upstream compares whole menus), and the presentational pieces: the
// worktree setup card, the workspace choice and the confirmation dialogs.

import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vite-plus/test';
import {
  buildGitPaletteActions,
  buildMenuItems,
  formatGitActionElapsed,
  type GitActionMenuItem,
  type GitStatusView,
  requiresDefaultBranchConfirmation,
  resolveDefaultBranchActionDialogCopy,
  resolveLiveThreadBranchUpdate,
  resolveQuickAction,
  stepsOf,
} from '../src/git/actions.logic';
import { BranchToolbar } from '../src/git/BranchToolbar';
import {
  resolveBranchSelectionTarget,
  resolveBranchToolbarValue,
  resolveBranchTriggerLabel,
  resolveDraftEnvModeAfterBranchChange,
  resolveEffectiveEnvMode,
  resolveLocalCheckoutBranchMismatch,
  resolvePreviousWorktreeLabel,
  resolvePreviousWorktreeSeed,
  shouldIncludeBranchPickerItem,
} from '../src/git/BranchToolbar.logic';
import { GitActions, type GitActionsProps } from '../src/git/GitActions';
import {
  deriveLocalBranchNameFromRemoteRef,
  forgeRepositoryOfUrl,
  isTemporaryWorktreeBranch,
  resolveAutoFeatureBranchName,
} from '../src/git/shared';
import { WorktreeSetupCard, type WorktreeSetupSnapshot } from '../src/git/WorktreeSetupCard';

afterEach(cleanup);

function status(overrides: Partial<GitStatusView> = {}): GitStatusView {
  return {
    isDefaultRef: false,
    refName: 'feature/test',
    hasWorkingTreeChanges: false,
    hasUpstream: true,
    aheadCount: 0,
    behindCount: 0,
    pr: null,
    ...overrides,
  };
}

const openPr = (number: number) => ({
  number,
  title: 'Open PR',
  url: `https://example.com/pr/${number}`,
  state: 'open' as const,
});

/** Upstream's menus, without the merge entry Studio adds. */
const upstreamMenu = (items: GitActionMenuItem[]) =>
  items.filter((i) => i.id !== 'merge').map((i) => [i.id, i.label, i.disabled]);

describe('git_actions_logic_matches_upstream_cases', () => {
  it('rests in the up-to-date state with an open PR and omits the PR entry', () => {
    expect(resolveQuickAction(status({ pr: openPr(10) }), false)).toMatchObject({
      kind: 'show_hint',
      label: 'Commit',
      disabled: true,
      hint: 'Branch is up to date. No action needed.',
    });
    expect(upstreamMenu(buildMenuItems(status({ pr: openPr(11) }), false))).toEqual([
      ['commit', 'Commit', true],
      ['push', 'Push', true],
    ]);
  });

  it('disables everything while busy, and offers nothing without a status', () => {
    expect(resolveQuickAction(status(), true)).toMatchObject({
      kind: 'show_hint',
      hint: 'Git action in progress.',
    });
    expect(upstreamMenu(buildMenuItems(status(), true))).toEqual([
      ['commit', 'Commit', true],
      ['push', 'Push', true],
      ['pr', 'Create PR', true],
    ]);
    expect(resolveQuickAction(null, false)).toMatchObject({
      kind: 'show_hint',
      hint: 'Git status is unavailable.',
    });
    expect(buildMenuItems(null, false)).toEqual([]);
  });

  it('prefers push when ahead with an open PR, and push & PR without one', () => {
    expect(resolveQuickAction(status({ aheadCount: 3, pr: openPr(13) }), false)).toMatchObject({
      kind: 'run_action',
      action: 'push',
      label: 'Push',
    });
    expect(upstreamMenu(buildMenuItems(status({ aheadCount: 2, pr: openPr(12) }), false))).toEqual([
      ['commit', 'Commit', true],
      ['push', 'Push', false],
    ]);
    expect(resolveQuickAction(status({ aheadCount: 2 }), false)).toMatchObject({
      kind: 'run_action',
      action: 'create_pr',
      label: 'Push & create PR',
    });
    expect(upstreamMenu(buildMenuItems(status({ aheadCount: 2 }), false))).toEqual([
      ['commit', 'Commit', true],
      ['push', 'Push', false],
      ['pr', 'Create PR', false],
    ]);
  });

  it('enables create PR when synced with upstream but ahead of default', () => {
    const synced = status({ aheadOfDefaultCount: 1 });
    expect(resolveQuickAction(synced, false)).toMatchObject({
      label: 'Create PR',
      kind: 'run_action',
      action: 'create_pr',
    });
    expect(buildMenuItems(synced, false).find((i) => i.id === 'pr')?.disabled).toBe(false);
    expect(
      buildMenuItems({ ...synced, hasWorkingTreeChanges: true }, false).find((i) => i.id === 'pr')
        ?.disabled,
    ).toBe(false);
    expect(resolveQuickAction(status(), false)).toMatchObject({
      kind: 'show_hint',
      label: 'Commit',
      disabled: true,
    });
  });

  it('pulls when behind and refuses to sync a diverged branch', () => {
    expect(resolveQuickAction(status({ behindCount: 2 }), false)).toMatchObject({
      kind: 'run_pull',
      label: 'Pull',
      disabled: false,
    });
    expect(upstreamMenu(buildMenuItems(status({ behindCount: 1 }), false))).toEqual([
      ['commit', 'Commit', true],
      ['push', 'Push', true],
      ['pr', 'Create PR', true],
    ]);
    expect(resolveQuickAction(status({ aheadCount: 2, behindCount: 1 }), false)).toEqual({
      label: 'Sync ref',
      disabled: true,
      kind: 'show_hint',
      hint: 'Branch has diverged from upstream. Rebase/merge first.',
    });
  });

  it('commits, pushes and opens a PR from local changes', () => {
    expect(resolveQuickAction(status({ hasWorkingTreeChanges: true }), false)).toMatchObject({
      kind: 'run_action',
      action: 'commit_push_pr',
      label: 'Commit, push & PR',
    });
    expect(
      resolveQuickAction(
        status({ hasWorkingTreeChanges: true, hasUpstream: false }),
        false,
        false,
        false,
      ),
    ).toMatchObject({ kind: 'run_action', action: 'commit', label: 'Commit', disabled: false });
    expect(
      resolveQuickAction(status({ hasWorkingTreeChanges: true, pr: openPr(16) }), false),
    ).toMatchObject({ kind: 'run_action', action: 'commit_push', label: 'Commit & push' });
    expect(
      resolveQuickAction(status({ hasWorkingTreeChanges: true, behindCount: 1 }), false),
    ).toMatchObject({ kind: 'run_action', action: 'commit_push_pr' });
  });

  it('on the default ref commits and pushes, never a PR', () => {
    expect(
      resolveQuickAction(status({ refName: 'main', hasWorkingTreeChanges: true }), false, true),
    ).toMatchObject({ kind: 'run_action', action: 'commit_push', label: 'Commit & push' });
    expect(
      resolveQuickAction(status({ refName: 'main', aheadCount: 2 }), false, true),
    ).toMatchObject({
      kind: 'run_action',
      action: 'commit_push',
      label: 'Push',
    });
  });

  it('handles a detached HEAD and a branch with no upstream', () => {
    expect(resolveQuickAction(status({ refName: null, hasUpstream: false }), false)).toMatchObject({
      kind: 'show_hint',
      label: 'Commit',
      disabled: true,
    });
    expect(upstreamMenu(buildMenuItems(status({ refName: null }), false))).toEqual([
      ['commit', 'Commit', true],
      ['push', 'Push', true],
      ['pr', 'Create PR', true],
    ]);
    expect(resolveQuickAction(status({ hasUpstream: false }), false)).toMatchObject({
      kind: 'show_hint',
      label: 'Push',
      hint: 'No local commits to push.',
    });
    expect(resolveQuickAction(status({ hasUpstream: false, pr: openPr(14) }), false)).toMatchObject(
      {
        kind: 'show_hint',
        label: 'Commit',
        hint: 'Branch is up to date. No action needed.',
      },
    );
    expect(resolveQuickAction(status({ hasUpstream: false, aheadCount: 2 }), false)).toMatchObject({
      kind: 'run_action',
      action: 'create_pr',
      label: 'Push & create PR',
    });
    expect(
      resolveQuickAction(status({ hasUpstream: false, aheadCount: 2 }), false, false, false),
    ).toEqual({ kind: 'open_publish', label: 'Publish repository', disabled: false });
    expect(
      upstreamMenu(buildMenuItems(status({ hasUpstream: false, aheadCount: 2 }), false)),
    ).toEqual([
      ['commit', 'Commit', true],
      ['push', 'Push', false],
      ['pr', 'Create PR', false],
    ]);
    expect(
      resolveQuickAction(status({ refName: 'main', hasUpstream: false }), false, true),
    ).toMatchObject({ kind: 'show_hint', label: 'Push', hint: 'No local commits to push.' });
    expect(
      resolveQuickAction(
        status({ refName: 'main', hasUpstream: false, aheadCount: 1 }),
        false,
        true,
      ),
    ).toMatchObject({ kind: 'run_action', action: 'commit_push', label: 'Push' });
  });

  it("confirms default-ref pushes with upstream's copy", () => {
    expect(requiresDefaultBranchConfirmation('commit', true)).toBe(false);
    expect(requiresDefaultBranchConfirmation('push', true)).toBe(true);
    expect(requiresDefaultBranchConfirmation('create_pr', true)).toBe(true);
    expect(requiresDefaultBranchConfirmation('commit_push', true)).toBe(true);
    expect(requiresDefaultBranchConfirmation('commit_push_pr', true)).toBe(true);
    expect(requiresDefaultBranchConfirmation('commit_push', false)).toBe(false);
    expect(requiresDefaultBranchConfirmation('push', false)).toBe(false);
    const suffix =
      ' on "main". You can continue on this ref or create a feature ref and run the same action there.';
    expect(
      resolveDefaultBranchActionDialogCopy({
        action: 'commit_push',
        branchName: 'main',
        includesCommit: false,
      }),
    ).toEqual({
      title: 'Push to default ref?',
      description: `This action will push local commits${suffix}`,
      continueLabel: 'Push to main',
    });
    expect(
      resolveDefaultBranchActionDialogCopy({
        action: 'commit_push_pr',
        branchName: 'main',
        includesCommit: false,
      }),
    ).toEqual({
      title: 'Push & create PR from default ref?',
      description: `This action will push local commits and create a pull request${suffix}`,
      continueLabel: 'Push & create PR',
    });
    expect(
      resolveDefaultBranchActionDialogCopy({
        action: 'commit_push_pr',
        branchName: 'main',
        includesCommit: true,
      }),
    ).toEqual({
      title: 'Commit, push & create PR from default ref?',
      description: `This action will commit, push, and create a pull request${suffix}`,
      continueLabel: 'Commit, push & create PR',
    });
  });

  it('formats elapsed time without going negative', () => {
    expect(formatGitActionElapsed(null, 5_000)).toBeNull();
    expect(formatGitActionElapsed(10_000, 5_000)).toBe('0s');
    expect(formatGitActionElapsed(0, 59_999)).toBe('59s');
    expect(formatGitActionElapsed(0, 125_000)).toBe('2m 5s');
  });

  it("reconciles a thread's branch with the live status, never back to a temporary one", () => {
    expect(
      resolveLiveThreadBranchUpdate({
        threadBranch: 'feature/old-ref',
        gitStatus: status({ refName: 'effect-atom' }),
      }),
    ).toEqual({ branch: 'effect-atom' });
    expect(resolveLiveThreadBranchUpdate({ threadBranch: 'x', gitStatus: null })).toBeNull();
    expect(
      resolveLiveThreadBranchUpdate({ threadBranch: 'a', gitStatus: status({ refName: 'a' }) }),
    ).toBeNull();
    expect(
      resolveLiveThreadBranchUpdate({ threadBranch: 'a', gitStatus: status({ refName: null }) }),
    ).toBeNull();
    // Upstream's `t3code/<hex>` is loams-agentd's `loams-desktop/<adjective>-<noun>`.
    expect(
      resolveLiveThreadBranchUpdate({
        threadBranch: 'feature/query-rate-limit',
        gitStatus: status({ refName: 'loams-desktop/brave-otter' }),
      }),
    ).toBeNull();
    expect(
      resolveLiveThreadBranchUpdate({
        threadBranch: 'loams-desktop/brave-otter',
        gitStatus: status({ refName: 'feature/diff-panel-toggle' }),
      }),
    ).toEqual({ branch: 'feature/diff-panel-toggle' });
  });

  it('names feature branches as upstream does', () => {
    expect(resolveAutoFeatureBranchName(['main', 'feature/other'], 'fix toast copy')).toBe(
      'feature/fix-toast-copy',
    );
    expect(resolveAutoFeatureBranchName(['main'], 'feature/refine-toolbar-actions')).toBe(
      'feature/refine-toolbar-actions',
    );
    expect(
      resolveAutoFeatureBranchName(
        ['main', 'feature/fix-toast-copy', 'feature/fix-toast-copy-2'],
        'fix toast copy',
      ),
    ).toBe('feature/fix-toast-copy-3');
    expect(resolveAutoFeatureBranchName(['Feature/Ticket-1'], 'feature/ticket-1')).toBe(
      'feature/ticket-1-2',
    );
    expect(resolveAutoFeatureBranchName(['main'])).toBe('feature/update');
    expect(deriveLocalBranchNameFromRemoteRef('origin/feature/demo')).toBe('feature/demo');
    expect(deriveLocalBranchNameFromRemoteRef('origin/')).toBe('origin/');
    expect(isTemporaryWorktreeBranch('loams-desktop/brave-otter')).toBe(true);
    expect(isTemporaryWorktreeBranch('feature/brave-otter')).toBe(false);
  });

  it('splits a stacked action into the writes one confirmation names', () => {
    expect(stepsOf('commit_push_pr', { hasWorkingTreeChanges: true })).toEqual([
      'commit',
      'push',
      'create_pr',
    ]);
    expect(stepsOf('commit_push', { hasWorkingTreeChanges: false })).toEqual(['push']);
    expect(stepsOf('create_pr', { hasWorkingTreeChanges: false, needsPush: false })).toEqual([
      'create_pr',
    ]);
    expect(stepsOf('create_pr', { hasWorkingTreeChanges: false })).toEqual(['push', 'create_pr']);
  });

  it('offers merge only from a clean feature branch with commits', () => {
    const merge = (s: GitStatusView) => buildMenuItems(s, false).find((i) => i.id === 'merge');
    expect(merge(status({ aheadOfDefaultCount: 2 }))?.disabled).toBe(false);
    expect(merge(status({ aheadOfDefaultCount: 2, hasWorkingTreeChanges: true }))?.disabled).toBe(
      true,
    );
    expect(merge(status({ refName: 'main', isDefaultRef: true, aheadCount: 1 }))?.disabled).toBe(
      true,
    );
    expect(buildGitPaletteActions(status(), false)).toContainEqual({
      action: 'commit',
      label: 'Commit',
      disabledReason: 'Worktree is clean. Make changes before committing.',
    });
  });
});

describe("the branch toolbar's logic (upstream cases)", () => {
  it('picks the most recent previous worktree', () => {
    const seed = resolvePreviousWorktreeSeed({
      threads: [
        { branch: 'a', worktreePath: '/w/a', updatedAt: '2026-01-01T00:00:00Z' },
        { branch: 'b', worktreePath: '/w/b', updatedAt: '2026-01-02T00:00:00Z' },
        { branch: 'c', worktreePath: '/w/c', updatedAt: '2026-01-03T00:00:00Z', archivedAt: 'x' },
        { branch: 'd', worktreePath: '/w/d', updatedAt: 'not a date' },
        { branch: 'e', worktreePath: null, updatedAt: '2026-01-04T00:00:00Z' },
      ],
      currentWorktreePath: null,
    });
    expect(seed).toEqual({ branch: 'b', worktreePath: '/w/b' });
    expect(
      resolvePreviousWorktreeSeed({
        threads: [{ branch: 'b', worktreePath: '/w/b', updatedAt: 5 }],
        currentWorktreePath: '/w/b',
      }),
    ).toBeNull();
    expect(resolvePreviousWorktreeLabel({ branch: 'b', worktreePath: '/w/b' })).toBe(
      'Previous worktree (b)',
    );
    expect(resolvePreviousWorktreeLabel({ branch: null, worktreePath: '/w/b' })).toBe(
      'Previous worktree',
    );
  });

  it('chooses the env mode and the branch the toolbar shows', () => {
    expect(
      resolveDraftEnvModeAfterBranchChange({
        nextWorktreePath: null,
        currentWorktreePath: '/w/a',
        effectiveEnvMode: 'worktree',
      }),
    ).toBe('local');
    expect(
      resolveDraftEnvModeAfterBranchChange({
        nextWorktreePath: null,
        currentWorktreePath: null,
        effectiveEnvMode: 'worktree',
      }),
    ).toBe('worktree');
    expect(
      resolveDraftEnvModeAfterBranchChange({
        nextWorktreePath: '/w/b',
        currentWorktreePath: null,
        effectiveEnvMode: 'local',
      }),
    ).toBe('worktree');
    expect(
      resolveBranchToolbarValue({
        envMode: 'worktree',
        activeWorktreePath: null,
        activeThreadBranch: null,
        currentGitBranch: 'main',
      }),
    ).toBe('main');
    expect(
      resolveBranchToolbarValue({
        envMode: 'worktree',
        activeWorktreePath: null,
        activeThreadBranch: 'dev',
        currentGitBranch: 'main',
      }),
    ).toBe('dev');
    expect(
      resolveBranchToolbarValue({
        envMode: 'local',
        activeWorktreePath: null,
        activeThreadBranch: 'dev',
        currentGitBranch: 'main',
      }),
    ).toBe('main');
    expect(
      resolveBranchTriggerLabel({
        activeWorktreePath: null,
        effectiveEnvMode: 'worktree',
        resolvedActiveBranch: 'main',
      }),
    ).toBe('From main');
    expect(
      resolveBranchTriggerLabel({
        activeWorktreePath: null,
        effectiveEnvMode: 'local',
        resolvedActiveBranch: null,
      }),
    ).toBe('Select ref');
    expect(
      resolveEffectiveEnvMode({
        activeWorktreePath: null,
        hasServerThread: true,
        draftThreadEnvMode: undefined,
        preparingWorktree: true,
      }),
    ).toBe('worktree');
    expect(
      resolveEffectiveEnvMode({
        activeWorktreePath: '/w/a',
        hasServerThread: false,
        draftThreadEnvMode: 'worktree',
      }),
    ).toBe('local');
  });

  it('detects a local checkout on another branch, and where a pick goes', () => {
    expect(
      resolveLocalCheckoutBranchMismatch({
        effectiveEnvMode: 'local',
        activeWorktreePath: null,
        activeThreadBranch: 'feature/a',
        currentGitBranch: 'main',
      }),
    ).toEqual({ threadBranch: 'feature/a', currentBranch: 'main' });
    expect(
      resolveLocalCheckoutBranchMismatch({
        effectiveEnvMode: 'local',
        activeWorktreePath: '/w/a',
        activeThreadBranch: 'feature/a',
        currentGitBranch: 'main',
      }),
    ).toBeNull();
    expect(
      resolveBranchSelectionTarget({
        activeProjectCwd: '/repo',
        activeWorktreePath: null,
        refName: { isDefault: false, worktreePath: '/w/b' },
      }),
    ).toEqual({ checkoutCwd: '/w/b', nextWorktreePath: '/w/b', reuseExistingWorktree: true });
    expect(
      resolveBranchSelectionTarget({
        activeProjectCwd: '/repo',
        activeWorktreePath: null,
        refName: { isDefault: true, worktreePath: '/repo' },
      }),
    ).toEqual({ checkoutCwd: '/repo', nextWorktreePath: null, reuseExistingWorktree: true });
    expect(
      resolveBranchSelectionTarget({
        activeProjectCwd: '/repo',
        activeWorktreePath: '/w/a',
        refName: { isDefault: true, worktreePath: null },
      }),
    ).toEqual({ checkoutCwd: '/repo', nextWorktreePath: null, reuseExistingWorktree: false });
    expect(
      resolveBranchSelectionTarget({
        activeProjectCwd: '/repo',
        activeWorktreePath: '/w/a',
        refName: { isDefault: false, worktreePath: null },
      }),
    ).toEqual({ checkoutCwd: '/w/a', nextWorktreePath: '/w/a', reuseExistingWorktree: false });
  });

  it('filters the picker, matching a sanitized query', () => {
    const include = (itemValue: string, q: string) =>
      shouldIncludeBranchPickerItem({
        itemValue,
        normalizedQuery: q,
        createBranchItemValue: null,
        checkoutPullRequestItemValue: null,
      });
    expect(include('main', '')).toBe(true);
    expect(include('feature/login', 'login')).toBe(true);
    expect(include('feature/login', 'signup')).toBe(false);
    expect(include('new-branch', 'new branch')).toBe(true);
  });

  it('reads a forge repository from a pull request URL, http(s) only', () => {
    expect(forgeRepositoryOfUrl('https://git.example.test/acme/site/pulls/12')).toBe('acme/site');
    expect(forgeRepositoryOfUrl('https://github.com/acme/site.git')).toBe('acme/site');
    expect(forgeRepositoryOfUrl('javascript:alert(1)')).toBeNull();
    expect(forgeRepositoryOfUrl('https://git.example.test/acme')).toBeNull();
    expect(forgeRepositoryOfUrl(undefined)).toBeNull();
  });
});

describe('the worktree setup card', () => {
  const base = (over: Partial<WorktreeSetupSnapshot> = {}): WorktreeSetupSnapshot => ({
    phase: 'running',
    startedAt: Date.now() - 3_000,
    branch: 'loams-desktop/brave-otter',
    baseRef: 'main',
    worktreePath: '/w/site/brave-otter',
    setupScript: { name: 'Install', command: 'pnpm install' },
    stages: [
      {
        id: 'worktree',
        status: 'done',
        startedAt: Date.now() - 3_000,
        endedAt: Date.now() - 2_000,
        tail: [],
      },
      {
        id: 'setup-script',
        status: 'running',
        startedAt: Date.now() - 2_000,
        tail: ['resolving', 'fetching 12 packages', 'linking'],
      },
    ],
    ...over,
  });

  it('setup_card_shows_progress', () => {
    const onOpenTerminal = vi.fn();
    const onCancel = vi.fn();
    const { container, rerender } = render(
      <WorktreeSetupCard snapshot={base()} onOpenTerminal={onOpenTerminal} onCancel={onCancel} />,
    );
    const section = screen.getByRole('region', { name: 'Worktree setup' });
    expect(section.getAttribute('data-worktree-setup-phase')).toBe('running');
    expect(screen.getByText('Setting up worktree…')).toBeTruthy();
    const rows = container.querySelectorAll('[data-worktree-setup-stage]');
    expect([...rows].map((r) => r.getAttribute('data-worktree-setup-status'))).toEqual([
      'done',
      'running',
    ]);
    // The setup action's name, and its last lines as text.
    expect(screen.getByText('Install')).toBeTruthy();
    expect(screen.getByText('fetching 12 packages')).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: /Open terminal/ }));
    expect(onOpenTerminal).toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: /Cancel/ }));
    expect(onCancel).toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: /Details/ }));
    expect(screen.getByText('pnpm install')).toBeTruthy();

    rerender(
      <WorktreeSetupCard
        snapshot={base({
          phase: 'failed',
          endedAt: Date.now(),
          error: 'git worktree add failed: <b>no</b>',
          stages: [{ id: 'worktree', status: 'failed', tail: [] }],
        })}
        onCancel={onCancel}
      />,
    );
    expect(screen.getByText('Worktree setup failed')).toBeTruthy();
    // Text, never markup.
    expect(screen.getByText('git worktree add failed: <b>no</b>')).toBeTruthy();
    expect(screen.queryByRole('button', { name: /Cancel/ })).toBeNull();
  });
});

describe('the workspace choice', () => {
  it('offers Local, New worktree and the previous worktree, and a base for a new one', async () => {
    const onEnvModeChange = vi.fn();
    const onSelectBranch = vi.fn();
    const onCreateWorktree = vi.fn();
    const onUsePreviousWorktree = vi.fn();
    const refs = [
      { name: 'main', isDefault: true, current: true },
      { name: 'feature/login', worktreePath: '/w/login' },
    ];
    const { rerender } = render(
      <BranchToolbar
        refs={refs}
        envMode="local"
        onEnvModeChange={onEnvModeChange}
        activeWorktreePath={null}
        currentBranch="main"
        previousWorktree={{ branch: 'feature/login', worktreePath: '/w/login' }}
        onUsePreviousWorktree={onUsePreviousWorktree}
        onSelectBranch={onSelectBranch}
        onCreateWorktree={onCreateWorktree}
      />,
    );
    expect(screen.getByRole('button', { name: 'Branch: main' })).toBeTruthy();
    expect(screen.queryByRole('button', { name: /Create worktree/ })).toBeNull();
    fireEvent.click(screen.getByRole('combobox', { name: 'Workspace' }));
    const option = await screen.findByRole('option', { name: /New worktree/ });
    fireEvent.pointerDown(option, { pointerType: 'mouse' });
    fireEvent.pointerUp(option, { pointerType: 'mouse' });
    fireEvent.mouseUp(option);
    fireEvent.click(option);
    expect(onEnvModeChange).toHaveBeenCalledWith('worktree');

    rerender(
      <BranchToolbar
        refs={refs}
        envMode="worktree"
        onEnvModeChange={onEnvModeChange}
        activeWorktreePath={null}
        currentBranch="main"
        onSelectBranch={onSelectBranch}
        onCreateWorktree={onCreateWorktree}
      />,
    );
    expect(screen.getByRole('button', { name: 'Branch: From main' })).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: /Create worktree/ }));
    expect(onCreateWorktree).toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: 'Branch: From main' }));
    const list = await screen.findByRole('list', { name: 'Branches' });
    fireEvent.click(within(list).getByRole('button', { name: /feature\/login/ }));
    expect(onSelectBranch).toHaveBeenCalledWith(refs[1]);
  });
});

function actionsProps(over: Partial<GitActionsProps> = {}): GitActionsProps {
  return {
    status: status({ hasWorkingTreeChanges: true }),
    files: [
      { path: 'src/a.ts', insertions: 3, deletions: 1 },
      { path: 'src/b.ts', insertions: 1, deletions: 0 },
    ],
    defaultBranch: 'main',
    canWrite: true,
    onRun: vi.fn(async () => ({ ok: true as const, title: 'Committed' })),
    onMerge: vi.fn(async () => ({ ok: true as const, title: 'Merged' })),
    onDiscard: vi.fn(async () => ({ ok: true as const, title: 'Discarded' })),
    ...over,
  };
}

describe("the git actions' confirmations", () => {
  it('a commit needs a message, signs off by default and names its files', async () => {
    const props = actionsProps({
      status: status({ hasWorkingTreeChanges: true, hasUpstream: false }),
    });
    render(<GitActions {...props} hasPrimaryRemote={false} />);
    fireEvent.click(screen.getByRole('button', { name: /^Commit$/ }));
    const dialog = await screen.findByRole('dialog', { name: 'Commit changes' });
    const confirm = within(dialog.ownerDocument.body)
      .getAllByRole('button', { name: 'Commit' })
      .at(-1);
    expect(confirm?.hasAttribute('disabled')).toBe(true);
    fireEvent.change(within(dialog).getByLabelText('Commit message'), {
      target: { value: 'Fix the login form' },
    });
    // Leave src/b.ts out.
    fireEvent.click(within(dialog).getByLabelText('Include src/b.ts'));
    fireEvent.submit(dialog.querySelector('form') as HTMLFormElement);
    await waitFor(() =>
      expect(props.onRun).toHaveBeenCalledWith({
        steps: ['commit'],
        commit: { message: 'Fix the login form', signoff: true, paths: ['src/a.ts'] },
      }),
    );
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
    expect(screen.getByText('Committed')).toBeTruthy();
  });

  it('refuses every write without agentd:git-write', () => {
    const props = actionsProps({ canWrite: false });
    render(<GitActions {...props} />);
    fireEvent.click(screen.getByRole('button', { name: /Commit, push & PR/ }));
    expect(screen.queryByRole('dialog')).toBeNull();
    expect(props.onRun).not.toHaveBeenCalled();
  });

  it('discarding a file asks first', async () => {
    const props = actionsProps({
      status: status({ hasWorkingTreeChanges: true, hasUpstream: false }),
    });
    render(<GitActions {...props} hasPrimaryRemote={false} />);
    fireEvent.click(screen.getByRole('button', { name: /^Commit$/ }));
    const dialog = await screen.findByRole('dialog', { name: 'Commit changes' });
    fireEvent.click(within(dialog).getByRole('button', { name: 'Discard changes to src/a.ts' }));
    expect(props.onDiscard).not.toHaveBeenCalled();
    const alert = await screen.findByRole('alertdialog', { name: 'Discard these changes?' });
    fireEvent.click(within(alert).getByRole('button', { name: 'Discard' }));
    await waitFor(() => expect(props.onDiscard).toHaveBeenCalledWith(['src/a.ts']));
  });
});
