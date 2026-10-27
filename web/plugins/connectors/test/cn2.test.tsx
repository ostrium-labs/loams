import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vite-plus/test';
import { CatalogPage, filterCatalog, runtimeClass } from '../src/catalog-page.js';
import { formatLag } from '../src/flow-client.js';
import { buildFields, secretValues } from '../src/form.js';
import { canPause, canResume, InstanceDetailPage, InstancesPage } from '../src/instances-page.js';
import { createMockFlowClient } from '../src/mock-flow.js';
import { bindingOf, WizardPage } from '../src/wizard-page.js';
import { fakeDesktop, load } from './fake.js';

afterEach(cleanup);

globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};

const SECRET = 'hunter2-SECRET';
const KEY = '-----BEGIN PRIVATE KEY-----abc';

describe('catalog filters (CN2)', () => {
  const all = ['kafka', 'adyen', 'postgresql'].map((id) => load(id).summary);
  const base = { q: '', family: '', status: '', direction: '', runtime: '' } as const;

  it('classes runtimes as native or fallback', () => {
    expect(runtimeClass(load('kafka').summary)).toBe('native');
    expect(runtimeClass(load('adyen').summary)).toBe('fallback');
  });

  it('filters by runtime class', () => {
    expect(filterCatalog(all, { ...base, runtime: 'native' }).map((c) => c.id)).not.toContain(
      'adyen',
    );
    expect(filterCatalog(all, { ...base, runtime: 'fallback' }).map((c) => c.id)).toEqual([
      'adyen',
    ]);
  });

  it('filters by direction', () => {
    for (const c of filterCatalog(all, { ...base, direction: 'source' }))
      expect(c.source).not.toBeNull();
    for (const c of filterCatalog(all, { ...base, direction: 'sink' }))
      expect(c.sink).not.toBeNull();
  });

  it('filters by family', () => {
    const fam = load('kafka').summary.category;
    for (const c of filterCatalog(all, { ...base, family: fam })) expect(c.category).toBe(fam);
  });

  it('renders the direction, runtime and family filter groups and a Create action', async () => {
    const { api } = fakeDesktop(['kafka', 'adyen', 'postgresql']);
    const nav: string[] = [];
    render(<CatalogPage desktop={api} navigate={(p) => void nav.push(p)} />);
    await screen.findAllByRole('article');
    expect(screen.getByRole('group', { name: 'Filter by family' })).toBeTruthy();
    expect(screen.getByRole('group', { name: 'Filter by direction' })).toBeTruthy();
    fireEvent.click(
      within(screen.getByRole('group', { name: 'Filter by runtime' })).getByRole('button', {
        name: 'fallback',
      }),
    );
    const cards = screen.getAllByRole('article');
    expect(cards).toHaveLength(1);
    expect(screen.getByText('Adyen')).toBeTruthy();
    fireEvent.click(
      within(screen.getByRole('group', { name: 'Filter by runtime' })).getByRole('button', {
        name: 'native',
      }),
    );
    fireEvent.click(screen.getByRole('button', { name: 'Create Kafka instance' }));
    expect(nav).toEqual(['/connectors/kafka/new']);
  });
});

