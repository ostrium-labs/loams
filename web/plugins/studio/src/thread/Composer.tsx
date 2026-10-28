// Studio's composer (SD1 Task 19; §53 §5.6; inventory (c)). Written for Studio with
// T3 Code's ChatComposer.tsx as the reference (its 7 800 lines are rewritten, not
// vendored): the vendored Tiptap editor from @loams/studio-ui, its `@` file, `/`
// command and `$` skill menus, attachments, the harness, model and reasoning pickers,
// and the actions, all over loams-agentd's Connect services (SD1 Task 6):
//
// - Send: `ThreadService.Send` with a `RunRequest` (a new thread's first run names a
//   new worktree or the project's checkout).
// - While a turn runs: Enter queues (`QueueService.QueueMessage`, held for the turn's
//   end), Steer sends into the turn (`ThreadService.Steer`), Stop interrupts it
//   (`ThreadService.Interrupt`). The queue is shown with Send now and Remove.
// - Attachments: chunked uploads (`AttachmentService`, uploads.ts).
// - Mentions: files through Task 21's quick-open search (`FileService.Search`); the file
//   tree's queue (mentions.tsx, T21-5) becomes chips. Terminal selections ("Add to composer",
//   Task 22) wait as chips above the editor and join the prompt when it is sent.
// - "Run in parallel…" when the daemon lists `run-groups` (RunInParallel.tsx).
//
// Drafts are per thread in @loams/studio-ui's draft store (localStorage, zod-checked),
// so they survive a reload and a mode switch.

import { createClient, type Transport } from '@connectrpc/connect';
import { skipToken } from '@connectrpc/connect-query';
import { useAgentdQuery, useAgentdTransport } from '@loams/console-host/query';
import {
  ApprovalService,
  GitService,
  type HarnessDescriptor,
  HarnessId,
  HarnessService,
  QueueService,
  ReasoningLevel,
  RunGroupService,
  StudioService,
  ThreadService,
} from '@loams/proto/agentd';
import {
  ATTACHMENT_ACCEPT,
  type ComposerMenuItem,
  type ComposerPromptEditorHandle,
  ComposerPromptEditorTiptap,
  type ComposerTrigger,
  collapseExpandedComposerCursor,
  composerDraftStore,
  composerMenuReplacement,
  detectComposerTrigger,
  fileMenuItems,
  formatTerminalContextLabel,
  hasSendableContent,
  isComposerSendKey,
  isMenuTrigger,
  promptForSend,
  replaceTextRange,
  type ServerProviderSkill,
  searchSkillMenu,
  searchSlashMenu,
  serializeComposerFileLink,
  useComposerDraft,
} from '@loams/studio-ui';
import {
  Button,
  cx,
  Empty,
  formatBytes,
  IconButton,
  Kbd,
  Menu,
  MenuItem,
  MenuLabel,
  MenuSeparator,
  Segmented,
  SelectMenu,
  Spinner,
  Tooltip,
} from '@loams/ui';
import { useQuery } from '@tanstack/react-query';
import {
  Archive,
  ArrowUp,
  FileText,
  Folder,
  GitBranch,
  ListPlus,
  Navigation,
  Paperclip,
  Sparkles,
  Split,
  Square,
  SquareTerminal,
  X,
} from 'lucide-react';
import {
  type ClipboardEvent,
  type DragEvent,
  useCallback,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
} from 'react';
import { applyNewThreadApprovalMode, newThreadSandbox } from '../sidebar/useThreadActions.js';
import { agentdFilesSource } from './files-source.js';
import { isBusy, useChat, useQueue, useSessionStatus } from './live.js';
import { mentionsFor, useMentions } from './mentions.js';
import { buildCreateRunGroupRequest, RunInParallelDialog } from './RunInParallel.js';
import {
  removeTerminalContext,
  takeTerminalContexts,
  useTerminalContexts,
} from './terminalContexts.js';
import { agentdUploader, useUploads } from './uploads.js';

/** The project a new thread starts in. */
export interface ComposerProject {
  path: string;
  name: string;
  branch: string | null;
}

export interface ThreadComposerProps {
  /** The thread; none for a new thread in `project`, whose id is chosen on send. */
  threadId?: string;
  /** Where a new thread's first run starts (a new worktree or the checkout). */
  project?: ComposerProject;
  /** A new thread started: its id (the page navigates to it). */
  onStarted?: (threadId: string) => void;
  /** A run group started (the page navigates to it). */
  onGroupStarted?: (groupId: string) => void;
  /** Opens a mentioned file (the thread's Files panel). */
  openFile?: (path: string) => void;
  /** "Add a harness…" in the harness picker: the Seed Exchange's harnesses (MK1 Task 9). */
  onAddHarness?: () => void;
}

/** The harness picker's "Add a harness…" entry. */
const ADD_HARNESS = 'add-harness';

