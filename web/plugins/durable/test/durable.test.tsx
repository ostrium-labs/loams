import { validateManifest } from '@loams/console-host';
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import pkg from '../package.json';
import {
  createDurableApi,
  decodeValue,
  EnvelopeError,
  envelope,
  toBase64,
} from '../src/envelope.js';
import { PromisesTab, RunsTab } from '../src/pages.js';
import { buildRuns } from '../src/runs.js';

afterEach(cleanup);

// jsdom has no modal <dialog>: open it by attribute.
HTMLDialogElement.prototype.showModal ??= function (this: HTMLDialogElement) {
  this.setAttribute('open', '');
};
HTMLDialogElement.prototype.close ??= function (this: HTMLDialogElement) {
  this.removeAttribute('open');
};

const rec = (id: string, over: Record<string, unknown> = {}) => ({
  id,
  state: 'pending',
  param: {},
  value: {},
  tags: {},
  timeoutAt: 2_000_000,
  createdAt: 1_000_000,
  ...over,
});

describe('envelope', () => {
  it('envelope_shape_and_error', async () => {
    const fetchImpl = vi.fn(
      async (_u: string, _i?: RequestInit) =>
        new Response(
          JSON.stringify({
            kind: 'promise.get',
            head: { corrId: 'x', status: 200, version: '2026-04-01' },
            data: { promise: rec('a') },
          }),
        ),
    );
    const send = envelope(fetchImpl as never, 'loams-app://console');
    expect(await send('promise.get', { id: 'a' })).toEqual({ promise: rec('a') });
    const [url, init] = fetchImpl.mock.calls[0] as [string, RequestInit];
    expect(url).toBe('loams-app://console/durable/');
    expect(init.method).toBe('POST');
    const body = JSON.parse(init.body as string);
    expect(body).toMatchObject({
      kind: 'promise.get',
      head: { version: '2026-04-01' },
      data: { id: 'a' },
    });
    expect(body.head.corrId).toMatch(/^[0-9a-f-]{36}$/);

    const bad = envelope(
      (async () =>
        new Response(
          JSON.stringify({
            kind: 'promise.get',
            head: { corrId: 'x', status: 404, version: 'v' },
            data: 'promise not found',
          }),
        )) as never,
    );
    await expect(bad('promise.get', { id: 'z' })).rejects.toMatchObject({
      status: 404,
      message: 'promise not found',
    });
    // a proxy refusal is {code, message}
    const refused = envelope(
      (async () =>
        new Response(JSON.stringify({ code: 'not_available_remote', message: 'nope' }), {
          status: 404,
        })) as never,
    );
    await expect(refused('promise.get', {})).rejects.toBeInstanceOf(EnvelopeError);
  });
});

describe('payloads', () => {
  it('param_decoding_json_or_base64', () => {
    expect(decodeValue({ data: toBase64('{"a":1}') })).toMatchObject({
      kind: 'json',
      text: '{\n  "a": 1\n}',
    });
    const plain = decodeValue({ data: toBase64('hello') });
    expect(plain).toMatchObject({ kind: 'base64', raw: toBase64('hello'), text: 'hello' });
    expect(decodeValue({ data: '/w==' })).toEqual({ kind: 'base64', raw: '/w==' });
    expect(decodeValue({})).toEqual({ kind: 'empty' });
  });
});

describe('promises tab', () => {
  it('promise_filters_and_paging', async () => {
    const send = vi.fn(async (_k: string, d: Record<string, any>) =>
      d.cursor ? { promises: [rec('p3')] } : { promises: [rec('p1'), rec('p2')], cursor: 'c1' },
    );
    render(<PromisesTab api={createDurableApi(send)} />);
    await screen.findByText('p1');
    expect(send).toHaveBeenLastCalledWith('promise.search', { limit: 50 });
    fireEvent.change(screen.getByLabelText('State'), { target: { value: 'resolved' } });
    await waitFor(() =>
      expect(send).toHaveBeenLastCalledWith('promise.search', { state: 'resolved', limit: 50 }),
    );
    fireEvent.change(screen.getByLabelText('Tag (key=value)'), { target: { value: 'team=core' } });
    fireEvent.click(screen.getByText('Add tag'));
    await waitFor(() =>
      expect(send).toHaveBeenLastCalledWith('promise.search', {
        state: 'resolved',
        tags: { team: 'core' },
        limit: 50,
      }),
    );
    await screen.findByText('p2');
    fireEvent.click(screen.getByText('Load more'));
    await screen.findByText('p3');
    expect(send).toHaveBeenLastCalledWith('promise.search', {
      state: 'resolved',
      tags: { team: 'core' },
      limit: 50,
      cursor: 'c1',
    });
    expect(screen.getByText('p1')).toBeTruthy();
    expect(screen.queryByText('Load more')).toBeNull();
  });

  it('cancel_requires_confirm_and_sends_rejected_canceled', async () => {
    const send = vi.fn(async (kind: string, d: Record<string, any>) =>
      kind === 'promise.settle'
        ? { promise: rec(d.id, { state: d.state }) }
        : { promises: [rec('job-1')] },
    );
    render(<PromisesTab api={createDurableApi(send)} />);
    fireEvent.click(await screen.findByText('job-1'));
    const dialog = await screen.findByRole('dialog');
    fireEvent.click(within(dialog).getByText('Cancel promise'));
    expect(send.mock.calls.some(([k]) => k === 'promise.settle')).toBe(false);
    fireEvent.click(within(dialog).getByText('Confirm cancel'));
    await waitFor(() =>
      expect(send).toHaveBeenCalledWith('promise.settle', {
        id: 'job-1',
        state: 'rejected_canceled',
        value: {},
      }),
    );
    await within(dialog).findByText('rejected_canceled');
  });
});

describe('runs', () => {
  const tags = (origin: string, parent: string) => ({
    'resonate:origin': origin,
    'resonate:branch': origin,
    'resonate:parent': parent,
  });
  it('runs_tree_from_tags', async () => {
    const list = [
      rec('wf', { tags: tags('wf', 'wf'), createdAt: 1 }),
      rec('wf:1', { tags: tags('wf', 'wf'), createdAt: 2 }),
      rec('wf:1:1', { tags: tags('wf', 'wf:1'), createdAt: 3 }),
      rec('wf:2', { tags: tags('wf', 'wf'), createdAt: 4 }),
      rec('lone', { createdAt: 5 }),
      rec('orphan:1', { tags: tags('orphan', 'orphan'), createdAt: 6 }),
    ];
    const runs = buildRuns(list as never);
    const wf = runs.find((r) => r.promise.id === 'wf');
    expect(runs.map((r) => r.promise.id).sort()).toEqual(['orphan:1', 'wf']);
    expect(wf?.children.map((c) => c.promise.id)).toEqual(['wf:1', 'wf:2']);
    expect(wf?.children[0]?.children.map((c) => c.promise.id)).toEqual(['wf:1:1']);

    render(<RunsTab api={createDurableApi(async () => ({ promises: list }))} />);
    await screen.findByText('wf:1');
    expect(screen.getByText('wf:2')).toBeTruthy();
    expect(screen.queryByText('lone')).toBeNull();
  });
});

describe('manifest', () => {
  it('declares a desktop-only plugin', () => {
    expect(() => validateManifest(pkg)).not.toThrow();
    expect(pkg.loams.plugin.editions).toEqual(['desktop']);
  });
});
