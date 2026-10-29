import { create } from '@bufbuild/protobuf';
import { Code, ConnectError, createRouterTransport } from '@connectrpc/connect';
import type { SessionService } from '@loams/console-host';
import { collection, instance } from '@loams/proto';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vite-plus/test';
import { listNamespaces, NamespacesPage } from '../src/index.js';

afterEach(cleanup);

describe('NamespacesPage', () => {
  it('lists environments with their namespaces and switches the selection', () => {
    // A fake session (plugins never import each other's values).
    const listeners = new Set<() => void>();
    const environments = [
      { id: 'env_dev', name: 'development', namespace: 'search-dev', protected: false },
      { id: 'env_prod', name: 'production', namespace: 'search-prod', protected: true },
    ];
    let selected = 'env_dev';
    const session: SessionService = {
      principal: () => create(instance.PrincipalSchema, { id: 'usr_omar', displayName: 'Omar' }),
      environments: () => environments,
      environment: () => environments.find((e) => e.id === selected),
      select: (id) => {
        selected = id;
        for (const l of listeners) l();
      },
      authenticatedAt: () => undefined,
      subscribe: (l) => {
        listeners.add(l);
        return () => listeners.delete(l);
      },
    };
    render(<NamespacesPage session={session} />);
    expect(screen.getByText('Signed in as Omar')).toBeTruthy();
    expect(screen.getByText('search-prod')).toBeTruthy();
    expect(screen.getByText('protected')).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Select' }));
    expect(session.environment()?.id).toBe('env_prod');
  });

  it('with sign-in off and no environments, offers the namespaces to open (B7)', () => {
    localStorage.setItem('loams.data.namespaces', JSON.stringify(['e2e']));
    const session: SessionService = {
      principal: () =>
        create(instance.PrincipalSchema, { id: 'dev-loopback', displayName: 'Local developer' }),
      environments: () => [],
      environment: () => undefined,
      select: () => {},
      authenticatedAt: () => undefined,
      subscribe: () => () => {},
    };
    const went: string[] = [];
    render(<NamespacesPage session={session} navigate={(to) => went.push(to)} />);
    expect(screen.getByText(/runs with sign-in off/)).toBeTruthy();
    expect(screen.queryByText(/cannot reach any environment/)).toBeNull();
    expect(screen.getByText('e2e')).toBeTruthy();
    expect(screen.getAllByRole('row')).toHaveLength(3); // the header, default and e2e
    fireEvent.click(screen.getAllByRole('button', { name: 'Data explorer' })[1] as HTMLElement);
    fireEvent.click(screen.getAllByRole('button', { name: 'Streams & Links' })[0] as HTMLElement);
    expect(went).toEqual(['/data/e2e', '/streams/default']);
    localStorage.clear();
  });

  const authOff: SessionService = {
    principal: () =>
      create(instance.PrincipalSchema, { id: 'dev-loopback', displayName: 'Local developer' }),
    environments: () => [],
    environment: () => undefined,
    select: () => {},
    authenticatedAt: () => undefined,
    subscribe: () => () => {},
  };

  it('lists the engine namespaces with their counts, across pages (B7)', async () => {
    const asked: string[] = [];
    const transport = createRouterTransport(({ service }) => {
      service(collection.NamespaceService, {
        createNamespace: () => ({}),
        listNamespaces: (req) => {
          asked.push(req.pageToken);
          return req.pageToken === ''
            ? {
                namespaces: [{ namespace: 'alpha', collectionCount: 2, streamCount: 1 }],
                nextPageToken: '1',
              }
            : { namespaces: [{ namespace: 'e2e', collectionCount: 5 }] };
        },
      });
    });
    localStorage.setItem('loams.data.namespaces', JSON.stringify(['only-in-browser']));
    render(
      <NamespacesPage
        session={authOff}
        navigate={() => {}}
        load={() => listNamespaces(transport)}
      />,
    );
    expect(await screen.findByText('e2e')).toBeTruthy();
    expect(asked).toEqual(['', '1']);
    expect(screen.getByText('alpha')).toBeTruthy();
    expect(screen.getByRole('columnheader', { name: 'Collections' })).toBeTruthy();
    expect(screen.getByText('5')).toBeTruthy();
    // The engine's list replaces the browser's guesses.
    expect(screen.queryByText('only-in-browser')).toBeNull();
    expect(screen.queryByText(/does not list namespaces/)).toBeNull();
    localStorage.clear();
  });

  it('falls back to the browser list on an engine without ListNamespaces', async () => {
    const transport = createRouterTransport(({ service }) => {
      service(collection.NamespaceService, {
        createNamespace: () => ({}),
        listNamespaces: () => {
          throw new ConnectError('no such method', Code.Unimplemented);
        },
      });
    });
    expect(await listNamespaces(transport)).toBeUndefined();
    render(
      <NamespacesPage
        session={authOff}
        navigate={() => {}}
        load={() => listNamespaces(transport)}
      />,
    );
    expect(await screen.findByText(/does not list namespaces/)).toBeTruthy();
    expect(screen.getAllByRole('row')).toHaveLength(2); // the header and default
    expect(screen.queryByRole('columnheader', { name: 'Collections' })).toBeNull();
  });

  it('a failing list is reported, not swallowed', async () => {
    const transport = createRouterTransport(({ service }) => {
      service(collection.NamespaceService, {
        createNamespace: () => ({}),
        listNamespaces: () => {
          throw new ConnectError('store down', Code.Unavailable);
        },
      });
    });
    await expect(listNamespaces(transport)).rejects.toThrow(/store down/);
  });
});
