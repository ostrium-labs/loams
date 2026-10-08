// The agent panel: chat list, messages, tool and approval cards, composer
// with the provider picker. State lives in AgentStore; the shell keeps the
// dock mounted, so a hidden panel keeps its draft and its running turn.

import type { ChatApproval, ChatStopReason, LoamsDesktopApi } from '@loams/desktop/contracts';
import { Button, Input, Select } from '@loams/ui';
import { History, Plus, Square, Trash2 } from 'lucide-react';
import {
  type KeyboardEvent,
  type MouseEvent,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from 'react';
import { renderMarkdown, safeHref } from './markdown.js';
import type { AgentStore, Item } from './store.js';

export const SETTINGS_HREF = '#/settings/agent';

const STOP_LABEL: Record<ChatStopReason, string | undefined> = {
  end_turn: undefined,
  iteration_cap: 'Stopped: iteration cap (25)',
  wall_clock_budget: 'Stopped: time budget (10 minutes)',
  token_budget: 'Stopped: token budget (200k)',
  llm_error: 'Stopped: the model returned an error',
  cancelled: 'Stopped by you',
};
export const stopLabel = (s: ChatStopReason): string | undefined => STOP_LABEL[s];

const json = (v: unknown): string => {
  try {
    return JSON.stringify(v, null, 2) ?? String(v);
  } catch {
    return String(v);
  }
};

/** Model Markdown, sanitised. Links open in the system browser; code blocks copy. */
export function Markdown({ text, desktop }: { text: string; desktop: LoamsDesktopApi }) {
  const html = useMemo(() => renderMarkdown(text), [text]);
  const onClick = (e: MouseEvent<HTMLDivElement>) => {
    const el = e.target instanceof Element ? e.target : null;
    const link = el?.closest('a');
    if (link) {
      e.preventDefault();
      const href = safeHref(link.getAttribute('href'));
      if (href) void desktop.shell.openExternal(href);
      return;
    }
    const copy = el?.closest('button[data-copy]');
    if (copy) {
      e.preventDefault();
      const code = copy.closest('.lc-code')?.querySelector('code')?.textContent ?? '';
      void desktop.shell.clipboardWrite(code);
      copy.textContent = 'Copied';
      setTimeout(() => {
        if (copy.isConnected) copy.textContent = 'Copy';
      }, 1500);
    }
  };
  return (
    // biome-ignore lint/a11y/noStaticElementInteractions: delegated clicks on sanitised links and copy buttons
    // biome-ignore lint/a11y/useKeyWithClickEvents: links and buttons inside handle the keyboard natively
    <div
      className="text-sm break-words"
      onClick={onClick}
      onAuxClick={(e) => e.target instanceof Element && e.target.closest('a') && e.preventDefault()}
      // biome-ignore lint/security/noDangerouslySetInnerHtml: renderMarkdown sanitises with DOMPurify, no raw HTML or images
      dangerouslySetInnerHTML={{ __html: html }}
    />
  );
}

const STATUS_TEXT: Record<Extract<Item, { kind: 'tool' }>['status'], string> = {
  running: 'running',
  awaiting: 'waiting for approval',
  ok: 'done',
  error: 'failed',
  denied: 'denied',
};

export function ToolCard({ item }: { item: Extract<Item, { kind: 'tool' }> }) {
  const failed = item.status === 'error' || item.status === 'denied';
  return (
    <details className="border border-rule bg-surface text-xs">
      <summary className="cursor-pointer px-2 py-1 flex items-center gap-2">
        <span className="font-mono font-medium">{item.tool}</span>
        {item.risk === 'write' && <span className="text-muted">write</span>}
        <span className={failed ? 'text-danger' : 'text-muted'}>{STATUS_TEXT[item.status]}</span>
      </summary>
      <div className="flex flex-col gap-2 px-2 pb-2">
        <div>
          <p className="m-0 text-muted">Arguments</p>
          <pre className="m-0 overflow-x-auto font-mono whitespace-pre-wrap">{json(item.args)}</pre>
        </div>
        {item.result !== undefined && (
          <div>
            <p className="m-0 text-muted">Result</p>
            {/* Tool results are untrusted data: plain text only. */}
            <pre className="m-0 max-h-64 overflow-auto font-mono whitespace-pre-wrap">
              {item.result}
            </pre>
          </div>
        )}
      </div>
    </details>
  );
}

export function ApprovalCard({
  item,
  onDecide,
}: {
  item: Extract<Item, { kind: 'tool' }>;
  onDecide: (callId: string, d: ChatApproval) => void;
}) {
  return (
    <fieldset className="m-0 min-w-0 border border-accent bg-accent-soft p-2 flex flex-col gap-2 text-sm">
      <legend className="sr-only">Approval needed for {item.tool}</legend>
      <p className="m-0">
        The agent wants to run <code className="box-border font-mono">{item.tool}</code>, which
        changes data.
      </p>
      <pre className="m-0 max-h-40 overflow-auto font-mono text-xs whitespace-pre-wrap">
        {json(item.args)}
      </pre>
      <div className="flex flex-wrap gap-2">
        <Button variant="primary" size="sm" onClick={() => onDecide(item.callId, 'once')}>
          Approve once
        </Button>
        <Button size="sm" onClick={() => onDecide(item.callId, 'always')}>
          Always for this chat
        </Button>
        <Button variant="danger" size="sm" onClick={() => onDecide(item.callId, 'deny')}>
          Deny
        </Button>
      </div>
    </fieldset>
  );
}

function Messages({
  items,
  desktop,
  onDecide,
  running,
}: {
  items: Item[];
  desktop: LoamsDesktopApi;
  onDecide: (callId: string, d: ChatApproval) => void;
  running: boolean;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const pinned = useRef(true);
  useLayoutEffect(() => {
    const el = ref.current;
    if (el && pinned.current) el.scrollTop = el.scrollHeight;
  });
  return (
    <div
      ref={ref}
      role="log"
      aria-label="Conversation"
      aria-live="polite"
      className="flex-1 min-h-0 overflow-y-auto p-3 flex flex-col gap-3"
      onScroll={(e) => {
        const el = e.currentTarget;
        pinned.current = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
      }}
    >
      {items.length === 0 && (
        <p className="m-0 text-sm text-muted">
          Ask about your collections, SQL, durable promises, streams or connectors. Changes always
          wait for your approval.
        </p>
      )}
      {items.map((item, i) => {
        // Items have no ids of their own; position is stable because the list only grows.
        const key = `${i}:${item.kind}`;
        switch (item.kind) {
          case 'user':
            return (
              <div
                key={key}
                className="self-end max-w-[90%] bg-accent-soft p-2 text-sm whitespace-pre-wrap break-words"
              >
                {item.text}
              </div>
            );
          case 'text':
            return <Markdown key={key} text={item.text} desktop={desktop} />;
          case 'thinking':
            return (
              <details key={key} className="text-xs text-muted">
                <summary className="cursor-pointer">Thinking</summary>
                <p className="m-0 whitespace-pre-wrap">{item.text}</p>
              </details>
            );
          case 'tool':
            return (
              <div key={key} className="flex flex-col gap-2">
                <ToolCard item={item} />
                {item.status === 'awaiting' && <ApprovalCard item={item} onDecide={onDecide} />}
              </div>
            );
          case 'model':
            return (
              <p key={key} className="m-0 text-xs text-muted">
                Answered by <span className="box-border font-mono">{item.model}</span>
                {item.fallbackFrom && (
                  <>
                    {' '}
                    (server-side fallback from{' '}
                    <span className="box-border font-mono">{item.fallbackFrom}</span>)
                  </>
                )}
              </p>
            );
          case 'stop':
            return (
              <p key={key} className="m-0 text-xs text-muted">
                {stopLabel(item.stop)}
              </p>
            );
          case 'error':
            return (
              <p key={key} role="alert" className="m-0 text-sm text-danger">
                {item.message}
              </p>
            );
          default:
            return null;
        }
      })}
      {running && items.at(-1)?.kind !== 'text' && (
        <p className="m-0 text-xs text-muted">Working…</p>
      )}
    </div>
  );
}

function ChatList({ store, onClose }: { store: AgentStore; onClose: () => void }) {
  const snap = useSyncExternalStore(store.subscribe, store.getSnapshot);
  const [confirm, setConfirm] = useState<string>();
  return (
    <div className="flex-1 min-h-0 overflow-y-auto p-2">
      {snap.chats.length === 0 && <p className="m-0 p-2 text-sm text-muted">No chats yet.</p>}
      <ul className="list-none m-0 p-0 flex flex-col gap-1">
        {snap.chats.map((c) => (
          <li key={c.id} className="flex items-center gap-1">
            {confirm === c.id ? (
              <>
                <span className="flex-1 min-w-0 truncate text-sm px-2">Delete this chat?</span>
                <Button
                  size="sm"
                  variant="danger"
                  onClick={() => {
                    setConfirm(undefined);
                    void store.remove(c.id);
                  }}
                >
                  Delete
                </Button>
                <Button size="sm" onClick={() => setConfirm(undefined)}>
                  Keep
                </Button>
              </>
            ) : (
              <>
                <button
                  type="button"
                  aria-current={c.id === snap.activeId ? 'true' : undefined}
                  className={
                    c.id === snap.activeId
                      ? 'flex-1 min-w-0 text-left truncate px-2 py-1 text-sm bg-accent-soft cursor-pointer border-0 text-ink'
                      : 'flex-1 min-w-0 text-left truncate px-2 py-1 text-sm bg-transparent cursor-pointer border-0 text-ink'
                  }
                  onClick={() => {
                    void store.select(c.id);
                    onClose();
                  }}
                >
                  {c.title || 'New chat'}
                </button>
                <Button
                  size="icon"
                  variant="quiet"
                  aria-label={`Delete chat ${c.title || 'New chat'}`}
                  onClick={() => setConfirm(c.id)}
                >
                  <Trash2 aria-hidden="true" size={14} />
                </Button>
              </>
            )}
          </li>
        ))}
      </ul>
    </div>
  );
}

function Composer({ store }: { store: AgentStore }) {
  const snap = useSyncExternalStore(store.subscribe, store.getSnapshot);
  const [draft, setDraft] = useState('');
  const provider = snap.providers.find((p) => p.id === snap.pick?.provider);
  const ready = provider?.configured === true;
  const running = snap.state.running;
  const submit = useCallback(async () => {
    if (!draft.trim() || !ready || running) return;
    const text = draft;
    setDraft('');
    if (!(await store.send(text))) setDraft((d) => d || text);
  }, [draft, ready, running, store]);
  const onKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === 'Enter' && !e.shiftKey && !e.nativeEvent.isComposing) {
      e.preventDefault();
      void submit();
    }
  };
  return (
    <div className="border-t border-rule p-2 flex flex-col gap-2">
      {provider && !ready && (
        <div className="text-sm border border-rule bg-raised p-2" data-testid="provider-cta">
          <strong>{provider.label}</strong> has no API key yet.{' '}
          <a href={SETTINGS_HREF}>Set it up in Settings › Agent providers</a>
        </div>
      )}
      {snap.providersError && (
        <p role="alert" className="m-0 text-sm text-danger">
          {snap.providersError}
        </p>
      )}
      <div className="flex gap-2 items-end">
        <div className="flex-1 min-w-0">
          <textarea
            data-dock-autofocus=""
            aria-label="Message"
            rows={2}
            className="loams-input box-border resize-none"
            placeholder="Message the agent. Enter sends, Shift+Enter adds a line."
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={onKeyDown}
          />
        </div>
        {running ? (
          <Button aria-label="Stop" onClick={() => void store.cancel()}>
            <Square aria-hidden="true" size={14} /> Stop
          </Button>
        ) : (
          <Button
            variant="primary"
            disabled={!draft.trim() || !ready}
            onClick={() => void submit()}
          >
            Send
          </Button>
        )}
      </div>
      <div className="flex gap-2 items-center">
        <div className="w-36 shrink-0">
          <Select
            aria-label="Provider"
            className="box-border"
            value={snap.pick?.provider ?? ''}
            disabled={running}
            onChange={(e) => {
              const p = snap.providers.find((x) => x.id === e.target.value);
              if (p) store.setPick({ provider: p.id, model: p.model });
            }}
          >
            {snap.providers.map((p) => (
              <option key={p.id} value={p.id}>
                {p.label}
                {p.configured ? '' : ' (no key)'}
              </option>
            ))}
          </Select>
        </div>
        <div className="flex-1 min-w-0">
          <Input
            aria-label="Model"
            className="box-border font-mono"
            value={snap.pick?.model ?? ''}
            disabled={running}
            onChange={(e) =>
              snap.pick && store.setPick({ provider: snap.pick.provider, model: e.target.value })
            }
          />
        </div>
      </div>
    </div>
  );
}

