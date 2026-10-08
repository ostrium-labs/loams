// The panel's state, outside React so it survives the dock being hidden or the
// panel remounting (the shell also keeps the dock mounted). One store per
// desktop bridge; components read it through `useSyncExternalStore`.

import type {
  ChatApproval,
  ChatEvent,
  ChatMessage,
  ChatProviderId,
  ChatProviderInfo,
  ChatStopReason,
  ChatSummary,
  ChatView,
  LoamsDesktopApi,
} from '@loams/desktop/contracts';

export const DENIED_TEXT = 'The user denied this action.';

export type ToolStatus = 'running' | 'awaiting' | 'ok' | 'error' | 'denied';

export type Item =
  | { kind: 'user'; text: string }
  | { kind: 'text'; text: string; streaming?: boolean }
  | { kind: 'thinking'; text: string; streaming?: boolean }
  | {
      kind: 'tool';
      callId: string;
      tool: string;
      args: unknown;
      risk: 'read' | 'write';
      status: ToolStatus;
      result?: string;
    }
  | { kind: 'model'; model: string; fallbackFrom?: string }
  | { kind: 'stop'; stop: ChatStopReason }
  | { kind: 'error'; message: string };

export interface ChatState {
  id: string;
  items: Item[];
  running: boolean;
}

export interface Pick {
  provider: ChatProviderId;
  model: string;
}

export interface Snapshot {
  providers: ChatProviderInfo[];
  providersError?: string;
  chats: ChatSummary[];
  activeId?: string;
  /** The active chat's state; an empty one while a new chat has not been created yet. */
  state: ChatState;
  pick?: Pick;
  loading: boolean;
  error?: string;
}

const EMPTY: ChatState = { id: '', items: [], running: false };

/** Items from stored messages: tool results are folded into their tool cards. */
export function itemsFromView(view: ChatView): Item[] {
  const items: Item[] = [];
  const tools = new Map<string, Extract<Item, { kind: 'tool' }>>();
  const resolve = (m: ChatMessage) => {
    for (const p of m.content) {
      if (p.type !== 'tool_result') continue;
      const t = tools.get(p.toolUseId);
      if (!t) continue;
      t.result = p.text;
      t.status = p.text === DENIED_TEXT ? 'denied' : p.isError ? 'error' : 'ok';
    }
  };
  for (const m of view.messages) {
    if (m.role === 'user') {
      const text = m.content.flatMap((p) => (p.type === 'text' ? [p.text] : [])).join('\n');
      if (text) items.push({ kind: 'user', text });
      resolve(m);
      continue;
    }
    for (const p of m.content) {
      if (p.type === 'text' && p.text) items.push({ kind: 'text', text: p.text });
      else if (p.type === 'thinking' && p.text) items.push({ kind: 'thinking', text: p.text });
      else if (p.type === 'tool_use') {
        const t: Extract<Item, { kind: 'tool' }> = {
          kind: 'tool',
          callId: p.id,
          tool: p.name,
          args: p.input,
          risk: 'read',
          status: 'running',
        };
        tools.set(p.id, t);
        items.push(t);
      }
    }
    // Same rule as the live event: only when the serving model differs or a fallback happened.
    if (m.model && (m.fallbackFrom || m.model !== view.model)) {
      items.push({
        kind: 'model',
        model: m.model,
        ...(m.fallbackFrom ? { fallbackFrom: m.fallbackFrom } : {}),
      });
    }
    if (m.stop && m.stop !== 'end_turn') items.push({ kind: 'stop', stop: m.stop });
  }
  for (const p of view.pending) {
    const t = tools.get(p.callId);
    if (t) {
      t.status = 'awaiting';
      t.risk = p.risk;
    } else {
      items.push({
        kind: 'tool',
        callId: p.callId,
        tool: p.tool,
        args: p.args,
        risk: p.risk,
        status: 'awaiting',
      });
    }
  }
  for (const t of tools.values()) {
    if (t.status === 'running' && !view.running) {
      t.status = 'error';
      t.result = 'No result was recorded for this call.';
    }
  }
  return items;
}

