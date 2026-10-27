// FT0 Task 9 (plan FT1): Studio's Runs page over a fake run record, and the Live source
// over a fake Connect router (no engine).

import { createRouterTransport } from '@connectrpc/connect';
import { live } from '@loams/proto';
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vite-plus/test';
import { liveRunsSource, type RunRow, RunsPage, type RunsSource } from '../src/index.js';

afterEach(cleanup);

globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};

const run = (patch: Partial<RunRow> = {}): RunRow => ({
  run: 'run-1',
  seq: 2,
  project: 'comet',
  intent: 'Rate limit signup',
  stage: 'plan',
  state: 'waiting',
  approvals: [{ kind: 'plan', id: 'fa-1', state: 'pending' }],
  budget: { limit: 5, spent: 0.42 },
  createdAt: 1,
  updatedAt: 2,
  ...patch,
});

/** A source whose rows the test changes between polls. */
function mutableSource(rows: RunRow[]): RunsSource & { set(next: RunRow[]): void } {
  let current = rows;
  return {
    list: async () => current,
    set(next) {
      current = next;
    },
  };
}

describe('the Runs page', () => {
  it('lists runs and shows the selected run, then updates on the next read', async () => {
    const source = mutableSource([
      run(),
      run({ run: 'run-0', intent: 'Older', state: 'succeeded', stage: 'close' }),
    ]);
    render(<RunsPage source={source} pollMs={20} />);
    const table = await screen.findByRole('table');
    expect(within(table).getByText('Rate limit signup')).toBeTruthy();
    expect(within(table).getByText('Older')).toBeTruthy();
    const detail = screen.getByRole('region', { name: 'Run run-1' });
    expect(within(detail).getByRole('listitem', { current: 'step' }).textContent).toBe('plan');
    expect(within(detail).getByText('$0.42 of $5.00')).toBeTruthy();
    expect(within(detail).getByText(/plan:/)).toBeTruthy();

    source.set([
      run({
        stage: 'review',
        state: 'open',
        seq: 5,
        pr: { number: 7, url: 'http://forgejo.invalid/dev/comet/pulls/7' },
        approvals: [{ kind: 'plan', id: 'fa-1', state: 'approved' }],
      }),
    ]);
    await waitFor(() =>
      expect(
        within(screen.getByRole('region', { name: 'Run run-1' })).getByRole('listitem', {
          current: 'step',
        }).textContent,
      ).toBe('review'),
    );
    const pr = screen.getByRole('link', { name: '#7' });
    expect(pr.getAttribute('href')).toBe('http://forgejo.invalid/dev/comet/pulls/7');
  });

  it('selects another run from the table', async () => {
    const source = mutableSource([run(), run({ run: 'run-0', intent: 'Older', stage: 'close' })]);
    render(<RunsPage source={source} pollMs={1_000} />);
    fireEvent.click(await screen.findByText('Older'));
    expect(await screen.findByRole('region', { name: 'Run run-0' })).toBeTruthy();
  });

  it('says so when there are no runs, or the record cannot be read', async () => {
    render(<RunsPage source={mutableSource([])} pollMs={1_000} />);
    expect(await screen.findByText('No runs yet')).toBeTruthy();
    cleanup();
    render(
      <RunsPage
        source={{
          list: async () => {
            throw new Error('unimplemented');
          },
        }}
        pollMs={1_000}
      />,
    );
    expect(await screen.findByText(/could not be read/)).toBeTruthy();
  });
});

describe('the Live source', () => {
  it('queries runs:list on the factory app', async () => {
    const seen: { fn: string; app: string | null }[] = [];
    const s = (value: string) => ({ kind: { case: 'stringValue' as const, value } });
    const i = (value: number) => ({ kind: { case: 'int64Value' as const, value: BigInt(value) } });
    const transport = createRouterTransport(({ service }) => {
      service(live.LiveService, {
        query(req: { function: string }, ctx: { requestHeader: Headers }) {
          seen.push({ fn: req.function, app: ctx.requestHeader.get('loams-live-app') });
          return {
            ts: 1n,
            result: {
              kind: {
                case: 'arrayValue',
                value: {
                  values: [
                    {
                      kind: {
                        case: 'objectValue',
                        value: {
                          fields: {
                            run: s('run-9'),
                            seq: i(3),
                            stage: s('ci'),
                            state: s('open'),
                            createdAt: i(1),
                            updatedAt: i(2),
                          },
                        },
                      },
                    },
                  ],
                },
              },
            },
          };
        },
      } as never);
    });
    const rows = await liveRunsSource(transport).list();
    expect(seen).toEqual([{ fn: 'runs:list', app: 'factory' }]);
    expect(rows).toEqual([
      { run: 'run-9', seq: 3, stage: 'ci', state: 'open', createdAt: 1, updatedAt: 2 },
    ]);
  });
});
