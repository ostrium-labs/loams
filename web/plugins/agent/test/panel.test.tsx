import type {
  ChatApproval,
  ChatEvent,
  ChatProviderInfo,
  ChatView,
  IpcResult,
  LoamsDesktopApi,
} from '@loams/desktop/contracts';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import pkg from '../package.json';
import { contextHint } from '../src/index.js';
import { AgentPanel } from '../src/panel.js';
import { AgentStore, applyEvent, itemsFromView } from '../src/store.js';

afterEach(cleanup);

const provider = (over: Partial<ChatProviderInfo> = {}): ChatProviderInfo => ({
  id: 'anthropic',
  label: 'Anthropic',
  kind: 'anthropic',
  baseUrl: 'https://api.anthropic.com',
  model: 'claude-sonnet-5-5',
  defaultModel: 'claude-sonnet-5-5',
  needsKey: true,
  hasKey: true,
  configured: true,
  persistent: true,
  fallback: false,
  ...over,
});
const ok = <T,>(value: T): IpcResult<T> => ({ ok: true, value });

function fakeDesktop(providers: ChatProviderInfo[] = [provider()]) {
  const listeners = new Set<(e: ChatEvent) => void>();
  const sent: { chatId: string; text: string; opts: unknown }[] = [];
  const approvals: { callId: string; decision: ChatApproval }[] = [];
  const open = vi.fn(async () => ok(undefined));
  const clip = vi.fn(async () => {});
  let view: ChatView | undefined;
  const chat = {
    providers: vi.fn(async () => providers),
    list: vi.fn(async () => (view ? [view] : [])),
    create: vi.fn(async (o?: { provider?: string; model?: string }) => {
      view = {
        id: 'c_abcdefgh',
        title: '',
        createdAt: 1,
        updatedAt: 1,
        provider: (o?.provider ?? 'anthropic') as ChatView['provider'],
        model: o?.model ?? 'm',
        alwaysAllow: [],
        messages: [],
        running: false,
        pending: [],
      };
      return ok(view);
    }),
    get: vi.fn(async () =>
      view ? ok(view) : { ok: false as const, code: 'not_found', message: 'nope' },
    ),
    send: vi.fn(async (chatId: string, text: string, opts?: unknown) => {
      sent.push({ chatId, text, opts });
      return ok(undefined);
    }),
    cancel: vi.fn(async () => {}),
    approve: vi.fn(async (_c: string, callId: string, decision: ChatApproval) => {
      approvals.push({ callId, decision });
      return ok(undefined);
    }),
    remove: vi.fn(async () => ok(undefined)),
    configureProvider: vi.fn(),
    testProvider: vi.fn(),
    onEvent: (cb: (e: ChatEvent) => void) => {
      listeners.add(cb);
      return () => listeners.delete(cb);
    },
  };
  const desktop = {
    chat,
    shell: { openExternal: open, clipboardWrite: clip },
  } as unknown as LoamsDesktopApi;
  const emit = (e: ChatEvent) =>
    act(() => {
      for (const l of listeners) l(e);
    });
  return { desktop, chat, emit, sent, approvals, open, clip, setView: (v: ChatView) => (view = v) };
}

async function mount(d = fakeDesktop(), context?: () => string | undefined) {
  const store = new AgentStore({ desktop: d.desktop, ...(context ? { context } : {}) });
  render(<AgentPanel store={store} desktop={d.desktop} />);
  await waitFor(() => expect(screen.getByLabelText('Provider')).toBeTruthy());
  await waitFor(() =>
    expect((screen.getByLabelText('Model') as HTMLInputElement).value).not.toBe(''),
  );
  return { ...d, store };
}

const type = (text: string) =>
  fireEvent.change(screen.getByLabelText('Message'), { target: { value: text } });
const CID = 'c_abcdefgh';

