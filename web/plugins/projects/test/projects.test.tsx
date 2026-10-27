// A project, its environments and its access, ported from the original console
// (API2 Task 10), and the top bar's project pill (CF1 Task 10).

import { Code, ConnectError } from '@connectrpc/connect';
import {
  createAdminTransport,
  stubRouter,
  stubSession,
  withConsole,
} from '@loams/admin-ui/testing';
import type { admin } from '@loams/proto';
import type { PaletteCommand } from '@loams/slots';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vite-plus/test';
import { AccessPage } from '../src/access.js';
import { ProjectPill } from '../src/context.js';
import { connectSnippet, EnvironmentPage } from '../src/environment.js';
import { ProjectsSource, projectOf } from '../src/index.js';
import { ProjectPage } from '../src/project.js';

afterEach(() => {
  cleanup();
  localStorage.clear();
});

/** The mock serves no loams.collection.v1: the card says so calmly. */
const noCollections = () => {
  throw new ConnectError('not served', Code.Unimplemented);
};

describe('@loams/plugin-projects', () => {
  it('the palette lists every project, once something is typed (T11-3)', async () => {
    const published: PaletteCommand[][] = [];
    const router = stubRouter('/');
    const publish = (c: readonly PaletteCommand[]) => published.push([...c]);
    const { rerender } = render(
      withConsole(<ProjectsSource query="" publish={publish} router={router} />),
    );
    await waitFor(() => expect(published.length).toBeGreaterThan(0));
    expect(published.at(-1)).toEqual([]);
    rerender(withConsole(<ProjectsSource query="docs" publish={publish} router={router} />));
    await waitFor(() =>
      expect(published.at(-1)?.map((c) => c.title)).toContain('Go to project Docs search'),
    );
    published
      .at(-1)
      ?.find((c) => c.title === 'Go to project Docs search')
      ?.run();
    expect(router.navigated).toEqual(['/projects/docs-search']);
  });

  it('project_page_lists_environments_and_agents', async () => {
    const router = stubRouter('/projects/docs-search');
    render(
      withConsole(
        <ProjectPage
          project="docs-search"
          router={router}
          session={stubSession()}
          auditPath="/audit"
        />,
      ),
    );
    expect(await screen.findByRole('heading', { name: 'Docs search' })).toBeTruthy();
    expect(await screen.findByText('docs-search-production')).toBeTruthy();
    expect(screen.getByText('655k')).toBeTruthy();
    expect(await screen.findByText('2 live')).toBeTruthy();
    expect(screen.getByText('Suspended')).toBeTruthy();
    expect(router.trail()?.map((c) => c.label)).toEqual(['Acme Robotics', 'Docs search']);
  });

  it('unknown_project_says_so', async () => {
    render(
      withConsole(
        <ProjectPage
          project="nope"
          router={stubRouter()}
          session={stubSession()}
          auditPath="/audit"
        />,
      ),
    );
    expect(await screen.findByText(/There is no project nope in this org/)).toBeTruthy();
  });

  it('environment_page_shows_keys_activity_and_connect_snippet', async () => {
    const transport = createAdminTransport();
    const router = stubRouter('/projects/docs-search/environments/development');
    render(
      withConsole(
        <EnvironmentPage
          project="docs-search"
          environment="development"
          router={router}
          session={stubSession()}
          server="https://loams.example"
        />,
        transport,
      ),
    );
    expect(await screen.findByRole('heading', { name: /Development/ })).toBeTruthy();
    expect(screen.getByText('2,223')).toBeTruthy();
    expect(await screen.findByText('ci-deploy')).toBeTruthy();
    expect(await screen.findByText(/queries, .* writes on/)).toBeTruthy();
    expect(screen.getByText(/endpoint: 'https:\/\/loams.example'/)).toBeTruthy();
    expect(await screen.findByText('embedding 1024d · 2 fields')).toBeTruthy();
    expect(router.trail()?.map((c) => c.label)).toEqual([
      'Acme Robotics',
      'Docs search',
      'Development',
    ]);
  });

  it('collections_card_is_calm_when_the_server_does_not_serve_them', async () => {
    render(
      withConsole(
        <EnvironmentPage
          project="docs-search"
          environment="development"
          router={stubRouter()}
          session={stubSession()}
          server="https://loams.example"
        />,
        createAdminTransport({ collections: { listCollections: noCollections } }),
      ),
    );
    expect(
      await screen.findByText(/coming soon in a later preview/, {}, { timeout: 4000 }),
    ).toBeTruthy();
  });

  it('create_key_sends_an_idempotency_key_and_shows_the_secret_once', async () => {
    const requests: admin.CreateKeyRequest[] = [];
    const transport = createAdminTransport({
      keys: {
        createKey: (req) => {
          requests.push(req);
          return { key: { id: 'key_new', name: req.name }, secret: 'lk_NEW_secret' };
        },
      },
    });
    render(
      withConsole(
        <EnvironmentPage
          project="docs-search"
          environment="production"
          router={stubRouter()}
          session={stubSession()}
          server="https://loams.example"
        />,
        transport,
      ),
    );
    fireEvent.click(await screen.findByRole('button', { name: /Create API key/ }));
    expect(await screen.findByText('Protected environment')).toBeTruthy();
    fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'ci' } });
    fireEvent.click(screen.getByRole('button', { name: 'Create key' }));
    expect(await screen.findByText('lk_NEW_secret')).toBeTruthy();
    expect(requests[0]).toMatchObject({
      project: 'docs-search',
      environment: 'production',
      name: 'ci',
      scopes: ['query'],
      expiresInDays: 90,
    });
    expect(requests[0]?.idempotencyKey).toMatch(/^[0-9a-f-]{36}$/);
  });

  it('access_page_lists_grants_and_service_accounts', async () => {
    render(
      withConsole(
        <AccessPage project="docs-search" router={stubRouter()} session={stubSession()} />,
      ),
    );
    expect(await screen.findByText('Platform')).toBeTruthy();
    expect(screen.getByText('Team')).toBeTruthy();
    expect(screen.getByText('Admin')).toBeTruthy();
    expect(await screen.findByText('CI deploys')).toBeTruthy();
  });

  it('project_pill_switches_projects_and_keeps_the_last_one', async () => {
    const router = stubRouter('/projects/docs-search/environments/production');
    render(withConsole(<ProjectPill router={router} />));
    const pill = await screen.findByRole('combobox', { name: 'Project' });
    expect(pill.textContent).toContain('Docs search');
    expect(localStorage.getItem('loams-project')).toBe('docs-search');
    // The project's pages are pages now (W20): the pill lists no links.
    expect(screen.queryByRole('link')).toBeNull();
    // An org page: the last project stays, marked as last opened.
    router.go('/teams');
    expect(
      await screen.findByRole('combobox', { name: 'Open a project (last opened shown)' }),
    ).toBeTruthy();
  });

  it('reads the project from a route and writes the SDK snippet', () => {
    expect(projectOf('/projects/docs-search/agents/agt_1')).toBe('docs-search');
    expect(projectOf('/teams')).toBeUndefined();
    expect(connectSnippet('https://loams.example', 'docs-dev')).toContain(
      "endpoint: 'https://loams.example'",
    );
  });
});
