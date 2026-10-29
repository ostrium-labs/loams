// The canvas page against the seeded mock client: nodes with live status, the details
// panel and its deep link, the add-service palette, keyboard navigation, persisted
// positions and the plugin manifest.

import { validateManifest } from '@loams/console-host';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeAll, describe, expect, it, vi } from 'vite-plus/test';
import pkg from '../package.json';
import { CanvasPage, initialPositions, toFlowEdges } from '../src/CanvasPage.js';
import type { CanvasClient } from '../src/client.js';
import { serviceHref } from '../src/kinds.js';
import { createMockCanvasClient, SEED_PROJECT_ID, seedProject } from '../src/mock.js';
import { relation } from '../src/SidePanel.js';

// @loams/charts draws with ECharts; the page only needs its contract here.
vi.mock('@loams/charts', () => ({
  Sparkline: ({ ariaLabel, points }: { ariaLabel: string; points: readonly unknown[] }) => (
    <svg role="img" aria-label={ariaLabel} data-points={points.length} />
  ),
  LiveChart: ({ ariaLabel, data }: { ariaLabel: string; data: readonly unknown[] }) => (
    <figure aria-label={ariaLabel} data-rows={data.length} />
  ),
  useMetricStream: () => [
    { t: 1, v: 10 },
    { t: 2, v: 12.5 },
  ],
}));

beforeAll(() => {
  // What React Flow needs from a browser that jsdom lacks.
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver;
  (globalThis as { DOMMatrixReadOnly?: unknown }).DOMMatrixReadOnly ??= class {
    m22: number;
    constructor(transform?: string) {
      const scale = transform?.match(/scale\(([0-9.]+)\)/)?.[1];
      this.m22 = scale ? Number(scale) : 1;
    }
  };
  Object.defineProperties(HTMLElement.prototype, {
    offsetWidth: { configurable: true, get: () => 800 },
    offsetHeight: { configurable: true, get: () => 600 },
  });
  (SVGElement.prototype as unknown as { getBBox: () => DOMRect }).getBBox = () =>
    ({ x: 0, y: 0, width: 0, height: 0 }) as DOMRect;
  // jsdom has no modal <dialog>: open it by attribute.
  HTMLDialogElement.prototype.showModal ??= function (this: HTMLDialogElement) {
    this.setAttribute('open', '');
  };
  HTMLDialogElement.prototype.close ??= function (this: HTMLDialogElement) {
    this.removeAttribute('open');
  };
  globalThis.requestAnimationFrame ??= (cb: FrameRequestCallback) =>
    setTimeout(() => cb(0), 0) as unknown as number;
});

afterEach(cleanup);

/** A mock client whose status stream stays quiet unless a test drives it. */
function quietClient(over: Partial<CanvasClient> = {}) {
  const base = createMockCanvasClient({ intervalMs: 60_000 });
  return Object.assign(base, over);
}

function mount(client: CanvasClient = quietClient()) {
  const navigate = vi.fn();
  const view = render(
    <CanvasPage client={client} projectId={SEED_PROJECT_ID} navigate={navigate} />,
  );
  return { client, navigate, view };
}

/** React Flow lays nodes out a tick after the page title renders: wait for all of them. */
async function allNodes() {
  const n = seedProject().services.length;
  await waitFor(() =>
    expect(document.querySelectorAll('.react-flow__node').length).toBeGreaterThanOrEqual(n),
  );
}

/** The React Flow node wrapper for a service id. */
function nodeEl(container: HTMLElement, id: string): HTMLElement {
  const el = [...container.querySelectorAll<HTMLElement>('.react-flow__node')].find(
    (n) => n.dataset.id === id,
  );
  if (!el) throw new Error(`no node ${id}`);
  return el;
}