describe('mock flow client', () => {
  it('keeps secret names only, never values', async () => {
    const flow = createMockFlowClient([]);
    const r = await flow.createInstance({
      connector: 'kafka',
      name: 'k1',
      direction: 'source',
      runtime: 'native',
      fabric: { kind: 'topic', topic: 't' },
      config: { sasl: { password: '${secret:sasl.password}' } },
      secrets: { 'sasl.password': SECRET },
      enabled: true,
    });
    expect(r.ok).toBe(true);
    expect(JSON.stringify([...flow.store.values()])).not.toContain('hunter2');
    expect(flow.store.get('k1')?.secrets).toEqual(['sasl.password']);
  });

  it('refuses bad and duplicate names', async () => {
    const flow = createMockFlowClient();
    const req = {
      connector: 'kafka',
      direction: 'source' as const,
      runtime: 'native' as const,
      fabric: { kind: 'topic' as const, topic: 't' },
      config: {},
      secrets: {},
      enabled: false,
    };
    expect((await flow.createInstance({ ...req, name: 'Bad Name' })).ok).toBe(false);
    const dup = await flow.createInstance({ ...req, name: 'orders-cdc' });
    expect(dup.ok ? '' : dup.code).toBe('already_exists');
  });

  it('pauses a running instance and resumes a paused one', async () => {
    const flow = createMockFlowClient();
    const p = await flow.pause('orders-cdc');
    expect(p.ok && p.value.state).toBe('paused');
    expect((await flow.pause('orders-cdc')).ok).toBe(false);
    const r = await flow.resume('orders-cdc');
    expect(r.ok && r.value.state).toBe('starting');
    expect((await flow.pause('missing')).ok).toBe(false);
  });

  it('state guards and lag formatting', () => {
    expect(canPause('running')).toBe(true);
    expect(canPause('paused')).toBe(false);
    expect(canResume('failed')).toBe(true);
    expect(formatLag(0.2)).toBe('<1 s');
    expect(formatLag(840)).toBe('14 min');
  });
});

describe('form helpers (CN2)', () => {
  it('secretValues returns only filled secret leaves', () => {
    const { detail } = load('kafka');
    const fields = buildFields(detail.schema, detail.manifest.secrets as string[]);
    expect(secretValues(fields, { 'sasl.password': SECRET, brokers: 'b1' })).toEqual({
      'sasl.password': SECRET,
    });
  });

  it('bindingOf builds the Fabric binding', () => {
    expect(bindingOf('topic', ' t ', '4')).toEqual({ kind: 'topic', topic: 't', partitions: 4 });
    expect(bindingOf('table', 't', '')).toEqual({ kind: 'table', table: 't' });
    expect(bindingOf('collection', 'c', '3')).toEqual({ kind: 'collection', collection: 'c' });
  });
});

describe('create-connector wizard', () => {
  it('walks the steps, sends secrets write-only and never renders them back', async () => {
    const { api } = fakeDesktop(['kafka']);
    const flow = createMockFlowClient([]);
    const nav: string[] = [];
    render(<WizardPage desktop={api} flow={flow} id="kafka" navigate={(p) => void nav.push(p)} />);
    const basics = await screen.findByRole('heading', { name: 'Basics' });
    expect(document.activeElement).toBe(basics);

    fireEvent.change(screen.getByLabelText(/^Instance name/), { target: { value: 'Bad Name' } });
    fireEvent.click(screen.getByRole('button', { name: 'Next' }));
    expect(await screen.findByRole('alert')).toBeTruthy();
    fireEvent.change(screen.getByLabelText(/^Instance name/), { target: { value: 'orders-in' } });
    fireEvent.click(screen.getByRole('button', { name: 'Next' }));
    await screen.findByRole('heading', { name: 'Configure' });

    const set = (re: RegExp, value: string) =>
      fireEvent.change(screen.getByLabelText(re), { target: { value } });
    set(/^brokers/, 'b1:9092');
    set(/^topics/, 'orders\nrefunds');
    set(/^group_id_prefix/, 'loams-dev');
    // The SASL group's fields (basic_auth has a username and password too, and
    // username_secret starts with username).
    const sasl = within(screen.getByRole('group', { name: /^sasl\b/ }));
    fireEvent.change(sasl.getByLabelText(/^username\b/), { target: { value: 'svc' } });
    fireEvent.change(sasl.getByLabelText(/^password\b/), { target: { value: SECRET } });
    set(/^key_pem/, KEY);
    expect((sasl.getByLabelText(/^password\b/) as HTMLInputElement).type).toBe('password');
    await waitFor(() => {
      fireEvent.click(screen.getByRole('button', { name: 'Next' }));
      expect(screen.getByRole('heading', { name: 'Fabric binding' })).toBeTruthy();
    });

    fireEvent.click(screen.getByRole('button', { name: 'Next' }));
    expect(await screen.findByRole('alert')).toBeTruthy();
    set(/^Topic/, 'shop.orders');
    set(/^Partitions/, '4');
    fireEvent.click(screen.getByRole('button', { name: 'Next' }));
    await screen.findByRole('heading', { name: 'Review' });

    expect(document.body.textContent).not.toContain('hunter2');
    expect(document.body.textContent).not.toContain('BEGIN PRIVATE KEY');
    expect(screen.getByLabelText('Config').textContent).toContain('${secret:sasl.password}');

    fireEvent.click(screen.getByRole('button', { name: 'Create instance' }));
    await waitFor(() => expect(nav).toEqual(['/connectors/instances/orders-in']));
    const stored = flow.store.get('orders-in');
    expect(stored?.fabric).toEqual({ kind: 'topic', topic: 'shop.orders', partitions: 4 });
    expect(stored?.secrets).toEqual(['sasl.password', 'tls.key_pem']);
    expect(JSON.stringify(stored)).not.toContain('hunter2');
    for (const store of [localStorage, sessionStorage]) {
      for (let i = 0; i < store.length; i++) {
        expect(store.getItem(store.key(i) as string)).not.toContain('hunter2');
      }
    }
  });

  it('refuses a planned (stub) connector', async () => {
    const { api } = fakeDesktop(['adyen']);
    render(
      <WizardPage
        desktop={api}
        flow={createMockFlowClient([])}
        id="adyen"
        navigate={() => undefined}
      />,
    );
    expect(await screen.findByText('Not available yet')).toBeTruthy();
  });
});