const NO_RECORDS = new Map();

/** At most this many `@` matches are asked for; the menu shows the best 50. */
const COMPOSER_FILE_LIMIT = 200;

/**
 * Takes the paths waiting in the thread's mention queue ("Mention in composer" from
 * the file tree, T21-5) into the draft as file chips, and clears the queue.
 */
function useTakeQueuedMentions(draftKey: string): void {
  const queue = mentionsFor(draftKey);
  const paths = useMentions(queue);
  useEffect(() => {
    if (paths.length === 0) return;
    const store = composerDraftStore();
    const prompt = store.getState().drafts[draftKey]?.prompt ?? '';
    const links = paths.map((path) => `${serializeComposerFileLink(path)} `).join('');
    const separator = prompt === '' || /\s$/.test(prompt) ? '' : ' ';
    store.getState().update(draftKey, { prompt: `${prompt}${separator}${links}` });
    queue.clear();
  }, [draftKey, paths, queue]);
}

/** A client-chosen id the daemon accepts for a chat or a message. */
function newId(): string {
  return typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function'
    ? crypto.randomUUID()
    : `${Date.now().toString(16)}-${Math.random().toString(16).slice(2)}`;
}

function parseEnum<E extends Record<string, string | number>>(
  e: E,
  name: string | undefined,
): E[keyof E] | undefined {
  if (!name || !(name in e)) return undefined;
  const value = e[name as keyof E];
  return typeof value === 'number' && value !== 0 ? value : undefined;
}

function reasoningLabel(level: ReasoningLevel): string {
  const name = ReasoningLevel[level] ?? '';
  return name.charAt(0) + name.slice(1).toLowerCase();
}

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** Upstream's rule: a replacement ending in a space eats the space already there. */
function extendRangeForTrailingSpace(text: string, rangeEnd: number, replacement: string) {
  return replacement.endsWith(' ') && text[rangeEnd] === ' ' ? rangeEnd + 1 : rangeEnd;
}

export function ThreadComposer(props: ThreadComposerProps) {
  const transport = useAgentdTransport();
  if (!transport) {
    return (
      <Empty title="Not connected" seed={7}>
        The composer opens once Studio reaches loams-agentd.
      </Empty>
    );
  }
  return <ComposerWithDaemon {...props} transport={transport} />;
}