describe('CanvasPage', () => {
  it('draws every seeded service with its status and a sparkline', async () => {
    const { view } = mount();
    await screen.findByText('Storefront');
    await allNodes();
    for (const s of seedProject().services) {
      const el = nodeEl(view.container, s.id);
      expect(el.getAttribute('aria-label')).toContain(s.name);
    }
    const api = nodeEl(view.container, 'live-api');
    expect(api.getAttribute('aria-label')).toBe('Live deployment api, Deploying');
    expect(within(api).getByText('Deploying')).toBeTruthy();
    expect(within(api).getByLabelText('Requests for api, last two minutes')).toBeTruthy();
    expect(within(api).getByText('12.5/s')).toBeTruthy();
  });

  it('shows a load failure with a retry', async () => {
    let fail = true;
    const client = quietClient();
    const real = client.getProject.bind(client);
    client.getProject = (id, signal) =>
      fail ? Promise.reject(new Error('project service down')) : real(id, signal);
    mount(client);
    expect(await screen.findByText('project service down')).toBeTruthy();
    fail = false;
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }));
    await screen.findByText('Storefront');
    await allNodes();
  });

  it('opens the details panel on click and deep links to the product page', async () => {
    const { view, navigate } = mount();
    await screen.findByText('Storefront');
    await allNodes();
    // React Flow mounts its nodes a tick after the page renders.
    const node = await waitFor(() => nodeEl(view.container, 'pg-main'));
    fireEvent.click(within(node).getByText('main'));
    const panel = await screen.findByRole('complementary', { name: 'main' });
    expect(within(panel).getByText('2 CU')).toBeTruthy();
    expect(within(panel).getByLabelText(/^CPU \(%\) for main/)).toBeTruthy();
    // Live -> Postgres: the api depends on main.
    expect(within(panel).getByText(/needed by/)).toBeTruthy();
    fireEvent.click(within(panel).getByRole('button', { name: 'Open in Postgres' }));
    expect(navigate).toHaveBeenCalledWith('/postgres');
  });

  it('moves the panel along a connection and closes it with Escape', async () => {
    const { view } = mount();
    await screen.findByText('Storefront');
    await allNodes();
    fireEvent.click(within(nodeEl(view.container, 'conn-orders-cdc')).getByText('orders-cdc'));
    const panel = await screen.findByRole('complementary', { name: 'orders-cdc' });
    fireEvent.click(within(panel).getByRole('button', { name: 'Go to House analytics' }));
    const house = await screen.findByRole('complementary', { name: 'analytics' });
    fireEvent.keyDown(house, { key: 'Escape' });
    await waitFor(() => expect(screen.queryByRole('complementary')).toBeNull());
  });

  it('navigates between nodes with the arrow keys and opens one with Enter', async () => {
    const { view } = mount();
    await screen.findByText('Storefront');
    await allNodes();
    const repo = nodeEl(view.container, 'repo-storefront');
    repo.focus();
    fireEvent.keyDown(repo, { key: 'ArrowRight' });
    const focused = document.activeElement as HTMLElement;
    expect(focused.classList.contains('react-flow__node')).toBe(true);
    // The repo builds the web deployment, which sits one column to its right.
    expect(focused.dataset.id).toBe('live-web');
    fireEvent.keyDown(focused, { key: 'Enter' });
    expect(await screen.findByRole('complementary')).toBeTruthy();
  });

  it('nudges a node with Shift+arrow and persists its position', async () => {
    const client = quietClient();
    const save = vi.spyOn(client, 'savePositions');
    const { view } = mount(client);
    await screen.findByText('Storefront');
    await allNodes();
    const repo = nodeEl(view.container, 'repo-storefront');
    repo.focus();
    fireEvent.keyDown(repo, { key: 'ArrowDown', shiftKey: true });
    await waitFor(() => expect(save).toHaveBeenCalled());
    const [projectId, positions] = save.mock.calls.at(-1) ?? [];
    expect(projectId).toBe(SEED_PROJECT_ID);
    const laid = initialPositions(seedProject());
    expect(positions).toEqual({
      'repo-storefront': {
        x: laid['repo-storefront']?.x,
        y: (laid['repo-storefront']?.y ?? 0) + 24,
      },
    });
  });

  it('saves the whole layout on Auto-layout', async () => {
    const client = quietClient();
    const save = vi.spyOn(client, 'savePositions');
    mount(client);
    await screen.findByText('Storefront');
    await allNodes();
    fireEvent.click(screen.getByRole('button', { name: 'Auto-layout' }));
    await waitFor(() => expect(save).toHaveBeenCalled());
    expect(Object.keys(save.mock.calls.at(-1)?.[1] ?? {}).sort()).toEqual(
      seedProject()
        .services.map((s) => s.id)
        .sort(),
    );
  });

  it('adds a service from the palette through the client', async () => {
    const client = quietClient();
    const create = vi.spyOn(client, 'createService');
    const { view } = mount(client);
    await screen.findByText('Storefront');
    await allNodes();
    fireEvent.click(screen.getByRole('button', { name: 'Add service' }));
    const dialog = await screen.findByRole('dialog', { name: 'Add a service' });
    fireEvent.change(within(dialog).getByLabelText('Find a kind'), {
      target: { value: 'connector' },
    });
    fireEvent.click(within(dialog).getByLabelText(/^Connector/));
    fireEvent.change(within(dialog).getByLabelText('Name'), { target: { value: 'pg-to-house' } });
    fireEvent.change(within(dialog).getByLabelText('Connect from (optional)'), {
      target: { value: 'pg-main' },
    });
    fireEvent.click(within(dialog).getByRole('button', { name: 'Add service' }));
    await waitFor(() =>
      expect(create).toHaveBeenCalledWith(SEED_PROJECT_ID, {
        kind: 'connector',
        name: 'pg-to-house',
        connectFrom: 'pg-main',
      }),
    );
    const panel = await screen.findByRole('complementary', { name: 'pg-to-house' });
    expect(within(panel).getByText('Deploying')).toBeTruthy();
    const id = client.peek().services.at(-1)?.id as string;
    expect(nodeEl(view.container, id)).toBeTruthy();
    expect(client.peek().positions[id]).toBeDefined();
    expect(screen.getByText(/^Added Connector pg-to-house/)).toBeTruthy();
  });

  it('shows why the client refused a new service', async () => {
    mount();
    await screen.findByText('Storefront');
    await allNodes();
    fireEvent.click(screen.getByRole('button', { name: 'Add service' }));
    const dialog = await screen.findByRole('dialog', { name: 'Add a service' });
    fireEvent.click(within(dialog).getByLabelText(/^Postgres branch/));
    fireEvent.change(within(dialog).getByLabelText('Name'), { target: { value: 'main' } });
    fireEvent.click(within(dialog).getByRole('button', { name: 'Add service' }));
    expect(await within(dialog).findByText(/already exists/)).toBeTruthy();
  });

  it('keeps the palette open on Escape while a service is being added', async () => {
    const client = quietClient();
    let finish: (() => void) | undefined;
    vi.spyOn(client, 'createService').mockImplementation(
      () =>
        new Promise((_resolve, reject) => {
          finish = () => reject(new Error('stopped'));
        }),
    );
    mount(client);
    await screen.findByText('Storefront');
    await allNodes();
    fireEvent.click(screen.getByRole('button', { name: 'Add service' }));
    const dialog = await screen.findByRole('dialog', { name: 'Add a service' });
    fireEvent.click(within(dialog).getByLabelText(/^Postgres branch/));
    fireEvent.change(within(dialog).getByLabelText('Name'), { target: { value: 'pg-new' } });
    fireEvent.click(within(dialog).getByRole('button', { name: 'Add service' }));
    await within(dialog).findByRole('button', { name: 'Adding…' });
    fireEvent.keyDown(dialog, { key: 'Escape' });
    expect(screen.getByRole('dialog', { name: 'Add a service' })).toBeTruthy();
    expect(
      (within(dialog).getByRole('button', { name: 'Close' }) as HTMLButtonElement).disabled,
    ).toBe(true);
    finish?.();
    expect(await within(dialog).findByText('stopped')).toBeTruthy();
    fireEvent.keyDown(dialog, { key: 'Escape' });
    await waitFor(() => expect(screen.queryByRole('dialog', { name: 'Add a service' })).toBeNull());
  });

  it('validates the name before calling the client', async () => {
    const client = quietClient();
    const create = vi.spyOn(client, 'createService');
    mount(client);
    await screen.findByText('Storefront');
    await allNodes();
    fireEvent.click(screen.getByRole('button', { name: 'Add service' }));
    const dialog = await screen.findByRole('dialog', { name: 'Add a service' });
    fireEvent.click(within(dialog).getByRole('button', { name: 'Add service' }));
    expect(await within(dialog).findByText('Give the service a name.')).toBeTruthy();
    expect(create).not.toHaveBeenCalled();
  });

  it('updates a badge when the status stream reports a change', async () => {
    let push: ((b: { serviceId: string; status: 'failed' }[]) => void) | undefined;
    const client = quietClient({
      async *watchStatus(_p: string, signal: AbortSignal) {
        while (!signal.aborted) {
          yield await new Promise<{ serviceId: string; status: 'failed' }[]>((r) => {
            push = r;
          });
        }
      },
    });
    const { view } = mount(client);
    await screen.findByText('Storefront');
    await allNodes();
    await waitFor(() => expect(push).toBeDefined());
    await act(async () => push?.([{ serviceId: 'pg-main', status: 'failed' }]));
    await waitFor(() =>
      expect(nodeEl(view.container, 'pg-main').getAttribute('aria-label')).toBe(
        'Postgres branch main, Failed',
      ),
    );
  });
});

