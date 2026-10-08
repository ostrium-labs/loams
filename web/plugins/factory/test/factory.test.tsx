import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';
import { ConfigurePage } from '../src/configure.js';
import { FactoryHome } from '../src/home.js';
import { PanelsPage } from '../src/panels-page.js';
import { SummaryCard } from '../src/summary-card.js';
import { fakeDesktop, IDS, info } from './fake.js';

afterEach(cleanup);

// The empty state's grain canvas observes its size; jsdom has no ResizeObserver.
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};
const nav = () => {
  const to: string[] = [];
  return { to, navigate: (p: string) => void to.push(p) };
};

describe('factory home', () => {
  it('home_renders_eight_tiles_with_health', async () => {
    const apps = IDS.map((id) =>
      info(id, id === 'forgejo' ? 'ok' : id === 'zulip' ? 'auth_failed' : 'unconfigured'),
    );
    const { api } = fakeDesktop({ apps });
    render(<FactoryHome desktop={api} navigate={nav().navigate} />);
    const tiles = await screen.findAllByRole('article');
    expect(tiles).toHaveLength(8);
    expect(within(tiles[0] as HTMLElement).getByText('Forgejo')).toBeTruthy();
    expect(within(tiles[0] as HTMLElement).getByText('Repos, PRs, CI')).toBeTruthy();
    expect(within(tiles[0] as HTMLElement).getByText('Connected')).toBeTruthy();
    expect(within(tiles[0] as HTMLElement).getByText('https://forgejo.example.test')).toBeTruthy();
    expect(within(tiles[1] as HTMLElement).getByText('Auth failed')).toBeTruthy();
    expect(within(tiles[2] as HTMLElement).getByText('Not configured')).toBeTruthy();
    expect(screen.getByText('Plane (ItsAPlan)')).toBeTruthy();
  });

  it('session_only_banner', async () => {
    const apps = IDS.map((id) => info(id, 'unconfigured', false));
    const { api } = fakeDesktop({ apps });
    render(<FactoryHome desktop={api} navigate={nav().navigate} />);
    expect(
      await screen.findByText(
        /Credentials are kept for this session only: no system keychain found\./,
      ),
    ).toBeTruthy();
  });

  it('no_banner_when_persistent', async () => {
    const { api } = fakeDesktop();
    render(<FactoryHome desktop={api} navigate={nav().navigate} />);
    await screen.findAllByRole('article');
    expect(screen.queryByText(/session only/)).toBeNull();
  });

  it('open_app_error_is_inline_and_buttons_navigate', async () => {
    const n = nav();
    const apps = IDS.map((id) => info(id, id === 'forgejo' ? 'ok' : 'unconfigured'));
    const { api, calls } = fakeDesktop({ apps });
    render(<FactoryHome desktop={api} navigate={n.navigate} />);
    const tile = (await screen.findAllByRole('article'))[0] as HTMLElement;
    fireEvent.click(within(tile).getByRole('button', { name: 'Open app' }));
    expect(await within(tile).findByText('Opening apps is not available yet.')).toBeTruthy();
    expect(calls).toContain('open:forgejo');
    fireEvent.click(within(tile).getByRole('button', { name: 'Configure' }));
    fireEvent.click(within(tile).getByRole('button', { name: 'Panels' }));
    expect(n.to).toEqual(['/factory/forgejo/configure', '/factory/forgejo']);
  });
});

