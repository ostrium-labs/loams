// The org's pages, ported from the original console (API2 Task 10): the same text
// against the same seed, now read over loams.admin.v1 with Connect Query.

import {
  createAdminTransport,
  SEED,
  stubRouter,
  stubSession,
  withConsole,
} from '@loams/admin-ui/testing';
import type { FlagsService } from '@loams/console-host';
import type { admin } from '@loams/proto';
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vite-plus/test';
import { Home } from '../src/home.js';
import { orgPaths } from '../src/index.js';
import { MembersPage } from '../src/members.js';
import { SettingsPage } from '../src/settings.js';
import { TeamPage, TeamsPage } from '../src/teams.js';

afterEach(cleanup);

const flags: FlagsService = {
  edition: 'oss',
  instanceName: 'Loams (mock)',
  serverVersion: '0.1.0-preview.1',
  features: {},
  apiVersions: ['loams.admin.v1'],
  has: () => true,
};

describe('@loams/plugin-org', () => {
  it('home_lists_projects_with_their_environments_and_recent_activity', async () => {
    const router = stubRouter('/');
    render(withConsole(<Home router={router} session={stubSession()} auditPath="/audit" />));
    expect(await screen.findByRole('heading', { name: 'Acme Robotics' })).toBeTruthy();
    const card = (await screen.findByText('Docs search')).closest('a') as HTMLElement;
    expect(card.getAttribute('href')).toBe('#/projects/docs-search');
    await waitFor(() => expect(within(card).getByText('Production')).toBeTruthy());
    expect(await screen.findByText('query.search')).toBeTruthy();
    expect(screen.getByText(/2 projects, 2 environments/)).toBeTruthy();
    expect(screen.getByText('Signed in as Priya Raman')).toBeTruthy();
    expect(router.trail()).toEqual([{ label: 'Acme Robotics' }]);
  });

  it('home_with_no_projects_fills_its_tiles_and_says_where_the_data_is', async () => {
    const router = stubRouter('/');
    const transport = createAdminTransport({
      projects: { listProjects: () => ({ projects: [] }) },
    });
    render(
      withConsole(<Home router={router} session={stubSession()} auditPath="/audit" />, transport),
    );
    expect(await screen.findByText('No projects yet')).toBeTruthy();
    expect(screen.getByText(/0 projects, 0 environments/)).toBeTruthy();
    // B7: every tile resolves; none stays at the "…" placeholder.
    await waitFor(() => expect(screen.queryByText('…')).toBeNull());
    expect(screen.getByRole('link', { name: 'Data explorer' }).getAttribute('href')).toBe('#/data');
    // Nothing reports a size: a dash with the reason, not "0" and "0 B".
    const dashes = screen.getAllByText('—');
    expect(dashes).toHaveLength(3);
    expect(dashes[0]?.parentElement?.getAttribute('title')).toMatch(
      /does not measure environments/,
    );
    expect(screen.queryByText('0 B')).toBeNull();
  });

  it('home_shows_the_numbers_an_environment_reports', async () => {
    const router = stubRouter('/');
    render(withConsole(<Home router={router} session={stubSession()} auditPath="/audit" />));
    await screen.findByText(/2 projects, 2 environments/);
    await waitFor(() => expect(screen.queryByText('…')).toBeNull());
    expect(screen.queryByText('—')).toBeNull();
  });

  it('home_namespaces_tile_counts_what_the_engine_lists_or_falls_back_to_environments', async () => {
    const router = stubRouter('/');
    const listed = createAdminTransport({
      projects: { listProjects: () => ({ projects: [] }) },
      namespaces: {
        listNamespaces: () => ({
          namespaces: [
            { namespace: 'default', collectionCount: 2 },
            { namespace: 'e2e', collectionCount: 3, streamCount: 1 },
          ],
        }),
      },
    });
    render(
      withConsole(<Home router={router} session={stubSession()} auditPath="/audit" />, listed),
    );
    expect(await screen.findByText('5 collections')).toBeTruthy();
    cleanup();
    // An engine without ListNamespaces: the tile counts project environments (none here).
    const older = createAdminTransport({ projects: { listProjects: () => ({ projects: [] }) } });
    render(withConsole(<Home router={router} session={stubSession()} auditPath="/audit" />, older));
    expect(await screen.findByText('Project environments')).toBeTruthy();
    await waitFor(() => expect(screen.queryByText('…')).toBeNull());
  });

  it('teams_page_lists_teams_and_creates_one', async () => {
    const created: string[] = [];
    const transport = createAdminTransport({
      org: {
        createTeam: (req) => {
          created.push(req.name);
          return { team: { id: 'team_new', name: req.name } };
        },
      },
    });
    const router = stubRouter('/teams');
    render(withConsole(<TeamsPage router={router} session={stubSession()} />, transport));
    expect(await screen.findByText('Platform')).toBeTruthy();
    expect(screen.getByText('acme-platform')).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: /New team/ }));
    fireEvent.change(await screen.findByLabelText('Name'), { target: { value: 'Search' } });
    fireEvent.click(screen.getByRole('button', { name: 'Create team' }));
    await waitFor(() => expect(created).toEqual(['Search']));
    expect(await screen.findByText('Created Search')).toBeTruthy();
  });

  it('team_page_shows_members_and_project_roles', async () => {
    const router = stubRouter('/teams/team_platform');
    render(withConsole(<TeamPage team="team_platform" router={router} session={stubSession()} />));
    expect(await screen.findByRole('heading', { name: 'Platform' })).toBeTruthy();
    expect(screen.getByText('Priya Raman')).toBeTruthy();
    expect(screen.getByText('Admin')).toBeTruthy();
    expect(router.trail()?.map((c) => c.label)).toEqual(['Acme Robotics', 'Teams', 'Platform']);
  });

  it('members_page_shows_members_and_pending_invitations', async () => {
    render(withConsole(<MembersPage router={stubRouter('/members')} session={stubSession()} />));
    expect(await screen.findByText('priya@acme.dev')).toBeTruthy();
    expect(screen.getByText('owner')).toBeTruthy();
    expect(await screen.findByText('lena@acme.dev')).toBeTruthy();
    expect(screen.getByText('Pending invitations')).toBeTruthy();
  });

  it('settings_saves_the_org_with_a_field_mask', async () => {
    const updates: admin.UpdateOrgRequest[] = [];
    const transport = createAdminTransport({
      org: {
        updateOrg: (req) => {
          updates.push(req);
          return { org: req.org };
        },
      },
    });
    render(
      withConsole(
        <SettingsPage router={stubRouter('/settings')} session={stubSession()} flags={flags} />,
        transport,
      ),
    );
    const name = (await screen.findByLabelText('Name')) as HTMLInputElement;
    expect(name.value).toBe('Acme Robotics');
    fireEvent.change(name, { target: { value: 'Acme' } });
    fireEvent.change(screen.getByLabelText('Allowed email domains'), {
      target: { value: 'acme.dev, acme.io' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Save changes' }));
    await waitFor(() => expect(updates).toHaveLength(1));
    expect(updates[0]?.org?.name).toBe('Acme');
    expect(updates[0]?.org?.allowedDomains).toEqual(['acme.dev', 'acme.io']);
    expect(updates[0]?.updateMask?.paths).toEqual([
      'name',
      'require_two_factor',
      'allowed_domains',
    ]);
    expect(screen.getByText('0.1.0-preview.1')).toBeTruthy();
    expect(await screen.findByText('Email and password')).toBeTruthy();
    expect(SEED.org.id).toBe('org_acme');
    expect(screen.getByText('org_acme')).toBeTruthy();
  });

  it('org_pages_move_off_the_root_on_the_desktop', () => {
    expect(orgPaths('web')).toEqual({ home: '/', settings: '/settings' });
    expect(orgPaths('desktop')).toEqual({ home: '/org', settings: '/org/settings' });
  });
});
