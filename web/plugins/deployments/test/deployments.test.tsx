// The Deployments plugin: the pure rules (model, layout, keyboard order), the mock
// client's stream, and the page against the mock. @loams/charts is replaced with
// stand-ins here: ECharts does not draw in jsdom, and the charts package has its own
// tests; this file checks what the page hands it.

import { validateManifest } from '@loams/console-host';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vite-plus/test';
import pkg from '../package.json';
import {
  DeploymentsPage,
  FAILURE_SPEC,
  FREQUENCY_SPEC,
  TIMELINE_WINDOW_MS,
} from '../src/DeploymentsPage.js';
import { deploymentGraph, PROMOTION_LABEL, promotionGraph } from '../src/layout.js';
import { createMockDeployClient, fixtureDeployments, fixturePromotions } from '../src/mock.js';
import {
  applyEvent,
  canRollback,
  deriveState,
  formatMs,
  stageDuration,
  summariseSecrets,
} from '../src/model.js';
import { nextStage } from '../src/PipelineCanvas.js';
import type { DeployEvent, Deployment, Stage } from '../src/types.js';

const chartProps: Record<string, unknown>[] = [];

vi.mock('@loams/charts', async () => {
  const { createElement } = await import('react');
  return {
    LiveChart: (p: Record<string, unknown>) => {
      chartProps.push(p);
      return createElement('figure', { 'aria-label': p.ariaLabel as string });
    },
    Sparkline: (p: { ariaLabel: string }) => createElement('span', { 'aria-label': p.ariaLabel }),
    useMetricStream: () => [{ t: 1, v: 7, series: 'deploys/h' }],
  };
});

// React Flow measures its viewport; jsdom has neither of these.
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};
(globalThis as { DOMMatrixReadOnly?: unknown }).DOMMatrixReadOnly ??= class {
  m22 = 1;
};
HTMLDialogElement.prototype.showModal ??= function (this: HTMLDialogElement) {
  this.setAttribute('open', '');
};
HTMLDialogElement.prototype.close ??= function (this: HTMLDialogElement) {
  this.removeAttribute('open');
};

function setReducedMotion(reduce: boolean) {
  window.matchMedia = ((query: string) => ({
    matches: reduce && query.includes('reduce'),
    media: query,
    addEventListener() {},
    removeEventListener() {},
  })) as unknown as typeof window.matchMedia;
}

const NOW = 1_800_000_000_000;

function firstPromotion() {
  const first = fixturePromotions(NOW)[0];
  if (!first) throw new Error('the fixtures have a promotion');
  return first;
}

beforeEach(() => {
  chartProps.length = 0;
  setReducedMotion(false);
});
afterEach(cleanup);

// ---- manifest ------------------------------------------------------------------------------

describe('manifest', () => {
  it('is a valid first-party console plugin for every edition', () => {
    const m = validateManifest(pkg);
    expect(m.inject).toEqual(['router', 'slots']);
    expect(pkg.loams.plugin.editions).toEqual(['oss', 'desktop', 'cloud']);
    expect(pkg.dependencies['@xyflow/react']).toBeDefined();
    expect(pkg.dependencies['@loams/charts']).toBe('workspace:*');
  });
});

// ---- model ---------------------------------------------------------------------------------

const stage = (kind: Stage['kind'], state: Stage['state'], extra: Partial<Stage> = {}): Stage => ({
  kind,
  state,
  progress: state === 'succeeded' ? 1 : 0,
  ...extra,
});

