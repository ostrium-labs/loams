// Portions of this file are from T3 Code (https://github.com/pingdotgg/t3code),
// apps/web/src/components/GitActionsControl.tsx at commit 6524a8d4, Copyright (c) 2026 T3 Tools Inc., MIT licence
// (see web/packages/studio-ui/LICENSE.t3code). Modified by The Loams Authors:
// a presentational control over a GitStatusView whose writes go through the container's callbacks (loams-agentd's CommitWorktree, PushBranch, CreateChangeRequest, MergeBranch, DiscardPaths, FetchAll); one confirmation dialog names every write a stacked action makes (a required message, sign-off on by default, force-with-lease only, the provider, draft and linked issue); a merge dialog with the strategy and the conflict report; atoms, Effect, toasts, auth scopes, publishing, generated messages and the editor hand-off dropped; @loams/ui primitives and tokens. Loams' changes are Apache-2.0.

// The git actions (§53 §5.7, §8.2; SD1 Task 23): commit, push, open a pull request,
// merge into the base, fetch. Every write is confirmed in a dialog that names it, and
// the container passes the confirmation on to the daemon as the checksum or head the
// person saw: a stale one comes back `stale_diff` and the dialog stays open.

import {
  Button,
  Checkbox,
  ConfirmDialog,
  cx,
  Dialog,
  Field,
  Input,
  Menu,
  MenuItem,
  MenuSeparator,
  Notice,
  SelectMenu,
  Switch,
  Textarea,
  Tooltip,
} from "@loams/ui";
import {
  ArrowDown,
  ArrowUp,
  Check,
  ChevronDown,
  CloudDownload,
  CloudUpload,
  GitCommitHorizontal,
  GitMerge,
  GitPullRequest,
  Info,
  Trash2,
} from "lucide-react";
import { type ReactNode, useEffect, useId, useState } from "react";
import {
  buildMenuItems,
  type ChangeRequestProviderKind,
  GIT_ACTION_SUCCESS_VISIBLE_MS,
  type GitActionMenuItem,
  type GitDialogAction,
  type GitQuickAction,
  type GitStackedAction,
  type GitStatusView,
  type GitWriteStep,
  getMenuActionDisabledReason,
  PROVIDER_LABEL,
  requiresDefaultBranchConfirmation,
  resolveDefaultBranchActionDialogCopy,
  resolveQuickAction,
  stepsOf,
} from "./actions.logic";

export interface GitChangedFile {
  path: string;
  insertions: number;
  deletions: number;
}

/** A refused or failed write, as the container maps loams-agentd's error. */
export interface GitActionError {
  /** loams-agentd's reason (`stale_diff`, `merge_conflict`, `push_rejected`…). */
  reason?: string;
  message: string;
  /** `merge_conflict`: the conflicting paths. */
  paths?: readonly string[];
}

export type GitActionOutcome =
  | { ok: true; title: string; description?: string; url?: string }
  | {
      ok: false;
      error: GitActionError;
      /** A stack that failed part-way: the writes that did happen, dropped from a retry. */
      completed?: readonly GitWriteStep[];
    };

export interface GitCommitInput {
  message: string;
  signoff: boolean;
  /** Only these paths; unset commits every change. */
  paths?: string[];
}

export interface GitPushInput {
  /** `--force-with-lease` against this checkout's remote-tracking ref (T9-2); never a plain force. */
  forceWithLease: boolean;
}

export interface GitPullRequestInput {
  provider: ChangeRequestProviderKind;
  base: string;
  title: string;
  body: string;
  draft: boolean;
  /** An issue number the pull request closes. */
  linkIssue?: string;
}

export type MergeStrategyName = "merge" | "squash" | "rebase" | "ff";

export interface GitMergeInput {
  target: string;
  strategy: MergeStrategyName;
  message?: string;
  deleteSource: boolean;
  removeWorktree: boolean;
}

/** The writes one confirmation runs, in order. */
export interface GitRunPlan {
  steps: GitWriteStep[];
  commit?: GitCommitInput;
  push?: GitPushInput;
  pr?: GitPullRequestInput;
}