function ComposerWithDaemon({
  threadId: givenThreadId,
  project,
  onStarted,
  onGroupStarted,
  openFile,
  onAddHarness,
  transport,
}: ThreadComposerProps & { transport: Transport }) {
  const isNew = givenThreadId === undefined;
  const draftKey = givenThreadId ?? `project:${project?.path ?? ''}`;
  const store = composerDraftStore();
  const draft = useComposerDraft(draftKey, store);
  const prompt = draft.prompt;
  const clients = useMemo(
    () => ({
      threads: createClient(ThreadService, transport),
      queue: createClient(QueueService, transport),
      groups: createClient(RunGroupService, transport),
      approvals: createClient(ApprovalService, transport),
    }),
    [transport],
  );
  const liveTransport = isNew ? undefined : transport;
  const status = useSessionStatus(liveTransport, draftKey);
  const busy = !isNew && isBusy(status);
  const chat = useChat(liveTransport, draftKey);
  const queued = useQueue(liveTransport, draftKey);
  const terminal = useTerminalContexts(draftKey);
  useTakeQueuedMentions(draftKey);
  const uploader = useMemo(() => agentdUploader(transport), [transport]);
  const uploads = useUploads(draftKey, uploader);

  // ── The daemon's catalogs ───────────────────────────────────────────────
  const host = useAgentdQuery(StudioService.method.getHost, {});
  const canRunGroups = host.data?.host?.features.includes('run-groups') ?? false;
  const harnessList = useAgentdQuery(HarnessService.method.listHarnesses, {});
  const usable: HarnessDescriptor[] = useMemo(
    () => (harnessList.data?.harnesses ?? []).filter((h) => h.installed && h.enabled !== false),
    [harnessList.data],
  );
  // Harnesses installed from the Seed Exchange (MK1 Tasks 4b, 9) share one HarnessId and
  // are told apart by their listing. A thread cannot run on one yet (Task 4b ruling 8), so
  // the picker lists them but does not offer them.
  const harnesses = useMemo(() => usable.filter((h) => h.id !== HarnessId.MARKETPLACE), [usable]);
  const fromMarketplace = useMemo(
    () => usable.filter((h) => h.id === HarnessId.MARKETPLACE),
    [usable],
  );
  const chatHarness = chat?.config?.harness || undefined;
  const harness: HarnessId | undefined =
    parseEnum(HarnessId, draft.harness) ?? chatHarness ?? harnesses[0]?.id;
  const descriptor = harnesses.find((h) => h.id === harness);
  const scope = isNew ? { path: project?.path ?? '' } : { chatId: draftKey };
  const models = useAgentdQuery(
    HarnessService.method.listModels,
    harness ? { harness } : skipToken,
  );
  const commands = useAgentdQuery(
    HarnessService.method.listCommands,
    harness ? { harness, ...scope } : skipToken,
  );
  const skillList = useAgentdQuery(
    HarnessService.method.listSkills,
    harness ? { harness, ...scope } : skipToken,
  );
  const skills = useMemo(
    () => (skillList.data?.skills ?? []).filter((s) => s.enabled),
    [skillList.data],
  );
  const editorSkills: ServerProviderSkill[] = useMemo(
    () =>
      skills.map((s) => ({
        name: s.name,
        path: s.path,
        enabled: s.enabled,
        ...(s.description ? { description: s.description } : {}),
      })),
    [skills],
  );
  const model = draft.model ?? chat?.config?.model;
  const modelInfo = models.data?.models.find((m) => m.id === model);
  const reasoningLevels = modelInfo?.reasoningLevels.length
    ? modelInfo.reasoningLevels
    : (descriptor?.reasoningLevels ?? []);
  const reasoning = parseEnum(ReasoningLevel, draft.reasoning) ?? chat?.config?.reasoning;
  const branches = useAgentdQuery(
    GitService.method.listBranches,
    isNew && project ? { repoPath: project.path } : skipToken,
  );
  const worktreeMode = draft.worktree ?? 'worktree';
  const base = draft.base ?? project?.branch ?? branches.data?.branches[0] ?? '';

  // ── The editor and its menu ─────────────────────────────────────────────
  const editorRef = useRef<ComposerPromptEditorHandle | null>(null);
  const [cursor, setCursor] = useState(() => collapseExpandedComposerCursor(prompt, prompt.length));
  const [trigger, setTrigger] = useState<ComposerTrigger | null>(() =>
    detectComposerTrigger(prompt, prompt.length),
  );
  const [highlight, setHighlight] = useState(0);
  const historyRef = useRef<{ index: number; saved: string } | null>(null);
  const listId = useId();
  const menuTrigger = isMenuTrigger(trigger) ? trigger : null;
  // `@` searches the checkout as Task 21's quick open does (`searchFiles`, then
  // `rankFileMatches` in fileMenuItems).
  const filesSource = useMemo(
    () =>
      agentdFilesSource(
        transport,
        isNew ? { checkoutPath: project?.path ?? '' } : { chatId: draftKey },
      ),
    [draftKey, isNew, project?.path, transport],
  );
  const fileQuery = menuTrigger?.kind === 'path' ? menuTrigger.query.trim() : '';
  const files = useQuery({
    queryKey: ['studio', 'composer', 'files', draftKey, fileQuery],
    queryFn: ({ signal }) => filesSource.searchFiles(fileQuery, COMPOSER_FILE_LIMIT, signal),
    enabled: fileQuery !== '',
    staleTime: 10_000,
  });
  const menuItems: ComposerMenuItem[] = useMemo(() => {
    if (!menuTrigger) return [];
    const skillInfos = skills.map((s) => ({
      name: s.name,
      description: s.description,
      path: s.path,
    }));
    if (menuTrigger.kind === 'path') return fileMenuItems(files.data ?? [], menuTrigger.query);
    if (menuTrigger.kind === 'skill') return searchSkillMenu(menuTrigger.query, skillInfos);
    return searchSlashMenu(
      menuTrigger.query,
      (commands.data?.commands ?? []).map((c) => ({
        name: c.name,
        description: c.description,
        ...(c.inputHint ? { inputHint: c.inputHint } : {}),
      })),
      skillInfos,
    );
  }, [commands.data, files.data, menuTrigger, skills]);
  const menuOpen = menuTrigger !== null && (menuItems.length > 0 || menuTrigger.kind === 'path');
  const active = menuItems[Math.min(highlight, menuItems.length - 1)];

  const setPrompt = useCallback(
    (next: string) => store.getState().update(draftKey, { prompt: next }),
    [draftKey, store],
  );

  const onChange = useCallback(
    (next: string, nextCursor: number, expanded: number, adjacent: boolean) => {
      setPrompt(next);
      setCursor(nextCursor);
      setTrigger(adjacent ? null : detectComposerTrigger(next, expanded));
      setHighlight(0);
      if (historyRef.current && next !== draft.history[historyRef.current.index]) {
        historyRef.current = null;
      }
    },
    [draft.history, setPrompt],
  );

  const select = useCallback(
    (item: ComposerMenuItem) => {
      const snapshot = editorRef.current?.readSnapshot() ?? {
        value: prompt,
        expandedCursor: prompt.length,
      };
      const current = detectComposerTrigger(snapshot.value, snapshot.expandedCursor) ?? trigger;
      if (!current) return;
      const replacement = composerMenuReplacement(item);
      const end = extendRangeForTrailingSpace(snapshot.value, current.rangeEnd, replacement);
      const next = replaceTextRange(snapshot.value, current.rangeStart, end, replacement);
      const nextCursor = collapseExpandedComposerCursor(next.text, next.cursor);
      setPrompt(next.text);
      setCursor(nextCursor);
      setTrigger(null);
      setHighlight(0);
      requestAnimationFrame(() => editorRef.current?.focusAt(nextCursor));
    },
    [prompt, setPrompt, trigger],
  );

  // ── Sending ─────────────────────────────────────────────────────────────
  const [sending, setSending] = useState<null | 'send' | 'steer' | 'stop'>(null);
  const [error, setError] = useState<string | null>(null);
  const [parallelOpen, setParallelOpen] = useState(false);
  const [parallelError, setParallelError] = useState<string | undefined>(undefined);
  const attachments = draft.attachments;
  const sendable =
    hasSendableContent({
      prompt,
      attachments: attachments.length,
      terminalContexts: terminal.length,
    }) && uploads.pending.length === 0;

  const finishSent = useCallback(() => {
    takeTerminalContexts(draftKey);
    store.getState().clearSent(draftKey, prompt);
    setCursor(0);
    setTrigger(null);
    historyRef.current = null;
  }, [draftKey, prompt, store]);

  const submit = useCallback(
    async (mode: 'send' | 'steer') => {
      if (!sendable || sending) return;
      const text = promptForSend({ prompt, terminalContexts: terminal });
      const paths = attachments.map((a) => a.path);
      setSending(mode);
      setError(null);
      try {
        if (mode === 'steer') {
          await clients.threads.steer({ chatId: draftKey, prompt: text, messageId: newId() });
        } else if (busy) {
          await clients.queue.queueMessage({
            chatId: draftKey,
            text,
            attachments: paths,
            holdForTurnEnd: true,
          });
        } else {
          const chatId = isNew ? newId() : draftKey;
          const firstRunInWorktree = isNew && project && worktreeMode === 'worktree';
          // Settings › Studio's approval mode, before the first run (SD1 Task 30).
          if (isNew) await applyNewThreadApprovalMode(clients.approvals, chatId);
          await clients.threads.send({
            chatId,
            messageId: newId(),
            request: {
              prompt: text,
              ...(harness ? { harness } : {}),
              ...(model ? { model } : {}),
              ...(reasoning ? { reasoning } : {}),
              // The daemon refuses an unspecified sandbox (SD1 Task 29 found it): the
              // thread's own, else this device's default for a new thread.
              sandbox: (!isNew && chat?.config?.sandbox) || newThreadSandbox(),
              cwd: isNew ? (firstRunInWorktree ? '' : (project?.path ?? '')) : (chat?.cwd ?? ''),
              attachments: paths,
              ...(firstRunInWorktree ? { worktree: { repoPath: project.path, base } } : {}),
            },
          });
          finishSent();
          if (isNew) onStarted?.(chatId);
          return;
        }
        finishSent();
      } catch (e) {
        setError(errorText(e));
      } finally {
        setSending(null);
      }
    },
    [
      attachments,
      base,
      busy,
      chat?.config?.sandbox,
      chat?.cwd,
      clients,
      draftKey,
      finishSent,
      harness,
      isNew,
      model,
      onStarted,
      project,
      prompt,
      reasoning,
      sendable,
      sending,
      terminal,
      worktreeMode,
    ],
  );

  const stop = useCallback(async () => {
    setSending('stop');
    setError(null);
    try {
      await clients.threads.interrupt({ chatId: draftKey });
    } catch (e) {
      setError(errorText(e));
    } finally {
      setSending(null);
    }
  }, [clients, draftKey]);

  const startGroup = useCallback(
    async (choices: Parameters<typeof buildCreateRunGroupRequest>[0]['choices']) => {
      const repoPath = project?.path ?? chat?.sourceContext?.repoRoot ?? '';
      setParallelError(undefined);
      try {
        const { groupId } = await clients.groups.createRunGroup(
          buildCreateRunGroupRequest({
            ...(chat?.spaceId ? { spaceId: chat.spaceId } : {}),
            repoPath,
            prompt: promptForSend({ prompt, terminalContexts: terminal }),
            attachments: attachments.map((a) => a.path),
            choices,
          }),
        );
        setParallelOpen(false);
        finishSent();
        onGroupStarted?.(groupId);
      } catch (e) {
        setParallelError(errorText(e));
      }
    },
    [attachments, chat, clients, finishSent, onGroupStarted, project, prompt, terminal],
  );

  // ── Keys: the menu, sending, history ────────────────────────────────────
  const onCommandKeyDown = (key: string, event: KeyboardEvent): boolean => {
    if (menuOpen) {
      if (key === 'Escape') {
        setTrigger(null);
        return true;
      }
      if (key === 'ArrowDown' && menuItems.length > 0) {
        setHighlight((h) => (h + 1) % menuItems.length);
        return true;
      }
      if (key === 'ArrowUp' && menuItems.length > 0) {
        setHighlight((h) => (h - 1 + menuItems.length) % menuItems.length);
        return true;
      }
      if ((key === 'Enter' || key === 'Tab') && active) {
        select(active);
        return true;
      }
    }
    if (key === 'Enter' && isComposerSendKey(event)) {
      void submit('send');
      return true;
    }
    const history = draft.history;
    if (key === 'ArrowUp' && history.length > 0 && !event.shiftKey) {
      const browsing = historyRef.current;
      if (!browsing && prompt !== '') return false;
      if (!editorRef.current?.isCaretOnVisualEdge('start')) return false;
      const index = browsing ? Math.max(0, browsing.index - 1) : history.length - 1;
      historyRef.current = { index, saved: browsing?.saved ?? prompt };
      const recalled = history[index] ?? '';
      setPrompt(recalled);
      setCursor(collapseExpandedComposerCursor(recalled, recalled.length));
      return true;
    }
    if (key === 'ArrowDown' && historyRef.current && !event.shiftKey) {
      if (!editorRef.current?.isCaretOnVisualEdge('end')) return false;
      const { index, saved } = historyRef.current;
      const next = index + 1;
      const recalled = next < history.length ? (history[next] ?? '') : saved;
      historyRef.current = next < history.length ? { index: next, saved } : null;
      setPrompt(recalled);
      setCursor(collapseExpandedComposerCursor(recalled, recalled.length));
      return true;
    }
    return false;
  };

  // ── Attachments: picker, paste, drop ────────────────────────────────────
  const fileInput = useRef<HTMLInputElement | null>(null);
  const onPaste = (event: ClipboardEvent<HTMLElement>) => {
    const pasted = Array.from(event.clipboardData?.files ?? []);
    if (pasted.length === 0) return;
    event.preventDefault();
    void uploads.add(pasted);
  };
  const onDrop = (event: DragEvent<HTMLDivElement>) => {
    const dropped = Array.from(event.dataTransfer?.files ?? []);
    if (dropped.length === 0) return;
    event.preventDefault();
    void uploads.add(dropped);
  };

  const placeholder = busy
    ? 'Queue a follow-up, or steer the running turn…'
    : isNew
      ? `Describe a task for ${project?.name ?? 'this project'}. @ a file, / a command.`
      : 'Ask for a change. @ a file, / a command, $ a skill.';

  return (
    // oxlint-disable-next-line jsx-a11y/no-static-element-interactions -- a drop target for image files; the Attach button is the keyboard path.
    <div
      data-composer=""
      className="relative flex flex-col gap-2"
      onDragOver={(e) => e.preventDefault()}
      onDrop={onDrop}
    >
      {busy && queued.length > 0 && (
        <QueuedMessages
          items={queued.map((q) => ({ id: q.id, text: q.text }))}
          onRemove={(id) =>
            void clients.queue
              .removeQueuedMessage({ chatId: draftKey, id })
              .catch((e) => setError(errorText(e)))
          }
          onSendNow={(id) =>
            void clients.queue
              .sendQueuedMessageNow({ chatId: draftKey, id })
              .catch((e) => setError(errorText(e)))
          }
        />
      )}
      {(uploads.refusals.length > 0 || error) && (
        <div
          role="alert"
          className="flex items-start gap-2 rounded-lg border border-danger bg-danger-soft px-3 py-2 text-body text-ink"
        >
          <div className="flex min-w-0 flex-1 flex-col gap-1">
            {error && <p className="m-0">{error}</p>}
            {uploads.refusals.map((reason) => (
              <p key={reason} className="m-0">
                {reason}
              </p>
            ))}
          </div>
          <Button
            size="icon"
            variant="quiet"
            aria-label="Dismiss"
            onClick={() => {
              uploads.dismissRefusals();
              setError(null);
            }}
          >
            <X size={14} aria-hidden="true" />
          </Button>
        </div>
      )}
      <div
        data-composer-box=""
        aria-busy={sending !== null || undefined}
        className="relative rounded-lg border border-control-border bg-surface shadow-composer transition-colors duration-(--op-duration-fast) ease-out hover:border-input focus-within:border-focus focus-within:hover:border-focus motion-reduce:transition-none"
      >
        {menuOpen && menuTrigger && (
          <SuggestionMenu
            id={listId}
            kind={menuTrigger.kind}
            items={menuItems}
            active={active}
            emptyText={
              fileQuery === ''
                ? 'Type to search files.'
                : files.isFetching
                  ? 'Searching…'
                  : 'No matching files.'
            }
            onSelect={select}
            onHover={(index) => setHighlight(index)}
          />
        )}
        {(terminal.length > 0 || attachments.length > 0 || uploads.pending.length > 0) && (
          <ul
            aria-label="Context"
            className="m-0 flex list-none flex-wrap gap-1.5 border-rule-soft border-b px-3 py-2"
          >
            {terminal.map((selection, index) => (
              <Chip
                // oxlint-disable-next-line react/no-array-index-key -- the same lines can be added twice; the list is positional.
                key={`t:${selection.terminalId}:${selection.lineStart}:${index}`}
                icon={<SquareTerminal size={12} aria-hidden="true" className="text-info" />}
                label={formatTerminalContextLabel(selection)}
                title={selection.text}
                onRemove={() => removeTerminalContext(draftKey, index)}
              />
            ))}
            {attachments.map((a) => (
              <Chip
                key={a.id}
                icon={<Paperclip size={12} aria-hidden="true" className="text-accent-fg" />}
                label={`${a.name} · ${formatBytes(a.size)}`}
                onRemove={() => uploads.remove(a.id)}
              />
            ))}
            {uploads.pending.map((p) => (
              <Chip
                key={p.id}
                icon={<Spinner size="sm" tone="muted" label={`Uploading ${p.name}`} />}
                label={`${p.name} · ${p.total ? Math.round((p.sent / p.total) * 100) : 0}%`}
              />
            ))}
          </ul>
        )}
        <div className="px-3 pt-2 pb-1">
          <ComposerPromptEditorTiptap
            value={prompt}
            cursor={cursor}
            richTextEnabled
            contextRecords={NO_RECORDS}
            chipActions={openFile ? { openMention: openFile } : {}}
            skills={editorSkills}
            disabled={sending === 'send' && !busy}
            placeholder={placeholder}
            ariaLabel={isNew ? 'New thread prompt' : 'Message'}
            suggestionListId={listId}
            activeSuggestionId={
              menuOpen && active ? `${listId}-${menuItems.indexOf(active)}` : undefined
            }
            isPathQueryActive={() => menuTrigger?.kind === 'path'}
            onChange={onChange}
            onCommandKeyDown={onCommandKeyDown}
            onPaste={onPaste}
            editorRef={editorRef}
          />
        </div>
        <div className="flex flex-wrap items-center gap-1.5 px-2 pb-2">
          {harnesses.length > 0 && harness !== undefined && (
            <SelectMenu
              label="Harness"
              value={String(harness)}
              onValueChange={(v) => {
                if (v === ADD_HARNESS) {
                  onAddHarness?.();
                  return;
                }
                store.getState().update(draftKey, {
                  harness: HarnessId[Number(v) as HarnessId],
                  model: undefined,
                  reasoning: undefined,
                });
              }}
              options={[
                ...harnesses.map((h) => ({ value: String(h.id), label: h.name })),
                ...fromMarketplace.map((h) => ({
                  value: `marketplace:${h.marketplaceListing ?? h.name}`,
                  label: `${h.name} (from the Seed Exchange; threads on it come later)`,
                  disabled: true,
                })),
                ...(onAddHarness ? [{ value: ADD_HARNESS, label: 'Add a harness…' }] : []),
              ]}
              className={PICKER}
            />
          )}
          {(models.data?.models.length ?? 0) > 0 && (
            <SelectMenu
              label="Model"
              value={model ?? ''}
              placeholder="Default model"
              onValueChange={(v) =>
                store.getState().update(draftKey, { model: v || undefined, reasoning: undefined })
              }
              options={[
                { value: '', label: 'Default model' },
                ...(models.data?.models ?? []).map((m) => ({ value: m.id, label: m.label })),
              ]}
              className={PICKER}
            />
          )}
          {reasoningLevels.length > 0 && (
            <SelectMenu
              label="Reasoning"
              value={reasoning ? String(reasoning) : ''}
              placeholder="Reasoning"
              onValueChange={(v) =>
                store.getState().update(draftKey, {
                  reasoning: v ? ReasoningLevel[Number(v) as ReasoningLevel] : undefined,
                })
              }
              options={[
                { value: '', label: 'Default reasoning' },
                ...reasoningLevels.map((level) => ({
                  value: String(level),
                  label: reasoningLabel(level),
                })),
              ]}
              className={PICKER}
            />
          )}
          {isNew && project && (
            <>
              <Segmented
                label="Where the first run works"
                value={worktreeMode}
                onValueChange={(v) => store.getState().update(draftKey, { worktree: v })}
                options={[
                  { value: 'worktree', label: 'New worktree', icon: <Split size={13} /> },
                  { value: 'local', label: 'Local', icon: <Folder size={13} /> },
                ]}
              />
              {worktreeMode === 'worktree' && (
                <SelectMenu
                  label="Base branch"
                  value={base || null}
                  placeholder="Base branch"
                  leading={<GitBranch size={13} aria-hidden="true" className="text-faint" />}
                  onValueChange={(v) => store.getState().update(draftKey, { base: v })}
                  options={Array.from(new Set([base, ...(branches.data?.branches ?? [])]))
                    .filter(Boolean)
                    .map((b) => ({ value: b, label: b }))}
                  className={PICKER}
                />
              )}
            </>
          )}
          <span className="flex-1" />
          <input
            ref={fileInput}
            type="file"
            accept={ATTACHMENT_ACCEPT}
            multiple
            hidden
            aria-label="Attach images"
            onChange={(e) => {
              const picked = Array.from(e.target.files ?? []);
              e.target.value = '';
              if (picked.length) void uploads.add(picked);
            }}
          />
          <IconButton
            label="Attach images"
            tooltip="Attach PNG, JPEG, GIF or WebP images"
            onClick={() => fileInput.current?.click()}
          >
            <Paperclip aria-hidden="true" />
          </IconButton>
          <StashMenu
            stash={draft.stash}
            canStash={prompt.trim() !== ''}
            onStash={() => {
              store.getState().stash(draftKey);
              setCursor(0);
            }}
            onRestore={(index) => {
              store.getState().unstash(draftKey, index);
              const restored = store.getState().drafts[draftKey]?.prompt ?? '';
              setCursor(collapseExpandedComposerCursor(restored, restored.length));
            }}
          />
          {canRunGroups && !busy && (
            <Button
              size="sm"
              variant="secondary"
              disabled={!sendable || harnesses.length === 0}
              onClick={() => setParallelOpen(true)}
            >
              <ListPlus size={14} aria-hidden="true" /> Run in parallel…
            </Button>
          )}
          {busy && (
            <>
              <Button
                size="sm"
                variant="danger"
                aria-label="Stop"
                loading={sending === 'stop'}
                onClick={() => void stop()}
              >
                <Square size={13} aria-hidden="true" /> Stop
              </Button>
              <Button
                size="sm"
                variant="secondary"
                aria-label="Steer"
                title={
                  attachments.length
                    ? 'Steering sends text only; send images with Queue.'
                    : 'Send this into the running turn'
                }
                disabled={!sendable || attachments.length > 0 || sending !== null}
                onClick={() => void submit('steer')}
              >
                <Navigation size={13} aria-hidden="true" /> Steer
              </Button>
            </>
          )}
          <Tooltip
            content={
              <span className="inline-flex items-center gap-1.5">
                {busy ? 'Queue for when the turn ends' : 'Send'}
                <Kbd>Enter</Kbd>
              </span>
            }
          >
            <Button
              size="sm"
              variant="primary"
              aria-label={busy ? 'Queue' : 'Send'}
              aria-keyshortcuts="Enter"
              loading={sending === 'send'}
              disabled={!sendable || (sending !== null && sending !== 'send')}
              onClick={() => void submit('send')}
            >
              <ArrowUp size={14} aria-hidden="true" />
              {busy ? 'Queue' : 'Send'}
            </Button>
          </Tooltip>
        </div>
      </div>
      {parallelOpen && harness !== undefined && (
        <RunInParallelDialog
          open
          onClose={() => setParallelOpen(false)}
          harnesses={harnesses}
          initial={{ harness, ...(model ? { model } : {}), base: base || chat?.branch || '' }}
          {...(branches.data ? { branches: branches.data.branches } : {})}
          {...(parallelError ? { error: parallelError } : {})}
          onSubmit={(choices) => void startGroup(choices)}
        />
      )}
    </div>
  );
}

