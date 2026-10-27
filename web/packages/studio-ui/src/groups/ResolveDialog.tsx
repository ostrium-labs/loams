// "Resolve…" (SD1 Task 25; design §53 §5.5 step 4, §8.1): pick the winner and what to do
// with it (merge into the base, open a pull request, or keep it as a thread), and what to do
// with the others. Every resolve is confirmed here, and the dialog names each write it will
// make before it makes them (T23-1's rule): the winner's uncommitted work is committed first,
// at the diff checksum and head shown here, which the request carries (T12-8). Archiving the
// others is on by default; removing their worktrees and stopping runs still going are off
// until asked for. A refusal keeps the dialog open with the daemon's reason.

import {
  Button,
  Checkbox,
  ConfirmDialog,
  cx,
  Dialog,
  Field,
  Input,
  Select,
  Textarea,
} from "@loams/ui";
import { useEffect, useId, useMemo, useState } from "react";

import type { MergeStrategyName } from "../git/GitActions";
import { type GroupMemberView, isLive, type ResolveActionName } from "./model";

export type ChangeRequestProviderName = "forgejo" | "github";

export interface ResolveChoice {
  readonly winnerChatId: string;
  readonly action: ResolveActionName;
  readonly strategy: MergeStrategyName;
  readonly pr: {
    readonly provider: ChangeRequestProviderName;
    readonly title: string;
    readonly body: string;
    readonly draft: boolean;
  };
  readonly archiveOthers: boolean;
  readonly removeOtherWorktrees: boolean;
  /** Stop the runs still going first (the daemon's `force`). */
  readonly force: boolean;
}

/** What the winner's checkout looks like now: what the person confirms. */
export interface WinnerState {
  readonly loading: boolean;
  readonly error?: string;
  /** Uncommitted files in the winner's worktree. */
  readonly uncommittedFiles?: number;
  readonly checksum?: string;
  /** The winner branch's head commit. */
  readonly head?: string;
}

export interface ResolveError {
  readonly reason?: string;
  readonly message: string;
  readonly paths?: readonly string[];
}

export type ResolveOutcome =
  | { readonly ok: true }
  | { readonly ok: false; readonly error: ResolveError };

const STRATEGY_LABEL: Record<MergeStrategyName, string> = {
  merge: "merge commit",
  squash: "squash",
  rebase: "rebase",
  ff: "fast-forward",
};

const PROVIDER_LABEL: Record<ChangeRequestProviderName, string> = {
  forgejo: "Forgejo",
  github: "GitHub",
};

const short = (sha: string | undefined) => (sha ? sha.slice(0, 8) : "…");

function firstLine(text: string): string {
  const line = text.split("\n").find((l) => l.trim()) ?? "Run group result";
  return line.trim().slice(0, 72);
}

function errorTitle(reason: string | undefined): string {
  switch (reason) {
    case "group_not_ready":
      return "Not ready";
    case "permission_denied":
    case "forbidden_role":
      return "Not allowed";
    case "stale_diff":
      return "Changed";
    case "merge_conflict":
      return "Merge conflict";
    case "push_rejected":
      return "Push rejected";
    case "provider_not_configured":
      return "No provider";
    default:
      return "Could not resolve";
  }
}

function errorText(error: ResolveError): string {
  switch (error.reason) {
    case "permission_denied":
      return "Studio needs the agentd:git-write permission to resolve a run group.";
    case "stale_diff":
      return "The winner’s changes or head changed since you opened this dialog. Check them again below, then confirm.";
    default:
      return error.message;
  }
}

/** The writes the resolve will make, in order, as the confirmation lists them. */
export function resolveSteps(input: {
  choice: ResolveChoice;
  members: readonly GroupMemberView[];
  base: string;
  winner: WinnerState;
}): string[] {
  const { choice, members, base, winner } = input;
  const won = members.find((m) => m.chatId === choice.winnerChatId);
  if (!won) return [];
  const others = members.filter((m) => m.chatId !== won.chatId && !m.detached);
  const live = others.filter((m) => isLive(m.status));
  const steps: string[] = [];
  if (choice.force && (live.length > 0 || isLive(won.status))) {
    const n = live.length + (isLive(won.status) ? 1 : 0);
    steps.push(`Stop ${n === 1 ? "the run" : `the ${n} runs`} still going.`);
  }
  const files = winner.uncommittedFiles ?? 0;
  if (choice.action !== "keep" && files > 0) {
    steps.push(
      `Commit ${files} uncommitted ${files === 1 ? "file" : "files"} in ${won.branch} first.`,
    );
  }
  switch (choice.action) {
    case "merge":
      steps.push(
        `Merge ${won.branch} (${short(winner.head)}) into ${base} with a ${STRATEGY_LABEL[choice.strategy]}.`,
      );
      break;
    case "pr":
      steps.push(
        `Push ${won.branch} to the remote and open a ${choice.pr.draft ? "draft " : ""}${PROVIDER_LABEL[choice.pr.provider]} pull request into ${base}.`,
      );
      break;
    case "keep":
      steps.push(`Keep ${won.label} as an ordinary thread.`);
      break;
  }
  if (choice.archiveOthers && others.length > 0) {
    steps.push(
      `Archive the other ${others.length === 1 ? "run" : "runs"}: ${others.map((m) => m.label).join(", ")}.`,
    );
  }
  if (choice.removeOtherWorktrees) {
    for (const m of others) steps.push(`Delete the worktree ${m.cwd} (${m.label}).`);
  }
  return steps;
}

