import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vite-plus/test';
import { AgentdSection } from '../src/agentd-section.js';
import { createMockAgentdApi, sampleSessions } from '../src/mock.js';

HTMLDialogElement.prototype.showModal ??= function (this: HTMLDialogElement) {
  this.setAttribute('open', '');
};
HTMLDialogElement.prototype.close ??= function (this: HTMLDialogElement) {
  this.removeAttribute('open');
};
// The empty state's grain canvas observes its size; jsdom has no ResizeObserver.
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};

afterEach(cleanup);

const NOW = 1_791_500_000_000;

function renderSection(opts: Parameters<typeof createMockAgentdApi>[0] = {}) {
  const mock = createMockAgentdApi({
    now: NOW,
    dataDir: '/home/u/.config/Loams Desktop/agentd',
    ...opts,
  });
  const routes: string[] = [];
  render(<AgentdSection agentd={mock.api} navigate={(p) => routes.push(p)} />);
  return { ...mock, routes };
}

describe('background agents section', () => {
  it('shows_running_state_version_and_data_dir', async () => {
    renderSection({ running: true, background: 'on' });
    await screen.findByText('Running');
    expect(screen.getByText('Background service')).toBeTruthy();
    expect(screen.getByText('0.2.1 (protocol 1)')).toBeTruthy();
    expect(screen.getByText('/home/u/.config/Loams Desktop/agentd')).toBeTruthy();
    expect(screen.getByText('pid 41234')).toBeTruthy();
  });

  it('lists_sessions_and_reattaches', async () => {
    const { calls, routes } = renderSection({ running: true, background: 'off' });
    const list = await screen.findByRole('list', { name: 'Agent sessions' });
    expect(within(list).getAllByRole('listitem')).toHaveLength(4);
    fireEvent.click(
      screen.getByRole('button', { name: 'Reattach to Refactor the ingest pipeline tests' }),
    );
    await waitFor(() => expect(routes).toEqual(['/agent/s_01hq7y']));
    expect(calls).toContain('reattach:s_01hq7y');
  });

  it('unresumable_session_cannot_reattach', async () => {
    renderSection({ running: true, background: 'off' });
    const b = await screen.findByRole('button', { name: 'Reattach to Old exploration' });
    expect((b as HTMLButtonElement).disabled).toBe(true);
  });

  it('consent_asked_once_without_default', async () => {
    const { calls } = renderSection({ background: 'ask' });
    fireEvent.click(await screen.findByRole('button', { name: 'Start background agents' }));
    const dialog = await screen.findByRole('dialog', {
      name: 'Keep agents running in the background?',
    });
    const on = within(dialog).getByRole('button', { name: 'Keep running in the background' });
    const off = within(dialog).getByRole('button', { name: 'Only while the app is open' });
    // Neither choice is focused or marked as the default.
    expect(document.activeElement).not.toBe(on);
    expect(document.activeElement).not.toBe(off);
    expect(on.className).toBe(off.className);
    expect(calls).not.toContain('start');
    fireEvent.click(off);
    await screen.findByText('Running');
    expect(calls).toEqual(['background:off', 'start']);
    expect(screen.getByText('While the app is open')).toBeTruthy();
  });

  it('closing_consent_starts_nothing', async () => {
    const { calls } = renderSection({ background: 'ask' });
    fireEvent.click(await screen.findByRole('button', { name: 'Start background agents' }));
    const dialog = await screen.findByRole('dialog');
    fireEvent.click(within(dialog).getByRole('button', { name: 'Close' }));
    expect(calls).toEqual([]);
    expect(screen.getByText('Stopped')).toBeTruthy();
  });

  it('start_with_a_choice_skips_the_dialog', async () => {
    const { calls } = renderSection({ background: 'on' });
    fireEvent.click(await screen.findByRole('button', { name: 'Start background agents' }));
    await screen.findByText('Running');
    expect(calls).toEqual(['start']);
    expect(screen.queryByRole('dialog')).toBeNull();
  });

  it('stop_needs_explicit_confirmation', async () => {
    const { calls } = renderSection({ running: true, background: 'on' });
    fireEvent.click(await screen.findByRole('button', { name: 'Stop background agents' }));
    const dialog = await screen.findByRole('dialog', { name: 'Stop background agents?' });
    expect(within(dialog).getByText(/1 running turn will be checkpointed/)).toBeTruthy();
    fireEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }));
    expect(calls).toEqual([]);
    fireEvent.click(screen.getByRole('button', { name: 'Stop background agents' }));
    fireEvent.click(
      within(await screen.findByRole('dialog', { name: 'Stop background agents?' })).getByRole(
        'button',
        { name: 'Stop agents' },
      ),
    );
    await screen.findByText('Stopped');
    expect(calls).toEqual(['stop:true']);
  });

  it('status_pushes_update_the_panel', async () => {
    const { emit } = renderSection({ running: true, background: 'off' });
    await screen.findByText('Running');
    act(() => emit({ state: 'unavailable', reason: 'protocol_mismatch' }));
    expect(screen.getByText('Unavailable')).toBeTruthy();
    expect(screen.getByText('protocol_mismatch')).toBeTruthy();
  });

  it('status_push_refetches_sessions_with_unchanged_counts', async () => {
    const sessions = sampleSessions(NOW);
    const { api, emit } = renderSection({ running: true, background: 'off', sessions });
    await screen.findByText('Refactor the ingest pipeline tests');
    const same = await api.status();
    const first = sessions[0];
    if (!first) throw new Error('fixture has sessions');
    first.title = 'Renamed while running';
    act(() => emit({ ...same }));
    await screen.findByText('Renamed while running');
  });

  it('stopped_shows_empty_sessions', async () => {
    renderSection({ background: 'off' });
    await screen.findByText('Stopped');
    expect(screen.getByText('No daemon running')).toBeTruthy();
  });
});
