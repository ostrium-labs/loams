// The Jobs page against the fixture-backed mock client (src/mock.ts), plus the pure helpers.

import { validateManifest } from '@loams/console-host';
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vite-plus/test';
import pkg from '../package.json';
import { type JobSummary, type JobsClient, JobsClientError, type Page } from '../src/client.js';
import {
  ageText,
  backlog,
  PREVIEW_LIMIT,
  payloadPreview,
  rateText,
  stateStatus,
  timeText,
} from '../src/format.js';
import { JobsPage } from '../src/JobsPage.js';
import { createMockJobsClient, sampleFixtures } from '../src/mock.js';
import { failingClient, recordingClient } from './fixtures.js';

afterEach(cleanup);

/** The first element, failing the test when the list is empty. */
function first<T>(xs: readonly T[]): T {
  const x = xs[0];
  if (x === undefined) throw new Error('expected a non-empty list');
  return x;
}

// The empty state's grain canvas observes its size; jsdom has no ResizeObserver.
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

const tab = (name: string) => fireEvent.click(screen.getByRole('tab', { name }));

describe('manifest', () => {
  it('is a valid desktop console plugin', () => {
    expect(() => validateManifest(pkg)).not.toThrow();
    expect(pkg.loams.plugin.editions).toEqual(['desktop']);
    expect(pkg.loams.plugin.slots).toEqual(['console.page', 'shell.nav.section']);
  });
});

describe('format', () => {
  it('formats rates and ages', () => {
    expect(rateText(0)).toBe('0/s');
    expect(rateText(14.23)).toBe('14.2/s');
    expect(rateText(250.4)).toBe('250/s');
    expect(ageText(0)).toBe('—');
    expect(ageText(38)).toBe('38 s');
    expect(ageText(5400)).toBe('1.5 h');
  });

  it('prints times in UTC', () => {
    expect(timeText('2026-10-10T12:00:05.000Z')).toBe('2026-10-10 12:00:05 UTC');
    expect(timeText('not a time')).toBe('not a time');
  });

  it('maps states onto the status palette', () => {
    expect(stateStatus('completed')).toBe('done');
    expect(stateStatus('active')).toBe('progress');
    expect(stateStatus('dead_lettered')).toBe('failed');
    expect(stateStatus('delayed')).toBe('planned');
    expect(stateStatus('canceled')).toBe('neutral');
  });

  it('pretty-prints JSON payloads and cuts long ones', () => {
    expect(
      payloadPreview({ contentType: 'application/json', sizeBytes: 7, text: '{"a":1}' }),
    ).toEqual({
      kind: 'text',
      text: '{\n  "a": 1\n}',
      truncated: false,
    });
    const long = 'x'.repeat(PREVIEW_LIMIT + 10);
    const p = payloadPreview({ contentType: 'text/plain', sizeBytes: long.length, text: long });
    expect(p).toMatchObject({ kind: 'text', truncated: true });
    expect(p.kind === 'text' && p.text.length).toBe(PREVIEW_LIMIT);
    expect(
      payloadPreview({ contentType: 'application/json', sizeBytes: 3, text: '{x' }),
    ).toMatchObject({
      text: '{x',
    });
  });

  it('names an object-store payload and leaves binary alone', () => {
    expect(
      payloadPreview({ contentType: 'application/x-msgpack', sizeBytes: 9, blobRef: 'blob:1' }),
    ).toEqual({
      kind: 'external',
      blobRef: 'blob:1',
    });
    expect(payloadPreview({ contentType: 'application/x-msgpack', sizeBytes: 9 })).toEqual({
      kind: 'binary',
    });
  });

  it('sums the backlog', () => {
    const q = first(sampleFixtures().queues);
    expect(backlog(q.counts)).toBe(42 + 3 + 7);
  });
});

