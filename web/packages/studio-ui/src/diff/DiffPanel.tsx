// Portions of this file are from T3 Code (https://github.com/pingdotgg/t3code),
// apps/web/src/components/DiffPanel.tsx at commit 6524a8d4, Copyright (c) 2026 T3 Tools Inc., MIT licence
// (see web/packages/studio-ui/LICENSE.t3code). Modified by The Loams Authors:
// data from the DiffSource seam (agentd's DiffService, ReviewService and GitService) with TanStack Query in place of atoms and Effect; agentd's four sources (working tree, branch against a base, commit, latest turn); @loams/ui primitives on tokens; review comments stored by the daemon with "send to the agent" and "post to PR"; discard with confirmation and the expected checksum; viewed marks and word diff added; no find-in-diff (§53 §9.1), editor hand-off, context menu, settings store or lazy per-file patches; the panel's state kept per source key and an unchanged snapshot kept as is, so a draft survives the thread's updates. Loams' changes are Apache-2.0.

// The thread panel's diff (SD1 Task 20; §53 §5.6, D905). The working tree against HEAD is
// the default; a branch against a base, one commit, or the latest turn are the other
// sources. Comments are anchored to the diff the reader sees and stored by the daemon per
// checkout; "Send to the agent" queues one run on the thread, "Post to PR" posts them as a
// review when a pull request is open. Discard restores files only when the diff the reader
// confirmed is still the checkout's (the expected checksum; `stale_diff` otherwise).

import {
  Button,
  ConfirmDialog,
  cx,
  Empty,
  Notice,
  Segmented,
  Select,
  Toggle,
  Tooltip,
} from "@loams/ui";
import type { FileDiffMetadata } from "@pierre/diffs";
import { useQuery } from "@tanstack/react-query";
import {
  ChevronsDownUpIcon,
  ChevronsUpDownIcon,
  FolderTreeIcon,
  PilcrowIcon,
  SendIcon,
  TextWrapIcon,
  Trash2Icon,
  UploadIcon,
  WholeWordIcon,
} from "lucide-react";
import { type ReactNode, useCallback, useEffect, useMemo, useRef, useState } from "react";

import { useColourScheme } from "../files/codeTheme";
import { DiffPanelLoadingState, DiffPanelShell } from "./DiffPanelShell";
import { areAllDiffFilesCollapsed, toggleAllDiffFiles } from "./diffCollapse";
import { createDiffFileContentsLoader } from "./diffFileContents";
import { type DiffLayout, selectThreadDiffSelection, useDiffPanelStore } from "./diffPanelStore";
import {
  buildFileDiffContentVersion,
  buildFileDiffIdentityKey,
  getRenderablePatch,
  resolveFileDiffPath,
} from "./diffRendering";
import {
  AnnotatableCodeView,
  type AnnotatableFile,
  type CommentSave,
} from "./review/AnnotatableCodeView";
import {
  type ChangeRequestInfo,
  type CheckoutDiffView,
  type DiffSelection,
  type DiffSource,
  type DiffSourceKind,
  type ReviewComment,
  StaleDiffError,
} from "./source";
import { DiffFileTree } from "./tree/DiffFileTree";
import { diffFileTreeEntries } from "./tree/diffFileTree.logic";

const SOURCE_LABEL: Record<DiffSourceKind, string> = {
  working: "Working tree",
  branch: "Branch",
  commit: "Commit",
  turn: "Latest turn",
};

const PREFERRED_BASES = ["main", "master", "dev", "trunk", "origin/main", "origin/master"];

const fileEntryCache = new WeakMap<
  FileDiffMetadata,
  { fileDiff: FileDiffMetadata; fileKey: string; fileVersion: number }
>();

function getCachedFileEntry(fileDiff: FileDiffMetadata) {
  const cached = fileEntryCache.get(fileDiff);
  if (cached) return cached;
  const entry = {
    fileDiff,
    fileKey: buildFileDiffIdentityKey(fileDiff),
    fileVersion: buildFileDiffContentVersion(fileDiff),
  };
  fileEntryCache.set(fileDiff, entry);
  return entry;
}

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** The same snapshot of the same checkout: what the reader sees would not change. */
function sameDiff(a: CheckoutDiffView, b: CheckoutDiffView): boolean {
  return (
    a.checkoutId === b.checkoutId &&
    a.cwd === b.cwd &&
    a.checksum === b.checksum &&
    a.truncated === b.truncated &&
    a.patch === b.patch
  );
}