export function AgentPanel({ store, desktop }: { store: AgentStore; desktop: LoamsDesktopApi }) {
  const snap = useSyncExternalStore(store.subscribe, store.getSnapshot);
  const [listOpen, setListOpen] = useState(false);
  useEffect(() => {
    void store.start();
    // A key added in Settings shows up when the user comes back to the window.
    const refresh = () => void store.refreshProviders();
    window.addEventListener('focus', refresh);
    return () => window.removeEventListener('focus', refresh);
  }, [store]);
  const title = snap.chats.find((c) => c.id === snap.activeId)?.title || 'New chat';
  return (
    <section aria-label="Agent chat" className="h-full flex flex-col min-h-0">
      <header className="flex items-center gap-2 px-3 py-2 border-b border-rule">
        <h2 className="m-0 text-sm font-medium flex-1 min-w-0 truncate">{title}</h2>
        <Button
          size="icon"
          variant="quiet"
          aria-label="Chats"
          aria-expanded={listOpen}
          onClick={() => setListOpen((o) => !o)}
        >
          <History aria-hidden="true" size={16} />
        </Button>
        <Button
          size="icon"
          variant="quiet"
          aria-label="New chat"
          onClick={() => {
            store.newChat();
            setListOpen(false);
          }}
        >
          <Plus aria-hidden="true" size={16} />
        </Button>
      </header>
      {snap.error && (
        <p role="alert" className="m-0 px-3 py-2 text-sm text-danger">
          {snap.error}
        </p>
      )}
      {listOpen ? (
        <ChatList store={store} onClose={() => setListOpen(false)} />
      ) : (
        <Messages
          items={snap.state.items}
          running={snap.state.running}
          desktop={desktop}
          onDecide={(callId, d) => void store.approve(callId, d)}
        />
      )}
      <Composer store={store} />
    </section>
  );
}