describe('model', () => {
  it('derives a deployment state from its stages', () => {
    const ok = (['build', 'upload', 'validate', 'activate'] as const).map((k) =>
      stage(k, 'succeeded'),
    );
    expect(deriveState([...ok, stage('rollback', 'pending')])).toBe('succeeded');
    expect(
      deriveState([
        stage('build', 'succeeded'),
        stage('upload', 'running'),
        stage('rollback', 'pending'),
      ]),
    ).toBe('running');
    expect(
      deriveState([
        stage('validate', 'failed'),
        stage('activate', 'skipped'),
        stage('rollback', 'pending'),
      ]),
    ).toBe('failed');
    expect(deriveState([...ok, stage('rollback', 'running')])).toBe('rolling_back');
    expect(deriveState([...ok, stage('rollback', 'succeeded')])).toBe('rolled_back');
  });

  it('applies snapshot, deployment and stage events, newest first, without mutating', () => {
    const [a, b] = fixtureDeployments(NOW) as [Deployment, Deployment];
    let list = applyEvent([], { kind: 'snapshot', deployments: [b, a] });
    expect(list.map((d) => d.id)).toEqual([a.id, b.id]);
    const fresh: Deployment = { ...a, id: 'dep-new', createdAt: NOW + 1 };
    list = applyEvent(list, { kind: 'deployment', deployment: fresh });
    expect(list[0]?.id).toBe('dep-new');
    const before = list;
    const ev: DeployEvent = {
      kind: 'stage',
      deploymentId: a.id,
      stage: stage('upload', 'running', { progress: 0.9 }),
      state: 'running',
    };
    list = applyEvent(list, ev);
    expect(list.find((d) => d.id === a.id)?.stages.find((s) => s.kind === 'upload')?.progress).toBe(
      0.9,
    );
    expect(
      before.find((d) => d.id === a.id)?.stages.find((s) => s.kind === 'upload')?.progress,
    ).toBe(0.4);
  });

  it('allows a rollback only with a previous version and nothing in flight', () => {
    const [running, live, failed] = fixtureDeployments(NOW) as [Deployment, Deployment, Deployment];
    expect(canRollback(running)).toBe(false);
    expect(canRollback(live)).toBe(true);
    expect(canRollback(failed)).toBe(true);
    expect(canRollback({ ...live, previousVersion: undefined })).toBe(false);
    expect(canRollback({ ...live, state: 'rolled_back' })).toBe(false);
  });

  it('formats durations and running time', () => {
    expect(formatMs(850)).toBe('850 ms');
    expect(formatMs(12_400)).toBe('12.4 s');
    expect(formatMs(185_000)).toBe('3 min 05 s');
    expect(stageDuration(stage('build', 'running', { startedAt: NOW - 5_000 }), NOW)).toBe(5_000);
    expect(stageDuration(stage('build', 'pending'), NOW)).toBeUndefined();
  });

  it('summarises secret events by kind', () => {
    const { promotion } = firstPromotion();
    expect(summariseSecrets(promotion.secretEvents)).toBe('1 promoted, 1 added');
  });
});

// ---- layout --------------------------------------------------------------------------------

describe('layout', () => {
  it('draws the four stages left to right, as stage nodes, rollback hidden until it runs', () => {
    const [running] = fixtureDeployments(NOW) as [Deployment];
    const g = deploymentGraph(running, NOW, false);
    expect(g.nodes.map((n) => n.id)).toEqual(['build', 'upload', 'validate', 'activate']);
    expect(g.nodes.every((n) => n.type === 'stage')).toBe(true);
    const xs = g.nodes.map((n) => n.position.x);
    expect([...xs].sort((a, b) => a - b)).toEqual(xs);
    expect(new Set(xs).size).toBe(4);
    expect(g.edges.map((e) => e.id)).toEqual([
      'build->upload',
      'upload->validate',
      'validate->activate',
    ]);
    // The running stage's incoming edge animates; with reduced motion nothing does.
    expect(g.edges.find((e) => e.target === 'upload')?.animated).toBe(true);
    expect(deploymentGraph(running, NOW, true).edges.some((e) => e.animated)).toBe(false);
    expect(g.nodes.find((n) => n.id === 'upload')?.ariaLabel).toMatch(/^Upload: Running, /);
  });

  it('adds the rollback branch below activate once it has started', () => {
    const [, live] = fixtureDeployments(NOW) as [Deployment, Deployment];
    const rolling: Deployment = {
      ...live,
      stages: live.stages.map((s) =>
        s.kind === 'rollback' ? { ...s, state: 'running', startedAt: NOW, progress: 0.2 } : s,
      ),
    };
    const g = deploymentGraph(rolling, NOW, false);
    const rb = g.nodes.find((n) => n.id === 'rollback');
    const act = g.nodes.find((n) => n.id === 'activate');
    expect(rb?.data.branch).toBe(true);
    expect(rb?.position.x).toBe(act?.position.x);
    expect(rb?.position.y).toBeGreaterThan(act?.position.y ?? 0);
    expect(g.edges.find((e) => e.id === 'activate->rollback')?.style).toMatchObject({
      strokeDasharray: '6 4',
    });
  });

  it('draws the promotion path with key events on the promote steps, names only', () => {
    const { promotion } = firstPromotion();
    const g = promotionGraph(promotion, false);
    expect(g.nodes.map((n) => n.data.label)).toEqual(Object.values(PROMOTION_LABEL));
    const dev = g.nodes.find((n) => n.id === 'promote_dev');
    expect(dev?.data.secrets?.map((e) => `${e.key} ${e.kind}`)).toEqual([
      'STRIPE_WEBHOOK_SECRET promoted',
      'FEATURE_LEDGER_V2 added',
    ]);
    for (const e of promotion.secretEvents) {
      expect(Object.keys(e).sort()).toEqual(['at', 'env', 'id', 'key', 'kind', 'step']);
    }
    expect(g.nodes.find((n) => n.id === 'merge_main')?.data.state).toBe('running');
  });

  it('moves between stages in pipeline order', () => {
    const order = ['build', 'upload', 'validate', 'activate'];
    expect(nextStage(order, 'build', 'ArrowRight')).toBe('upload');
    expect(nextStage(order, 'activate', 'ArrowRight')).toBe('activate');
    expect(nextStage(order, 'upload', 'ArrowLeft')).toBe('build');
    expect(nextStage(order, 'validate', 'Home')).toBe('build');
    expect(nextStage(order, 'build', 'End')).toBe('activate');
    expect(nextStage(order, 'build', 'Enter')).toBeUndefined();
  });
});