describe('mock client', () => {
  it('filters jobs by queue and state, newest first', async () => {
    const c = createMockJobsClient();
    const all = await c.listJobs('default', {});
    expect(all.items.length).toBe(5);
    const sorted = [...all.items].sort((a, b) => b.createdAt.localeCompare(a.createdAt));
    expect(all.items.map((j) => j.createdAt)).toEqual(sorted.map((j) => j.createdAt));
    const failed = await c.listJobs('default', { queue: 'reports', state: 'failed' });
    expect(failed.items.map((j) => j.name)).toEqual(['export_csv']);
    expect('payload' in first(failed.items)).toBe(false);
  });

  it('is empty for an unknown namespace', async () => {
    const c = createMockJobsClient();
    expect(await c.listQueues('other')).toEqual([]);
    expect((await c.listJobs('other', {})).items).toEqual([]);
    expect(await c.functionUsage('other', '1h')).toEqual([]);
  });

  it('retries a dead letter onto its original queue', async () => {
    const c = createMockJobsClient();
    const d = first(await c.listDeadLetters('default'));
    await c.retryDeadLetter('default', d.queue, d.id);
    expect((await c.listDeadLetters('default')).map((x) => x.id)).not.toContain(d.id);
    const job = await c.getJob('default', d.originalQueue, d.id);
    expect(job.state).toBe('waiting');
    const queues = await c.listQueues('default');
    expect(queues.find((q) => q.name === 'emails')?.counts.waiting).toBe(43);
    expect(queues.find((q) => q.name === 'emails.dlq')?.counts.dead_lettered).toBe(1);
  });

  it('discards a dead letter and refuses an unknown one', async () => {
    const c = createMockJobsClient();
    const d = first(await c.listDeadLetters('default'));
    await c.discardDeadLetter('default', d.queue, d.id);
    expect((await c.listDeadLetters('default')).length).toBe(1);
    await expect(c.discardDeadLetter('default', d.queue, d.id)).rejects.toBeInstanceOf(
      JobsClientError,
    );
  });

  it('returns copies, not its own records', async () => {
    const c = createMockJobsClient();
    const q = first(await c.listQueues('default'));
    q.counts.waiting = 0;
    expect(first(await c.listQueues('default')).counts.waiting).toBe(42);
  });
});