const close = (items: Item[]): Item[] =>
  items.map((i) =>
    (i.kind === 'text' || i.kind === 'thinking') && i.streaming ? { ...i, streaming: false } : i,
  );

/** Applies one event to a chat's state (pure). */
export function applyEvent(state: ChatState, e: ChatEvent): ChatState {
  const items = state.items;
  switch (e.kind) {
    case 'delta':
    case 'thinking': {
      const kind = e.kind === 'delta' ? 'text' : 'thinking';
      const last = items.at(-1);
      if (last && last.kind === kind && last.streaming) {
        return {
          ...state,
          items: [...items.slice(0, -1), { ...last, text: last.text + e.text }],
        };
      }
      return { ...state, items: [...close(items), { kind, text: e.text, streaming: true }] };
    }
    case 'tool_call':
      return {
        ...state,
        items: [
          ...close(items),
          {
            kind: 'tool',
            callId: e.callId,
            tool: e.tool,
            args: e.args,
            risk: e.risk,
            status: e.needsApproval ? 'awaiting' : 'running',
          },
        ],
      };
    case 'tool_result':
      return {
        ...state,
        items: items.map((i) =>
          i.kind === 'tool' && i.callId === e.callId
            ? {
                ...i,
                result: e.text,
                status: e.text === DENIED_TEXT ? 'denied' : e.ok ? 'ok' : 'error',
              }
            : i,
        ),
      };
    case 'model':
      return {
        ...state,
        items: [
          ...close(items),
          {
            kind: 'model',
            model: e.model,
            ...(e.fallbackFrom ? { fallbackFrom: e.fallbackFrom } : {}),
          },
        ],
      };
    case 'error':
      return { ...state, items: [...close(items), { kind: 'error', message: e.message }] };
    case 'done': {
      const next = close(items);
      if (e.stop !== 'end_turn') next.push({ kind: 'stop', stop: e.stop });
      return { ...state, running: false, items: next };
    }
  }
}

export interface StoreDeps {
  desktop: LoamsDesktopApi;
  /** The hint sent with each message ("The user is viewing Postgres › Branches."). */
  context?: () => string | undefined;
}

export class AgentStore {
  #states = new Map<string, ChatState>();
  #snap: Snapshot;
  #listeners = new Set<() => void>();
  #off?: () => void;
  #started?: Promise<void>;
  /** The new-chat draft state (no chat exists until the first message). */
  #draft: ChatState = EMPTY;

  constructor(private readonly deps: StoreDeps) {
    this.#snap = { providers: [], chats: [], state: EMPTY, loading: true };
  }

  subscribe = (l: () => void): (() => void) => {
    this.#listeners.add(l);
    return () => this.#listeners.delete(l);
  };
  getSnapshot = (): Snapshot => this.#snap;