// ---- the mock client -----------------------------------------------------------------------

describe('mock client', () => {
  it('streams a snapshot, then stage updates as the simulation ticks', async () => {
    const client = createMockDeployClient({ intervalMs: 60_000, now: () => NOW });
    const ac = new AbortController();
    const iter = client.watchDeployments(ac.signal)[Symbol.asyncIterator]();
    const first = await iter.next();
    expect(first.value).toMatchObject({ kind: 'snapshot' });
    client.tick();
    const next = (await iter.next()).value as DeployEvent;
    expect(next).toMatchObject({ kind: 'stage', deploymentId: 'dep-1042' });
    ac.abort();
    expect((await iter.next()).done).toBe(true);
  });

  it('runs a rollback to completion', async () => {
    const client = createMockDeployClient({ intervalMs: 60_000, now: () => NOW, seed: 3 });
    const ac = new AbortController();
    const events: DeployEvent[] = [];
    const done = (async () => {
      for await (const e of client.watchDeployments(ac.signal)) events.push(e);
    })();
    await Promise.resolve();
    await client.rollback('dep-1041');
    for (let i = 0; i < 12; i++) client.tick();
    await new Promise((r) => setTimeout(r, 0));
    ac.abort();
    await done;
    const states = events.flatMap((e) =>
      e.kind === 'stage' && e.deploymentId === 'dep-1041' ? [e.state] : [],
    );
    expect(states[0]).toBe('rolling_back');
    expect(states.at(-1)).toBe('rolled_back');
    await expect(client.rollback('dep-1041')).rejects.toThrow(/already rolling back/);
  });

  it('refuses a rollback with nowhere to go', async () => {
    const client = createMockDeployClient({ now: () => NOW });
    await expect(client.rollback('dep-nope')).rejects.toThrow(/No deployment/);
  });

  it('emits metric points on its interval and history over the window', async () => {
    vi.useFakeTimers();
    try {
      const client = createMockDeployClient({ intervalMs: 500, now: () => NOW });
      const iter = client.metricStream('failure_rate')[Symbol.asyncIterator]();
      const p = iter.next();
      await vi.advanceTimersByTimeAsync(500);
      const { value } = await p;
      expect(value).toEqual([{ t: NOW, v: expect.any(Number), series: 'failure %' }]);
      await iter.return?.();
      const hist = await client.metricHistory('frequency', 10 * 60_000);
      expect(hist).toHaveLength(11);
      expect(hist[0]?.t).toBe(NOW - 10 * 60_000);
    } finally {
      vi.useRealTimers();
    }
  });
});

// ---- the page ------------------------------------------------------------------------------