describe('JobsPage', () => {
  it('lists queues with depth and rates, and says the data is a sample', async () => {
    render(<JobsPage client={createMockJobsClient()} preview />);
    expect(screen.getByText('Sample data')).toBeTruthy();
    const table = await screen.findByRole('table', { name: /Queues/ });
    const row = within(table).getByText('emails').closest('tr') as HTMLElement;
    expect(within(row).getByText('52')).toBeTruthy();
    expect(within(row).getByText('13.8/s')).toBeTruthy();
    expect(within(row).getByText('8 / 16')).toBeTruthy();
    expect(within(table).getByText(/Paused · 2 shards/)).toBeTruthy();
  });

  it('shows a failure with a retry', async () => {
    render(<JobsPage client={failingClient(['listQueues'])} />);
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain('the jobs service is down');
    expect(screen.getByRole('button', { name: 'Try again' })).toBeTruthy();
  });

  it('filters jobs by state', async () => {
    const { client, calls } = recordingClient();
    render(<JobsPage client={client} />);
    tab('Jobs');
    await screen.findByRole('table', { name: /Jobs/ });
    fireEvent.change(screen.getByLabelText('State'), { target: { value: 'failed' } });
    await waitFor(() =>
      expect(calls.filter((c) => c.method === 'listJobs').at(-1)?.args[1]).toEqual({
        queue: undefined,
        state: 'failed',
      }),
    );
    await waitFor(() => expect(screen.getByText('export_csv')).toBeTruthy());
    expect(screen.queryByText('send_welcome')).toBeNull();
  });

  it('drops a Load more page that resolves after the filter changed', async () => {
    const inner = createMockJobsClient();
    let resolvePage: (page: Page<JobSummary>) => void = () => {};
    const client: JobsClient = {
      ...inner,
      async listJobs(n, opts) {
        if (opts.pageToken) {
          return new Promise<Page<JobSummary>>((r) => {
            resolvePage = r;
          });
        }
        const page = await inner.listJobs(n, opts);
        return { ...page, nextPageToken: opts.state ? undefined : 'page-2' };
      },
    };
    render(<JobsPage client={client} initialTab="jobs" />);
    fireEvent.click(await screen.findByRole('button', { name: 'Load more' }));
    fireEvent.change(screen.getByLabelText('State'), { target: { value: 'failed' } });
    await waitFor(() => expect(screen.getByText('export_csv')).toBeTruthy());
    const stale = first((await inner.listJobs('default', {})).items);
    resolvePage({ items: [{ ...stale, id: 'stale', name: 'stale_page_two' }] });
    await new Promise((r) => setTimeout(r, 0));
    expect(screen.queryByText('stale_page_two')).toBeNull();
    expect(screen.queryByRole('button', { name: 'Load more' })).toBeNull();
  });

  it('opens a job: payload, attempts and logs, with focus on its heading', async () => {
    render(<JobsPage client={createMockJobsClient()} initialTab="jobs" />);
    fireEvent.click(await screen.findByRole('button', { name: /Open job send_receipt/ }));
    const heading = await screen.findByRole('heading', {
      name: /send_receipt 01JA2Z6H1F0E8D7C6B5A4Z3Y2X/,
    });
    await waitFor(() => expect(document.activeElement).toBe(heading));
    expect(screen.getByLabelText('Payload preview').textContent).toBe('{}');
    const attempts = screen.getByRole('table', { name: /Attempts/ });
    expect(within(attempts).getAllByText('Retried').length).toBe(2);
    expect(screen.getByLabelText('Job logs').textContent).toContain('retry in 120 s');
    expect(screen.getByText('Last error: smtp_unavailable')).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Close job detail' }));
    expect(screen.queryByLabelText('Job logs')).toBeNull();
  });

  it('pretty-prints a JSON payload and names an object-store one', async () => {
    render(<JobsPage client={createMockJobsClient()} initialTab="jobs" />);
    fireEvent.click(await screen.findByRole('button', { name: /Open job send_welcome/ }));
    await waitFor(() =>
      expect(screen.getByLabelText('Payload preview').textContent).toContain(
        '"template": "welcome"',
      ),
    );
    expect(screen.getByLabelText('Job progress')).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: /Open job monthly_rollup/ }));
    expect(await screen.findByText(/it is not previewed here/)).toBeTruthy();
  });

  it('lists schedules', async () => {
    render(<JobsPage client={createMockJobsClient()} initialTab="schedules" />);
    const table = await screen.findByRole('table', { name: 'Cron schedules' });
    expect(within(table).getByText('0 2 * * *')).toBeTruthy();
    expect(within(table).getByText('Paused')).toBeTruthy();
  });

  it('retries a dead letter', async () => {
    const { client, calls } = recordingClient();
    render(<JobsPage client={client} initialTab="dead-letters" />);
    fireEvent.click(
      await screen.findByRole('button', { name: 'Retry send_receipt 01JA2Y9Z8X7W6V5T4S3R2Q1P0N' }),
    );
    expect(
      await screen.findByText(/Sent send_receipt \(01JA2Y9Z8X7W6V5T4S3R2Q1P0N\) back to emails/),
    ).toBeTruthy();
    expect(calls.find((c) => c.method === 'retryDeadLetter')?.args).toEqual([
      'default',
      'emails.dlq',
      '01JA2Y9Z8X7W6V5T4S3R2Q1P0N',
    ]);
    await waitFor(() =>
      expect(
        screen.queryByRole('button', { name: /Retry send_receipt 01JA2Y9Z8X7W6V5T4S3R2Q1P0N/ }),
      ).toBeNull(),
    );
  });

  it('discards a dead letter only after confirming', async () => {
    const { client, calls } = recordingClient();
    render(<JobsPage client={client} initialTab="dead-letters" />);
    fireEvent.click(
      await screen.findByRole('button', {
        name: 'Discard send_welcome 01JA2Y8M7L6K5J4H3G2F1E0D9C',
      }),
    );
    const dialog = await screen.findByRole('dialog');
    expect(calls.some((c) => c.method === 'discardDeadLetter')).toBe(false);
    fireEvent.click(within(dialog).getByRole('button', { name: 'Discard' }));
    expect(await screen.findByText(/Discarded send_welcome/)).toBeTruthy();
    expect(calls.find((c) => c.method === 'discardDeadLetter')?.args[2]).toBe(
      '01JA2Y8M7L6K5J4H3G2F1E0D9C',
    );
  });

  it('reports a failed retry', async () => {
    render(<JobsPage client={failingClient(['retryDeadLetter'])} initialTab="dead-letters" />);
    fireEvent.click(await screen.findByRole('button', { name: /Retry send_receipt/ }));
    expect(await screen.findByText(/Could not retry .*the jobs service is down/)).toBeTruthy();
  });

  it('shows per-function usage for the chosen window', async () => {
    const { client, calls } = recordingClient();
    render(<JobsPage client={client} initialTab="usage" />);
    const table = await screen.findByRole('table', { name: /Usage per function, last 24 hours/ });
    const rows = within(table).getAllByRole('row').slice(1);
    expect(within(first(rows)).getByText('send-email')).toBeTruthy();
    fireEvent.change(screen.getByLabelText('Window'), { target: { value: '1h' } });
    await screen.findByRole('table', { name: /last hour/ });
    expect(calls.filter((c) => c.method === 'functionUsage').map((c) => c.args[1])).toEqual([
      '24h',
      '1h',
    ]);
  });

  it('shows empty states in a namespace with nothing in it', async () => {
    render(<JobsPage client={createMockJobsClient()} namespace="empty-ns" />);
    expect(await screen.findByText('No queues in this namespace')).toBeTruthy();
    tab('Dead letters');
    expect(await screen.findByText('No dead letters')).toBeTruthy();
  });
});