export interface GitActionsProps {
  status: GitStatusView | null;
  /** The working tree's changes, from the diff whose checksum the commit carries. */
  files: readonly GitChangedFile[];
  /** The base a pull request or a merge targets. */
  defaultBranch: string | null;
  /** The plugin holds `agentd:git-write`. */
  canWrite: boolean;
  hasPrimaryRemote?: boolean;
  /** The forges a pull request can go to; the person picks one unless the checkout's is known. */
  providers?: readonly ChangeRequestProviderKind[];
  /** A write is running (here or elsewhere for this checkout). */
  busy?: boolean;
  onRun: (plan: GitRunPlan) => Promise<GitActionOutcome>;
  onMerge: (input: GitMergeInput) => Promise<GitActionOutcome>;
  onDiscard: (paths: string[]) => Promise<GitActionOutcome>;
  onFetch?: () => Promise<GitActionOutcome>;
  /** Opens a dialog from outside (the palette, T24-6); a new nonce opens it again. */
  request?: { action: GitDialogAction; nonce: number } | null;
  /** Whether a worktree removal can be offered with a merge (the source is a worktree). */
  sourceIsWorktree?: boolean;
}

const SPLIT =
  "inline-flex h-7 items-center gap-1.5 border border-rule-soft px-2.5 text-small text-ink transition-colors duration-(--op-duration-fast) ease-out hover:bg-raised motion-reduce:transition-none disabled:cursor-default disabled:text-faint disabled:hover:bg-transparent";

const WRITE_REFUSED = "Studio needs the agentd:git-write permission to write to git.";

function QuickActionIcon({ quickAction }: { quickAction: GitQuickAction }) {
  const size = 13;
  if (quickAction.kind === "run_pull") return <CloudDownload size={size} aria-hidden="true" />;
  if (quickAction.kind === "run_action") {
    if (quickAction.action === "commit")
      return <GitCommitHorizontal size={size} aria-hidden="true" />;
    if (quickAction.action === "push" || quickAction.action === "commit_push") {
      return <CloudUpload size={size} aria-hidden="true" />;
    }
    return <GitPullRequest size={size} aria-hidden="true" />;
  }
  if (quickAction.label === "Commit") return <GitCommitHorizontal size={size} aria-hidden="true" />;
  if (quickAction.label === "Push") return <CloudUpload size={size} aria-hidden="true" />;
  return <Info size={size} aria-hidden="true" />;
}

function MenuIcon({ item }: { item: GitActionMenuItem }) {
  const size = 14;
  if (item.icon === "commit") return <GitCommitHorizontal size={size} aria-hidden="true" />;
  if (item.icon === "push") return <CloudUpload size={size} aria-hidden="true" />;
  if (item.icon === "merge") return <GitMerge size={size} aria-hidden="true" />;
  return <GitPullRequest size={size} aria-hidden="true" />;
}

/** Ahead and behind, and the checkout's pull request. */
export function GitStatusChips({ status }: { status: GitStatusView | null }) {
  if (!status) return null;
  const pr = status.pr;
  return (
    <span className="inline-flex shrink-0 items-center gap-2 text-[0.76rem] text-muted tabular-nums">
      {status.aheadCount > 0 && (
        <span className="inline-flex items-center gap-0.5" title={`${status.aheadCount} ahead`}>
          <ArrowUp size={12} aria-hidden="true" />
          <span className="sr-only">ahead </span>
          {status.aheadCount}
        </span>
      )}
      {status.behindCount > 0 && (
        <span className="inline-flex items-center gap-0.5" title={`${status.behindCount} behind`}>
          <ArrowDown size={12} aria-hidden="true" />
          <span className="sr-only">behind </span>
          {status.behindCount}
        </span>
      )}
      {pr && <ChangeRequestChip pr={pr} />}
    </span>
  );
}

function httpUrl(url: string): string | undefined {
  try {
    const u = new URL(url);
    return u.protocol === "https:" || u.protocol === "http:" ? url : undefined;
  } catch {
    return undefined;
  }
}

function ChangeRequestChip({ pr }: { pr: NonNullable<GitStatusView["pr"]> }) {
  const href = httpUrl(pr.url);
  const body = (
    <>
      <GitPullRequest size={12} aria-hidden="true" />
      PR #{pr.number}
      <span className="text-faint">{pr.state}</span>
    </>
  );
  const className =
    "inline-flex items-center gap-1 rounded-pill border border-rule-soft px-2 py-0.5 text-muted no-underline hover:bg-raised hover:text-ink";
  return href ? (
    <a href={href} target="_blank" rel="noopener noreferrer" title={pr.title} className={className}>
      {body}
    </a>
  ) : (
    <span title={pr.title} className={className}>
      {body}
    </span>
  );
}

