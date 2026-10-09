import type { LoamsDesktopApi, StackId, StackState } from '@loams/desktop/contracts';
import { SlotProvider, SlotRegistry } from '@loams/slots';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';
import { AboutSection } from '../src/about.js';
import plugin from '../src/index.js';
import { SettingsPage } from '../src/settings-page.js';
import { StacksSection } from '../src/stacks.js';
import { UpdatesSection } from '../src/updates.js';

// <Empty> draws a canvas grain that observes its size.
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};

afterEach(cleanup);

type Update = Awaited<ReturnType<LoamsDesktopApi['update']['state']>>;

function fake(
  init: {
    stacks?: Partial<Record<StackId, StackState>>;
    update?: Update;
    liveStore?: 'embedded' | 'tikv-stack';
    liveNotice?: string;
  } = {},
) {
  const calls: string[] = [];
  const stacks: Record<string, StackState> = {
    postgres: { phase: 'stopped' },
    wesql: { phase: 'stopped' },
    tikv: { phase: 'stopped' },
    ...init.stacks,
  };
  let update: Update = init.update ?? { phase: 'idle' };
  const ok = { ok: true, value: undefined } as const;
  const api = {
    version: '1.2.3',
    platform: 'linux',
    engine: {
      openLogs: async () => void calls.push('openLogs'),
      liveStore: async () => init.liveStore ?? 'embedded',
      setLiveStore: async (c: string) => {
        calls.push(`setLiveStore:${c}`);
        return ok;
      },
      state: async () =>
        init.liveNotice
          ? {
              phase: 'ready',
              url: 'http://127.0.0.1:1',
              esUrl: '',
              flightUrl: '',
              durableUrl: '',
              liveUrl: 'http://127.0.0.1:2',
              liveNotice: init.liveNotice,
              pid: 1,
            }
          : { phase: 'stopped' },
      onState: () => () => undefined,
    },
    stacks: {
      state: async (id: string) => stacks[id],
      start: async (id: string) => {
        calls.push(`start:${id}`);
        return id === 'wesql' ? { ok: false, code: 'x', message: 'image pull failed' } : ok;
      },
      stop: async (id: string) => {
        calls.push(`stop:${id}`);
        return ok;
      },
      onState: () => () => undefined,
    },
    update: {
      state: async () => update,
      check: async () => {
        calls.push('check');
        update = { phase: 'available', version: '2.0.0', mode: 'manual' };
      },
      download: async () => void calls.push('download'),
      installAndRestart: async () => void calls.push('install'),
    },
  } as unknown as LoamsDesktopApi;
  return { api, calls };
}

describe('settings area', () => {
  const registry = () => {
    const r = new SlotRegistry();
    const reg = (id: string, label: string, order: number, text: string) =>
      r.register(
        { name: 'console.settings.section', plugin: 't', order, meta: { id, label } },
        () => <p>{text}</p>,
      );
    return { r, reg };
  };

  it('lists_sections_in_order_and_shows_the_chosen_one', () => {
    const { r, reg } = registry();
    reg('about', 'About', 40, 'about body');
    reg('servers', 'Servers', 10, 'servers body');
    const view = (section?: string) => (
      <SlotProvider registry={r}>
        <SettingsPage section={section} />
      </SlotProvider>
    );
    const { rerender } = render(view());
    const links = within(
      screen.getByRole('navigation', { name: 'Settings sections' }),
    ).getAllByRole('link');
    expect(links.map((l) => l.textContent)).toEqual(['Servers', 'About']);
    expect(screen.getByText('servers body')).toBeTruthy(); // no section: the first
    rerender(view('about'));
    expect(screen.getByText('about body')).toBeTruthy();
    expect(links[1]?.getAttribute('href')).toBe('#/settings/about');
    rerender(view('nope'));
    expect(screen.getByText('No such section')).toBeTruthy();
  });

  it('a_later_plugin_adds_its_section_through_the_slot', () => {
    const { r, reg } = registry();
    reg('servers', 'Servers', 10, 's');
    render(
      <SlotProvider registry={r}>
        <SettingsPage section="providers" />
      </SlotProvider>,
    );
    expect(screen.getByText('No such section')).toBeTruthy();
    act(() => void reg('providers', 'Agent providers', 15, 'providers body'));
    expect(screen.getByText('providers body')).toBeTruthy();
    expect(screen.getByRole('link', { name: 'Agent providers' })).toBeTruthy();
  });

  it('plugin_registers_stacks_updates_about_and_the_settings_nav', () => {
    const slots = new SlotRegistry();
    const registered: unknown[] = [];
    const ctx = {
      effect: (fn: () => unknown) => void registered.push(fn()),
    };
    const services = {
      desktop: fake().api,
      slots,
      router: { page: () => () => undefined },
    } as Record<string, unknown>;
    (ctx as Record<string, unknown>).root = {};
    // `service(ctx, name)` reads ctx[name]; the fake exposes the services directly.
    plugin.apply(Object.assign(ctx, services) as never, {});
    expect(slots.entries('console.settings.section').map((e) => e.meta?.id)).toEqual([
      'stacks',
      'updates',
      'about',
    ]);
    expect(slots.entries('shell.nav.section')[0]?.meta?.href).toBe('/settings');
  });
});