/** The base a branch diff compares against when the reader has not picked one. */
export function defaultBaseRef(branches: readonly string[]): string | null {
  for (const preferred of PREFERRED_BASES) if (branches.includes(preferred)) return preferred;
  return branches[0] ?? null;
}

type Pending =
  | { readonly kind: "discard-all" }
  | { readonly kind: "discard-file"; readonly path: string };

export interface DiffPanelProps {
  readonly threadId: string;
  readonly source: DiffSource;
  /** Opens a path in the Files tab. */
  readonly onOpenFile?: (path: string) => void;
  readonly className?: string;
}

export function DiffPanel({ threadId, source, onOpenFile, className }: DiffPanelProps) {
  const scheme = useColourScheme();
  const selection = useDiffPanelStore((state) =>
    selectThreadDiffSelection(state.byThread, threadId),
  );
  const view = useDiffPanelStore((state) => state.view);
  const viewedPaths = useDiffPanelStore((state) => state.viewedByThread[threadId]);
  const { select, selectBranchBaseRef, setView, setViewed } = useDiffPanelStore.getState();

  // The panel's diff, comments and drafts belong to the source's key (the daemon and the
  // thread). A host may hand over a new source object for the same key, as the thread's row
  // changes (its branch renamed after the first turn, its checkout stamped): the watches
  // stay open and the viewer stays mounted, so a half-written comment is kept. Calls that
  // read the thread (the open pull request, the branches) take the newest object.
  const [keyed, setKeyed] = useState(source);
  if (keyed.key !== source.key) setKeyed(source);
  const stable = keyed.key === source.key ? keyed : source;

  // The working tree's diff follows the daemon's watch. A snapshot with the diff the panel
  // already shows (the watch's first frame after the one-shot read, or any re-send) keeps
  // the current object, so the viewer does not render the file again.
  const [working, setWorking] = useState<{ diff?: CheckoutDiffView; error?: Error }>({});
  useEffect(() => {
    setWorking({});
    return stable.watchWorkingDiff(
      (diff) =>
        setWorking((current) =>
          current.diff && sameDiff(current.diff, diff)
            ? current.error
              ? { diff: current.diff }
              : current
            : { diff },
        ),
      (error) => setWorking((current) => ({ ...current, error })),
    );
  }, [stable]);

  const branches = useQuery({
    queryKey: ["studio-diff", source.key, "branches"],
    queryFn: ({ signal }) => source.listBranches(signal),
    enabled: selection.kind === "branch",
    staleTime: 30_000,
  });
  const commits = useQuery({
    queryKey: ["studio-diff", source.key, "commits"],
    queryFn: ({ signal }) => source.listCommits(signal),
    enabled: selection.kind === "commit",
    staleTime: 30_000,
  });

  // A branch or commit source needs its ref; until the reader picks one, the likely one.
  const effective = useMemo<DiffSelection | null>(() => {
    switch (selection.kind) {
      case "branch": {
        const baseRef = selection.baseRef ?? defaultBaseRef(branches.data ?? []);
        return baseRef ? { kind: "branch", baseRef } : null;
      }
      case "commit": {
        const commitSha = selection.commitSha ?? commits.data?.[0]?.sha ?? null;
        return commitSha ? { kind: "commit", commitSha } : null;
      }
      default:
        return selection;
    }
  }, [branches.data, commits.data, selection]);

  const other = useQuery({
    queryKey: ["studio-diff", source.key, "diff", effective],
    queryFn: ({ signal }) => source.getDiff(effective as DiffSelection, signal),
    enabled: effective !== null && effective.kind !== "working",
    retry: false,
  });

  const diff = selection.kind === "working" ? working.diff : other.data;
  const diffError =
    selection.kind === "working"
      ? working.diff
        ? undefined
        : working.error
      : (other.error ?? branches.error ?? commits.error ?? undefined);
  const loading =
    selection.kind === "working"
      ? !working.diff && !working.error
      : other.isPending &&
        (effective !== null || branches.isPending || commits.isPending) &&
        !diffError;
  const checkoutId = diff?.checkoutId ?? working.diff?.checkoutId ?? null;

  // The checkout's comments.
  const [comments, setComments] = useState<readonly ReviewComment[]>([]);
  const [commentsError, setCommentsError] = useState<Error | null>(null);
  useEffect(() => {
    setComments([]);
    setCommentsError(null);
    if (!checkoutId) return;
    return stable.watchComments(checkoutId, setComments, setCommentsError);
  }, [checkoutId, stable]);

  // The open pull request, for "Post to PR".
  const [changeRequest, setChangeRequest] = useState<ChangeRequestInfo | null>(null);
  useEffect(() => source.watchChangeRequest(setChangeRequest), [source]);

  const renderable = useMemo(
    () =>
      getRenderablePatch(diff?.patch, `studio-diff:${diff?.checksum ?? ""}`, {
        ignoreWhitespace: view.ignoreWhitespace,
      }),
    [diff?.checksum, diff?.patch, view.ignoreWhitespace],
  );
  const renderableFiles = useMemo(
    () => (renderable?.kind === "files" ? renderable.files : []),
    [renderable],
  );
  const stats = useMemo(() => new Map(diff?.files.map((file) => [file.path, file])), [diff?.files]);
  const files = useMemo<AnnotatableFile[]>(
    () =>
      renderableFiles.map((fileDiff) => {
        const entry = getCachedFileEntry(fileDiff);
        const filePath = resolveFileDiffPath(fileDiff);
        const stat = stats.get(filePath);
        return { ...entry, filePath, ...(stat ? { stat } : {}) };
      }),
    [renderableFiles, stats],
  );
  const fileKeys = useMemo(() => files.map((file) => file.fileKey), [files]);
  const treeEntries = useMemo(() => diffFileTreeEntries(renderableFiles), [renderableFiles]);

  // Folding is per source: a new source starts unfolded, apart from files already viewed.
  const scopeKey = `${threadId}:${JSON.stringify(effective)}`;
  const [collapsedState, setCollapsedState] = useState<{
    scopeKey: string;
    keys: ReadonlySet<string>;
  }>({ scopeKey, keys: new Set() });
  const isViewed = useCallback(
    (file: AnnotatableFile) => viewedPaths?.[file.filePath] === file.fileVersion,
    [viewedPaths],
  );
  const defaultCollapsed = useMemo(
    () => new Set(files.filter(isViewed).map((file) => file.fileKey)),
    [files, isViewed],
  );
  const collapsed = collapsedState.scopeKey === scopeKey ? collapsedState.keys : defaultCollapsed;
  const updateCollapsed = (update: (keys: Set<string>) => void) =>
    setCollapsedState((current) => {
      const keys = new Set(current.scopeKey === scopeKey ? current.keys : defaultCollapsed);
      update(keys);
      return { scopeKey, keys };
    });
  const allCollapsed = areAllDiffFilesCollapsed(fileKeys, collapsed);

  const loadDiffFiles = useMemo(
    () =>
      diff && effective && !diff.truncated
        ? createDiffFileContentsLoader(stable, effective, diff)
        : undefined,
    [diff, effective, stable],
  );

  // Review actions and discards report here.
  const [notice, setNotice] = useState<{ tone: "info" | "danger"; text: string } | null>(null);
  const [busy, setBusy] = useState(false);
  const [pending, setPending] = useState<Pending | null>(null);

  const run = async (action: () => Promise<string | undefined>) => {
    setBusy(true);
    setNotice(null);
    try {
      const done = await action();
      if (done) setNotice({ tone: "info", text: done });
    } catch (error) {
      setNotice({
        tone: "danger",
        text: error instanceof StaleDiffError ? error.message : errorText(error),
      });
    } finally {
      setBusy(false);
    }
  };

  const open = comments.filter((comment) => !comment.resolved);
  const postable = open.filter((comment) => !comment.outdated);
  const outdatedCount = open.length - postable.length;
  const canComment = Boolean(diff && checkoutId);

  const onSave = async (save: CommentSave) => {
    if (!diff || !checkoutId) return;
    const existing = save.id ? comments.find((comment) => comment.id === save.id) : undefined;
    try {
      await source.upsertComment({
        // A new comment has no id: the daemon assigns one, and refuses an id it does not
        // know (UpsertReviewComment, G9; SD1 Task 29 found the client inventing one).
        id: save.id ?? "",
        checkoutId,
        path: save.path,
        side: save.anchor.side,
        line: save.anchor.line,
        ...(save.anchor.endLine !== undefined ? { endLine: save.anchor.endLine } : {}),
        body: save.body,
        diffChecksum: existing?.diffChecksum ?? diff.checksum,
        ...(existing ? { createdAtMs: existing.createdAtMs, resolved: existing.resolved } : {}),
      });
    } catch (error) {
      setNotice({ tone: "danger", text: errorText(error) });
      throw error;
    }
  };

  const scroller = useRef<HTMLDivElement>(null);
  const [selectedPath, setSelectedPath] = useState<string | null>(null);
  const revealFile = (path: string) => {
    setSelectedPath(path);
    const file = files.find((candidate) => candidate.filePath === path);
    if (file && collapsed.has(file.fileKey)) updateCollapsed((keys) => keys.delete(file.fileKey));
    requestAnimationFrame(() => {
      for (const element of scroller.current?.querySelectorAll<HTMLElement>("[data-diff-file]") ??
        []) {
        if (element.dataset.diffFile === path) element.scrollIntoView?.({ block: "start" });
      }
    });
  };

  const working_ = selection.kind === "working";
  const header = (
    <>
      <div className="flex min-w-0 flex-wrap items-center gap-1.5">
        <Segmented<DiffSourceKind>
          label="Diff source"
          value={selection.kind}
          onValueChange={(kind) =>
            select(
              threadId,
              kind === "branch"
                ? { kind, baseRef: null }
                : kind === "commit"
                  ? { kind, commitSha: null }
                  : { kind },
            )
          }
          options={(Object.keys(SOURCE_LABEL) as DiffSourceKind[]).map((value) => ({
            value,
            label: SOURCE_LABEL[value],
          }))}
        />
        {selection.kind === "branch" && branches.data && branches.data.length > 0 ? (
          <Select
            aria-label="Compare against"
            className="h-7 max-w-40 py-0 text-[0.8rem]"
            value={effective?.kind === "branch" ? (effective.baseRef ?? "") : ""}
            onChange={(event) => selectBranchBaseRef(threadId, event.currentTarget.value)}
          >
            {branches.data.map((branch) => (
              <option key={branch} value={branch}>
                {branch}
              </option>
            ))}
          </Select>
        ) : null}
        {selection.kind === "commit" && commits.data && commits.data.length > 0 ? (
          <Select
            aria-label="Commit"
            className="h-7 max-w-48 py-0 text-[0.8rem]"
            value={effective?.kind === "commit" ? (effective.commitSha ?? "") : ""}
            onChange={(event) =>
              select(threadId, { kind: "commit", commitSha: event.currentTarget.value })
            }
          >
            {commits.data.map((commit) => (
              <option key={commit.sha} value={commit.sha}>
                {commit.sha.slice(0, 8)} {commit.subject}
              </option>
            ))}
          </Select>
        ) : null}
      </div>
      <div className="flex shrink-0 flex-wrap items-center gap-0.5">
        {diff && files.length > 0 ? (
          <span
            className="mr-1 text-[0.72rem] tabular-nums"
            title={`${diff.additions} additions, ${diff.deletions} deletions`}
          >
            <span className="text-grow">+{diff.additions}</span>{" "}
            <span className="text-danger">−{diff.deletions}</span>
          </span>
        ) : null}
        <Segmented<DiffLayout>
          label="Diff layout"
          value={view.layout}
          onValueChange={(layout) => setView({ layout })}
          options={[
            { value: "unified", label: "Unified" },
            { value: "split", label: "Split" },
          ]}
        />
        <Tooltip content={view.wrap ? "Disable line wrapping" : "Enable line wrapping"}>
          <Toggle
            size="sm"
            aria-label="Wrap lines"
            pressed={view.wrap}
            onPressedChange={(pressed) => setView({ wrap: pressed })}
          >
            <TextWrapIcon aria-hidden className="size-3.5" />
          </Toggle>
        </Tooltip>
        <Tooltip
          content={view.ignoreWhitespace ? "Show whitespace changes" : "Hide whitespace changes"}
        >
          <Toggle
            size="sm"
            aria-label="Hide whitespace changes"
            pressed={view.ignoreWhitespace}
            onPressedChange={(pressed) => setView({ ignoreWhitespace: pressed })}
          >
            <PilcrowIcon aria-hidden className="size-3.5" />
          </Toggle>
        </Tooltip>
        <Tooltip content={view.wordDiff ? "Hide word changes" : "Highlight word changes"}>
          <Toggle
            size="sm"
            aria-label="Word diff"
            pressed={view.wordDiff}
            onPressedChange={(pressed) => setView({ wordDiff: pressed })}
          >
            <WholeWordIcon aria-hidden className="size-3.5" />
          </Toggle>
        </Tooltip>
        {files.length > 0 ? (
          <>
            <Tooltip content={allCollapsed ? "Expand all files" : "Collapse all files"}>
              <Button
                variant="quiet"
                size="icon"
                aria-label={allCollapsed ? "Expand all files" : "Collapse all files"}
                onClick={() =>
                  setCollapsedState({ scopeKey, keys: toggleAllDiffFiles(fileKeys, collapsed) })
                }
              >
                {allCollapsed ? (
                  <ChevronsUpDownIcon aria-hidden className="size-3.5" />
                ) : (
                  <ChevronsDownUpIcon aria-hidden className="size-3.5" />
                )}
              </Button>
            </Tooltip>
            <Tooltip content={view.treeOpen ? "Hide file tree" : "Show file tree"}>
              <Toggle
                size="sm"
                aria-label="File tree"
                pressed={view.treeOpen}
                onPressedChange={(pressed) => setView({ treeOpen: pressed })}
              >
                <FolderTreeIcon aria-hidden className="size-3.5" />
              </Toggle>
            </Tooltip>
          </>
        ) : null}
        {working_ && diff && files.length > 0 ? (
          <Tooltip content="Discard all changes">
            <Button
              variant="quiet"
              size="icon"
              aria-label="Discard all changes"
              disabled={busy}
              onClick={() => setPending({ kind: "discard-all" })}
            >
              <Trash2Icon aria-hidden className="size-3.5" />
            </Button>
          </Tooltip>
        ) : null}
      </div>
    </>
  );

  const reviewBar =
    open.length > 0 ? (
      <div
        className="flex shrink-0 flex-wrap items-center gap-2 border-b border-rule-soft bg-surface px-3 py-1.5 text-[0.8rem]"
        data-review-bar
      >
        <span className="mr-auto text-muted">
          {open.length} open {open.length === 1 ? "comment" : "comments"}
          {outdatedCount > 0 ? ` · ${outdatedCount} outdated` : ""}
        </span>
        <Button
          variant="secondary"
          size="sm"
          disabled={busy}
          onClick={() =>
            void run(async () => {
              await source.sendReviewToThread(open.map((comment) => comment.id));
              return `Sent ${open.length} ${open.length === 1 ? "comment" : "comments"} to the agent.`;
            })
          }
        >
          <SendIcon aria-hidden className="size-3.5" />
          Send {open.length} {open.length === 1 ? "comment" : "comments"} to the agent
        </Button>
        {changeRequest && checkoutId ? (
          <Button
            variant="secondary"
            size="sm"
            disabled={busy || postable.length === 0}
            title={
              postable.length === 0 ? "Outdated comments cannot be posted." : changeRequest.title
            }
            onClick={() =>
              void run(async () => {
                const url = await source.postReviewToChangeRequest(
                  checkoutId,
                  postable.map((comment) => comment.id),
                  "comment",
                );
                return `Posted ${postable.length} to pull request #${changeRequest.number}${url ? ` (${url})` : ""}.`;
              })
            }
          >
            <UploadIcon aria-hidden className="size-3.5" />
            Post to PR #{changeRequest.number}
          </Button>
        ) : null}
      </div>
    ) : null;

  let body: ReactNode;
  if (loading) {
    body = (
      <DiffPanelLoadingState
        label={
          selection.kind === "working"
            ? "Loading changes"
            : `Loading ${SOURCE_LABEL[selection.kind].toLowerCase()} diff`
        }
      />
    );
  } else if (diffError && !diff) {
    body = (
      <div className="p-3">
        <Notice tone="danger" title="The diff could not be loaded">
          {errorText(diffError)}
        </Notice>
      </div>
    );
  } else if (!diff || effective === null) {
    body = (
      <div className="p-3">
        <Empty title="Nothing to compare" seed={3}>
          {selection.kind === "branch"
            ? "This repository has no branch to compare against."
            : selection.kind === "commit"
              ? "This repository has no commits yet."
              : "This thread has no checkout to diff."}
        </Empty>
      </div>
    );
  } else if (!renderable) {
    body = (
      <div className="p-3">
        <Empty title="No changes" seed={4}>
          {selection.kind === "working"
            ? "The working tree matches HEAD."
            : "There are no changes in this selection."}
        </Empty>
      </div>
    );
  } else if (renderable.kind === "raw") {
    body = (
      <div className="min-h-0 flex-1 overflow-auto p-2">
        <p className="m-0 mb-2 text-[0.75rem] text-muted">{renderable.reason}</p>
        <pre
          className={cx(
            /* mono-ok: code */ "m-0 border border-rule-soft bg-surface p-3 font-mono text-[0.75rem] leading-relaxed text-ink",
            view.wrap ? "whitespace-pre-wrap break-words" : "overflow-auto",
          )}
        >
          {renderable.text}
        </pre>
      </div>
    );
  } else {
    body = (
      <div className="flex min-h-0 flex-1 overflow-hidden">
        <div ref={scroller} className="min-h-0 min-w-0 flex-1 overflow-auto" data-diff-scroller>
          <AnnotatableCodeView
            files={files}
            comments={comments}
            canComment={canComment}
            options={{
              layout: view.layout,
              wrap: view.wrap,
              wordDiff: view.wordDiff,
              scheme,
              ...(loadDiffFiles ? { loadDiffFiles } : {}),
            }}
            collapsed={collapsed}
            onToggleCollapsed={(fileKey) =>
              updateCollapsed((keys) => {
                if (keys.has(fileKey)) keys.delete(fileKey);
                else keys.add(fileKey);
              })
            }
            viewed={isViewed}
            onToggleViewed={(file, viewed) => {
              setViewed(threadId, file.filePath, viewed ? file.fileVersion : null);
              updateCollapsed((keys) => {
                if (viewed) keys.add(file.fileKey);
                else keys.delete(file.fileKey);
              });
            }}
            onSave={onSave}
            onDelete={(comment) =>
              void run(async () => {
                await source.deleteComment(comment.checkoutId, comment.id);
                return undefined;
              })
            }
            onResolve={(comment, resolved) =>
              void run(async () => {
                await source.resolveComment(comment.checkoutId, comment.id, resolved);
                return undefined;
              })
            }
            {...(working_
              ? { onDiscardFile: (path: string) => setPending({ kind: "discard-file", path }) }
              : {})}
            {...(onOpenFile ? { onOpenFile } : {})}
          />
        </div>
        {view.treeOpen ? (
          <aside className="flex w-[min(14rem,40%)] min-w-36 shrink-0 border-l border-rule-soft">
            <DiffFileTree
              ariaLabel={`${SOURCE_LABEL[selection.kind]} files`}
              entries={treeEntries}
              selectedPath={selectedPath}
              onSelectFile={revealFile}
            />
          </aside>
        ) : null}
      </div>
    );
  }

  const confirmDiscard = () => {
    const target = pending;
    const current = diff;
    setPending(null);
    if (!target || !current) return;
    void run(async () => {
      if (target.kind === "discard-all") {
        await source.discardAll(current.checkoutId, current.checksum);
        return "Discarded all changes.";
      }
      await source.discardPaths(current.checkoutId, [target.path], current.checksum);
      return `Discarded changes to ${target.path}.`;
    });
  };

  return (
    <DiffPanelShell header={header} {...(className ? { className } : {})}>
      {notice ? (
        <div className="shrink-0 border-b border-rule-soft px-3 py-1.5" data-diff-notice>
          <Notice tone={notice.tone} title={notice.text} />
        </div>
      ) : null}
      {commentsError ? (
        <div className="shrink-0 border-b border-rule-soft px-3 py-1.5">
          <Notice tone="danger" title="Comments are unavailable">
            {errorText(commentsError)}
          </Notice>
        </div>
      ) : null}
      {diff?.truncated ? (
        <p className="m-0 shrink-0 border-b border-rule-soft bg-surface px-3 py-1.5 text-[0.75rem] text-muted">
          This diff is larger than the daemon sends. Changes shown are incomplete; totals include
          all changes.
        </p>
      ) : null}
      {reviewBar}
      {body}
      <ConfirmDialog
        open={pending !== null}
        onClose={() => setPending(null)}
        object={pending?.kind === "discard-file" ? `changes to ${pending.path}` : "all changes"}
        confirm={pending?.kind === "discard-file" ? "Discard file" : "Discard all"}
        onConfirm={confirmDiscard}
        impact={`${
          pending?.kind === "discard-file"
            ? "The file goes back to HEAD. This cannot be undone."
            : "Every file in this checkout goes back to HEAD and untracked files are removed. This cannot be undone."
        } It applies only if the diff is still the one shown here.`}
      />
    </DiffPanelShell>
  );
}
