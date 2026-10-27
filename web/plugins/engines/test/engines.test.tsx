// The Engines pages over the sample client (src/mock.ts): list, create, detail, SQL and Spark.

import { validateManifest } from '@loams/console-host';
import { setCodeEditorText } from '@loams/ui';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vite-plus/test';
import pkg from '../package.json';
import type { EnginesClient } from '../src/client.js';
import { createMockEnginesClient } from '../src/mock.js';
import { CreatePage } from '../src/pages/create.js';
import { DetailPage } from '../src/pages/detail.js';
import { ListPage } from '../src/pages/list.js';
import { SparkPage } from '../src/pages/spark.js';
import { SqlPage, toOutcome } from '../src/pages/sql.js';

afterEach(cleanup);

// jsdom has no layout: the code editor measures text ranges for its selection and tooltips.
Range.prototype.getClientRects ??= () => [] as unknown as DOMRectList;
Range.prototype.getBoundingClientRect ??= () => new DOMRect();
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};
HTMLDialogElement.prototype.showModal ??= function (this: HTMLDialogElement) {
  this.setAttribute('open', '');
};
HTMLDialogElement.prototype.close ??= function (this: HTMLDialogElement) {
  this.removeAttribute('open');
};
globalThis.requestAnimationFrame ??= (cb: FrameRequestCallback) =>
  setTimeout(() => cb(0), 0) as unknown as number;

const table = (name: string) => screen.findByRole('table', { name });

describe('manifest', () => {
  it('is a valid console plugin manifest', () => {
    expect(() => validateManifest(pkg)).not.toThrow();
  });
});

describe('list', () => {
  it('shows instances of all three kinds, with the Preview badge and the sample notice', async () => {
    render(<ListPage client={createMockEnginesClient()} navigate={() => {}} />);
    const rows = within(await table('Engine instances')).getAllByRole('row');
    const text = rows.map((r) => r.textContent ?? '').join('\n');
    for (const kind of ['RisingWave', 'Flink SQL', 'Spark']) expect(text).toContain(kind);
    expect(text).toContain('orders-stream');
    expect(text).toContain('nightly-batch');
    expect(screen.getAllByText('Preview').length).toBeGreaterThan(0);
    expect(screen.getByText(/Sample data/)).toBeTruthy();
  });

  it('filters by kind', async () => {
    render(<ListPage client={createMockEnginesClient()} navigate={() => {}} />);
    await table('Engine instances');
    fireEvent.change(screen.getByLabelText('Kind'), { target: { value: 'flink' } });
    await waitFor(() => expect(screen.queryByText('orders-stream')).toBeNull());
    expect(screen.getByText('fraud-rules')).toBeTruthy();
    expect(screen.queryByText('nightly-batch')).toBeNull();
  });

  it('opens an instance', async () => {
    const navigate = vi.fn();
    render(<ListPage client={createMockEnginesClient()} navigate={navigate} />);
    fireEvent.click(await screen.findByText('fraud-rules'));
    expect(navigate).toHaveBeenCalledWith('/engines/default-dev/eng_fl01fraud');
  });
});

describe('create', () => {
  it('posts the form as the contract body and opens the new instance', async () => {
    const client = createMockEnginesClient();
    const create = vi.spyOn(client, 'create');
    const navigate = vi.fn();
    render(<CreatePage client={client} navigate={navigate} />);
    fireEvent.change(await screen.findByLabelText('Engine'), { target: { value: 'spark' } });
    fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'etl' } });
    fireEvent.change(screen.getByLabelText('Version'), { target: { value: '3.5.7' } });
    fireEvent.change(screen.getByLabelText(/^Size/), { target: { value: 'l' } });
    fireEvent.change(screen.getByLabelText('Executors'), { target: { value: '4' } });
    fireEvent.click(screen.getByRole('button', { name: 'Create instance' }));
    await waitFor(() => expect(create).toHaveBeenCalled());
    expect(create).toHaveBeenCalledWith('default-dev', {
      name: 'etl',
      kind: 'spark',
      version: '3.5.7',
      size: { tier: 'l', replicas: 4 },
    });
    await waitFor(() =>
      expect(navigate).toHaveBeenCalledWith(expect.stringMatching(/^\/engines\/default-dev\/eng_/)),
    );
  });

  it('refuses a name that is not a DNS label', async () => {
    render(<CreatePage client={createMockEnginesClient()} navigate={() => {}} />);
    fireEvent.change(await screen.findByLabelText('Name'), { target: { value: 'Bad_Name' } });
    expect(screen.getByText(/Lowercase letters/)).toBeTruthy();
    expect(
      (screen.getByRole('button', { name: 'Create instance' }) as HTMLButtonElement).disabled,
    ).toBe(true);
  });
});

