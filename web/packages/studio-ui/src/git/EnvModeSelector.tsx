// Portions of this file are from T3 Code (https://github.com/pingdotgg/t3code),
// apps/web/src/components/BranchToolbarEnvModeSelector.tsx at commit 6524a8d4, Copyright (c) 2026 T3 Tools Inc., MIT licence
// (see web/packages/studio-ui/LICENSE.t3code). Modified by The Loams Authors:
// on @loams/ui's SelectMenu and Tooltip in place of T3's select and composer controls; PreviousWorktreeItemContent.tsx (same commit) folded in; the composer event scope dropped. Loams' changes are Apache-2.0.

// Where work happens (§53 §5.7; SD1 Task 23): the project's checkout ("Local"), a new
// worktree, or the worktree last worked in.

import { MiddleTruncate, SelectMenu, Tooltip } from "@loams/ui";
import { Folder, FolderGit, FolderGit2, History } from "lucide-react";
import type { ReactNode } from "react";
import {
  type EnvMode,
  resolveCurrentWorkspaceLabel,
  resolveEnvModeLabel,
  resolveLockedWorkspaceLabel,
} from "./BranchToolbar.logic";

const PREVIOUS_WORKTREE_SELECT_VALUE = "previous-worktree";
type EnvModeValue = EnvMode | typeof PREVIOUS_WORKTREE_SELECT_VALUE;

export interface EnvModeSelectorProps {
  forceNewWorktree?: boolean;
  envLocked: boolean;
  effectiveEnvMode: EnvMode;
  activeWorktreePath: string | null;
  onEnvModeChange: (mode: EnvMode) => void;
  previousWorktreeLabel?: string | null;
  previousWorktreeBranch?: string | null;
  onUsePreviousWorktree?: () => void;
}

function PreviousWorktreeItemContent({ branch }: { branch: string | null }) {
  return (
    <span className="flex min-w-0 items-start gap-1.5">
      <History size={12} aria-hidden="true" className="mt-1 shrink-0" />
      <span className="flex min-w-0 flex-col">
        <span>Previous worktree</span>
        {branch ? (
          <span className="min-w-0 text-[0.76rem] text-muted">
            <MiddleTruncate value={branch} />
          </span>
        ) : null}
      </span>
    </span>
  );
}

function ModeIcon({ mode, worktree }: { mode: EnvMode; worktree: boolean }) {
  if (mode === "worktree") return <FolderGit2 size={13} aria-hidden="true" />;
  return worktree ? (
    <FolderGit size={13} aria-hidden="true" />
  ) : (
    <Folder size={13} aria-hidden="true" />
  );
}

export function EnvModeSelector({
  forceNewWorktree = false,
  envLocked,
  effectiveEnvMode,
  activeWorktreePath,
  onEnvModeChange,
  previousWorktreeLabel,
  previousWorktreeBranch = null,
  onUsePreviousWorktree,
}: EnvModeSelectorProps) {
  const showPreviousWorktree = Boolean(previousWorktreeLabel && onUsePreviousWorktree);

  if (envLocked || forceNewWorktree) {
    const label = forceNewWorktree
      ? resolveEnvModeLabel("worktree")
      : resolveLockedWorkspaceLabel(activeWorktreePath, effectiveEnvMode);
    return (
      <Tooltip content={forceNewWorktree ? "Each run starts in its own worktree." : label}>
        <span
          className="inline-flex h-7 min-w-0 items-center gap-1 px-1.5 text-small text-muted"
          data-studio-workspace
        >
          <ModeIcon mode={effectiveEnvMode} worktree={activeWorktreePath !== null} />
          <span className="truncate">{label}</span>
        </span>
      </Tooltip>
    );
  }

  const options: { value: EnvModeValue; label: ReactNode }[] = [
    {
      value: "local",
      label: (
        <span className="inline-flex items-center gap-1.5">
          <ModeIcon mode="local" worktree={activeWorktreePath !== null} />
          {resolveCurrentWorkspaceLabel(activeWorktreePath)}
        </span>
      ),
    },
    {
      value: "worktree",
      label: (
        <span className="inline-flex items-center gap-1.5">
          <ModeIcon mode="worktree" worktree={false} />
          {resolveEnvModeLabel("worktree")}
        </span>
      ),
    },
    ...(showPreviousWorktree
      ? [
          {
            value: PREVIOUS_WORKTREE_SELECT_VALUE as EnvModeValue,
            label: <PreviousWorktreeItemContent branch={previousWorktreeBranch} />,
          },
        ]
      : []),
  ];

  return (
    <SelectMenu<EnvModeValue>
      label="Workspace"
      value={effectiveEnvMode}
      options={options}
      onValueChange={(value) => {
        if (value === PREVIOUS_WORKTREE_SELECT_VALUE) {
          onUsePreviousWorktree?.();
          return;
        }
        onEnvModeChange(value);
      }}
      className="inline-flex h-7 min-w-0 shrink items-center gap-1 rounded-pill border border-rule-soft px-2.5 text-small text-muted transition-colors duration-(--op-duration-fast) ease-out hover:bg-raised hover:text-ink motion-reduce:transition-none"
    />
  );
}
