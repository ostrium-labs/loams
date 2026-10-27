// The Metrics page over the mock client, with LiveChart stubbed (test/charts-stub.tsx; ECharts
// needs a canvas): dashboards as tabs, panels as charts and stat tiles, and the edit, keyboard
// and save paths.

import type { DashboardSpec } from '@loams-core/types';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vite-plus/test';
import { MetricsPage } from '../src/MetricsPage.js';
import { createMockMetricsClient } from '../src/mock.js';

vi.mock('@loams/charts', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@loams/charts')>()),
  LiveChart: (await import('./charts-stub.js')).LiveChart,
}));

afterEach(cleanup);

// The empty state's grain canvas and react-grid-layout's width provider observe sizes.
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};
// jsdom has no modal <dialog>: open it by attribute.
HTMLDialogElement.prototype.showModal ??= function (this: HTMLDialogElement) {
  this.setAttribute('open', '');
};
HTMLDialogElement.prototype.close ??= function (this: HTMLDialogElement) {
  this.removeAttribute('open');
};
globalThis.CSS ??= { escape: (s: string) => s } as typeof CSS;

const NOW = 1_760_000_000_000;

async function mount(opts: Parameters<typeof createMockMetricsClient>[0] = {}) {
  const client = createMockMetricsClient({ now: () => NOW, ...opts });
  render(<MetricsPage client={client} />);
  await screen.findByRole('tablist', { name: 'Dashboards' });
  return client;
}

const tab = (name: string) => screen.getByRole('tab', { name: new RegExp(`^${name}`) });
const panel = (id: string) => document.querySelector(`[data-panel="${id}"]`) as HTMLElement;

