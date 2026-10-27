import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vite-plus/test';
import {
  DeployConflictError,
  isSkipped,
  type LogLine,
  type LogsSkipped,
  validatorText,
} from '../src/admin.js';
import { createMockAdminApi, defaultFixtures } from '../src/admin-mock.js';
import { DeploymentsPage, PREVIEW_NOTICE } from '../src/deployments.js';

afterEach(cleanup);

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

const fx = defaultFixtures();
const idAt = (i: number): string => {
  const d = fx.deployments[i];
  if (!d) throw new Error(`no fixture deployment at ${i}`);
  return d.id;
};
const [D4, D3, D2, D1] = [idAt(0), idAt(1), idAt(2), idAt(3)];
const short = (id: string) => id.slice(-8);

async function collect(it: AsyncIterable<LogLine | LogsSkipped>, n: number): Promise<LogLine[]> {
  const out: LogLine[] = [];
  for await (const l of it) {
    if (isSkipped(l)) continue;
    out.push(l);
    if (out.length >= n) break;
  }
  return out;
}

describe('validatorText', () => {
  it('renders the v.* vocabulary', () => {
    expect(validatorText(undefined)).toBe('(none)');
    expect(validatorText({ kind: 'string' })).toBe('v.string()');
    expect(
      validatorText({
        kind: 'object',
        fields: {
          a: { kind: 'id', table: 'users' },
          b: { kind: 'optional', inner: { kind: 'array', items: { kind: 'int64' } } },
          c: {
            kind: 'union',
            members: [{ kind: 'literal', value: 'x' }, { kind: 'null' }],
          },
        },
      }),
    ).toBe(
      'v.object({ a: v.id("users"), b: v.optional(v.array(v.int64())), c: v.union(v.literal("x"), v.null()) })',
    );
    expect(validatorText({ kind: 'object', fields: {} })).toBe('v.object({})');
  });
});

describe('createMockAdminApi', () => {
  it('lists deployments newest first', async () => {
    const api = createMockAdminApi();
    const ds = await api.listDeployments();
    expect(ds.map((d) => d.id)).toEqual([D4, D3, D2, D1]);
    expect(ds.filter((d) => d.state === 'active').map((d) => d.id)).toEqual([D3]);
  });

  it('activates with a compare-and-swap on the active deployment', async () => {
    const api = createMockAdminApi();
    await expect(api.activateDeployment(D2, D1)).rejects.toBeInstanceOf(DeployConflictError);
    const d = await api.activateDeployment(D2, D3);
    expect(d.state).toBe('active');
    const ds = await api.listDeployments();
    expect(ds.find((x) => x.id === D3)?.state).toBe('superseded');
    const workers = (await api.workerHealth?.()) ?? [];
    expect(workers.length).toBeGreaterThan(0);
    expect(workers.every((w) => w.deploymentId === D2 && !w.degraded)).toBe(true);
  });

  it('activates a failed deployment, as the engine does (LV1 T7-1)', async () => {
    const api = createMockAdminApi();
    const d = await api.activateDeployment(D4, D3);
    expect(d.state).toBe('active');
    expect(d.failureReason).toBeUndefined();
  });

  it('tails logs filtered by function, then follows', async () => {
    const api = createMockAdminApi(defaultFixtures(), { followEveryMs: 1 });
    const ctl = new AbortController();
    const sent = await collect(
      api.tailLogs({ follow: false, function: 'messages:send' }, ctl.signal),
      100,
    );
    expect(sent.length).toBeGreaterThan(0);
    expect(sent.every((l) => l.function === 'messages:send')).toBe(true);
    const followed = await collect(api.tailLogs({ follow: true }, ctl.signal), fx.logs.length + 2);
    expect(followed).toHaveLength(fx.logs.length + 2);
    ctl.abort();
  });

  it('stops following when aborted', async () => {
    const api = createMockAdminApi(defaultFixtures(), { followEveryMs: 10_000 });
    const ctl = new AbortController();
    const it = api.tailLogs({ follow: true }, ctl.signal)[Symbol.asyncIterator]();
    for (let i = 0; i < fx.logs.length; i++) await it.next();
    const next = it.next();
    ctl.abort();
    await expect(next).resolves.toEqual({ done: true, value: undefined });
  });
});