function confirmLabel(choice: ResolveChoice, base: string): string {
  switch (choice.action) {
    case "merge":
      return `Merge into ${base}`;
    case "pr":
      return "Push and open the pull request";
    case "keep":
      return "Keep this run";
  }
}

export function ResolveDialog({
  open,
  onClose,
  members,
  base,
  prompt,
  running,
  initialWinner,
  defaultProvider = "github",
  winner,
  onWinnerChange,
  onConfirm,
}: {
  readonly open: boolean;
  readonly onClose: () => void;
  readonly members: readonly GroupMemberView[];
  readonly base: string;
  readonly prompt: string;
  /** Members are still queued or running: the daemon refuses unless they are stopped. */
  readonly running: boolean;
  readonly initialWinner?: string;
  readonly defaultProvider?: ChangeRequestProviderName;
  /** The chosen winner's checkout now, read by the caller. */
  readonly winner: WinnerState;
  readonly onWinnerChange: (chatId: string) => void;
  readonly onConfirm: (choice: ResolveChoice) => Promise<ResolveOutcome>;
}) {
  const candidates = useMemo(
    () => members.filter((m) => !m.detached && m.status !== "cancelled"),
    [members],
  );
  const [winnerChatId, setWinnerChatId] = useState(initialWinner ?? candidates[0]?.chatId ?? "");
  const [action, setAction] = useState<ResolveActionName>("merge");
  const [strategy, setStrategy] = useState<MergeStrategyName>("merge");
  const [provider, setProvider] = useState<ChangeRequestProviderName>(defaultProvider);
  const [title, setTitle] = useState(() => firstLine(prompt));
  const [body, setBody] = useState("");
  const [draft, setDraft] = useState(false);
  const [archiveOthers, setArchiveOthers] = useState(true);
  const [removeOtherWorktrees, setRemoveOtherWorktrees] = useState(false);
  const [force, setForce] = useState(false);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<ResolveError | null>(null);
  const [confirmRemove, setConfirmRemove] = useState(false);
  const actionName = useId();

  // Tell the caller which checkout to read, now and on each change.
  // oxlint-disable-next-line react/exhaustive-deps -- the callback is the caller's; follow the winner only.
  useEffect(() => {
    if (open && winnerChatId) onWinnerChange(winnerChatId);
  }, [open, winnerChatId]);

  const choice: ResolveChoice = {
    winnerChatId,
    action,
    strategy,
    pr: { provider, title: title.trim(), body, draft },
    archiveOthers,
    removeOtherWorktrees,
    force,
  };
  const steps = resolveSteps({ choice, members, base, winner });
  const needsWinnerState = action !== "keep";
  const ready =
    winnerChatId !== "" &&
    !pending &&
    (!needsWinnerState || (!winner.loading && !winner.error)) &&
    (action !== "pr" || choice.pr.title !== "");

  const submit = async () => {
    setPending(true);
    setError(null);
    try {
      const outcome = await onConfirm(choice);
      if (!outcome.ok) setError(outcome.error);
    } finally {
      setPending(false);
    }
  };

  const radio = (value: ResolveActionName, label: string, hint: string) => (
    <label
      className={cx(
        "flex cursor-pointer items-start gap-2 border px-3 py-2",
        action === value ? "border-accent bg-accent-soft" : "border-rule-soft",
      )}
    >
      <input
        type="radio"
        name={actionName}
        value={value}
        checked={action === value}
        onChange={() => setAction(value)}
        aria-label={label}
        className="mt-1"
      />
      <span className="flex flex-col">
        <span className="text-[0.86rem] font-medium text-ink">{label}</span>
        <span className="text-[0.78rem] text-muted">{hint}</span>
      </span>
    </label>
  );

  return (
    <>
      <Dialog
        open={open}
        onClose={() => {
          if (!pending) onClose();
        }}
        dismissible={!pending}
        title="Resolve the run group"
        footer={
          <>
            <Button variant="quiet" onClick={onClose} disabled={pending}>
              Cancel
            </Button>
            <Button
              variant="primary"
              disabled={!ready}
              onClick={() => {
                if (removeOtherWorktrees) setConfirmRemove(true);
                else void submit();
              }}
            >
              {pending ? "Resolving…" : confirmLabel(choice, base)}
            </Button>
          </>
        }
      >
        <div className="flex flex-col gap-4">
          <Field label="Winner">
            {(props) => (
              <Select
                {...props}
                value={winnerChatId}
                onChange={(e) => setWinnerChatId(e.target.value)}
              >
                {candidates.map((m) => (
                  <option key={m.chatId} value={m.chatId}>
                    {m.label} ({m.branch})
                  </option>
                ))}
              </Select>
            )}
          </Field>
          <fieldset className="m-0 flex flex-col gap-2 border-0 p-0">
            <legend className="mb-1 text-micro font-medium text-faint">What to do with it</legend>
            {radio("merge", "Merge into the base", `Merge the winner’s branch into ${base}.`)}
            {radio(
              "pr",
              "Open a pull request",
              `Push the winner’s branch and ask to merge it into ${base}.`,
            )}
            {radio(
              "keep",
              "Keep as a thread",
              "Leave it as an ordinary thread; nothing is merged.",
            )}
          </fieldset>
          {action === "merge" && (
            <Field label="Strategy">
              {(props) => (
                <Select
                  {...props}
                  value={strategy}
                  onChange={(e) => setStrategy(e.target.value as MergeStrategyName)}
                >
                  <option value="merge">Merge commit</option>
                  <option value="squash">Squash</option>
                  <option value="rebase">Rebase</option>
                  <option value="ff">Fast-forward only</option>
                </Select>
              )}
            </Field>
          )}
          {action === "pr" && (
            <div className="flex flex-col gap-3">
              <Field label="Provider">
                {(props) => (
                  <Select
                    {...props}
                    value={provider}
                    onChange={(e) => setProvider(e.target.value as ChangeRequestProviderName)}
                  >
                    <option value="forgejo">Forgejo</option>
                    <option value="github">GitHub</option>
                  </Select>
                )}
              </Field>
              <Field label="Title">
                {(props) => (
                  <Input {...props} value={title} onChange={(e) => setTitle(e.target.value)} />
                )}
              </Field>
              <Field label="Description">
                {(props) => (
                  <Textarea
                    {...props}
                    rows={3}
                    value={body}
                    onChange={(e) => setBody(e.target.value)}
                  />
                )}
              </Field>
              <Checkbox
                label="Draft"
                checked={draft}
                onChange={(e) => setDraft(e.target.checked)}
              />
            </div>
          )}
          <div className="flex flex-col gap-1.5">
            <Checkbox
              label="Archive the other runs"
              checked={archiveOthers}
              onChange={(e) => setArchiveOthers(e.target.checked)}
            />
            <Checkbox
              label="Remove the other worktrees"
              checked={removeOtherWorktrees}
              onChange={(e) => setRemoveOtherWorktrees(e.target.checked)}
            />
            {running && (
              <Checkbox
                label="Stop the runs still going"
                checked={force}
                onChange={(e) => setForce(e.target.checked)}
              />
            )}
          </div>
          <section
            aria-label="This will"
            className="flex flex-col gap-1 border border-rule-soft bg-bg p-3"
          >
            <h3 className="m-0 text-micro font-medium text-faint">This will</h3>
            {needsWinnerState && winner.loading ? (
              <p className="m-0 text-[0.82rem] text-muted">Reading the winner’s changes…</p>
            ) : needsWinnerState && winner.error ? (
              <p role="alert" className="m-0 text-[0.82rem] text-danger">
                {winner.error}
              </p>
            ) : (
              <ol className="m-0 flex flex-col gap-1 pl-5 text-[0.84rem] text-ink">
                {steps.map((step) => (
                  <li key={step}>{step}</li>
                ))}
              </ol>
            )}
            {running && !force && (
              <p className="m-0 text-[0.78rem] text-muted">
                Some runs are still going; the daemon refuses to resolve until they finish or are
                stopped.
              </p>
            )}
          </section>
          {error && (
            <div
              role="alert"
              className="flex flex-col gap-1 border border-danger bg-danger-soft p-3"
            >
              <strong className="text-[0.86rem] text-danger">{errorTitle(error.reason)}</strong>
              <span className="text-[0.82rem] text-ink">{errorText(error)}</span>
              {error.paths && error.paths.length > 0 && (
                <ul className={/* mono-ok: path */ "m-0 pl-5 font-mono text-[0.76rem] text-ink"}>
                  {error.paths.map((p) => (
                    <li key={p}>{p}</li>
                  ))}
                </ul>
              )}
            </div>
          )}
        </div>
      </Dialog>
      <ConfirmDialog
        open={confirmRemove}
        onClose={() => setConfirmRemove(false)}
        object="the other worktrees"
        confirm="Remove and resolve"
        impact="Each one is deleted from disk with what is in it. Their threads stay."
        onConfirm={() => {
          setConfirmRemove(false);
          void submit();
        }}
      />
    </>
  );
}