describe('metrics page', () => {
  it('shows the five dashboards as tabs, Usage first, with its panels', async () => {
    await mount();
    const tabs = screen.getAllByRole('tab').map((t) => t.textContent);
    expect(tabs).toEqual(['Usage', 'Postgres', 'Loams SQL', 'Live', 'House']);
    expect(tab('Usage').getAttribute('aria-selected')).toBe('true');
    const chart = await screen.findByRole('figure', {
      name: /Invocations by function \(loams_function_invocations_total\), line chart/,
    });
    expect(chart.getAttribute('data-chart-type')).toBe('Line Chart');
    expect(Number(chart.getAttribute('data-rows'))).toBeGreaterThan(0);
    expect(chart.getAttribute('data-live')).toBe('yes');
    expect(chart.getAttribute('data-window')).toBe(String(60 * 60_000));
    // The p95 duration panel is a stat tile: one value and sparkline per function.
    const stat = panel('function-duration-ms-p95-2');
    await waitFor(() => expect(within(stat).getAllByRole('img')).toHaveLength(3));
    expect(within(stat).getByText('checkout')).toBeTruthy();
  });

  it('arrow keys move between dashboard tabs', async () => {
    await mount();
    fireEvent.keyDown(tab('Usage'), { key: 'ArrowRight' });
    expect(tab('Postgres').getAttribute('aria-selected')).toBe('true');
    expect(document.activeElement).toBe(tab('Postgres'));
    await screen.findByRole('figure', { name: /WAL lag/ });
    fireEvent.keyDown(tab('Postgres'), { key: 'End' });
    expect(tab('House').getAttribute('aria-selected')).toBe('true');
    fireEvent.keyDown(tab('House'), { key: 'ArrowRight' });
    expect(tab('Usage').getAttribute('aria-selected')).toBe('true');
  });

  it('a time range and refresh change restarts the panels and can be saved', async () => {
    const client = await mount();
    fireEvent.change(screen.getByLabelText('Time range'), { target: { value: '6h' } });
    fireEvent.change(screen.getByLabelText('Refresh'), { target: { value: 'off' } });
    const chart = await screen.findByRole('figure', { name: /Invocations by function/ });
    await waitFor(() => expect(chart.getAttribute('data-window')).toBe(String(6 * 3_600_000)));
    expect(
      screen.getByRole('figure', { name: /Invocations by function/ }).getAttribute('data-live'),
    ).toBe('no');
    expect(tab('Usage').textContent).toMatch(/•/);
    fireEvent.click(screen.getByRole('button', { name: 'Save dashboard' }));
    await waitFor(() => expect(client.saves).toHaveLength(1));
    const saved = client.saves[0] as DashboardSpec;
    expect(saved.version).toBe(1);
    expect(saved.params).toEqual(
      expect.arrayContaining([
        { name: 'range', type: 'select', default: '6h' },
        { name: 'refresh', type: 'select', default: 'off' },
      ]),
    );
    expect(screen.queryByRole('button', { name: 'Save dashboard' })).toBeNull();
    await screen.findByText('Usage saved.');
  });

  it('adds a panel with a chosen metric and chart type', async () => {
    const client = await mount();
    fireEvent.click(screen.getByRole('button', { name: 'Edit layout' }));
    expect(screen.getByRole('button', { name: 'Done editing' }).getAttribute('aria-pressed')).toBe(
      'true',
    );
    fireEvent.click(screen.getByRole('button', { name: 'Add panel' }));
    const dialog = screen.getByRole('dialog');
    fireEvent.change(within(dialog).getByLabelText('Metric'), {
      target: { value: 'loams_live_errors_total' },
    });
    fireEvent.click(within(dialog).getByLabelText('Bar'));
    fireEvent.change(within(dialog).getByLabelText('Title'), {
      target: { value: 'Live errors' },
    });
    fireEvent.click(within(dialog).getByRole('button', { name: 'Add panel' }));
    const added = await screen.findByRole('figure', {
      name: /Live errors \(loams_live_errors_total\), bar chart/,
    });
    expect(added.getAttribute('data-chart-type')).toBe('Bar Chart');
    fireEvent.click(screen.getByRole('button', { name: 'Save dashboard' }));
    await waitFor(() => expect(client.saves).toHaveLength(1));
    const saved = client.saves[0] as DashboardSpec;
    const w = Object.values(saved.widgets).find(
      (x) => x.chartSpec?.chartProperties?.metric === 'loams_live_errors_total',
    );
    expect(w).toMatchObject({
      type: 'chart',
      chartSpec: { chartType: 'Bar Chart', title: 'Live errors' },
    });
    expect(saved.layout.some((l) => l.id === w?.id)).toBe(true);
  });

  it('edits a panel into a stat tile', async () => {
    await mount();
    fireEvent.click(screen.getByRole('button', { name: 'Edit layout' }));
    fireEvent.click(screen.getByRole('button', { name: 'Edit Invocations by function' }));
    const dialog = screen.getByRole('dialog');
    fireEvent.click(within(dialog).getByLabelText('Stat'));
    fireEvent.click(within(dialog).getByRole('button', { name: 'Apply' }));
    await waitFor(() =>
      expect(screen.queryByRole('figure', { name: /Invocations by function/ })).toBeNull(),
    );
    const tile = panel('function-invocations-total-1');
    await waitFor(() => expect(within(tile).getAllByRole('img').length).toBeGreaterThan(0));
  });

  it('moves, resizes and removes a focused panel from the keyboard', async () => {
    const client = await mount();
    fireEvent.click(screen.getByRole('button', { name: 'Edit layout' }));
    const p = panel('function-invocations-total-1');
    expect(p.tabIndex).toBe(0);
    act(() => p.focus());
    // At the left edge the panel cannot move; the announcement says where it is.
    fireEvent.keyDown(p, { key: 'ArrowLeft' });
    await screen.findByText(/stays at column 1, row 1/);
    // Backspace is not a delete shortcut: only Delete (named in aria-keyshortcuts) removes.
    fireEvent.keyDown(panel('function-invocations-total-1'), { key: 'Backspace' });
    expect(panel('function-invocations-total-1')).not.toBeNull();
    fireEvent.keyDown(panel('function-invocations-total-1'), { key: 'ArrowRight' });
    await screen.findByText(/moved to column 2, row 1/);
    fireEvent.keyDown(panel('function-invocations-total-1'), { key: 'ArrowDown', shiftKey: true });
    await screen.findByText(/is 8 columns by 4 rows/);
    fireEvent.keyDown(panel('function-invocations-total-1'), { key: 'Delete' });
    await waitFor(() => expect(panel('function-invocations-total-1')).toBeNull());
    await screen.findByText(/Invocations by function removed/);
    fireEvent.click(screen.getByRole('button', { name: 'Save dashboard' }));
    await waitFor(() => expect(client.saves).toHaveLength(1));
    const saved = client.saves[0] as DashboardSpec;
    expect(saved.widgets['function-invocations-total-1']).toBeUndefined();
  });

  it('panels are not focus stops outside edit mode, and keys do nothing there', async () => {
    await mount();
    const p = panel('function-invocations-total-1');
    expect(p.hasAttribute('tabindex')).toBe(false);
    fireEvent.keyDown(p, { key: 'Delete' });
    expect(panel('function-invocations-total-1')).not.toBeNull();
  });

  it('discard drops the draft', async () => {
    await mount();
    fireEvent.change(screen.getByLabelText('Time range'), { target: { value: '5m' } });
    fireEvent.click(screen.getByRole('button', { name: 'Discard' }));
    expect((screen.getByLabelText('Time range') as HTMLSelectElement).value).toBe('1h');
    expect(screen.queryByRole('button', { name: 'Save dashboard' })).toBeNull();
  });

  it('a stale save shows the conflict and offers the saved version', async () => {
    const client = await mount();
    // Someone else saves Usage first.
    const [usage] = await client.listDashboards();
    await client.saveDashboard({ ...(usage as DashboardSpec), title: 'Theirs' }, 0);
    fireEvent.change(screen.getByLabelText('Time range'), { target: { value: '24h' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save dashboard' }));
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toMatch(/Someone saved this dashboard/);
    fireEvent.click(within(alert).getByRole('button', { name: 'Load the saved version' }));
    const theirs = await screen.findByRole('tab', { name: /^Theirs/ });
    // The stale draft is gone: nothing to save, and the range is the saved one.
    expect(theirs.textContent).toBe('Theirs');
    expect(screen.queryByRole('button', { name: 'Save dashboard' })).toBeNull();
  });

  it('a failed load says so and retries', async () => {
    const client = createMockMetricsClient({ now: () => NOW });
    const list = vi
      .spyOn(client, 'listDashboards')
      .mockRejectedValueOnce(new Error('server unavailable'));
    render(<MetricsPage client={client} />);
    expect((await screen.findByRole('alert')).textContent).toMatch(/server unavailable/);
    fireEvent.click(screen.getByRole('button', { name: 'Retry' }));
    await screen.findByRole('tablist', { name: 'Dashboards' });
    expect(list).toHaveBeenCalledTimes(2);
  });
});
