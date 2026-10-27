// The Pipelines page against the mock client: build from the palette, wire nodes from
// the keyboard path (the side panel's Connect), see a schema mismatch, set a write-only
// secret, deploy, and get a throughput and a lag chart per node. @loams/charts is
// stubbed: its own package tests the charts; here only what the page hands them matters.

import { validateManifest } from '@loams/console-host';
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeAll, describe, expect, it, vi } from 'vite-plus/test';
import pkg from '../package.json';
import { createMockPipelinesClient, type MockOptions } from '../src/mock.js';
import { metricSpec } from '../src/NodeMetrics.js';
import { PipelinesPage } from '../src/PipelinesPage.js';

const charts = vi.hoisted(() => ({ liveSpecs: [] as unknown[] }));

vi.mock('@loams/charts', () => ({
  LiveChart: (p: { ariaLabel: string; spec: unknown }) => {
    charts.liveSpecs.push(p.spec);
    return <div role="img" aria-label={p.ariaLabel} />;
  },
  Sparkline: (p: { ariaLabel: string }) => <div role="img" aria-label={p.ariaLabel} />,
  useMetricStream: () => [],
}));

beforeAll(() => {
  // React Flow measures; jsdom does not.
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver;
  (globalThis as { DOMMatrixReadOnly?: unknown }).DOMMatrixReadOnly ??= class {
    m22 = 1;
  };
  window.matchMedia ??= ((q: string) => ({
    matches: q.includes('reduce'),
    media: q,
    onchange: null,
    addListener() {},
    removeListener() {},
    addEventListener() {},
    removeEventListener() {},
    dispatchEvent: () => false,
  })) as typeof window.matchMedia;
  HTMLDialogElement.prototype.showModal ??= function (this: HTMLDialogElement) {
    this.setAttribute('open', '');
  };
});

afterEach(() => {
  cleanup();
  charts.liveSpecs.length = 0;
});

async function mount(opts: MockOptions = {}) {
  const client = createMockPipelinesClient({ deployMs: 0, intervalMs: 60_000, ...opts });
  const view = render(<PipelinesPage client={client} />);
  await screen.findByRole('navigation', { name: 'Node palette' });
  return { client, ...view };
}

const palette = () => within(screen.getByRole('navigation', { name: 'Node palette' }));
const outline = () => within(screen.getByRole('navigation', { name: 'Pipeline outline' }));
const panel = (label: string) =>
  within(screen.getByRole('complementary', { name: `Configure ${label}` }));

function connectFromPanel(from: string, to: string) {
  fireEvent.click(outline().getByRole('button', { name: new RegExp(`^${from} `) }));
  const p = panel(from);
  fireEvent.change(p.getByLabelText('Connect output to'), {
    target: { value: p.getByRole('option', { name: to }).getAttribute('value') },
  });
  fireEvent.click(p.getByRole('button', { name: 'Connect' }));
}

