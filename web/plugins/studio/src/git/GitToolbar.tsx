// Studio's branch and git toolbar (§53 §5.7, §8.2; SD1 Task 23), in the layout's
// toolbar after the title:
//
//   - On a project: where work happens (its checkout, a new worktree, the previous
//     one) and on which branch; creating a worktree shows its setup card.
//   - On a thread: its checkout and branch (fixed: the thread runs there), its project
//     actions, and the Forgejo pull request of its branch (T27-5).
//   - Both: the git actions (commit, push, open a PR, merge into the base, fetch) and
//     ahead/behind and the PR's state.
//
// Every write is confirmed first: the git actions' dialogs, and here a switch of branch
// and a new worktree. The palette's git commands open the same dialogs (bus.ts).

import { useAgentdMutation } from '@loams/console-host/query';
import { GitService } from '@loams/proto/agentd';
import {
  type BranchRef,
  BranchToolbar,
  buildGitPaletteActions,
  type EnvMode,
  forgeRepositoryOfUrl,
  GitActions,
  type GitDialogAction,
  GitStatusChips,
  resolveBranchSelectionTarget,
  resolvePreviousWorktreeSeed,
  WorktreeSetupCard,
  type WorktreeSetupSnapshot,
} from '@loams/studio-ui';
import { AlertDialog, Button, Dialog, Notice, toastManager } from '@loams/ui';
import { useQueryClient } from '@tanstack/react-query';
import { useEffect, useRef, useState } from 'react';
import type { Project, StudioAgentd } from '../agentd.js';
import type { StudioFactory } from '../factory.js';
import { useProjects } from '../projects/queries.js';
import type { StudioRoute } from '../routes.js';
import { BranchPr } from '../thread/FactoryLinks.js';
import type { GitActionBus } from './bus.js';
import { gitErrorOf } from './errors.js';
import { ProjectActionsEditor } from './ProjectActions.js';
import {
  type CheckoutGit,
  invalidateGit,
  useBranchRefs,
  useCheckoutGit,
  useGitWrites,
  useThreadChat,
} from './queries.js';

/** The git actions, status and PR of one checkout. */
function CheckoutActions({
  repoPath,
  cwd,
  git,
  factory,
  canWrite,
  request,
}: {
  repoPath: string;
  cwd: string;
  git: CheckoutGit;
  factory?: StudioFactory | undefined;
  canWrite: boolean;
  request: { action: GitDialogAction; nonce: number } | null;
}) {
  const writes = useGitWrites({ repoPath, cwd, git });
  const pr = git.status?.pr ?? null;
  const forgeRepo = pr?.provider === 'forgejo' ? forgeRepositoryOfUrl(pr.url) : null;
  const branch = git.status?.refName ?? null;
  return (
    <>
      <GitStatusChips status={git.status} />
      {factory && forgeRepo && branch && (
        <BranchPr factory={factory} repo={forgeRepo} branch={branch} />
      )}
      <GitActions
        status={git.status}
        files={git.files}
        defaultBranch={git.defaultBranch}
        canWrite={canWrite}
        onRun={writes.run}
        onMerge={writes.merge}
        onDiscard={writes.discard}
        onFetch={writes.fetch}
        request={request}
        sourceIsWorktree={
          git.worktree !== undefined && git.worktree.path !== git.worktrees[0]?.path
        }
      />
    </>
  );
}

/** The palette's requests for this thread's dialogs (T24-6). */
function useBusEntry(bus: GitActionBus | undefined, threadId: string, git: CheckoutGit) {
  const [request, setRequest] = useState<{ action: GitDialogAction; nonce: number } | null>(null);
  const status = useRef(git.status);
  status.current = git.status;
  useEffect(() => {
    if (!bus) return;
    return bus.register(threadId, {
      actions: () => buildGitPaletteActions(status.current, false),
      request: (action) => setRequest((prev) => ({ action, nonce: (prev?.nonce ?? 0) + 1 })),
    });
  }, [bus, threadId]);
  return request;
}

