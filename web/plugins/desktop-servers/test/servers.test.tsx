import type { EngineState } from '@loams/desktop/contracts';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';
import { EngineCard } from '../src/engine-card.js';
import { ServersPage } from '../src/servers-page.js';
import { Switcher } from '../src/switcher.js';
import { fakeDesktop } from './fake.js';

afterEach(cleanup);

const ready: EngineState = {
  phase: 'ready',
  url: 'http://127.0.0.1:8080',
  esUrl: 'http://127.0.0.1:9200',
  flightUrl: 'grpc://127.0.0.1:50051',
  durableUrl: 'http://127.0.0.1:8081',
  pid: 4242,
};

describe('servers page', () => {
  it('embedded_in_settings_has_no_page_header', async () => {
    const { api } = fakeDesktop();
    render(<ServersPage desktop={api} embedded />);
    await screen.findByRole('button', { name: 'Activate Demo' });
    expect(screen.queryByRole('heading', { level: 1 })).toBeNull();
  });

  it('lists_and_activates_servers', async () => {
    const { api, calls } = fakeDesktop();
    render(<ServersPage desktop={api} />);
    await screen.findByRole('button', { name: 'Activate Demo' });
    expect(screen.getByText('This computer')).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Activate Demo' }));
    await waitFor(() => expect(calls).toContain('activate:demo'));
  });

  it('add_shows_insecure_url_error', async () => {
    const { api } = fakeDesktop();
    render(<ServersPage desktop={api} />);
    await screen.findByRole('button', { name: 'Activate Demo' });
    fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'Prod' } });
    fireEvent.change(screen.getByLabelText('URL'), {
      target: { value: 'http://prod.example.com' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Add server' }));
    expect((await screen.findByText('Use https:// for a remote server.')).textContent).toBeTruthy();
    expect(screen.queryByText('Prod')).toBeNull();
  });

  it('adds and removes a remote server', async () => {
    const { api, calls } = fakeDesktop();
    render(<ServersPage desktop={api} />);
    await screen.findByRole('button', { name: 'Activate Demo' });
    fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'Staging' } });
    fireEvent.change(screen.getByLabelText('URL'), {
      target: { value: 'https://staging.example.com' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Add server' }));
    await screen.findByText('Staging');
    fireEvent.click(screen.getByRole('button', { name: 'Remove Staging' }));
    await waitFor(() => expect(screen.queryByText('Staging')).toBeNull());
    expect(calls).toContain('remove:s2');
  });
});

describe('engine card', () => {
  it('engine_card_renders_each_phase', async () => {
    const { api, emit } = fakeDesktop();
    render(<EngineCard desktop={api} />);
    await screen.findByText('Stopped');
    expect(screen.getByRole('button', { name: 'Start' })).toBeTruthy();
    act(() => emit({ phase: 'starting', attempt: 2 }));
    expect(screen.getByText('Starting')).toBeTruthy();
    expect(screen.getByText(/attempt 2/)).toBeTruthy();
    act(() => emit(ready));
    expect(screen.getByText('Ready')).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Stop' })).toBeTruthy();
    for (const label of ['HTTP', 'Elasticsearch', 'Flight']) {
      expect(screen.getByRole('button', { name: `Copy ${label} URL` })).toBeTruthy();
    }
    expect(screen.getByText('grpc://127.0.0.1:50051')).toBeTruthy();
  });

  it('copies a url', async () => {
    const { api, calls } = fakeDesktop({ engine: ready });
    render(<EngineCard desktop={api} />);
    await screen.findByText('Ready');
    fireEvent.click(screen.getByRole('button', { name: 'Copy HTTP URL' }));
    await waitFor(() => expect(calls).toContain('copy:http://127.0.0.1:8080'));
  });

  it('failed_shows_reason_and_logs_button', async () => {
    const { api, calls } = fakeDesktop({
      engine: { phase: 'failed', reason: 'port 8080 is in use', logPath: '/tmp/engine.log' },
    });
    render(<EngineCard desktop={api} />);
    expect(await screen.findByText('port 8080 is in use')).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Open logs' }));
    await waitFor(() => expect(calls).toContain('engine.openLogs'));
    fireEvent.click(screen.getByRole('button', { name: 'Start' }));
    await waitFor(() => expect(calls).toContain('engine.start'));
  });
});

describe('switcher', () => {
  it('shows the active server with the engine phase and lists the others', async () => {
    const { api, calls } = fakeDesktop({ engine: ready });
    const navigated: string[] = [];
    render(<Switcher desktop={api} navigate={(p) => navigated.push(p)} />);
    const trigger = await screen.findByRole('button', { name: /This computer/ });
    expect(within(trigger).getByTitle('Ready')).toBeTruthy();
    fireEvent.click(trigger);
    fireEvent.click(screen.getByRole('menuitem', { name: /Demo/ }));
    await waitFor(() => expect(calls).toContain('activate:demo'));
    fireEvent.click(await screen.findByRole('button', { name: /Demo/ }));
    fireEvent.click(screen.getByRole('menuitem', { name: 'Manage servers…' }));
    expect(navigated).toEqual(['/settings/servers']);
  });
});