describe('pipelines page', () => {
  it('manifest is a desktop console plugin on router and slots', () => {
    const m = validateManifest(pkg);
    expect(m.inject).toEqual(['router', 'slots']);
  });

  it('palette lists sources and sinks by family, and transforms', async () => {
    await mount();
    const sources = within(screen.getByRole('region', { name: 'Sources' }));
    expect(sources.getByRole('heading', { name: 'Kafka protocol' })).toBeTruthy();
    expect(sources.getByRole('button', { name: 'Add Postgres CDC source' })).toBeTruthy();
    // Postgres CDC is source-only; the Postgres sink is sink-only.
    expect(sources.queryByRole('button', { name: 'Add Postgres source' })).toBeNull();
    const sinks = within(screen.getByRole('region', { name: 'Sinks' }));
    expect(sinks.getByRole('button', { name: 'Add Postgres sink' })).toBeTruthy();
    expect(palette().getByRole('button', { name: 'Add Schema mapping transform' })).toBeTruthy();
  });

  it('filters the palette', async () => {
    await mount();
    fireEvent.change(palette().getByLabelText('Filter connectors'), { target: { value: 'click' } });
    expect(palette().getByRole('button', { name: 'Add ClickHouse sink' })).toBeTruthy();
    expect(palette().queryByRole('button', { name: 'Add Kafka source' })).toBeNull();
  });

  it('a mismatched connection is an error the person can read', async () => {
    await mount();
    fireEvent.click(palette().getByRole('button', { name: 'Add Postgres CDC source' }));
    fireEvent.click(palette().getByRole('button', { name: 'Add NATS sink' }));
    connectFromPanel('Postgres CDC', 'NATS');
    await waitFor(() =>
      expect(document.body.textContent).toContain(
        'the schemas do not match: Missing required field data (bytes).',
      ),
    );
    fireEvent.click(screen.getByRole('button', { name: 'Validate' }));
    const results = await screen.findByRole('region', { name: 'Validation results' });
    expect(
      within(results).getByText(/Postgres CDC → NATS: Missing required field data/),
    ).toBeTruthy();
  });

  it('secrets are write-only, and deploy runs the pipeline with live charts per node', async () => {
    const { client, container } = await mount();
    fireEvent.click(palette().getByRole('button', { name: 'Add Webhook source' }));
    let p = panel('Webhook');
    fireEvent.change(p.getByLabelText(/^Path/), { target: { value: '/hooks/orders' } });
    const secret = p.getByLabelText(/^Signing secret/) as HTMLInputElement;
    expect(secret.type).toBe('password');
    fireEvent.change(secret, { target: { value: 'sekrit-value-123' } });
    fireEvent.click(p.getByRole('button', { name: 'Save' }));
    p = panel('Webhook');
    // Saved: the field is empty again, says "set", and offers to replace.
    expect((p.getByLabelText(/^Signing secret/) as HTMLInputElement).value).toBe('');
    expect(p.getByText('set')).toBeTruthy();
    expect(p.getByRole('button', { name: 'Replace' })).toBeTruthy();

    fireEvent.click(palette().getByRole('button', { name: 'Add Iceberg sink' }));
    p = panel('Iceberg');
    fireEvent.change(p.getByLabelText('Catalog required'), { target: { value: 'main' } });
    fireEvent.change(p.getByLabelText(/^Table/), { target: { value: 'events' } });
    connectFromPanel('Webhook', 'Iceberg');

    fireEvent.click(screen.getByRole('button', { name: 'Deploy' }));
    await screen.findByRole('button', { name: 'Stop' });
    expect(screen.getByText('Running')).toBeTruthy();

    // The secret went to deploy by ref, and nowhere else.
    expect(container.innerHTML).not.toContain('sekrit-value-123');
    expect(JSON.stringify(client.calls)).not.toContain('sekrit-value-123');
    expect(client.calls.find((c) => c.method === 'deploy')?.arg).toEqual({
      name: 'untitled-pipeline',
      secretRefs: ['pipelines/untitled-pipeline/webhook-1/signing_secret'],
    });

    // Each node: a throughput and a lag LiveChart, and a sparkline on the canvas.
    const metrics = within(screen.getByRole('region', { name: 'Live metrics' }));
    for (const label of ['Webhook', 'Iceberg']) {
      expect(
        metrics.getByRole('img', { name: `${label} throughput in events/s, last five minutes` }),
      ).toBeTruthy();
      expect(
        metrics.getByRole('img', { name: `${label} lag in s, last five minutes` }),
      ).toBeTruthy();
    }
    expect(charts.liveSpecs).toContainEqual(metricSpec('throughput'));
    expect(charts.liveSpecs).toContainEqual(metricSpec('lag'));

    // Running is read-only; Stop gives the graph back.
    expect((screen.getByLabelText('Pipeline name') as HTMLInputElement).disabled).toBe(true);
    fireEvent.click(screen.getByRole('button', { name: 'Stop' }));
    await screen.findByRole('button', { name: 'Deploy' });
    expect(screen.queryByRole('region', { name: 'Live metrics' })).toBeNull();

    // A rename changes every secret ref, and the page sent its values already: the
    // secret reads unset again, so the next deploy cannot name a ref with no value.
    fireEvent.change(screen.getByLabelText('Pipeline name'), { target: { value: 'orders' } });
    fireEvent.click(outline().getByRole('button', { name: /Webhook/ }));
    p = panel('Webhook');
    expect(p.queryByText('set')).toBeNull();
    expect(p.queryByRole('button', { name: 'Replace' })).toBeNull();
  });

  it('a rename keeps a secret the page still holds', async () => {
    await mount();
    fireEvent.click(palette().getByRole('button', { name: 'Add Webhook source' }));
    const s = panel('Webhook').getByLabelText(/^Signing secret/);
    fireEvent.change(s, { target: { value: 'x' } });
    fireEvent.keyDown(s, { key: 'Enter' });
    fireEvent.change(screen.getByLabelText('Pipeline name'), { target: { value: 'orders' } });
    expect(panel('Webhook').getByText('set')).toBeTruthy();
  });

  it('refuses to deploy an invalid graph and lists why', async () => {
    const { client } = await mount();
    fireEvent.click(palette().getByRole('button', { name: 'Add Kafka source' }));
    fireEvent.click(screen.getByRole('button', { name: 'Deploy' }));
    const results = await screen.findByRole('region', { name: 'Validation results' });
    expect(within(results).getByText('Add a sink.')).toBeTruthy();
    expect(client.calls.some((c) => c.method === 'deploy')).toBe(false);
  });

  it('shows a refused deploy', async () => {
    await mount({ failDeploy: 'quota exceeded' });
    fireEvent.click(palette().getByRole('button', { name: 'Add Webhook source' }));
    fireEvent.change(panel('Webhook').getByLabelText(/^Path/), { target: { value: '/h' } });
    const s = panel('Webhook').getByLabelText(/^Signing secret/);
    fireEvent.change(s, { target: { value: 'x' } });
    fireEvent.keyDown(s, { key: 'Enter' });
    fireEvent.click(palette().getByRole('button', { name: 'Add S3 sink' }));
    const p = panel('S3');
    fireEvent.change(p.getByLabelText(/^Bucket/), { target: { value: 'b' } });
    fireEvent.change(p.getByLabelText(/^Format/), { target: { value: 'parquet' } });
    fireEvent.change(p.getByLabelText(/^Access key id/), { target: { value: 'id' } });
    const k = p.getByLabelText(/^Secret access key/);
    fireEvent.change(k, { target: { value: 'k' } });
    fireEvent.keyDown(k, { key: 'Enter' });
    connectFromPanel('Webhook', 'S3');
    fireEvent.click(screen.getByRole('button', { name: 'Deploy' }));
    expect(await screen.findByText('quota exceeded')).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Deploy' })).toBeTruthy();
  });
});