function ErrorNotice({ error }: { error: GitActionError }) {
  if (error.reason === "merge_conflict") {
    return (
      <Notice tone="danger" title="The merge stopped on conflicts">
        <p className="m-0">Nothing was merged and the tree is clean. These paths conflict:</p>
        <ul
          aria-label="Conflicting paths"
          className={/* mono-ok: path */ "m-0 mt-1 list-none p-0 font-mono text-[0.78rem]"}
        >
          {(error.paths ?? []).map((path) => (
            <li key={path}>{path}</li>
          ))}
        </ul>
      </Notice>
    );
  }
  if (error.reason === "stale_diff") {
    return (
      <Notice tone="warn" title="The changes moved since you opened this">
        Nothing was written. Review what is shown now and confirm again.
      </Notice>
    );
  }
  const titles: Record<string, string> = {
    push_rejected: "The remote refused the push",
    provider_not_configured: "No credential for this forge",
    nothing_to_commit: "Nothing to commit",
    permission_denied: "Not allowed",
  };
  return (
    <Notice tone="danger" title={(error.reason && titles[error.reason]) ?? "The git action failed"}>
      {error.message}
    </Notice>
  );
}

function confirmLabel(steps: readonly GitWriteStep[]): string {
  const words: Record<GitWriteStep, string> = {
    commit: "Commit",
    push: "push",
    create_pr: "open PR",
  };
  const parts = steps.map((step, i) => {
    const word = words[step];
    return i === 0 ? word.charAt(0).toUpperCase() + word.slice(1) : word;
  });
  if (parts.length <= 1) return parts[0] ?? "Confirm";
  return `${parts.slice(0, -1).join(", ")} & ${parts.at(-1)}`;
}

function dialogTitle(steps: readonly GitWriteStep[]): string {
  if (steps.length === 1 && steps[0] === "commit") return "Commit changes";
  if (steps.length === 1 && steps[0] === "push") return "Push";
  if (steps.length === 1 && steps[0] === "create_pr") return "Open a pull request";
  return confirmLabel(steps);
}

interface WriteDialogProps {
  open: boolean;
  action: GitStackedAction;
  status: GitStatusView;
  files: readonly GitChangedFile[];
  defaultBranch: string | null;
  providers: readonly ChangeRequestProviderKind[];
  onClose: () => void;
  onRun: GitActionsProps["onRun"];
  onDiscard: GitActionsProps["onDiscard"];
  onDone: (outcome: Extract<GitActionOutcome, { ok: true }>) => void;
}