describe('running instances', () => {
  it('lists instances and pauses one from the table', async () => {
    const flow = createMockFlowClient();
    render(<InstancesPage flow={flow} navigate={() => undefined} />);
    expect(await screen.findByRole('row', { name: /orders-cdc/ })).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Pause orders-cdc' }));
    await screen.findByRole('button', { name: 'Resume orders-cdc' });
    expect(flow.store.get('orders-cdc')?.state).toBe('paused');
  });

  it('shows checkpoints, lag, throughput and errors, and resumes', async () => {
    const flow = createMockFlowClient();
    render(<InstanceDetailPage flow={flow} name="clicks-to-house" navigate={() => undefined} />);
    expect(await screen.findByRole('heading', { name: 'clicks-to-house' })).toBeTruthy();
    expect(screen.getByRole('row', { name: /partition-0/ })).toBeTruthy();
    expect(screen.getByRole('row', { name: /partition-1/ })).toBeTruthy();
    expect(screen.getByRole('img', { name: /Events per second/ })).toBeTruthy();
    expect(screen.getAllByText(/DateTime64 to String/).length).toBeGreaterThan(0);
    fireEvent.click(screen.getByRole('button', { name: 'Resume clicks-to-house' }));
    await screen.findByRole('button', { name: 'Pause clicks-to-house' });
    expect(flow.store.get('clicks-to-house')?.state).toBe('starting');
  });

  it('shows the re-snapshot hint for a lost position', async () => {
    render(
      <InstanceDetailPage
        flow={createMockFlowClient()}
        name="erp-oracle-cdc"
        navigate={() => undefined}
      />,
    );
    expect((await screen.findAllByText(/Re-snapshot the instance/)).length).toBeGreaterThan(0);
  });

  it('reports a missing instance', async () => {
    render(
      <InstanceDetailPage flow={createMockFlowClient()} name="nope" navigate={() => undefined} />,
    );
    expect(await screen.findByText('No instance "nope".')).toBeTruthy();
  });

  it('drops the previous instance at once when the name changes', async () => {
    const flow = createMockFlowClient();
    const { rerender } = render(
      <InstanceDetailPage flow={flow} name="orders-cdc" navigate={() => undefined} />,
    );
    await screen.findByRole('heading', { name: 'orders-cdc' });
    rerender(<InstanceDetailPage flow={flow} name="clicks-to-house" navigate={() => undefined} />);
    expect(screen.queryByRole('heading', { name: 'orders-cdc' })).toBeNull();
    expect(await screen.findByRole('heading', { name: 'clicks-to-house' })).toBeTruthy();
  });

  it('labels fixture data as sample data', async () => {
    render(<InstancesPage flow={createMockFlowClient()} navigate={() => undefined} />);
    expect(screen.getByText('Sample data')).toBeTruthy();
  });
});