const PICKER =
  'inline-flex h-7 max-w-[14rem] items-center gap-1.5 rounded-pill border border-rule-soft bg-bg px-2.5 text-small text-muted transition-colors duration-(--op-duration-fast) ease-out hover:border-rule hover:text-ink motion-reduce:transition-none';

function Chip({
  icon,
  label,
  title,
  onRemove,
}: {
  icon: React.ReactNode;
  label: string;
  title?: string;
  onRemove?: () => void;
}) {
  return (
    <li
      className="inline-flex max-w-[18rem] items-center gap-1.5 rounded-pill border border-rule bg-raised py-0.5 pr-1 pl-2 text-[0.78rem] text-ink"
      title={title}
    >
      {icon}
      <span className="truncate">{label}</span>
      {onRemove && (
        <button
          type="button"
          aria-label={`Remove ${label}`}
          onClick={onRemove}
          className="inline-flex cursor-pointer items-center rounded-pill border-0 bg-transparent p-0.5 text-faint hover:text-ink"
        >
          <X size={12} aria-hidden="true" />
        </button>
      )}
    </li>
  );
}

function menuRow(item: ComposerMenuItem) {
  switch (item.type) {
    case 'path':
      return {
        icon: item.isDir ? (
          <Folder size={14} aria-hidden="true" className="text-faint" />
        ) : (
          <FileText size={14} aria-hidden="true" className="text-accent-fg" />
        ),
        title: item.path.split('/').pop() || item.path,
        detail: item.path,
      };
    case 'slash-command':
      return {
        icon: (
          <span className={/* mono-ok: code */ 'w-3.5 text-center font-mono text-faint'}>/</span>
        ),
        title: `/${item.command.name}`,
        detail: item.command.description,
      };
    case 'skill':
      return {
        icon: <Sparkles size={14} aria-hidden="true" className="text-grow" />,
        title: item.skill.name,
        detail: item.skill.description,
      };
  }
}