function WriteDialog({
  open,
  action,
  status,
  files,
  defaultBranch,
  providers,
  onClose,
  onRun,
  onDiscard,
  onDone,
}: WriteDialogProps) {
  // The writes a stack already made before one failed; a retry runs the rest.
  const [completed, setCompleted] = useState<readonly GitWriteStep[]>([]);
  const planned = stepsOf(action, {
    hasWorkingTreeChanges: status.hasWorkingTreeChanges,
    needsPush: !status.hasUpstream || status.aheadCount > 0,
  });
  const steps = planned.filter((step) => !completed.includes(step));
  const branch = status.refName ?? "(detached HEAD)";
  const knownProvider = status.pr?.provider ?? status.provider ?? null;
  const [message, setMessage] = useState("");
  const [signoff, setSignoff] = useState(true);
  const [excluded, setExcluded] = useState<ReadonlySet<string>>(new Set());
  const [forceWithLease, setForceWithLease] = useState(false);
  const [provider, setProvider] = useState<ChangeRequestProviderKind | null>(
    knownProvider ?? (providers.length === 1 ? (providers[0] ?? null) : null),
  );
  const [base, setBase] = useState(defaultBranch ?? "");
  const [title, setTitle] = useState("");
  const [body, setBody] = useState("");
  const [draft, setDraft] = useState(false);
  const [linkIssue, setLinkIssue] = useState("");
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<GitActionError | null>(null);
  const [discarding, setDiscarding] = useState<string | null>(null);
  const formId = useId();

  const commits = steps.includes("commit");
  const pushes = steps.includes("push");
  const opensPr = steps.includes("create_pr");
  const selected = files.filter((f) => !excluded.has(f.path));
  const issueOk = linkIssue.trim() === "" || /^\d{1,9}$/.test(linkIssue.trim());
  const prTitle = title.trim() || message.trim().split("\n")[0] || status.refName || "";
  const ready =
    !pending &&
    (!commits || (message.trim().length > 0 && selected.length > 0)) &&
    (!opensPr || (provider !== null && base.trim().length > 0 && prTitle.length > 0 && issueOk));
  const defaultCopy =
    action !== "commit" && requiresDefaultBranchConfirmation(action, status.isDefaultRef)
      ? resolveDefaultBranchActionDialogCopy({
          action,
          branchName: branch,
          includesCommit: commits,
        })
      : null;

  const submit = async () => {
    if (!ready) return;
    setPending(true);
    setError(null);
    const plan: GitRunPlan = { steps };
    if (commits) {
      plan.commit = {
        message: message.trim(),
        signoff,
        ...(excluded.size > 0 ? { paths: selected.map((f) => f.path) } : {}),
      };
    }
    if (pushes) plan.push = { forceWithLease };
    if (opensPr && provider) {
      plan.pr = {
        provider,
        base: base.trim(),
        title: prTitle,
        body,
        draft,
        ...(linkIssue.trim() ? { linkIssue: linkIssue.trim() } : {}),
      };
    }
    const outcome = await onRun(plan);
    setPending(false);
    if (outcome.ok) onDone(outcome);
    else {
      setError(outcome.error);
      if (outcome.completed) setCompleted((prev) => [...prev, ...(outcome.completed ?? [])]);
    }
  };

  return (
    <>
      <Dialog
        open={open}
        onClose={onClose}
        dismissible={!pending}
        title={defaultCopy?.title ?? dialogTitle(steps)}
        footer={
          <>
            <Button variant="quiet" onClick={onClose} disabled={pending}>
              Cancel
            </Button>
            <Button
              variant="primary"
              type="submit"
              form={formId}
              disabled={!ready}
              aria-busy={pending || undefined}
            >
              {defaultCopy?.continueLabel ?? confirmLabel(steps)}
            </Button>
          </>
        }
      >
        <form
          id={formId}
          className="flex w-full min-w-0 flex-col gap-3"
          onSubmit={(event) => {
            event.preventDefault();
            void submit();
          }}
        >
          {defaultCopy && (
            <Notice tone="warn" title="This is the default branch">
              {defaultCopy.description}
            </Notice>
          )}
          <dl className="m-0 grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-[0.84rem]">
            <dt className="text-muted">Branch</dt>
            <dd className={/* mono-ok: id */ "m-0 font-mono text-ink"}>{branch}</dd>
            {pushes && (
              <>
                <dt className="text-muted">Push to</dt>
                <dd className={/* mono-ok: id */ "m-0 font-mono text-ink"}>
                  origin/{status.refName ?? ""}
                </dd>
              </>
            )}
          </dl>

          {commits && (
            <fieldset className="m-0 flex flex-col gap-2 border-0 p-0">
              <legend className="mb-1 text-[0.84rem] font-medium text-ink">
                Files ({selected.length} of {files.length})
              </legend>
              {files.length === 0 ? (
                <p className="m-0 text-[0.84rem] text-muted">No changes.</p>
              ) : (
                <ul
                  aria-label="Changed files"
                  className="m-0 max-h-44 list-none overflow-y-auto border border-rule-soft p-1"
                >
                  {files.map((file) => {
                    const isExcluded = excluded.has(file.path);
                    return (
                      <li key={file.path} className="flex min-w-0 items-center gap-2 px-1 py-0.5">
                        <Checkbox
                          label={<span className="sr-only">Include {file.path}</span>}
                          checked={!isExcluded}
                          onChange={() =>
                            setExcluded((prev) => {
                              const next = new Set(prev);
                              if (next.has(file.path)) next.delete(file.path);
                              else next.add(file.path);
                              return next;
                            })
                          }
                        />
                        <span
                          className={cx(
                            /* mono-ok: path */ "min-w-0 flex-1 truncate font-mono text-[0.78rem]",
                            isExcluded ? "text-faint" : "text-ink",
                          )}
                          title={file.path}
                        >
                          {file.path}
                        </span>
                        <span className="shrink-0 text-[0.74rem] tabular-nums">
                          <span className="text-grow">+{file.insertions}</span>
                          <span className="text-faint"> / </span>
                          <span className="text-danger">-{file.deletions}</span>
                        </span>
                        <button
                          type="button"
                          className="shrink-0 p-0.5 text-faint hover:text-danger"
                          aria-label={`Discard changes to ${file.path}`}
                          title="Discard changes"
                          onClick={() => setDiscarding(file.path)}
                        >
                          <Trash2 size={13} aria-hidden="true" />
                        </button>
                      </li>
                    );
                  })}
                </ul>
              )}
              <Field label="Commit message">
                {(p) => (
                  <Textarea
                    {...p}
                    rows={3}
                    required
                    value={message}
                    onChange={(event) => setMessage(event.target.value)}
                    placeholder="What this change does"
                  />
                )}
              </Field>
              <Switch
                label="Sign off (Signed-off-by)"
                checked={signoff}
                onCheckedChange={setSignoff}
              />
            </fieldset>
          )}

          {pushes && (
            <div className="flex flex-col gap-1">
              <Checkbox
                label="Force with lease"
                checked={forceWithLease}
                onChange={(event) => setForceWithLease(event.target.checked)}
              />
              <p className="m-0 ml-6 text-[0.78rem] text-muted">
                Overwrites the remote branch only if it is still where this checkout last fetched
                it. Studio never force-pushes without a lease.
              </p>
            </div>
          )}

          {opensPr && (
            <fieldset className="m-0 flex flex-col gap-2 border-0 p-0">
              <legend className="mb-1 text-[0.84rem] font-medium text-ink">Pull request</legend>
              <div className="flex flex-col gap-1">
                <span className="text-[0.84rem] text-muted">Forge</span>
                <SelectMenu<ChangeRequestProviderKind>
                  label="Forge"
                  placeholder="Choose Forgejo or GitHub"
                  value={provider}
                  onValueChange={setProvider}
                  options={providers.map((p) => ({ value: p, label: PROVIDER_LABEL[p] }))}
                />
              </div>
              <Field label="Base">
                {(p) => (
                  <Input {...p} value={base} onChange={(event) => setBase(event.target.value)} />
                )}
              </Field>
              <Field label="Title">
                {(p) => (
                  <Input
                    {...p}
                    value={title}
                    placeholder={prTitle || "Pull request title"}
                    onChange={(event) => setTitle(event.target.value)}
                  />
                )}
              </Field>
              <Field label="Description">
                {(p) => (
                  <Textarea
                    {...p}
                    rows={3}
                    value={body}
                    onChange={(event) => setBody(event.target.value)}
                  />
                )}
              </Field>
              <Field label="Closes issue" error={issueOk ? undefined : "An issue number, like 42."}>
                {(p) => (
                  <Input
                    {...p}
                    inputMode="numeric"
                    placeholder="Optional issue number"
                    value={linkIssue}
                    onChange={(event) => setLinkIssue(event.target.value)}
                  />
                )}
              </Field>
              <Switch label="Draft" checked={draft} onCheckedChange={setDraft} />
            </fieldset>
          )}

          {error && <ErrorNotice error={error} />}
        </form>
      </Dialog>
      <ConfirmDialog
        open={discarding !== null}
        onClose={() => setDiscarding(null)}
        object="these changes"
        confirm="Discard"
        impact={
          <>
            The working-tree changes to <code>{discarding}</code> are lost. This cannot be undone.
          </>
        }
        onConfirm={async () => {
          const path = discarding;
          setDiscarding(null);
          if (!path) return;
          const outcome = await onDiscard([path]);
          if (!outcome.ok) setError(outcome.error);
          else
            setExcluded((prev) => {
              const next = new Set(prev);
              next.delete(path);
              return next;
            });
        }}
      />
    </>
  );
}

