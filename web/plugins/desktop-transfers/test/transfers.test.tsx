import type { LoamsDesktopApi, TransferJob, TransferSnapshot } from '@loams/desktop/contracts';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vite-plus/test';
import { formatEta, formatRate, parseParams, stateLabel } from '../src/format.js';
import { TransfersPage } from '../src/TransfersPage.js';

afterEach(cleanup);

const upload: TransferJob = {
  id: 'j1',
  kind: 'upload',
  tid: 'T1',
  name: 'dataset.parquet',
  path: '/data/dataset.parquet',
  server: 'https://lab.tail:7790',
  serverId: 's1',
  ns: 'acme',
  state: 'running',
  resumed: true,
  progress: {
    tid: 'T1',
    direction: 'upload',
    phase: 'transferring',
    bytes_done: 6_000_000_000,
    bytes_total: 10_000_000_000,
    resumed_from: 4_200_000_000,
    rate_bps: 250_000_000,
    eta_s: 16,
    transport: 'h3',
    inflight: 8,
    retries: 2,
  },
  error: null,
  log: [],
};

function fake(jobs: TransferJob[], hasToken = true) {
  const calls: string[] = [];
  const snap: TransferSnapshot = { agent: { phase: 'ready', version: '0.1.3' }, jobs };
  const ok = { ok: true as const, value: undefined };
  const api = {
    servers: {
      list: async () => ({
        servers: [
          { id: 's1', name: 'Lab', kind: 'remote' as const, url: 'https://lab.tail:7790' },
          { id: 'demo', name: 'Demo', kind: 'demo' as const, url: 'http://127.0.0.1:8090' },
        ],
        activeId: 's1',
      }),
    },
    transfers: {
      snapshot: async () => snap,
      onChange: () => () => {},
      hasKey: async () => hasToken,
      saveKey: async (id: string, token: string) => {
        calls.push(`saveKey:${id}:${token.length}`);
        return ok;
      },
      upload: async () => {
        calls.push('upload');
        return { ok: true as const, value: null };
      },
      download: async (_t: unknown, tid: string) => {
        calls.push(`download:${tid}`);
        return { ok: true as const, value: null };
      },
      pause: async (id: string) => {
        calls.push(`pause:${id}`);
        return ok;
      },
      resume: async (id: string) => {
        calls.push(`resume:${id}`);
        return { ok: true as const, value: upload };
      },
      cancel: async (id: string) => {
        calls.push(`cancel:${id}`);
        return ok;
      },
      dismiss: async () => {},
      remoteList: async () => ({
        ok: true as const,
        value: {
          transfers: [
            {
              tid: 'T9',
              state: 'complete' as const,
              committed_chunks: 4,
              committed_bytes: 64_000_000,
            },
          ],
          runs: [{ rid: 'R1', state: 'running', job: 'ci' }],
        },
      }),
      startRun: async (_t: unknown, job: string, params: Record<string, string>) => {
        calls.push(`run:${job}:${JSON.stringify(params)}`);
        return { ok: true as const, value: upload };
      },
      followRun: async (_t: unknown, rid: string) => {
        calls.push(`follow:${rid}`);
        return { ok: true as const, value: upload };
      },
    },
  } as unknown as LoamsDesktopApi;
  return { api, calls };
}

describe('format', () => {
  it('eta_and_rate', () => {
    expect(formatRate(0)).toBe('—');
    expect(formatRate(250_000_000)).toBe('250 MB/s');
    expect(formatEta(undefined)).toBe('—');
    expect(formatEta(45)).toBe('45 s');
    expect(formatEta(245)).toBe('4 min 05 s');
    expect(formatEta(7800)).toBe('2 h 10 min');
  });

  it('labels_the_resume_state', () => {
    expect(stateLabel(upload)).toBe('Resumed from 4.2 GB');
    expect(stateLabel({ ...upload, state: 'paused' })).toBe('Paused, resumable from 6.0 GB');
    expect(
      stateLabel({ ...upload, state: 'failed', error: { code: 'lease_held', message: '' } }),
    ).toBe('Another device holds this transfer');
  });

  it('parses_params', () => {
    expect(parseParams('REF=main\n\nSHA=abc=d')).toEqual({ params: { REF: 'main', SHA: 'abc=d' } });
    expect(parseParams('lower=x').error).toBeTruthy();
  });
});

describe('transfers page', () => {
  it('page_shows_resume_state_rate_eta_and_transport', async () => {
    const { api, calls } = fake([upload]);
    render(<TransfersPage desktop={api} />);
    await screen.findByText('dataset.parquet');
    expect(screen.getByText('Resumed from 4.2 GB')).toBeTruthy();
    expect(screen.getByText('250 MB/s')).toBeTruthy();
    expect(screen.getByText('16 s')).toBeTruthy();
    expect(screen.getByText('HTTP/3')).toBeTruthy();
    expect(screen.getByText('8 chunks, 2 retries')).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Pause' }));
    await waitFor(() => expect(calls).toContain('pause:j1'));
  });

  it('asks_for_a_key_before_transfers', async () => {
    const { api, calls } = fake([], false);
    render(<TransfersPage desktop={api} />);
    const key = await screen.findByLabelText('API key');
    fireEvent.change(key, { target: { value: 'secret-key-123' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save key' }));
    await waitFor(() => expect(calls).toContain('saveKey:s1:14'));
    await screen.findByRole('button', { name: 'Upload a file…' });
  });

  it('lists_server_transfers_and_runs', async () => {
    const { api, calls } = fake([]);
    render(<TransfersPage desktop={api} />);
    fireEvent.click(await screen.findByRole('button', { name: 'Download…' }));
    await waitFor(() => expect(calls).toContain('download:T9'));
    fireEvent.click(screen.getByRole('button', { name: 'Follow log' }));
    await waitFor(() => expect(calls).toContain('follow:R1'));
    fireEvent.click(screen.getByRole('button', { name: 'Start run' }));
    await waitFor(() => expect(calls).toContain('run:ci:{}'));
  });

  it('log_view_follows_offset', async () => {
    const run: TransferJob = {
      ...upload,
      id: 'r1',
      kind: 'run',
      name: 'ci',
      progress: null,
      log: ['compiling', 'tests passed'],
      logOffset: 2048,
    };
    const { api } = fake([run]);
    render(<TransfersPage desktop={api} />);
    const log = await screen.findByLabelText('Run log');
    expect(log.textContent).toBe('compiling\ntests passed');
    expect(screen.getByText(/2\.0 KB received/)).toBeTruthy();
  });

  it('says_when_the_build_has_no_sidecar', async () => {
    const api = {
      servers: { list: async () => ({ servers: [], activeId: '' }) },
    } as unknown as LoamsDesktopApi;
    render(<TransfersPage desktop={api} />);
    expect(await screen.findByText('Transfers need a newer desktop build')).toBeTruthy();
  });
});