/** A thread's toolbar: its checkout is fixed, its git actions and PR are live. */
export function ThreadGitTools({
  threadId,
  factory,
  bus,
  canWrite = true,
}: {
  threadId: string;
  factory?: StudioFactory | undefined;
  bus?: GitActionBus | undefined;
  canWrite?: boolean;
}) {
  const chat = useThreadChat(threadId);
  const cwd = chat?.cwd ?? chat?.sourceContext?.cwd ?? '';
  const repoPath = chat?.sourceContext?.repoRoot || cwd;
  const git = useCheckoutGit({ repoPath, cwd });
  const request = useBusEntry(bus, threadId, git);
  const refs = useBranchRefs(repoPath, git.defaultBranch);
  if (!chat || !cwd) return null;
  const main = git.worktrees[0]?.path;
  const inWorktree = main !== undefined && cwd !== main;
  return (
    <BranchToolbar
      refs={refs.refs}
      envMode={inWorktree ? 'worktree' : 'local'}
      onEnvModeChange={() => {}}
      activeWorktreePath={inWorktree ? cwd : null}
      currentBranch={git.status?.refName ?? chat.branch ?? null}
      envLocked
      branchLocked
      onSelectBranch={() => {}}
    >
      {chat.spaceId && <ProjectActionsEditor spaceId={chat.spaceId} />}
      <CheckoutActions
        repoPath={repoPath}
        cwd={cwd}
        git={git}
        factory={factory}
        canWrite={canWrite}
        request={request}
      />
    </BranchToolbar>
  );
}

type Confirm = { kind: 'switch'; ref: BranchRef } | { kind: 'worktree'; base: string };