const STRATEGIES: { value: MergeStrategyName; label: string }[] = [
  { value: "merge", label: "Merge commit" },
  { value: "squash", label: "Squash" },
  { value: "rebase", label: "Rebase" },
  { value: "ff", label: "Fast-forward only" },
];

function MergeDialog({
  open,
  status,
  defaultBranch,
  sourceIsWorktree,
  onClose,
  onMerge,
  onDone,
}: {
  open: boolean;
  status: GitStatusView;
  defaultBranch: string | null;
  sourceIsWorktree: boolean;
  onClose: () => void;
  onMerge: GitActionsProps["onMerge"];
  onDone: (outcome: Extract<GitActionOutcome, { ok: true }>) => void;
}) {
  const [target, setTarget] = useState(defaultBranch ?? "");
  const [strategy, setStrategy] = useState<MergeStrategyName>("merge");
  const [message, setMessage] = useState("");
  const [deleteSource, setDeleteSource] = useState(false);
  const [removeWorktree, setRemoveWorktree] = useState(false);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<GitActionError | null>(null);
  const formId = useId();
  const source = status.refName ?? "";
  const ready = !pending && source !== "" && target.trim() !== "" && target.trim() !== source;
  const submit = async () => {
    if (!ready) return;
    setPending(true);
    setError(null);
    const outcome = await onMerge({
      target: target.trim(),
      strategy,
      ...(message.trim() && strategy !== "ff" && strategy !== "rebase"
        ? { message: message.trim() }
        : {}),
      deleteSource,
      removeWorktree: sourceIsWorktree && removeWorktree,
    });
    setPending(false);
    if (outcome.ok) onDone(outcome);
    else setError(outcome.error);
  };
  return (
    <Dialog
      open={open}
      onClose={onClose}
      dismissible={!pending}
      title={`Merge ${source} into ${target || "the base"}`}
      footer={
        <>
          <Button variant="quiet" onClick={onClose} disabled={pending}>
            Cancel
          </Button>
          <Button
            variant="primary"
            type="submit"
            form={formId}
            disabled={!ready}
            aria-busy={pending || undefined}
          >
            Merge
          </Button>
        </>
      }
    >
      <form
        id={formId}
        className="flex w-full min-w-0 flex-col gap-3"
        onSubmit={(event) => {
          event.preventDefault();
          void submit();
        }}
      >
        <Field label="Into">
          {(p) => (
            <Input {...p} value={target} onChange={(event) => setTarget(event.target.value)} />
          )}
        </Field>
        <div className="flex flex-col gap-1">
          <span className="text-[0.84rem] text-muted">Strategy</span>
          <SelectMenu<MergeStrategyName>
            label="Strategy"
            value={strategy}
            onValueChange={setStrategy}
            options={STRATEGIES}
          />
        </div>
        {(strategy === "merge" || strategy === "squash") && (
          <Field label="Message" hint="Optional; git's default otherwise.">
            {(p) => (
              <Input {...p} value={message} onChange={(event) => setMessage(event.target.value)} />
            )}
          </Field>
        )}
        <Checkbox
          label={`Delete ${source || "the branch"} after merging`}
          checked={deleteSource}
          onChange={(event) => setDeleteSource(event.target.checked)}
        />
        {sourceIsWorktree && (
          <Checkbox
            label="Remove its worktree after merging"
            checked={removeWorktree}
            onChange={(event) => setRemoveWorktree(event.target.checked)}
          />
        )}
        <p className="m-0 text-[0.78rem] text-muted">
          On a conflict the merge stops, nothing is merged and the tree is left clean.
        </p>
        {error && <ErrorNotice error={error} />}
      </form>
    </Dialog>
  );
}