describe('agent panel', () => {
  it('manifest declares the dock and settings slots, desktop only', () => {
    expect(pkg.loams.plugin.slots).toEqual(['shell.dock.right', 'console.settings.section']);
    expect(pkg.loams.plugin.editions).toEqual(['desktop']);
    expect(pkg.loams.plugin.inject).toContain('desktop');
  });

  it('streams_deltas_into_message', async () => {
    const d = await mount(
      undefined,
      () => 'The user is viewing Postgres (route /postgres). Active namespace: default.',
    );
    type('hello there');
    fireEvent.keyDown(screen.getByLabelText('Message'), { key: 'Enter' });
    await waitFor(() => expect(d.sent).toHaveLength(1));
    expect(d.sent[0]).toMatchObject({
      chatId: CID,
      text: 'hello there',
      opts: {
        provider: 'anthropic',
        model: 'claude-sonnet-5-5',
        context: expect.stringContaining('Active namespace: default'),
      },
    });
    expect((screen.getByLabelText('Message') as HTMLTextAreaElement).value).toBe('');
    d.emit({ kind: 'delta', chatId: CID, text: 'Hello ' });
    d.emit({ kind: 'delta', chatId: CID, text: '**wor' });
    d.emit({ kind: 'delta', chatId: CID, text: 'ld**' });
    const log = screen.getByRole('log');
    expect(within(log).getByText('hello there')).toBeTruthy();
    expect(log.querySelector('strong')?.textContent).toBe('world');
    expect(log.textContent).toContain('Hello world');
    d.emit({
      kind: 'done',
      chatId: CID,
      stop: 'end_turn',
      usage: { inputTokens: 1, outputTokens: 1 },
    });
    expect(screen.queryByRole('button', { name: 'Stop' })).toBeNull();
    // A normal end shows no footnote.
    expect(log.textContent).not.toMatch(/Stopped/);
  });

  it('shift_enter_adds_a_line_and_does_not_send', async () => {
    const d = await mount();
    type('a');
    fireEvent.keyDown(screen.getByLabelText('Message'), { key: 'Enter', shiftKey: true });
    expect(d.sent).toHaveLength(0);
  });

  it('approval_card_actions', async () => {
    const d = await mount();
    type('create it');
    fireEvent.keyDown(screen.getByLabelText('Message'), { key: 'Enter' });
    await waitFor(() => expect(d.sent).toHaveLength(1));
    d.emit({
      kind: 'tool_call',
      chatId: CID,
      callId: 'k1',
      tool: 'durable_promise_create',
      args: { id: 'p1' },
      risk: 'write',
      needsApproval: true,
    });
    const tool = screen.getByRole('log').querySelector('details') as HTMLDetailsElement;
    expect(tool.open).toBe(false);
    const card = screen.getByRole('group', { name: /approval needed for durable_promise_create/i });
    expect(card.textContent).toContain('"id": "p1"');
    fireEvent.click(within(card).getByRole('button', { name: 'Approve once' }));
    await waitFor(() => expect(d.approvals).toEqual([{ callId: 'k1', decision: 'once' }]));
    // The card goes away once decided.
    await waitFor(() =>
      expect(screen.queryByRole('group', { name: /approval needed/i })).toBeNull(),
    );

    d.emit({
      kind: 'tool_call',
      chatId: CID,
      callId: 'k2',
      tool: 'live_mutate',
      args: {},
      risk: 'write',
      needsApproval: true,
    });
    fireEvent.click(screen.getByRole('button', { name: 'Always for this chat' }));
    await waitFor(() => expect(d.approvals).toHaveLength(2));
    await waitFor(() =>
      expect(screen.queryByRole('group', { name: /approval needed/i })).toBeNull(),
    );
    d.emit({
      kind: 'tool_call',
      chatId: CID,
      callId: 'k3',
      tool: 'live_mutate',
      args: {},
      risk: 'write',
      needsApproval: true,
    });
    fireEvent.click(screen.getByRole('button', { name: 'Deny' }));
    await waitFor(() => expect(d.approvals).toHaveLength(3));
    expect(d.approvals.map((a) => a.decision)).toEqual(['once', 'always', 'deny']);
  });

  it('tool_cards_are_collapsible_plain_text', async () => {
    const d = await mount();
    type('x');
    fireEvent.keyDown(screen.getByLabelText('Message'), { key: 'Enter' });
    await waitFor(() => expect(d.sent).toHaveLength(1));
    d.emit({
      kind: 'tool_call',
      chatId: CID,
      callId: 'r1',
      tool: 'collections_list',
      args: {},
      risk: 'read',
      needsApproval: false,
    });
    d.emit({
      kind: 'tool_result',
      chatId: CID,
      callId: 'r1',
      ok: true,
      text: '<img src="https://evil.test/x.png"><script>1</script>',
    });
    const log = screen.getByRole('log');
    const details = log.querySelector('details') as HTMLDetailsElement;
    expect(details.open).toBe(false);
    expect(details.querySelector('summary')?.textContent).toContain('collections_list');
    expect(log.querySelector('img, script')).toBeNull();
    expect(details.textContent).toContain('<img src="https://evil.test/x.png">');
  });

  it('stop_button_cancels', async () => {
    const d = await mount();
    type('long job');
    fireEvent.keyDown(screen.getByLabelText('Message'), { key: 'Enter' });
    const stop = await screen.findByRole('button', { name: 'Stop' });
    fireEvent.click(stop);
    expect(d.chat.cancel).toHaveBeenCalledWith(CID);
    d.emit({
      kind: 'done',
      chatId: CID,
      stop: 'cancelled',
      usage: { inputTokens: 0, outputTokens: 0 },
    });
    expect(screen.getByText('Stopped by you')).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'Stop' })).toBeNull();
  });

  it('no_raw_html_rendered', async () => {
    const d = await mount();
    type('x');
    fireEvent.keyDown(screen.getByLabelText('Message'), { key: 'Enter' });
    await waitFor(() => expect(d.sent).toHaveLength(1));
    d.emit({
      kind: 'delta',
      chatId: CID,
      text: 'a <script>alert(1)</script> <img src=x onerror=alert(1)> ![p](https://evil.test/p.png) [go](https://example.com)',
    });
    const log = screen.getByRole('log');
    expect(log.querySelector('script, img')).toBeNull();
    expect(log.textContent).toContain('<script>alert(1)</script>');
    expect(log.innerHTML).not.toContain('evil.test/p.png');
    fireEvent.click(within(log).getByRole('link', { name: 'go' }));
    expect(d.open).toHaveBeenCalledWith('https://example.com/');
  });

  it('copy_button_copies_code', async () => {
    const d = await mount();
    type('x');
    fireEvent.keyDown(screen.getByLabelText('Message'), { key: 'Enter' });
    await waitFor(() => expect(d.sent).toHaveLength(1));
    d.emit({ kind: 'delta', chatId: CID, text: '```sql\nSELECT 1\n```' });
    fireEvent.click(screen.getByRole('button', { name: 'Copy' }));
    expect(d.clip).toHaveBeenCalledWith('SELECT 1');
  });

  it('unconfigured_provider_cta', async () => {
    const d = fakeDesktop([
      provider({
        id: 'deepseek',
        label: 'DeepSeek',
        kind: 'openai',
        hasKey: false,
        configured: false,
        model: 'deepseek-chat',
      }),
    ]);
    await mount(d);
    const cta = screen.getByTestId('provider-cta');
    expect(cta.textContent).toContain('DeepSeek has no API key yet');
    expect(within(cta).getByRole('link').getAttribute('href')).toBe('#/settings/agent');
    type('hi');
    expect((screen.getByRole('button', { name: 'Send' }) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.keyDown(screen.getByLabelText('Message'), { key: 'Enter' });
    expect(d.sent).toHaveLength(0);
  });

  it('picker_switches_provider_and_model', async () => {
    const d = await mount(
      fakeDesktop([
        provider(),
        provider({
          id: 'ollama',
          label: 'Ollama',
          kind: 'openai',
          needsKey: false,
          hasKey: false,
          model: 'llama3.1',
        }),
      ]),
    );
    fireEvent.change(screen.getByLabelText('Provider'), { target: { value: 'ollama' } });
    expect((screen.getByLabelText('Model') as HTMLInputElement).value).toBe('llama3.1');
    type('hi');
    fireEvent.keyDown(screen.getByLabelText('Message'), { key: 'Enter' });
    await waitFor(() => expect(d.sent).toHaveLength(1));
    expect(d.sent[0]?.opts).toMatchObject({ provider: 'ollama', model: 'llama3.1' });
  });

  it('shows model and stop notes', async () => {
    const d = await mount();
    type('x');
    fireEvent.keyDown(screen.getByLabelText('Message'), { key: 'Enter' });
    await waitFor(() => expect(d.sent).toHaveLength(1));
    d.emit({
      kind: 'model',
      chatId: CID,
      model: 'claude-opus-4-7',
      fallbackFrom: 'claude-sonnet-5-5',
    });
    expect(screen.getByRole('log').textContent).toContain('Answered by claude-opus-4-7');
    expect(screen.getByRole('log').textContent).toContain('fallback from claude-sonnet-5-5');
    d.emit({
      kind: 'done',
      chatId: CID,
      stop: 'iteration_cap',
      usage: { inputTokens: 0, outputTokens: 0 },
    });
    expect(screen.getByText('Stopped: iteration cap (25)')).toBeTruthy();
  });

  it('a send that fails shows the message and keeps the draft', async () => {
    const d = fakeDesktop();
    d.chat.send.mockResolvedValueOnce({
      ok: false,
      code: 'busy',
      message: 'This chat is still answering.',
    });
    await mount(d);
    type('hello');
    fireEvent.keyDown(screen.getByLabelText('Message'), { key: 'Enter' });
    expect(await screen.findByText('This chat is still answering.')).toBeTruthy();
    expect((screen.getByLabelText('Message') as HTMLTextAreaElement).value).toBe('hello');
  });

  it('chat list selects and deletes', async () => {
    const d = fakeDesktop();
    const v: ChatView = {
      id: CID,
      title: 'Earlier question',
      createdAt: 1,
      updatedAt: 2,
      provider: 'anthropic',
      model: 'm',
      alwaysAllow: [],
      running: false,
      pending: [],
      messages: [
        { role: 'user', content: [{ type: 'text', text: 'Earlier question' }], at: 1 },
        { role: 'assistant', content: [{ type: 'text', text: 'An answer' }], at: 2 },
      ],
    };
    d.setView(v);
    await mount(d);
    fireEvent.click(screen.getByRole('button', { name: 'Chats' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Earlier question' }));
    expect(await screen.findByText('An answer')).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Chats' }));
    fireEvent.click(screen.getByRole('button', { name: /Delete chat Earlier question/ }));
    fireEvent.click(screen.getByRole('button', { name: 'Delete' }));
    await waitFor(() => expect(d.chat.remove).toHaveBeenCalledWith(CID));
  });
});

describe('store model', () => {
  it('restores a running turn with a pending approval after a reload', () => {
    const items = itemsFromView({
      id: CID,
      title: 't',
      createdAt: 1,
      updatedAt: 1,
      provider: 'anthropic',
      model: 'm',
      alwaysAllow: [],
      running: true,
      pending: [{ callId: 'k1', tool: 'live_mutate', args: { action: 'insert' }, risk: 'write' }],
      messages: [
        { role: 'user', content: [{ type: 'text', text: 'add one' }], at: 1 },
        {
          role: 'assistant',
          content: [
            { type: 'tool_use', id: 'k1', name: 'live_mutate', input: { action: 'insert' } },
          ],
          at: 2,
        },
      ],
    });
    expect(items.at(-1)).toMatchObject({
      kind: 'tool',
      callId: 'k1',
      status: 'awaiting',
      risk: 'write',
    });
  });

  it('answered-by note only when the serving model differs, same as live', () => {
    const base = {
      id: CID,
      title: 't',
      createdAt: 1,
      updatedAt: 1,
      provider: 'anthropic' as const,
      model: 'm',
      alwaysAllow: [],
      running: false,
      pending: [],
    };
    const msg = (model: string, fallbackFrom?: string) => ({
      role: 'assistant' as const,
      content: [{ type: 'text' as const, text: 'hi' }],
      at: 1,
      model,
      ...(fallbackFrom ? { fallbackFrom } : {}),
    });
    expect(itemsFromView({ ...base, messages: [msg('m')] }).map((i) => i.kind)).toEqual(['text']);
    expect(itemsFromView({ ...base, messages: [msg('other')] }).map((i) => i.kind)).toEqual([
      'text',
      'model',
    ]);
    expect(itemsFromView({ ...base, messages: [msg('m', 'x')] }).map((i) => i.kind)).toEqual([
      'text',
      'model',
    ]);
  });

  it('folds tool results into cards and marks denials', () => {
    const items = itemsFromView({
      id: CID,
      title: 't',
      createdAt: 1,
      updatedAt: 1,
      provider: 'anthropic',
      model: 'm',
      alwaysAllow: [],
      running: false,
      pending: [],
      messages: [
        { role: 'user', content: [{ type: 'text', text: 'go' }], at: 1 },
        {
          role: 'assistant',
          content: [{ type: 'tool_use', id: 'a', name: 'x', input: {} }],
          at: 2,
        },
        {
          role: 'user',
          content: [
            {
              type: 'tool_result',
              toolUseId: 'a',
              text: 'The user denied this action.',
              isError: true,
            },
          ],
          at: 3,
        },
        { role: 'assistant', content: [{ type: 'text', text: 'ok' }], at: 4, stop: 'token_budget' },
      ],
    });
    expect(items.map((i) => i.kind)).toEqual(['user', 'tool', 'text', 'stop']);
    expect(items[1]).toMatchObject({ status: 'denied' });
  });

  it('ignores events for chats it has not loaded and applies done', () => {
    const s = applyEvent(
      { id: 'c', items: [], running: true },
      { kind: 'done', chatId: 'c', stop: 'llm_error', usage: { inputTokens: 0, outputTokens: 0 } },
    );
    expect(s.running).toBe(false);
    expect(s.items).toEqual([{ kind: 'stop', stop: 'llm_error' }]);
  });

  it('context hint names the page and namespace', () => {
    expect(contextHint({ path: '/postgres/branches' }, 'Postgres', 'prod')).toBe(
      'The user is viewing Postgres (route /postgres/branches). Active namespace: prod.',
    );
    expect(contextHint({ path: '/' }, undefined, undefined)).toBe(
      'The user is on the page at route /.',
    );
    expect(contextHint({ path: '/live?token=abc#x' }, 'Live', undefined)).toBe(
      'The user is viewing Live (route /live).',
    );
  });
});