describe('configure', () => {
  it('configure_never_prefills_secret', async () => {
    const { api } = fakeDesktop({ apps: IDS.map((id) => info(id, 'ok')) });
    render(<ConfigurePage desktop={api} app="forgejo" navigate={nav().navigate} />);
    const token = (await screen.findByLabelText('Access token')) as HTMLInputElement;
    expect(token.type).toBe('password');
    expect(token.value).toBe('');
    expect((screen.getByLabelText('SSO origin (optional)') as HTMLInputElement).type).toBe('text');
    expect((screen.getByLabelText('URL') as HTMLInputElement).value).toBe(
      'https://forgejo.example.test',
    );
  });

  it('save_configures_then_tests_and_shows_health', async () => {
    const { api, calls, configured } = fakeDesktop();
    render(<ConfigurePage desktop={api} app="zulip" navigate={nav().navigate} />);
    fireEvent.change(await screen.findByLabelText('URL'), {
      target: { value: 'https://chat.example.test' },
    });
    fireEvent.change(screen.getByLabelText('Email'), { target: { value: 'bot@example.test' } });
    fireEvent.change(screen.getByLabelText('API key'), { target: { value: 'not-a-real-key' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save and test' }));
    expect(await screen.findByText('Connected')).toBeTruthy();
    expect(calls).toEqual(['configure:zulip', 'test:zulip']);
    expect(configured[0]).toEqual({
      app: 'zulip',
      url: 'https://chat.example.test',
      fields: { email: 'bot@example.test', apiKey: 'not-a-real-key' },
    });
    expect((screen.getByLabelText('API key') as HTMLInputElement).value).toBe('');
  });

  it('configure_error_is_shown_and_test_skipped', async () => {
    const { api, calls } = fakeDesktop({
      configure: { ok: false, code: 'invalid_url', message: 'Use https://.' },
    });
    render(<ConfigurePage desktop={api} app="forgejo" navigate={nav().navigate} />);
    fireEvent.change(await screen.findByLabelText('URL'), { target: { value: 'ftp://x' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save and test' }));
    expect(await screen.findByText('Use https://.')).toBeTruthy();
    expect(calls).toEqual(['configure:forgejo']);
  });
});

const ok = (id: Parameters<typeof info>[0]) =>
  IDS.map((i) => info(i, i === id ? 'ok' : 'unconfigured'));

describe('panels', () => {
  it('forgejo_panel_renders_prs', async () => {
    const { api, queries } = fakeDesktop({
      apps: ok('forgejo'),
      queries: {
        'forgejo.repos': [
          { fullName: 'demo/loams', description: 'Demo repo', stars: 3, forks: 1, openIssues: 4 },
        ],
        'forgejo.version': { version: '11.0.0-demo' },
        'forgejo.issues': (q: { params: Record<string, unknown> }) => ({
          ok: true,
          value:
            q.params['type'] === 'pulls'
              ? [{ id: '#7', title: 'Add demo feature', state: 'open' }]
              : [{ id: '#2', title: 'Demo issue', state: 'open' }],
        }),
      },
    });
    render(<PanelsPage desktop={api} app="forgejo" navigate={nav().navigate} />);
    expect(await screen.findByText('Add demo feature')).toBeTruthy();
    expect(screen.getByText('Demo issue')).toBeTruthy();
    expect(screen.getAllByText('demo/loams').length).toBeGreaterThan(0);
    expect(screen.getByText('11.0.0-demo')).toBeTruthy();
    expect(queries).toContainEqual({
      app: 'forgejo',
      op: 'issues',
      params: { type: 'pulls', owner: 'demo', repo: 'loams', limit: 20 },
    });
  });

  it('glitchtip_panel_renders_issues', async () => {
    const { api } = fakeDesktop({
      apps: ok('glitchtip'),
      queries: {
        'glitchtip.organizations': [{ slug: 'demo-org', name: 'Demo Org' }],
        'glitchtip.issues': [{ id: '1', title: 'TypeError in demo', level: 'error', count: '12' }],
      },
    });
    render(<PanelsPage desktop={api} app="glitchtip" navigate={nav().navigate} />);
    expect(await screen.findByText('TypeError in demo')).toBeTruthy();
  });

  it('matomo_panel_renders_visits_and_pages', async () => {
    const { api } = fakeDesktop({
      apps: ok('matomo'),
      queries: {
        'matomo.visits': [{ date: '2026-10-07', nb_visits: 42 }],
        'matomo.pages': [{ label: '/demo', hits: 9, visits: 7 }],
      },
    });
    render(<PanelsPage desktop={api} app="matomo" navigate={nav().navigate} />);
    expect(await screen.findByText('/demo')).toBeTruthy();
    expect(screen.getByText('2026-10-07')).toBeTruthy();
  });

  it('plane_zulip_openpanel_langfuse_render', async () => {
    const cases = [
      [
        'plane',
        {
          'plane.stats': { total: 5 },
          'plane.issues': [{ id: 'DEMO-1', title: 'Plane demo issue' }],
        },
        'Plane demo issue',
      ],
      [
        'zulip',
        {
          'zulip.streams': [{ id: 1, name: 'demo-stream', private: false }],
          'zulip.server': { name: 'Demo Bot' },
          'zulip.messages': [{ id: 1, sender: 'Demo', topic: 't', content: 'hello zulip' }],
        },
        'hello zulip',
      ],
      [
        'openpanel',
        {
          'openpanel.insights': {
            summary: { sessions: 3 },
            series: [],
            topPages: [{ path: '/demo-page', sessions: 1, pageviews: 2 }],
          },
        },
        '/demo-page',
      ],
      [
        'langfuse',
        {
          'langfuse.traces': [{ id: 'o1', name: 'demo-trace' }],
          'langfuse.daily': { available: false, days: [] },
        },
        'demo-trace',
      ],
    ] as const;
    for (const [app, queries, text] of cases) {
      const { api } = fakeDesktop({ apps: ok(app), queries });
      render(<PanelsPage desktop={api} app={app} navigate={nav().navigate} />);
      expect(await screen.findByText(text)).toBeTruthy();
      cleanup();
    }
  });

  it('openobserve_is_full_ui_only', async () => {
    const { api, queries } = fakeDesktop({
      apps: IDS.map((i) => info(i, i === 'openobserve' ? 'ok' : 'unconfigured')),
    });
    render(<PanelsPage desktop={api} app="openobserve" navigate={nav().navigate} />);
    expect(await screen.findByText('Full UI only')).toBeTruthy();
    expect(queries).toHaveLength(0);
  });

  it('auth_failed_links_to_configure', async () => {
    // Handlers return a raw IpcResult.
    const bad = { ok: false, code: 'auth_failed', message: 'Token rejected' } as const;
    const d2 = fakeDesktop({
      apps: ok('forgejo'),
      queries: {
        'forgejo.repos': () => bad,
        'forgejo.version': () => bad,
        'forgejo.issues': () => bad,
      },
    });
    render(<PanelsPage desktop={d2.api} app="forgejo" navigate={nav().navigate} />);
    const links = await screen.findAllByRole('link', { name: 'Update credentials' });
    expect(links[0]?.getAttribute('href')).toBe('#/factory/forgejo/configure');
    expect(screen.getAllByText('Token rejected').length).toBeGreaterThan(0);
  });

  it('refresh_reruns_the_query', async () => {
    let n = 0;
    const { api } = fakeDesktop({
      apps: ok('matomo'),
      queries: {
        'matomo.visits': () => ({ ok: true, value: [{ date: `d${++n}`, nb_visits: 1 }] }),
        'matomo.pages': [],
      },
    });
    render(<PanelsPage desktop={api} app="matomo" navigate={nav().navigate} />);
    await screen.findByText('d1');
    fireEvent.click(screen.getByRole('button', { name: 'Refresh Visits' }));
    await waitFor(() => expect(screen.getByText('d2')).toBeTruthy());
  });

  it('unconfigured_app_points_to_configure', async () => {
    const { api, queries } = fakeDesktop();
    render(<PanelsPage desktop={api} app="forgejo" navigate={nav().navigate} />);
    expect(await screen.findByText('Forgejo is not configured')).toBeTruthy();
    expect(queries).toHaveLength(0);
  });
});

describe('overview summary', () => {
  it('summarises_configured_apps_and_hides_when_none', async () => {
    const { api } = fakeDesktop({
      apps: IDS.map((i) => info(i, i === 'forgejo' || i === 'glitchtip' ? 'ok' : 'unconfigured')),
      queries: {
        'forgejo.repos': [{ fullName: 'demo/loams', openIssues: 4 }],
        'forgejo.issues': [{ id: '#7' }, { id: '#8' }],
        'glitchtip.organizations': [{ slug: 'demo-org' }],
        'glitchtip.issues': [{ id: '1' }, { id: '2' }, { id: '3' }],
      },
    });
    const { container } = render(<SummaryCard desktop={api} />);
    expect(await screen.findByText('Open PRs')).toBeTruthy();
    await waitFor(() => expect(container.textContent).toContain('Unresolved errors3'));
    expect(container.textContent).toContain('Open PRs2');
    expect(container.textContent).toContain('Open issues4');
    cleanup();
    const none = fakeDesktop();
    const r = render(<SummaryCard desktop={none.api} />);
    await waitFor(() => expect(none.queries).toHaveLength(0));
    expect(r.container.textContent).toBe('');
  });
});