describe('DeploymentsPage', () => {
  it('shows the preview notice, the active deployment and the history', async () => {
    render(<DeploymentsPage api={createMockAdminApi()} />);
    expect(await screen.findByText(/Sample data/)).toBeTruthy();
    expect(screen.getByText(new RegExp(PREVIEW_NOTICE))).toBeTruthy();
    const history = await screen.findByRole('table', { name: 'Deploy history' });
    for (const id of [D1, D2, D3, D4]) expect(within(history).getByText(short(id))).toBeTruthy();
    const active = screen.getByRole('region', { name: 'Active deployment' });
    expect(within(active).getByText(D3)).toBeTruthy();
    expect(within(history).getByText(/live_schema_violation/)).toBeTruthy();
  });

  it('flags a degraded worker', async () => {
    render(<DeploymentsPage api={createMockAdminApi()} />);
    const workers = await screen.findByRole('table', { name: 'Worker health' });
    expect(within(workers).getByText('Degraded')).toBeTruthy();
    expect(within(workers).getByText('Healthy')).toBeTruthy();
    expect(screen.getByText(/five or more crashes in a minute/)).toBeTruthy();
  });

  it('lists the functions of the active deployment with their validators', async () => {
    render(<DeploymentsPage api={createMockAdminApi()} />);
    const fns = await screen.findByRole('table', { name: `Functions in deployment ${D3}` });
    expect(within(fns).getByText('messages:react')).toBeTruthy();
    expect(
      within(fns).getByText(/v\.union\(v\.literal\("up"\), v\.literal\("down"\)\)/),
    ).toBeTruthy();
    expect(within(fns).getByText('internal')).toBeTruthy();
    expect(within(fns).getByText('Not validated')).toBeTruthy();
  });

  it('switches the functions view to another deployment', async () => {
    render(<DeploymentsPage api={createMockAdminApi()} />);
    fireEvent.click(
      await screen.findByRole('button', { name: `Show functions of deployment ${D1}` }),
    );
    const fns = await screen.findByRole('table', { name: `Functions in deployment ${D1}` });
    expect(within(fns).queryByText('messages:react')).toBeNull();
    expect(within(fns).getByText('messages:send')).toBeTruthy();
  });

  it('hides the previous functions while another deployment loads', async () => {
    const api = createMockAdminApi();
    let release = () => {};
    const gate = new Promise<void>((r) => {
      release = r;
    });
    const slow = {
      ...api,
      getDeployment: async (id: string) => {
        if (id === D1) await gate;
        return api.getDeployment(id);
      },
    };
    render(<DeploymentsPage api={slow} />);
    await screen.findByRole('table', { name: `Functions in deployment ${D3}` });
    fireEvent.click(screen.getByRole('button', { name: `Show functions of deployment ${D1}` }));
    await waitFor(() =>
      expect(screen.queryByRole('table', { name: `Functions in deployment ${D3}` })).toBeNull(),
    );
    release();
    expect(
      await screen.findByRole('table', { name: `Functions in deployment ${D1}` }),
    ).toBeTruthy();
  });

  it('rolls back to an earlier deployment after confirmation', async () => {
    render(<DeploymentsPage api={createMockAdminApi()} />);
    fireEvent.click(await screen.findByRole('button', { name: `Roll back to ${D2}` }));
    const dialog = await screen.findByRole('dialog', { hidden: true });
    expect(within(dialog).getByText('Confirm rollback')).toBeTruthy();
    fireEvent.click(within(dialog).getByRole('button', { name: 'Roll back', hidden: true }));
    expect(await screen.findByText(`Rolled back to ${short(D2)}.`)).toBeTruthy();
    await waitFor(() => {
      const active = screen.getByRole('region', { name: 'Active deployment' });
      expect(within(active).getByText(D2)).toBeTruthy();
    });
  });

  it('offers no action on the active or a failed deployment', async () => {
    render(<DeploymentsPage api={createMockAdminApi()} />);
    await screen.findByRole('table', { name: 'Deploy history' });
    expect(screen.queryByRole('button', { name: new RegExp(D3) })).not.toBeNull(); // select only
    expect(screen.queryByRole('button', { name: `Roll back to ${D3}` })).toBeNull();
    expect(screen.queryByRole('button', { name: `Activate ${D4}` })).toBeNull();
    expect(screen.queryByRole('button', { name: `Roll back to ${D4}` })).toBeNull();
  });

  it('tails logs for one function from its row', async () => {
    render(
      <DeploymentsPage api={createMockAdminApi(defaultFixtures(), { followEveryMs: 60_000 })} />,
    );
    const log = await screen.findByRole('log', { name: 'Function log lines' });
    await waitFor(() => expect(within(log).getByText(/body too long/)).toBeTruthy());
    fireEvent.click(await screen.findByRole('button', { name: 'Tail logs of messages:list' }));
    await waitFor(() => {
      expect(within(log).queryByText(/body too long/)).toBeNull();
      expect(within(log).getByText(/cursor resumed/)).toBeTruthy();
    });
    expect((screen.getByLabelText('Function') as HTMLSelectElement).value).toBe('messages:list');
  });

  it('shows a load error with a retry', async () => {
    const api = createMockAdminApi();
    let fail = true;
    const flaky = {
      ...api,
      mocked: false,
      listDeployments: () => (fail ? Promise.reject(new Error('boom')) : api.listDeployments()),
    };
    render(<DeploymentsPage api={flaky} />);
    expect(await screen.findByText('boom')).toBeTruthy();
    expect(screen.queryByText(new RegExp(PREVIEW_NOTICE))).toBeNull();
    fail = false;
    fireEvent.click(screen.getByRole('button', { name: 'Retry' }));
    expect(await screen.findByRole('table', { name: 'Deploy history' })).toBeTruthy();
  });
});
