// The Git page against the fixture-backed mock (src/mock.ts), with no latency and no
// search debounce. The mock stands in for loams.repos.v1 until it is served; when the
// real client lands, these tests keep running against the same GitClient interface.

import { validateManifest } from '@loams/console-host';
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vite-plus/test';
import pkg from '../package.json';
import type { GitClient } from '../src/client.js';
import { keyStatusText } from '../src/EnvView.js';
import { GitPage } from '../src/GitPage.js';
import {
  createMockGitClient,
  MONOREPO_WORKSPACES,
  SEARCH_SCAN_WINDOW,
  wsIdAt,
} from '../src/mock.js';
import { visibleRange } from '../src/VirtualList.js';
import { AUTO_SCAN_PAGES } from '../src/WorkspaceList.js';

afterEach(cleanup);

globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};

const MONOREPO = 'r01j9x0monorepo00000000000a';
const ref = { namespace: 'default', repoId: MONOREPO };

function mount(client: GitClient = createMockGitClient({ latencyMs: 0 })) {
  render(<GitPage client={client} preview listHeight={440} debounceMs={0} />);
  return client;
}

const listbox = () => screen.findByRole('listbox', { name: 'Workspaces' });

describe('mock client', () => {
  const client = createMockGitClient({ latencyMs: 0 });

  it('pages_a_million_workspaces_without_materialising_them', async () => {
    const first = await client.listWorkspaces({ ...ref, pageSize: 100 });
    expect(first.totalSize).toBe(MONOREPO_WORKSPACES);
    expect(first.items).toHaveLength(100);
    expect(first.nextPageToken).toBe('o:100');
    const last = await client.listWorkspaces({
      ...ref,
      pageToken: `o:${MONOREPO_WORKSPACES - 10}`,
    });
    expect(last.items).toHaveLength(10);
    expect(last.nextPageToken).toBe('');
    expect(last.items.at(-1)!.wsId).toBe(wsIdAt(MONOREPO_WORKSPACES - 1));
  });

  it('workspace_ids_are_w_plus_26_characters', () => {
    expect(wsIdAt(0)).toMatch(/^w[0-9a-z]{26}$/);
    expect(wsIdAt(999_999)).toMatch(/^w[0-9a-z]{26}$/);
  });

  it('search_scans_in_bounded_windows_and_continues', async () => {
    const res = await client.listWorkspaces({ ...ref, query: 'no-such-owner' });
    expect(res.items).toHaveLength(0);
    expect(res.nextPageToken).toBe(`o:${SEARCH_SCAN_WINDOW}`);
    const hit = await client.listWorkspaces({ ...ref, query: 'grace/feature-', pageSize: 5 });
    expect(hit.items).toHaveLength(5);
    for (const w of hit.items) expect(w.name).toContain('grace/feature-');
  });

  it('env_listing_never_carries_values', async () => {
    const keys = await client.listEnvKeys(ref);
    const json = JSON.stringify(keys);
    expect(json).not.toMatch(/"value"/);
    for (const k of keys) expect(k.fingerprint).toMatch(/^[0-9a-f]{8}$/);
    const file = await client.readFile({ ...ref, wsId: wsIdAt(0), path: 'env/main.env' });
    expect(file.text).toMatch(/^DATABASE_URL=<redacted fp=[0-9a-f]{8}>$/m);
    expect(file.text).not.toMatch(/postgres:\/\//);
  });

  it('aborted_calls_reject_with_abort_error', async () => {
    const slow = createMockGitClient({ latencyMs: 50 });
    const ac = new AbortController();
    const p = slow.listRepos({ namespace: 'default' }, ac.signal);
    ac.abort();
    await expect(p).rejects.toMatchObject({ name: 'AbortError' });
  });

  it('unknown_workspace_is_not_found', async () => {
    await expect(client.getWorkspace({ ...ref, wsId: 'wnope' })).rejects.toMatchObject({
      code: 'not_found',
    });
  });
});

describe('VirtualList window', () => {
  it('renders_only_rows_in_view_plus_overscan', () => {
    expect(visibleRange(0, 440, 44, 1_000_000, 6)).toEqual([0, 16]);
    expect(visibleRange(44_000, 440, 44, 1_000_000, 6)).toEqual([994, 1016]);
    expect(visibleRange(0, 440, 44, 3, 6)).toEqual([0, 3]);
  });
});

describe('GitPage', () => {
  it('lists_repositories_and_opens_the_first', async () => {
    mount();
    const nav = await screen.findByRole('navigation', { name: 'Repositories' });
    const mono = await within(nav).findByRole('button', { name: /monorepo/ });
    await waitFor(() => expect(mono.getAttribute('aria-current')).toBe('true'));
    // UX-5: the shared notice only, no second badge in the page head.
    expect(screen.getAllByText('Sample data').length).toBe(1);
    const lb = await listbox();
    await waitFor(() => expect(within(lb).getAllByRole('option').length).toBeGreaterThan(0));
    // Only a window of the million rows is in the DOM.
    expect(within(lb).getAllByRole('option').length).toBeLessThanOrEqual(20);
    expect(within(lb).getAllByRole('option')[0]!.getAttribute('aria-setsize')).toBe(
      String(MONOREPO_WORKSPACES),
    );
  });

  it('keyboard_moves_the_active_row_and_enter_opens_it', async () => {
    mount();
    const lb = await listbox();
    await waitFor(() => expect(within(lb).getAllByRole('option').length).toBeGreaterThan(0));
    lb.focus();
    fireEvent.keyDown(lb, { key: 'ArrowDown' });
    fireEvent.keyDown(lb, { key: 'ArrowDown' });
    const active = lb.getAttribute('aria-activedescendant')!;
    expect(document.getElementById(active)!.getAttribute('aria-posinset')).toBe('3');
    fireEvent.keyDown(lb, { key: 'Enter' });
    expect(await screen.findByText('Base commit')).toBeTruthy();
    // The id shows in the list row, the title and the detail list.
    expect(screen.getAllByText(wsIdAt(2)).length).toBeGreaterThan(1);
  });

  it('search_is_sent_to_the_server', async () => {
    const client = createMockGitClient({ latencyMs: 0 });
    const spy = vi.spyOn(client, 'listWorkspaces');
    mount(client);
    await listbox();
    fireEvent.change(screen.getByLabelText('Search workspaces'), {
      target: { value: 'ada/feature-' },
    });
    await waitFor(() =>
      expect(spy).toHaveBeenCalledWith(
        expect.objectContaining({ query: 'ada/feature-', pageToken: '' }),
        expect.anything(),
      ),
    );
    const lb = await listbox();
    await waitFor(() => {
      const opts = within(lb).getAllByRole('option');
      expect(opts.length).toBeGreaterThan(0);
      for (const o of opts) expect(o.textContent).toContain('ada/feature-');
    });
  });

  it('sparse_search_stops_scanning_until_asked', async () => {
    const client = createMockGitClient({ latencyMs: 0 });
    const spy = vi.spyOn(client, 'listWorkspaces');
    mount(client);
    await listbox();
    fireEvent.change(screen.getByLabelText('Search workspaces'), {
      target: { value: 'no-such-owner' },
    });
    const searches = () => spy.mock.calls.filter(([r]) => r.query === 'no-such-owner');
    // The first page plus AUTO_SCAN_PAGES continuations, then it waits for the person.
    // The debounced search plus its continuations outlast findBy's 1 s default
    // on a loaded CI runner.
    const more = await screen.findByRole('button', { name: 'Keep searching' }, { timeout: 5000 });
    expect(searches()).toHaveLength(1 + AUTO_SCAN_PAGES);
    expect(searches().at(-1)?.[0].pageToken).toBe(`o:${AUTO_SCAN_PAGES * SEARCH_SCAN_WINDOW}`);
    fireEvent.click(more);
    await waitFor(() => expect(searches()).toHaveLength(1 + 2 * AUTO_SCAN_PAGES), {
      timeout: 5000,
    });
    expect(
      await screen.findByRole('button', { name: 'Keep searching' }, { timeout: 5000 }),
    ).toBeTruthy();
  });

  it('workspace_detail_shows_changes_snapshots_and_lazy_files', async () => {
    const client = createMockGitClient({ latencyMs: 0 });
    const tree = vi.spyOn(client, 'getTree');
    const read = vi.spyOn(client, 'readFile');
    mount(client);
    const lb = await listbox();
    await waitFor(() => expect(within(lb).getAllByRole('option').length).toBeGreaterThan(3));
    fireEvent.click(within(lb).getAllByRole('option')[3]!);
    await screen.findByText('Base commit');

    // Changes: workspace 3 has 21 changed paths.
    expect(screen.getByRole('tab', { name: /Changes \(21\)/ }).getAttribute('aria-selected')).toBe(
      'true',
    );
    expect(screen.getByRole('table')).toBeTruthy();

    // Snapshots: latest first, with a takeover marked.
    fireEvent.click(screen.getByRole('tab', { name: 'Snapshots' }));
    const snaps = await screen.findByRole('region', { name: 'Snapshots' });
    const items = await within(snaps).findAllByRole('listitem');
    expect(items[0]!.textContent).toContain('@4');
    expect(within(snaps).getByText('takeover')).toBeTruthy();

    // Browse at a snapshot: the root loads, nothing else until asked.
    fireEvent.click(within(snaps).getByRole('button', { name: 'Browse files at snapshot 2' }));
    const t = await screen.findByRole('tree');
    expect(tree).toHaveBeenCalledTimes(1);
    expect(tree).toHaveBeenLastCalledWith(
      expect.objectContaining({ n: 2, path: '' }),
      expect.anything(),
    );
    expect(read).not.toHaveBeenCalled();

    // Expanding a directory with the keyboard loads only that directory.
    const env = within(t).getByRole('treeitem', { name: /^env\// });
    env.focus();
    fireEvent.keyDown(env, { key: 'ArrowRight' });
    await waitFor(() => expect(env.getAttribute('aria-expanded')).toBe('true'));
    expect(tree).toHaveBeenLastCalledWith(
      expect.objectContaining({ path: 'env' }),
      expect.anything(),
    );

    // Workspace 3 deletes env/main.env: opening it says so instead of showing contents.
    fireEvent.click(await within(t).findByRole('treeitem', { name: /main\.env.*deleted/ }));
    await waitFor(() => expect(read).toHaveBeenCalledTimes(1));
    expect((await screen.findByRole('alert')).textContent).toContain('deleted in this workspace');
    expect(screen.queryByLabelText('File contents')).toBeNull();

    // Opening an env file it keeps (dev.env, added) reads it, redacted.
    fireEvent.click(within(t).getByRole('treeitem', { name: /dev\.env/ }));
    await waitFor(() => expect(read).toHaveBeenCalledTimes(2));
    const pre = await screen.findByLabelText('File contents');
    expect(pre.textContent).toMatch(/DATABASE_URL=<redacted fp=[0-9a-f]{8}>/);
  });

  it('overflowing_overlay_says_so', async () => {
    mount();
    const lb = await listbox();
    await waitFor(() => expect(within(lb).getAllByRole('option').length).toBeGreaterThan(7));
    fireEvent.click(within(lb).getAllByRole('option')[7]!);
    expect(await screen.findByText('Too many changed paths to list')).toBeTruthy();
  });

  it('env_view_shows_keys_status_rotation_and_audit_without_values', async () => {
    mount();
    fireEvent.click(await screen.findByRole('tab', { name: 'Env secrets' }));
    const keys = await screen.findByRole('table', { name: /Env keys/ });
    const row = within(keys).getByRole('row', { name: /DATABASE_URL/ });
    expect(row.textContent).toContain('rotated: pinned v5, live v6');
    expect(row.textContent).toContain('loams-postgres');
    expect(row.textContent).toMatch(/<redacted fp=[0-9a-f]{8}>/);
    const stripe = within(keys).getByRole('row', { name: /STRIPE_KEY/ });
    expect(stripe.textContent).toContain('pending promotion');
    const audit = await screen.findByRole('table', { name: /audit events/ });
    await waitFor(() => expect(within(audit).getAllByRole('row').length).toBeGreaterThan(1));

    fireEvent.change(screen.getByLabelText('Environment'), { target: { value: 'dev' } });
    await waitFor(() => {
      const rows = within(audit).getAllByRole('row').slice(1);
      expect(rows.length).toBeGreaterThan(0);
      for (const r of rows) expect(r.textContent).toContain('dev');
    });
  });

  it('repository_without_env_says_so', async () => {
    mount();
    fireEvent.click(await screen.findByRole('button', { name: /docs-site/ }));
    fireEvent.click(await screen.findByRole('tab', { name: 'Env secrets' }));
    expect(await screen.findByText(/has no env\/ folder/)).toBeTruthy();
    fireEvent.click(screen.getByRole('tab', { name: 'Workspaces' }));
    expect(await screen.findByText('No workspaces in this repository yet.')).toBeTruthy();
  });

  it('tabs_move_with_arrow_keys', async () => {
    mount();
    const tab = await screen.findByRole('tab', { name: 'Workspaces' });
    tab.focus();
    fireEvent.keyDown(tab, { key: 'ArrowRight' });
    expect(screen.getByRole('tab', { name: 'Env secrets' }).getAttribute('aria-selected')).toBe(
      'true',
    );
  });
});

describe('env helpers', () => {
  it('key_status_text', () => {
    expect(keyStatusText({ status: 'live', pinnedVersion: 2, liveVersion: 2 })).toBe('live v2');
    expect(keyStatusText({ status: 'absent' })).toBe('not set');
    expect(keyStatusText({ status: 'pending', pinnedVersion: 3, liveVersion: 2 })).toBe(
      'pending promotion: v3 (live v2)',
    );
  });
});

it('manifest_is_valid', () => {
  const m = validateManifest(pkg);
  expect(m.inject).toContain('transport');
  expect(m.slots).toContain('console.page');
});