describe('out of allowance (402 entitlement_exhausted)', () => {
  it('says so plainly when a create is refused', async () => {
    render(
      <CreatePage client={createMockEnginesClient({ exhausted: true })} navigate={() => {}} />,
    );
    fireEvent.change(await screen.findByLabelText('Name'), { target: { value: 'etl' } });
    fireEvent.click(screen.getByRole('button', { name: 'Create instance' }));
    expect(await screen.findByText('Out of allowance')).toBeTruthy();
    expect(screen.getByText(/Nothing was changed/)).toBeTruthy();
  });

  it('says so plainly when a resume is refused', async () => {
    render(
      <DetailPage
        client={createMockEnginesClient({ exhausted: true })}
        navigate={() => {}}
        namespace="default-dev"
        id="eng_rw02clicks"
        pollMs={0}
      />,
    );
    fireEvent.click(await screen.findByRole('button', { name: 'Resume' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Resume instance' }));
    expect(await screen.findByText('Out of allowance')).toBeTruthy();
  });
});

describe('detail', () => {
  function mount(id = 'eng_rw01orders', client: EnginesClient = createMockEnginesClient()) {
    const copy = vi.fn();
    render(
      <DetailPage
        client={client}
        navigate={() => {}}
        namespace="default-dev"
        id={id}
        copy={copy}
        pollMs={0}
      />,
    );
    return { client, copy };
  }

  it('shows status, endpoints, checks and usage', async () => {
    const { copy } = mount();
    const endpoints = await screen.findByRole('list', { name: 'Endpoints' });
    expect(endpoints.textContent).toContain('postgres://gateway.loams.internal:5432/orders-stream');
    fireEvent.click(within(endpoints).getAllByRole('button', { name: 'Copy' })[0] as HTMLElement);
    expect(copy).toHaveBeenCalledWith('postgres://gateway.loams.internal:5432/orders-stream');
    expect(screen.getByRole('list', { name: 'Health checks' }).textContent).toContain('compute');
    expect(await screen.findByRole('table', { name: 'Usage samples' })).toBeTruthy();
    expect(screen.getByText('vCPU-hours')).toBeTruthy();
    // The first window of the hour is billed from the reserved size and says so.
    expect(screen.getByText(/1 window is estimated/)).toBeTruthy();
    expect(screen.getAllByText('estimated').length).toBeGreaterThan(0);
    expect(screen.getByText('Preview')).toBeTruthy();
  });

  it('suspends after confirmation', async () => {
    const client = createMockEnginesClient();
    const suspend = vi.spyOn(client, 'suspend');
    mount('eng_rw01orders', client);
    fireEvent.click(await screen.findByRole('button', { name: 'Suspend' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Suspend instance' }));
    await waitFor(() => expect(suspend).toHaveBeenCalledWith('default-dev', 'eng_rw01orders'));
    await expect(client.get('default-dev', 'eng_rw01orders')).resolves.toMatchObject({
      status: { phase: 'suspending' },
    });
  }, 15_000);

  it('offers resume for a suspended instance', async () => {
    mount('eng_rw02clicks');
    expect(await screen.findByRole('button', { name: 'Resume' })).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'Suspend' })).toBeNull();
  });

  it('says what failed', async () => {
    mount('eng_sp02ml');
    expect(await screen.findByText(/driver ran out of memory/)).toBeTruthy();
  });
});

describe('SQL workspace', () => {
  it('maps a result to the workbench outcome', () => {
    expect(
      toOutcome({
        columns: [],
        rows: [],
        row_count: 3,
        truncated: false,
        elapsed_ms: 5,
      }),
    ).toEqual({ columns: [], rows: [], truncated: false, elapsedMs: 5, rowsAffected: 3 });
  });

  it('runs a query on RisingWave and shows the rows', async () => {
    const client = createMockEnginesClient();
    const sql = vi.spyOn(client, 'sql');
    render(
      <SqlPage client={client} navigate={() => {}} namespace="default-dev" id="eng_rw01orders" />,
    );
    const box = await screen.findByLabelText('SQL');
    act(() => setCodeEditorText(box, 'SELECT region, count(*) FROM orders GROUP BY region'));
    fireEvent.click(screen.getByRole('button', { name: 'Run' }));
    await waitFor(() => expect(sql).toHaveBeenCalled());
    expect(sql.mock.calls[0]?.[2]).toMatchObject({
      sql: 'SELECT region, count(*) FROM orders GROUP BY region',
    });
    expect(await screen.findByText('us-east')).toBeTruthy();
  });

  it('shows the job a Flink INSERT started', async () => {
    render(
      <SqlPage
        client={createMockEnginesClient()}
        navigate={() => {}}
        namespace="default-dev"
        id="eng_fl01fraud"
      />,
    );
    const box = await screen.findByLabelText('SQL');
    act(() => setCodeEditorText(box, 'INSERT INTO alerts SELECT * FROM payments'));
    fireEvent.click(screen.getByRole('button', { name: 'Run' }));
    expect(await screen.findByText(/Job submitted: a1b2c3d4e5f6/)).toBeTruthy();
  });

  it('keeps Run disabled while the instance is not running', async () => {
    render(
      <SqlPage
        client={createMockEnginesClient()}
        navigate={() => {}}
        namespace="default-dev"
        id="eng_rw02clicks"
      />,
    );
    await screen.findByLabelText('SQL');
    expect((screen.getByRole('button', { name: 'Run' }) as HTMLButtonElement).disabled).toBe(true);
  });
});

describe('Spark', () => {
  const mount = (client: EnginesClient) =>
    render(
      <SparkPage client={client} navigate={() => {}} namespace="default-dev" id="eng_sp01batch" />,
    );

  it('submits a job', async () => {
    const client = createMockEnginesClient();
    const submit = vi.spyOn(client, 'submitJob');
    mount(client);
    await table('Spark jobs');
    fireEvent.change(screen.getByLabelText('Job name'), { target: { value: 'backfill' } });
    fireEvent.change(screen.getByLabelText('SQL'), {
      target: { value: 'INSERT INTO lake.t SELECT 1' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Submit job' }));
    await waitFor(() =>
      expect(submit).toHaveBeenCalledWith('default-dev', 'eng_sp01batch', {
        name: 'backfill',
        kind: 'sql',
        entrypoint: 'INSERT INTO lake.t SELECT 1',
        args: [],
      }),
    );
    expect(await screen.findByText('backfill')).toBeTruthy();
  });

  it('cancels a running job', async () => {
    const client = createMockEnginesClient();
    const cancel = vi.spyOn(client, 'cancelJob');
    mount(client);
    fireEvent.click(await screen.findByRole('button', { name: 'Cancel job compact-events' }));
    await waitFor(() =>
      expect(cancel).toHaveBeenCalledWith('default-dev', 'eng_sp01batch', 'job_0002'),
    );
    await waitFor(() => expect(screen.getAllByText('cancelled').length).toBeGreaterThan(0));
  });

  it('starts and closes sessions', async () => {
    const client = createMockEnginesClient();
    const create = vi.spyOn(client, 'createSession');
    const close = vi.spyOn(client, 'closeSession');
    mount(client);
    await table('Spark sessions');
    fireEvent.click(screen.getByRole('button', { name: 'Start session' }));
    await waitFor(() =>
      expect(create).toHaveBeenCalledWith('default-dev', 'eng_sp01batch', { kind: 'sql' }),
    );
    fireEvent.click(await screen.findByRole('button', { name: 'Close session ses_01' }));
    await waitFor(() =>
      expect(close).toHaveBeenCalledWith('default-dev', 'eng_sp01batch', 'ses_01'),
    );
  });
});