  #set(patch: Partial<Snapshot>): void {
    const next = { ...this.#snap, ...patch };
    const id = next.activeId;
    next.state = id ? (this.#states.get(id) ?? EMPTY) : this.#draft;
    this.#snap = next;
    for (const l of this.#listeners) l();
  }

  /** Subscribes to events and loads providers and chats, once. */
  start(): Promise<void> {
    this.#started ??= this.#start();
    return this.#started;
  }

  async #start(): Promise<void> {
    this.#off = this.deps.desktop.chat.onEvent((e) => this.#onEvent(e));
    await Promise.all([this.refreshProviders(), this.refreshChats()]);
    this.#set({ loading: false });
  }

  dispose(): void {
    this.#off?.();
    this.#off = undefined;
    this.#started = undefined;
  }

  async refreshProviders(): Promise<void> {
    try {
      const providers = await this.deps.desktop.chat.providers();
      const pick = this.#snap.pick;
      const first = providers.find((p) => p.configured) ?? providers[0];
      this.#set({
        providers,
        providersError: undefined,
        pick:
          pick && providers.some((p) => p.id === pick.provider)
            ? pick
            : first && { provider: first.id, model: first.model },
      });
    } catch (e) {
      this.#set({ providersError: errMsg(e) });
    }
  }

  async refreshChats(): Promise<void> {
    try {
      this.#set({ chats: await this.deps.desktop.chat.list() });
    } catch (e) {
      this.#set({ error: errMsg(e) });
    }
  }

  #update(id: string, fn: (s: ChatState) => ChatState): void {
    const cur = this.#states.get(id);
    if (!cur) return;
    this.#states.set(id, fn(cur));
    this.#set({});
  }

  #onEvent(e: ChatEvent): void {
    if (!this.#states.has(e.chatId)) return;
    this.#update(e.chatId, (s) => applyEvent(s, e));
    if (e.kind === 'done') void this.refreshChats();
  }

  /** A fresh, empty chat (created on the first message). */
  newChat(): void {
    this.#draft = EMPTY;
    const first = this.#snap.providers.find((p) => p.configured) ?? this.#snap.providers[0];
    this.#set({
      activeId: undefined,
      error: undefined,
      pick: this.#snap.pick ?? (first && { provider: first.id, model: first.model }),
    });
  }

  async select(id: string): Promise<void> {
    const known = this.#states.get(id);
    if (known?.running) {
      this.#set({ activeId: id, error: undefined });
      return;
    }
    const r = await this.deps.desktop.chat.get(id);
    if (!r.ok) {
      this.#set({ error: r.message });
      return;
    }
    this.#states.set(id, {
      id,
      running: r.value.running,
      items: itemsFromView(r.value),
    });
    this.#set({
      activeId: id,
      error: undefined,
      pick: { provider: r.value.provider, model: r.value.model },
    });
  }

  setPick(pick: Pick): void {
    this.#set({ pick });
  }

  async remove(id: string): Promise<void> {
    const r = await this.deps.desktop.chat.remove(id);
    if (!r.ok) {
      this.#set({ error: r.message });
      return;
    }
    this.#states.delete(id);
    if (this.#snap.activeId === id) this.#set({ activeId: undefined });
    await this.refreshChats();
  }

  async send(text: string): Promise<boolean> {
    const body = text.trim();
    const pick = this.#snap.pick;
    if (!body || !pick || this.#snap.state.running) return false;
    let id = this.#snap.activeId;
    if (!id) {
      const c = await this.deps.desktop.chat.create({
        provider: pick.provider,
        model: pick.model,
      });
      if (!c.ok) {
        this.#set({ error: c.message });
        return false;
      }
      id = c.value.id;
      this.#states.set(id, { id, items: [], running: false });
      this.#set({ activeId: id, error: undefined });
    }
    const chatId = id;
    this.#update(chatId, (s) => ({
      ...s,
      running: true,
      items: [...close(s.items), { kind: 'user', text: body }],
    }));
    const ctx = this.deps.context?.();
    const r = await this.deps.desktop.chat.send(chatId, body, {
      provider: pick.provider,
      model: pick.model,
      ...(ctx ? { context: ctx } : {}),
    });
    if (!r.ok) {
      this.#update(chatId, (s) => ({
        ...s,
        running: false,
        items: [...s.items, { kind: 'error', message: r.message }],
      }));
      void this.refreshProviders();
      return false;
    }
    void this.refreshChats();
    return true;
  }

  async cancel(): Promise<void> {
    const id = this.#snap.activeId;
    if (id) await this.deps.desktop.chat.cancel(id);
  }

  async approve(callId: string, decision: ChatApproval): Promise<void> {
    const id = this.#snap.activeId;
    if (!id) return;
    const r = await this.deps.desktop.chat.approve(id, callId, decision);
    if (!r.ok) {
      this.#update(id, (s) => ({
        ...s,
        items: [...s.items, { kind: 'error', message: r.message }],
      }));
      return;
    }
    this.#update(id, (s) => ({
      ...s,
      items: s.items.map((i) =>
        i.kind === 'tool' && i.callId === callId && i.status === 'awaiting'
          ? { ...i, status: decision === 'deny' ? 'denied' : 'running' }
          : i,
      ),
    }));
  }
}

const errMsg = (e: unknown): string => (e instanceof Error ? e.message : String(e));