describe('DeploymentsPage', () => {
  it('shows each deployment as a pipeline with its state and stage nodes', async () => {
    const client = createMockDeployClient({ intervalMs: 60_000 });
    render(<DeploymentsPage client={client} />);
    expect(await screen.findByRole('heading', { name: 'api v1.42.0' })).toBeTruthy();
    expect(screen.getByRole('region', { name: 'api v1.42.0 pipeline' })).toBeTruthy();
    // The stage graph's nodes mount a tick after the heading.
    expect((await screen.findAllByText('Validate')).length).toBeGreaterThanOrEqual(3);
    expect(screen.getByRole('heading', { name: 'billing-worker v0.19.0' })).toBeTruthy();
    expect(screen.getByLabelText('Deploys per hour, last 15 minutes')).toBeTruthy();
  });

  it('marks nodes for reduced motion', async () => {
    setReducedMotion(true);
    const client = createMockDeployClient({ intervalMs: 60_000 });
    const { container } = render(<DeploymentsPage client={client} />);
    await screen.findByRole('heading', { name: 'api v1.42.0' });
    // The stage graph's nodes mount a tick after the heading: wait for them, not the heading.
    await waitFor(() => expect(container.querySelectorAll('.ld-node').length).toBeGreaterThan(0));
    const nodes = container.querySelectorAll('.ld-node');
    for (const n of nodes) expect(n.getAttribute('data-motion')).toBe('reduce');
  });

  it('updates a stage live when the client streams a change', async () => {
    const client = createMockDeployClient({ intervalMs: 60_000 });
    const { container } = render(<DeploymentsPage client={client} />);
    await screen.findByRole('heading', { name: 'api v1.42.0' });
    const bar = () =>
      container.querySelector('[aria-label="Upload progress"]')?.getAttribute('aria-valuenow');
    // The stage graph's nodes mount a tick after the heading.
    await waitFor(() => expect(bar()).toBe('40'));
    act(() => client.tick());
    await waitFor(() => expect(Number(bar())).toBeGreaterThan(40));
  });

  it('confirms and starts a rollback', async () => {
    const client = createMockDeployClient({ intervalMs: 60_000 });
    const spy = vi.spyOn(client, 'rollback');
    render(<DeploymentsPage client={client} />);
    fireEvent.click(await screen.findByRole('button', { name: 'Roll back to v3.8.0' }));
    const dialog = await screen.findByRole('dialog', { name: 'Roll back web?' });
    fireEvent.click(within(dialog).getByRole('button', { name: 'Roll back' }));
    await waitFor(() => expect(spy).toHaveBeenCalledWith('dep-1041'));
    await waitFor(() => expect(screen.getAllByText('Rolling back').length).toBeGreaterThan(0));
  });

  it('sends one rollback request while the first is in flight', async () => {
    const client = createMockDeployClient({ intervalMs: 60_000 });
    let finish: () => void = () => {};
    const spy = vi.spyOn(client, 'rollback').mockImplementation(
      () =>
        new Promise<void>((resolve) => {
          finish = resolve;
        }),
    );
    render(<DeploymentsPage client={client} />);
    fireEvent.click(await screen.findByRole('button', { name: 'Roll back to v3.8.0' }));
    const dialog = await screen.findByRole('dialog', { name: 'Roll back web?' });
    fireEvent.click(within(dialog).getByRole('button', { name: 'Roll back' }));
    const busy = await within(dialog).findByRole('button', { name: 'Rolling back…' });
    expect((busy as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(busy);
    expect(spy).toHaveBeenCalledTimes(1);
    await act(async () => finish());
    expect(within(dialog).queryByRole('alert')).toBeNull();
  });

  it('opens a stage log from its node', async () => {
    const client = createMockDeployClient({ intervalMs: 60_000 });
    render(<DeploymentsPage client={client} />);
    await screen.findByRole('heading', { name: 'billing-worker v0.19.0' });
    const buttons = await screen.findAllByLabelText('Validate logs');
    fireEvent.click(buttons[buttons.length - 1] as HTMLElement);
    expect(
      await screen.findByText(/Health check \/ready returned 503/, { selector: 'li' }),
    ).toBeTruthy();
  });

  it('switches views with the arrow keys and shows the promotion path', async () => {
    const client = createMockDeployClient({ intervalMs: 60_000 });
    render(<DeploymentsPage client={client} />);
    const live = screen.getByRole('tab', { name: 'Live deployments' });
    live.focus();
    fireEvent.keyDown(live, { key: 'ArrowRight' });
    const promo = screen.getByRole('tab', { name: 'Environment promotion' });
    expect(promo.getAttribute('aria-selected')).toBe('true');
    expect(document.activeElement).toBe(promo);
    expect(await screen.findByRole('heading', { name: 'Ledger v2 behind a flag' })).toBeTruthy();
    expect(screen.getByRole('region', { name: 'Ledger v2 behind a flag promotion' })).toBeTruthy();
    const table = screen.getByRole('table', {
      name: 'Env and secret changes for Rotate the search key, drop the legacy token (key names only)',
    });
    expect(within(table).getAllByText('SEARCH_API_KEY')).toHaveLength(2);
  });

  it('hands the timeline charts their specs, history and a live stream', async () => {
    const client = createMockDeployClient({ intervalMs: 60_000 });
    render(<DeploymentsPage client={client} />);
    fireEvent.click(screen.getByRole('tab', { name: 'Timeline' }));
    expect(await screen.findByLabelText('Deployments per hour over the last hour')).toBeTruthy();
    expect(
      screen.getByLabelText('Percent of deployments that failed over the last hour'),
    ).toBeTruthy();
    const freq = chartProps.find((p) => p.spec === FREQUENCY_SPEC);
    const fail = chartProps.find((p) => p.spec === FAILURE_SPEC);
    if (!freq || !fail) throw new Error('both timeline charts render');
    expect(freq.windowMs).toBe(TIMELINE_WINDOW_MS);
    expect((freq.data as unknown[]).length).toBe(61);
    expect(typeof (fail.stream as AsyncIterable<unknown>)[Symbol.asyncIterator]).toBe('function');
  });
});