describe('helpers', () => {
  it('starts from saved positions and places the rest below them', () => {
    const p = { ...seedProject(), positions: { 'pg-main': { x: 5, y: 7 } } };
    const pos = initialPositions(p);
    expect(pos['pg-main']).toEqual({ x: 5, y: 7 });
    const others = Object.entries(pos).filter(([id]) => id !== 'pg-main');
    expect(new Set(others.map(([, v]) => v.y)).size).toBe(others.length);
  });

  it('animates data flows only without reduced motion, and never makes edges tab stops', () => {
    const edges = seedProject().edges;
    const moving = toFlowEdges(edges, false);
    expect(moving.some((e) => e.animated)).toBe(true);
    expect(toFlowEdges(edges, true).some((e) => e.animated)).toBe(false);
    expect(moving.every((e) => e.focusable === false)).toBe(true);
    expect(moving.filter((e) => e.className === 'lc-edge-dep').every((e) => !e.animated)).toBe(
      true,
    );
  });

  it('deep links every kind, a connector to its own page', () => {
    const repo = seedProject().services[0];
    if (!repo) throw new Error('the seed project has services');
    expect(serviceHref({ ...repo, kind: 'connector', id: 'c 1' })).toBe('/connectors/c%201');
    expect(serviceHref({ ...repo, kind: 'graph', href: '/graph?g=x' })).toBe('/graph?g=x');
  });

  it('reads a connection from either end', () => {
    const e = {
      id: 'x',
      source: 'a',
      target: 'b',
      kind: 'data-flow' as const,
      label: 'streams to',
    };
    expect(relation(e, 'out')).toBe('streams to');
    expect(relation(e, 'in')).toBe('streams to this');
    expect(relation({ ...e, kind: 'depends-on' }, 'out')).toBe('depends on');
  });
});

describe('manifest', () => {
  it('is a valid desktop console plugin manifest', () => {
    const m = validateManifest(pkg);
    expect(m.editions).toEqual(['desktop']);
  });
});