describe('local stacks section', () => {
  it('says_nothing_about_the_runtime_until_all_three_are_read', async () => {
    const un: StackState = { phase: 'unavailable', reason: 'no_container_runtime' };
    const { api } = fake({ stacks: { postgres: un, wesql: un, tikv: un } });
    let release: (s: StackState) => void = () => undefined;
    const slow = new Promise<StackState>((r) => {
      release = r;
    });
    const state = api.stacks.state.bind(api.stacks);
    api.stacks.state = (id) => (id === 'tikv' ? slow : state(id));
    render(<StacksSection desktop={api} />);
    await screen.findAllByText('Unavailable');
    expect(screen.queryByText('No container runtime found')).toBeNull();
    expect(screen.queryByText('Container runtime: found.')).toBeNull();
    await act(async () => release(un));
    await screen.findByText('No container runtime found');
  });

  it('other_unavailable_reasons_are_not_no_runtime', async () => {
    const odd = { phase: 'unavailable', reason: 'something_else' } as unknown as StackState;
    const { api } = fake({ stacks: { postgres: odd, wesql: odd, tikv: odd } });
    render(<StacksSection desktop={api} />);
    await screen.findByText('Container runtime: found.');
    expect(screen.queryByText('No container runtime found')).toBeNull();
  });

  it('no_runtime_says_so_and_offers_no_start', async () => {
    const un: StackState = { phase: 'unavailable', reason: 'no_container_runtime' };
    const { api } = fake({ stacks: { postgres: un, wesql: un, tikv: un } });
    render(<StacksSection desktop={api} />);
    await screen.findByText('No container runtime found');
    expect(screen.queryByRole('button', { name: /Start/ })).toBeNull();
  });

  it('start_stop_and_error_reporting', async () => {
    const { api, calls } = fake({
      stacks: {
        postgres: { phase: 'running', services: [{ name: 'pg', state: 'up', ports: ['5432'] }] },
      },
    });
    render(<StacksSection desktop={api} />);
    await screen.findByRole('button', { name: 'Stop Postgres' });
    expect(screen.getByText('Container runtime: found.')).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Stop Postgres' }));
    await waitFor(() => expect(calls).toContain('stop:postgres'));
    fireEvent.click(screen.getByRole('button', { name: 'Start WeSQL' }));
    expect(await screen.findByText('wesql: image pull failed')).toBeTruthy();
  });
});

describe('updates section', () => {
  it('disabled_has_no_check', async () => {
    const { api } = fake({ update: { phase: 'disabled' } });
    render(<UpdatesSection desktop={api} />);
    await screen.findByText(/no update feed is configured/);
    expect((screen.getByRole('button', { name: 'Check now' }) as HTMLButtonElement).disabled).toBe(
      true,
    );
  });

  it('manual_mode_offers_download_vX_after_a_check', async () => {
    const { api, calls } = fake();
    render(<UpdatesSection desktop={api} />);
    await screen.findByText('You are on the latest version.');
    fireEvent.click(screen.getByRole('button', { name: 'Check now' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Download v2.0.0' }));
    await waitFor(() => expect(calls).toEqual(['check', 'download']));
  });

  it('ready_installs_and_downloading_shows_percent', async () => {
    const { api, calls } = fake({ update: { phase: 'ready', version: '2.0.0', mode: 'self' } });
    render(<UpdatesSection desktop={api} />);
    fireEvent.click(await screen.findByRole('button', { name: 'Restart and install' }));
    await waitFor(() => expect(calls).toContain('install'));
    cleanup();
    render(
      <UpdatesSection
        desktop={fake({ update: { phase: 'downloading', version: '2.0.0', percent: 41.6 } }).api}
      />,
    );
    expect(await screen.findByText(/: 42%/)).toBeTruthy();
  });
});

describe('about section', () => {
  it('shows_the_version_licences_and_opens_logs', async () => {
    const { api, calls } = fake();
    render(<AboutSection desktop={api} licences="Copyright 2026 The Loams Authors" />);
    expect(screen.getByText('1.2.3')).toBeTruthy();
    expect(screen.getByLabelText('Licences').textContent).toContain('The Loams Authors');
    fireEvent.click(screen.getByRole('button', { name: 'Open logs folder' }));
    expect(calls).toEqual(['openLogs']);
  });

  it('bundles_the_repository_notice_by_default', () => {
    render(<AboutSection desktop={fake().api} />);
    expect(screen.getByLabelText('Licences').textContent).toContain('dsh-desktop');
  });
});

describe('live store', () => {
  it('chooses_where_live_keeps_its_data', async () => {
    // Ruling T23-8: a persisted choice, embedded by default.
    const { api, calls } = fake();
    render(<StacksSection desktop={api} />);
    const select = (await screen.findByLabelText('Live store')) as HTMLSelectElement;
    await waitFor(() => expect(select.value).toBe('embedded'));
    fireEvent.change(select, { target: { value: 'tikv-stack' } });
    await waitFor(() => expect(calls).toContain('setLiveStore:tikv-stack'));
    expect(select.value).toBe('tikv-stack');
  });

  it('shows_the_notice_when_tikv_is_chosen_but_unavailable', async () => {
    const notice = 'Live on TiKV is unavailable; showing local data.';
    const { api } = fake({ liveStore: 'tikv-stack', liveNotice: notice });
    render(<StacksSection desktop={api} />);
    expect(await screen.findByText(notice)).toBeTruthy();
    const select = (await screen.findByLabelText('Live store')) as HTMLSelectElement;
    await waitFor(() => expect(select.value).toBe('tikv-stack'));
  });
});
