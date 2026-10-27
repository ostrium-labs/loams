// The shell's part in the console's own pages (API2 Task 10): bare pages, the
// session gate, page breadcrumbs, route queries, the top bar's project pill and
// signing out.

import type { FlagsService, SessionService } from '@loams/console-host';
import { SlotProvider, SlotRegistry } from '@loams/slots';
import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it } from 'vite-plus/test';
import { Layout } from '../src/layout.js';
import { HashRouter, type HashSource } from '../src/router.js';

function memory(start = '/'): HashSource {
  let path = start;
  return {
    get: () => path,
    set: (p) => {
      path = p;
    },
    listen: () => () => {},
  };
}

const flags = {
  edition: 'oss',
  instanceName: 'Acme Loams',
  serverVersion: '0.1.0',
  features: {},
  apiVersions: [],
  has: () => false,
} as FlagsService;

const who = { displayName: 'Omar Haddad', kind: 1 };
const gated = (gate: 'ready' | 'signed-out' | 'setup', signOut?: () => Promise<void>) =>
  ({
    principal: () => who,
    subscribe: () => () => {},
    gate: () => gate,
    account: () => undefined,
    ...(signOut ? { signOut } : {}),
  }) as unknown as SessionService;

function setup(start: string, session: SessionService) {
  const slots = new SlotRegistry();
  const router = new HashRouter(slots, memory(start));
  render(
    <SlotProvider registry={slots}>
      <Layout router={router} session={session} flags={flags} />
    </SlotProvider>,
  );
  return { slots, router };
}

beforeEach(() => {
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
});
afterEach(cleanup);

describe('the console pages in the shell', () => {
  it('bare_pages_render_without_the_chrome', () => {
    const { router } = setup('/sign-in', gated('signed-out'));
    act(() => {
      router.page(
        { id: 'sign-in', path: '/sign-in', title: 'Sign in', bare: true, public: true },
        () => <h1>Sign in here</h1>,
      );
    });
    expect(screen.getByRole('heading', { name: 'Sign in here' })).toBeTruthy();
    expect(screen.queryByRole('navigation', { name: 'Sidebar' })).toBeNull();
  });

  it('signed_out_visitor_is_sent_to_sign_in_with_next', () => {
    const { router } = setup('/teams?tab=all', gated('signed-out'));
    act(() => {
      router.page({ id: 'teams', path: '/teams', title: 'Teams' }, () => <p>teams</p>);
      router.page(
        { id: 'sign-in', path: '/sign-in', title: 'Sign in', bare: true, public: true },
        () => <p>sign in</p>,
      );
    });
    expect(router.current()).toMatchObject({
      path: '/sign-in',
      pageId: 'sign-in',
      query: { next: '/teams?tab=all' },
    });
    expect(screen.getByText('sign in')).toBeTruthy();
  });

  it('a_fresh_install_is_sent_to_setup', () => {
    const { router } = setup('/', gated('setup'));
    act(() => {
      router.page({ id: 'home', path: '/', title: 'Home' }, () => <p>home</p>);
      router.page({ id: 'setup', path: '/setup', title: 'Setup', bare: true, public: true }, () => (
        <p>set up</p>
      ));
    });
    expect(router.current().path).toBe('/setup');
  });

  it('a_page_trail_replaces_the_crumbs_and_names_the_tab', () => {
    const { router } = setup('/teams/t1', gated('ready'));
    act(() => {
      router.page({ id: 'team', path: '/teams/:team', title: 'Team' }, () => <p>team</p>);
      router.setTrail([
        { label: 'Acme', href: '/' },
        { label: 'Teams', href: '/teams' },
        { label: 'Platform' },
      ]);
    });
    const trail = within(screen.getByRole('navigation', { name: 'Breadcrumb' }));
    expect(trail.getByRole('link', { name: 'Teams' }).getAttribute('href')).toBe('#/teams');
    expect(trail.getByText('Platform')).toBeTruthy();
    expect(document.title).toBe('Platform · Loams');
  });

  it('the_project_pill_sits_in_the_top_bar_and_nav_context_is_not_rendered', () => {
    const { slots } = setup('/', gated('ready'));
    act(() => {
      slots.register({ name: 'shell.topbar.left', plugin: 'projects' }, () => <p>project pill</p>);
      slots.register({ name: 'shell.nav.context', plugin: 'old' }, () => <p>old context</p>);
    });
    expect(within(screen.getByRole('banner')).getByText('project pill')).toBeTruthy();
    // W20: the slot stays declared, and the shell no longer renders it.
    expect(screen.queryByText('old context')).toBeNull();
  });

  it('the_account_menu_signs_out', async () => {
    let ended = 0;
    const { router } = setup(
      '/',
      gated('ready', async () => {
        ended++;
      }),
    );
    fireEvent.click(screen.getByRole('button', { name: /Account/ }));
    fireEvent.click(await screen.findByText('Sign out'));
    await act(async () => {});
    expect(ended).toBe(1);
    expect(router.current().path).toBe('/sign-in');
  });

  it('matches_a_route_without_its_query_and_keeps_the_query', () => {
    const slots = new SlotRegistry();
    const router = new HashRouter(slots, memory('/consent?client_id=cc&scope=query%20mcp%3Atools'));
    router.page({ id: 'consent', path: '/consent', title: 'Allow access' }, () => null);
    expect(router.current()).toEqual({
      path: '/consent',
      pageId: 'consent',
      // The query's parameters also join the page's params (SD1 T3-7).
      params: { client_id: 'cc', scope: 'query mcp:tools' },
      query: { client_id: 'cc', scope: 'query mcp:tools' },
    });
    expect(router.spec()?.title).toBe('Allow access');
  });
});