/** A project's toolbar: choose the checkout or a worktree, and its branch. */
export function ProjectGitTools({
  project,
  factory,
  canWrite = true,
  openTerminal,
}: {
  project: Project;
  factory?: StudioFactory | undefined;
  canWrite?: boolean;
  /** Shows the terminal drawer (a setup action's terminal). */
  openTerminal?: () => void;
}) {
  const repoPath = project.path;
  const [envMode, setEnvMode] = useState<EnvMode>('local');
  const [worktreePath, setWorktreePath] = useState<string | null>(null);
  const [base, setBase] = useState<string | null>(null);
  const [confirm, setConfirm] = useState<Confirm | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [setup, setSetup] = useState<WorktreeSetupSnapshot | null>(null);
  const cwd = worktreePath ?? repoPath;
  const git = useCheckoutGit({ repoPath, cwd, defaultBranch: project.branch });
  const refs = useBranchRefs(repoPath, git.defaultBranch);
  const writes = useGitWrites({ repoPath, cwd, git });
  const client = useQueryClient();
  const create = useAgentdMutation(GitService.method.createWorktree);

  const previous = resolvePreviousWorktreeSeed({
    threads: git.worktrees.slice(1).map((w) => ({
      branch: w.branch === 'HEAD' ? null : w.branch,
      worktreePath: w.path,
      updatedAt: Number(w.lastActivityMs),
    })),
    currentWorktreePath: worktreePath,
  });

  const select = (ref: BranchRef) => {
    setError(null);
    if (envMode === 'worktree' && !worktreePath) {
      setBase(ref.name);
      return;
    }
    const target = resolveBranchSelectionTarget({
      activeProjectCwd: repoPath,
      activeWorktreePath: worktreePath,
      refName: ref,
    });
    if (target.reuseExistingWorktree) {
      // The branch is checked out somewhere already: work there; nothing is written.
      setWorktreePath(target.nextWorktreePath);
      setEnvMode('local');
      return;
    }
    if (target.nextWorktreePath !== worktreePath) setWorktreePath(target.nextWorktreePath);
    if (ref.name === git.status?.refName) return;
    setConfirm({ kind: 'switch', ref });
  };

  const createWorktree = async (from: string) => {
    const startedAt = Date.now();
    setSetup({
      phase: 'running',
      startedAt,
      baseRef: from,
      stages: [
        { id: 'worktree', status: 'running', startedAt, tail: [] },
        { id: 'setup-script', status: 'pending', tail: [] },
      ],
    });
    try {
      const { outcome } = await create.mutateAsync({ repoPath, branch: from });
      const endedAt = Date.now();
      const action = outcome?.setupAction;
      const failed = Boolean(outcome?.setupError);
      setSetup({
        phase: 'done',
        startedAt,
        endedAt,
        baseRef: from,
        branch: outcome?.branch ?? null,
        worktreePath: outcome?.path ?? null,
        setupScript: action?.actionName ? { name: action.actionName } : null,
        stages: [
          { id: 'worktree', status: 'done', startedAt, endedAt, tail: [] },
          {
            id: 'setup-script',
            status: failed ? 'failed' : action?.terminal ? 'running' : 'skipped',
            detail: failed
              ? (outcome?.setupError ?? null)
              : action?.terminal
                ? 'in a terminal'
                : 'no setup action',
            startedAt: endedAt,
            tail: failed && outcome?.setupError ? [outcome.setupError] : [],
          },
        ],
      });
      if (outcome?.path) {
        setWorktreePath(outcome.path);
        setEnvMode('local');
      }
      await invalidateGit(client);
    } catch (e) {
      const endedAt = Date.now();
      setSetup({
        phase: 'failed',
        startedAt,
        endedAt,
        baseRef: from,
        error: gitErrorOf(e).message,
        stages: [{ id: 'worktree', status: 'failed', startedAt, endedAt, tail: [] }],
      });
    }
  };

  const currentBranch = git.status?.refName ?? (worktreePath ? null : project.branch);
  return (
    <>
      <BranchToolbar
        refs={refs.refs}
        refsLoading={refs.loading}
        envMode={envMode}
        onEnvModeChange={(mode) => {
          setError(null);
          setEnvMode(mode);
          if (mode === 'worktree') {
            setWorktreePath(null);
            setBase(git.defaultBranch);
          }
        }}
        activeWorktreePath={worktreePath}
        currentBranch={currentBranch}
        baseBranch={base}
        previousWorktree={previous}
        onUsePreviousWorktree={() => {
          if (!previous) return;
          setWorktreePath(previous.worktreePath);
          setEnvMode('local');
        }}
        onSelectBranch={select}
        onCreateWorktree={() => {
          const from = base ?? git.defaultBranch;
          if (from) setConfirm({ kind: 'worktree', base: from });
        }}
      >
        {error && (
          <span role="alert" className="max-w-56 truncate text-small text-danger" title={error}>
            {error}
          </span>
        )}
        {!(envMode === 'worktree' && !worktreePath) && (
          <CheckoutActions
            repoPath={repoPath}
            cwd={cwd}
            git={git}
            factory={factory}
            canWrite={canWrite}
            request={null}
          />
        )}
      </BranchToolbar>
      <AlertDialog
        open={confirm?.kind === 'switch'}
        onClose={() => setConfirm(null)}
        title={`Switch to ${confirm?.kind === 'switch' ? confirm.ref.name : ''}?`}
        confirm="Switch branch"
        onConfirm={async () => {
          if (confirm?.kind !== 'switch') return;
          const ref = confirm.ref;
          setConfirm(null);
          const outcome = await writes.switchRef(ref.name);
          if (!outcome.ok) setError(outcome.error.message);
          else toastManager.add({ type: 'success', title: `Switched to ${ref.name}` });
        }}
      >
        {project.name}&apos;s checkout at <code>{cwd}</code> moves to this branch. Git refuses when
        uncommitted changes would be overwritten.
      </AlertDialog>
      <AlertDialog
        open={confirm?.kind === 'worktree'}
        onClose={() => setConfirm(null)}
        title="Create a worktree?"
        confirm="Create worktree"
        onConfirm={() => {
          if (confirm?.kind !== 'worktree') return;
          const from = confirm.base;
          setConfirm(null);
          void createWorktree(from);
        }}
      >
        A new checkout of {project.name} on a new branch, starting from{' '}
        <code>{confirm?.kind === 'worktree' ? confirm.base : ''}</code>. The project&apos;s setup
        action runs in it, if it has one.
      </AlertDialog>
      <Dialog
        open={setup !== null}
        onClose={() => setSetup(null)}
        dismissible={setup?.phase !== 'running'}
        title="New worktree"
        footer={
          <Button onClick={() => setSetup(null)} disabled={setup?.phase === 'running'}>
            Done
          </Button>
        }
      >
        <div className="w-full min-w-0">
          {setup && (
            <WorktreeSetupCard
              snapshot={setup}
              {...(openTerminal &&
              setup.stages.some((s) => s.id === 'setup-script' && s.status === 'running')
                ? { onOpenTerminal: openTerminal }
                : {})}
              {...(setup.phase !== 'running'
                ? {
                    onWorkLocally: () => {
                      setSetup(null);
                      setWorktreePath(null);
                      setEnvMode('local');
                    },
                  }
                : {})}
            />
          )}
          {setup?.phase === 'failed' && (
            <div className="mt-3">
              <Notice tone="danger" title="No worktree was created">
                The project&apos;s checkout is unchanged.
              </Notice>
            </div>
          )}
        </div>
      </Dialog>
    </>
  );
}

/** The toolbar for a Studio route: a project's or a thread's; nothing elsewhere. */
export function StudioGitTools({
  agentd,
  route,
  factory,
  bus,
  openTerminal,
}: {
  agentd: StudioAgentd;
  route: StudioRoute;
  factory?: StudioFactory | undefined;
  bus?: GitActionBus | undefined;
  openTerminal?: () => void;
}) {
  const projects = useProjects(agentd);
  if (route.kind === 'thread') {
    return (
      <ThreadGitTools key={route.threadId} threadId={route.threadId} factory={factory} bus={bus} />
    );
  }
  if (route.kind === 'project') {
    const project = projects.data?.find((p) => p.id === route.projectId);
    return project ? (
      <ProjectGitTools
        key={project.id}
        project={project}
        factory={factory}
        {...(openTerminal ? { openTerminal } : {})}
      />
    ) : null;
  }
  return null;
}