function SuggestionMenu({
  id,
  kind,
  items,
  active,
  emptyText,
  onSelect,
  onHover,
}: {
  id: string;
  kind: ComposerTrigger['kind'];
  items: ComposerMenuItem[];
  active: ComposerMenuItem | undefined;
  emptyText: string;
  onSelect: (item: ComposerMenuItem) => void;
  onHover: (index: number) => void;
}) {
  const heading = kind === 'path' ? 'Files' : kind === 'skill' ? 'Skills' : 'Commands and skills';
  return (
    <div className="absolute right-0 bottom-full left-0 z-20 mb-1 max-h-72 overflow-auto rounded-lg border border-rule bg-elevated shadow-pop">
      <div className="px-3 pt-2 pb-1 text-micro font-medium text-faint">{heading}</div>
      {items.length === 0 ? (
        <p className="m-0 px-3 pb-2 text-[0.84rem] text-muted">{emptyText}</p>
      ) : (
        // The editor keeps focus and points at the highlighted row with
        // aria-activedescendant (the WAI-ARIA combobox pattern), so the rows themselves
        // take no focus; the keyboard path is the editor's arrows, Enter and Tab.
        <div id={id} role="listbox" aria-label={heading} className="pb-1">
          {items.map((item, index) => {
            const row = menuRow(item);
            const selected = item === active;
            return (
              // oxlint-disable-next-line jsx-a11y/interactive-supports-focus -- focus stays in the editor (aria-activedescendant).
              // oxlint-disable-next-line jsx-a11y/click-events-have-key-events -- keys reach the menu through the editor.
              <div
                key={item.id}
                id={`${id}-${index}`}
                role="option"
                aria-selected={selected}
                className={cx(
                  'flex cursor-pointer items-center gap-2 px-3 py-1.5 text-[0.86rem]',
                  selected ? 'bg-accent-soft text-ink' : 'text-ink hover:bg-raised',
                )}
                onMouseDown={(e) => e.preventDefault()}
                onMouseEnter={() => onHover(index)}
                onClick={() => onSelect(item)}
              >
                {row.icon}
                <span className="shrink-0 font-medium">{row.title}</span>
                <span className="min-w-0 truncate text-[0.78rem] text-faint">{row.detail}</span>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}

function QueuedMessages({
  items,
  onRemove,
  onSendNow,
}: {
  items: { id: string; text: string }[];
  onRemove: (id: string) => void;
  onSendNow: (id: string) => void;
}) {
  return (
    <section aria-label="Queued messages" className="rounded-lg border border-rule-soft bg-surface">
      <h3 className="m-0 px-3 pt-2 text-micro font-medium text-faint">
        Queued for when the turn ends
      </h3>
      <ul className="m-0 list-none p-0 py-1">
        {items.map((item) => (
          <li key={item.id} className="flex items-center gap-2 px-3 py-1 text-[0.84rem]">
            <span className="min-w-0 flex-1 truncate text-ink">{item.text}</span>
            <Button size="sm" variant="quiet" onClick={() => onSendNow(item.id)}>
              Send now
            </Button>
            <Button
              size="icon"
              variant="quiet"
              aria-label="Remove queued message"
              onClick={() => onRemove(item.id)}
            >
              <X size={13} aria-hidden="true" />
            </Button>
          </li>
        ))}
      </ul>
    </section>
  );
}

function StashMenu({
  stash,
  canStash,
  onStash,
  onRestore,
}: {
  stash: readonly string[];
  canStash: boolean;
  onStash: () => void;
  onRestore: (index: number) => void;
}) {
  return (
    <Menu
      label="Prompt stash"
      trigger={
        <Button size="icon" variant="quiet" aria-label="Prompt stash" title="Prompt stash">
          <Archive size={15} aria-hidden="true" />
        </Button>
      }
    >
      <MenuItem onSelect={onStash} disabled={!canStash}>
        Stash this prompt
      </MenuItem>
      {stash.length > 0 && (
        <>
          <MenuSeparator />
          <MenuLabel>Stashed</MenuLabel>
          {stash.map((entry, index) => (
            // oxlint-disable-next-line react/no-array-index-key -- entries are positional.
            <MenuItem key={index} onSelect={() => onRestore(index)}>
              <span className="block max-w-[18rem] truncate">{entry}</span>
            </MenuItem>
          ))}
        </>
      )}
    </Menu>
  );
}
