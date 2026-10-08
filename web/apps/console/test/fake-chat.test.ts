import type { ChatEvent } from '@loams/desktop/contracts';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { createFakeDesktop } from '../src/cordis/fake-desktop.js';

afterEach(() => vi.useRealTimers());

describe('fake desktop chat', () => {
  it('scripted_turn_streams_and_waits_for_approval', async () => {
    vi.useFakeTimers();
    const { chat } = createFakeDesktop();
    const events: ChatEvent[] = [];
    chat.onEvent((e) => events.push(e));
    expect((await chat.providers()).find((p) => p.id === 'anthropic')?.configured).toBe(true);
    const c = await chat.create();
    if (!c.ok) throw new Error('create');
    expect((await chat.send(c.value.id, 'create a promise')).ok).toBe(true);
    await vi.advanceTimersByTimeAsync(5_000);
    const call = events.find((e) => e.kind === 'tool_call' && e.needsApproval);
    expect(call).toMatchObject({ tool: 'durable_promise_create', risk: 'write' });
    const view = await chat.get(c.value.id);
    expect(view.ok && view.value.pending).toHaveLength(1);
    if (call?.kind !== 'tool_call') throw new Error('no call');
    expect((await chat.approve(c.value.id, call.callId, 'deny')).ok).toBe(true);
    await vi.advanceTimersByTimeAsync(10_000);
    expect(events.at(-1)).toMatchObject({ kind: 'done', stop: 'end_turn' });
    expect(events.some((e) => e.kind === 'delta')).toBe(true);
    expect(events).toContainEqual(
      expect.objectContaining({
        kind: 'tool_result',
        ok: false,
        text: 'The user denied this action.',
      }),
    );
    expect((await chat.list())[0]?.title).toBe('create a promise');
  });

  it('unconfigured_provider_is_refused', async () => {
    const { chat } = createFakeDesktop();
    const c = await chat.create({ provider: 'deepseek' });
    if (!c.ok) throw new Error('create');
    expect(await chat.send(c.value.id, 'hi')).toMatchObject({ ok: false, code: 'unconfigured' });
  });
});
