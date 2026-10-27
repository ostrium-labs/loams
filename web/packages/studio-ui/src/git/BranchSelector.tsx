// Portions of this file are from T3 Code (https://github.com/pingdotgg/t3code),
// apps/web/src/components/BranchToolbarBranchSelector.tsx at commit 6524a8d4, Copyright (c) 2026 T3 Tools Inc., MIT licence
// (see web/packages/studio-ui/LICENSE.t3code). Modified by The Loams Authors:
// a presentational picker over loams-agentd's refs (ListRefs) on @loams/ui's Popover and Input; the selection is reported to the container, which confirms and calls SwitchRef or reuses the ref's worktree; paged refs, pull-request checkout, the draft store, toasts and the panel variant dropped. Loams' changes are Apache-2.0.

// The branch picker (§53 §5.7; SD1 Task 23). In the project's checkout a pick switches
// the branch (after a confirmation the container shows); in "New worktree" mode it picks
// the base the worktree starts from; a branch that lives in a worktree selects that
// worktree.

import { Badge, cx, Input, MiddleTruncate, Popover } from "@loams/ui";
import { ChevronDown, GitBranch } from "lucide-react";
import { useId, useState } from "react";
import {
  type BranchRef,
  type EnvMode,
  resolveBranchTriggerLabel,
  shouldIncludeBranchPickerItem,
} from "./BranchToolbar.logic";

export interface BranchSelectorProps {
  refs: readonly BranchRef[];
  /** The branch shown: the checkout's, or the chosen base in "New worktree" mode. */
  value: string | null;
  effectiveEnvMode: EnvMode;
  activeWorktreePath: string | null;
  /** The thread is running or its branch is fixed: show the branch, offer no picks. */
  locked?: boolean;
  loading?: boolean;
  onSelect: (ref: BranchRef) => void;
  className?: string;
}

const TRIGGER =
  "inline-flex h-7 min-w-0 max-w-64 items-center gap-1.5 rounded-pill border border-rule-soft px-2.5 text-small text-muted transition-colors duration-(--op-duration-fast) ease-out hover:bg-raised hover:text-ink motion-reduce:transition-none disabled:cursor-default disabled:hover:bg-transparent";

export function BranchSelector({
  refs,
  value,
  effectiveEnvMode,
  activeWorktreePath,
  locked = false,
  loading = false,
  onSelect,
  className,
}: BranchSelectorProps) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const listId = useId();
  const label = resolveBranchTriggerLabel({
    activeWorktreePath,
    effectiveEnvMode,
    resolvedActiveBranch: value,
  });
  const normalizedQuery = query.trim().toLowerCase();
  const shown = refs.filter((ref) =>
    shouldIncludeBranchPickerItem({
      itemValue: ref.name,
      normalizedQuery,
      createBranchItemValue: null,
      checkoutPullRequestItemValue: null,
    }),
  );

  const trigger = (
    <button
      type="button"
      className={cx(TRIGGER, className)}
      aria-label={`Branch: ${label}`}
      disabled={locked}
    >
      <GitBranch size={13} aria-hidden="true" className="shrink-0" />
      <span className="min-w-0 truncate">{label}</span>
      {!locked && <ChevronDown size={12} aria-hidden="true" className="shrink-0 text-faint" />}
    </button>
  );
  if (locked) return trigger;

  return (
    <Popover
      trigger={trigger}
      open={open}
      onOpenChange={(next) => {
        setOpen(next);
        if (!next) setQuery("");
      }}
      align="end"
      title={
        effectiveEnvMode === "worktree" && !activeWorktreePath ? "Start from" : "Switch branch"
      }
    >
      <Input
        aria-label="Filter branches"
        aria-controls={listId}
        placeholder="Filter branches"
        value={query}
        onChange={(event) => setQuery(event.target.value)}
      />
      <ul
        id={listId}
        aria-label="Branches"
        className="m-0 max-h-72 w-72 list-none overflow-y-auto p-0"
      >
        {loading && refs.length === 0 ? (
          <li className="px-2 py-2 text-[0.84rem] text-muted">Loading branches…</li>
        ) : shown.length === 0 ? (
          <li className="px-2 py-2 text-[0.84rem] text-muted">No branches match.</li>
        ) : (
          shown.map((ref) => (
            <li key={ref.name}>
              <button
                type="button"
                aria-current={ref.name === value ? "true" : undefined}
                className={cx(
                  "flex w-full min-w-0 items-center gap-2 px-2 py-1.5 text-left text-[0.84rem] text-ink hover:bg-raised",
                  ref.name === value && "bg-raised",
                )}
                onClick={() => {
                  setOpen(false);
                  setQuery("");
                  onSelect(ref);
                }}
              >
                <MiddleTruncate
                  value={ref.name}
                  className={/* mono-ok: id */ "min-w-0 flex-1 font-mono text-[0.8rem]"}
                />
                {ref.isDefault && <Badge>default</Badge>}
                {ref.worktreePath && <Badge title={ref.worktreePath}>worktree</Badge>}
              </button>
            </li>
          ))
        )}
      </ul>
    </Popover>
  );
}