type OpenDialog =
  | { kind: "write"; action: GitStackedAction; nonce: number }
  | { kind: "merge"; nonce: number };

export function GitActions({
  status,
  files,
  defaultBranch,
  canWrite,
  hasPrimaryRemote = true,
  providers = ["forgejo", "github"],
  busy = false,
  onRun,
  onMerge,
  onDiscard,
  onFetch,
  request = null,
  sourceIsWorktree = false,
}: GitActionsProps) {
  const [dialog, setDialog] = useState<OpenDialog | null>(null);
  const [success, setSuccess] = useState<{ title: string; description?: string } | null>(null);
  const [fetchError, setFetchError] = useState<string | null>(null);
  const [fetching, setFetching] = useState(false);
  const isBusy = busy || fetching;
  const quickAction = resolveQuickAction(
    status,
    isBusy,
    status?.isDefaultRef ?? false,
    hasPrimaryRemote,
  );
  const items = buildMenuItems(status, isBusy, hasPrimaryRemote);

  const open = (action: GitStackedAction | "merge") => {
    if (!canWrite || !status) return;
    setSuccess(null);
    setDialog((prev) =>
      action === "merge"
        ? { kind: "merge", nonce: (prev?.nonce ?? 0) + 1 }
        : { kind: "write", action, nonce: (prev?.nonce ?? 0) + 1 },
    );
  };

  // The palette asks for a dialog (T24-6): the same confirmation as the toolbar's.
  const requestNonce = request?.nonce;
  const requestAction = request?.action;
  // oxlint-disable-next-line react/exhaustive-deps -- a new nonce is the trigger.
  useEffect(() => {
    if (requestAction) open(requestAction);
  }, [requestNonce]);

  useEffect(() => {
    if (!success) return;
    const id = setTimeout(() => setSuccess(null), GIT_ACTION_SUCCESS_VISIBLE_MS);
    return () => clearTimeout(id);
  }, [success]);

  const done = (outcome: Extract<GitActionOutcome, { ok: true }>) => {
    setDialog(null);
    setSuccess({
      title: outcome.title,
      ...(outcome.description ? { description: outcome.description } : {}),
    });
  };

  const quickReason = !canWrite
    ? WRITE_REFUSED
    : quickAction.kind === "run_pull"
      ? "Behind upstream. Pull or rebase in a terminal; Studio has no pull yet."
      : quickAction.kind === "open_publish"
        ? 'Add an "origin" remote to publish this repository.'
        : quickAction.disabled
          ? (quickAction.hint ?? null)
          : null;
  const quickButton = (
    <button
      type="button"
      className={cx(SPLIT, "rounded-l-pill border-r-0")}
      aria-disabled={quickReason ? true : undefined}
      onClick={() => {
        if (quickReason || quickAction.kind !== "run_action" || !quickAction.action) return;
        open(quickAction.action);
      }}
    >
      <QuickActionIcon quickAction={quickAction} />
      <span className="truncate">{quickAction.label}</span>
    </button>
  );

  const runFetch = async () => {
    if (!onFetch) return;
    setFetching(true);
    setFetchError(null);
    const outcome = await onFetch();
    setFetching(false);
    if (outcome.ok) setSuccess({ title: outcome.title });
    else setFetchError(outcome.error.message);
  };

  let menuItems: ReactNode = items.map((item) => {
    const reason = canWrite
      ? getMenuActionDisabledReason({ item, gitStatus: status, isBusy, hasPrimaryRemote })
      : WRITE_REFUSED;
    return (
      <MenuItem
        key={item.id}
        disabled={reason !== null}
        onSelect={() => item.dialogAction && open(item.dialogAction)}
      >
        <MenuIcon item={item} />
        <span className="flex min-w-0 flex-col">
          <span>{item.label}</span>
          {reason && <span className="text-[0.74rem] text-muted">{reason}</span>}
        </span>
      </MenuItem>
    );
  });
  if (onFetch) {
    menuItems = (
      <>
        {menuItems}
        <MenuSeparator />
        <MenuItem disabled={isBusy} onSelect={() => void runFetch()}>
          <CloudDownload size={14} aria-hidden="true" />
          Fetch
        </MenuItem>
      </>
    );
  }

  return (
    <span className="inline-flex min-w-0 items-center gap-2">
      <span className="inline-flex min-w-0 items-center" data-studio-git-actions>
        {quickReason ? <Tooltip content={quickReason}>{quickButton}</Tooltip> : quickButton}
        <Menu
          label="Git actions"
          trigger={
            <button
              type="button"
              className={cx(SPLIT, "rounded-r-pill px-1.5")}
              aria-label="Git action options"
              disabled={isBusy}
            >
              <ChevronDown size={13} aria-hidden="true" />
            </button>
          }
        >
          {menuItems}
        </Menu>
      </span>
      <span aria-live="polite" className="inline-flex min-w-0 items-center">
        {success && (
          <span
            className="inline-flex min-w-0 items-center gap-1 text-small text-grow"
            title={success.description}
          >
            <Check size={13} aria-hidden="true" />
            <span className="truncate">{success.title}</span>
          </span>
        )}
        {fetchError && <span className="truncate text-small text-danger">{fetchError}</span>}
      </span>
      {status && dialog?.kind === "write" && (
        <WriteDialog
          key={dialog.nonce}
          open
          action={dialog.action}
          status={status}
          files={files}
          defaultBranch={defaultBranch}
          providers={providers}
          onClose={() => setDialog(null)}
          onRun={onRun}
          onDiscard={onDiscard}
          onDone={done}
        />
      )}
      {status && dialog?.kind === "merge" && (
        <MergeDialog
          key={dialog.nonce}
          open
          status={status}
          defaultBranch={defaultBranch}
          sourceIsWorktree={sourceIsWorktree}
          onClose={() => setDialog(null)}
          onMerge={onMerge}
          onDone={done}
        />
      )}
    </span>
  );
}
