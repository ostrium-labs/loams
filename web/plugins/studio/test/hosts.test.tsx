// Settings › Agentd hosts and the web edition's remote agentd (SD1 Task 7): the
// registry, `?host=` by id only, pasted development tokens, and the projects
// client over the selected host. No network: the client is a fake.

import { Code, ConnectError, createClient, createRouterTransport } from '@connectrpc/connect';
import { createHostRegistry, HOSTS_KEY, localHostStore } from '@loams/agentd-client/hosts';
import { ProjectService } from '@loams/proto/agentd';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vite-plus/test';
import type { StudioAgentd } from '../src/agentd.js';
import { hostedAgentd } from '../src/hosts/agentd.js';
import { wrap } from './fakes.js';
import { StudioInShell } from './shell.js';

const URL_A = 'https://devbox.example.ts.net:8443';
const URL_B = 'https://buildbox.example.ts.net:8443';

/** A fake daemon per host (its ProjectService), recording the token each call saw. */
function fakeClients() {
  const tokens: (string | undefined)[] = [];
  const transportFor = vi.fn(
    (host: { id: string; name: string; url: string }, getToken: () => unknown) =>
      createRouterTransport(({ service }) => {
        service(ProjectService, {
          listRepos: async () => {
            tokens.push((await getToken()) as string | undefined);
            return { repos: [{ path: `/srv/${host.name}`, name: `repo-of-${host.name}` }] };
          },
        });
      }),
  );
  return { tokens, transportFor };
}

/** The selected host's projects, over its transport. */
async function listProjects(agentd: StudioAgentd) {
  const transport = agentd.transport();
  if (!transport) throw new Error('No agentd host is connected');
  return (await createClient(ProjectService, transport).listRepos({})).repos;
}

beforeEach(() => {
  localStorage.clear();
  window.matchMedia = ((query: string) => ({
    matches: query.includes('min-width'),
    media: query,
    addEventListener: () => {},
    removeEventListener: () => {},
  })) as unknown as typeof window.matchMedia;
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
});
afterEach(cleanup);

describe('the hosted agentd', () => {
  it('needs a host and a token, then lists the host projects', async () => {
    const { tokens, transportFor } = fakeClients();
    const agentd = hostedAgentd({ transportFor });
    expect(agentd.reach()).toEqual({ state: 'no-host' });
    const host = agentd.hosts.registry.add({ name: 'devbox', url: URL_A });
    expect(agentd.hosts.selected()?.id).toBe(host.id);
    // A host but no token yet: still not connected.
    expect(agentd.reach()).toEqual({ state: 'no-host' });
    await expect(listProjects(agentd)).rejects.toThrow(/No agentd host/);
    agentd.hosts.setPastedToken(host.id, '  pasted-token  ');
    expect(agentd.reach()).toEqual({ state: 'ready' });
    expect((await listProjects(agentd)).map((p) => p.name)).toEqual(['repo-of-devbox']);
    expect(tokens).toEqual(['pasted-token']);
    expect(agentd.cacheKey()).toBe(`host:${host.id}`);
  });

  it('says what to check when the browser cannot reach the host (B21)', async () => {
    // What the web transport rejects with when `fetch` itself fails (or the CSP blocks it).
    const offline = () => Promise.reject(new ConnectError('Failed to fetch', Code.Unknown));
    const transportFor = vi.fn(() => ({ unary: offline, stream: offline }));
    const agentd = hostedAgentd({ transportFor });
    const host = agentd.hosts.registry.add({ name: 'devbox', url: URL_A });
    agentd.hosts.setPastedToken(host.id, 't');
    const failure = await listProjects(agentd).catch((e: unknown) => ConnectError.from(e));
    expect(failure).toBeInstanceOf(ConnectError);
    expect((failure as ConnectError).code).toBe(Code.Unavailable);
    expect((failure as ConnectError).rawMessage).toMatch(
      /LOAMS_AGENTD_ORIGINS lists https:\/\/devbox/,
    );
  });

  it('pasted tokens are never stored', () => {
    const agentd = hostedAgentd({ transportFor: fakeClients().transportFor });
    const host = agentd.hosts.registry.add({ name: 'devbox', url: URL_A });
    agentd.hosts.setPastedToken(host.id, 'secret-token-value');
    expect(JSON.stringify({ ...localStorage })).not.toContain('secret-token-value');
    expect(localStorage.getItem(HOSTS_KEY)).toContain(URL_A);
  });

  it('uses the sign-in token when one is provided', async () => {
    const { tokens, transportFor } = fakeClients();
    const agentd = hostedAgentd({ transportFor, getToken: () => () => 'oidc-access-token' });
    expect(agentd.hosts.pastedTokens).toBe(false);
    agentd.hosts.registry.add({ name: 'devbox', url: URL_A });
    expect(agentd.reach()).toEqual({ state: 'ready' });
    await listProjects(agentd);
    expect(tokens).toEqual(['oidc-access-token']);
  });

  it('host_param_selects_by_id_only', async () => {
    const { transportFor } = fakeClients();
    const registry = createHostRegistry(localHostStore());
    const a = registry.add({ name: 'devbox', url: URL_A });
    const b = registry.add({ name: 'buildbox', url: URL_B });
    const agentd = hostedAgentd({ registry, transportFor, getToken: () => () => 't' });
    expect(agentd.hosts.selected()?.id).toBe(a.id);

    render(
      wrap(
        <StudioInShell
          agentd={agentd}
          route={{ kind: 'home' }}
          navigate={vi.fn()}
          hostParam={b.id}
        />,
      ),
    );
    await waitFor(() => expect(agentd.hosts.selected()?.id).toBe(b.id));
    const sidebar = screen.getByRole('navigation', { name: 'Projects' });
    expect(await within(sidebar).findByRole('link', { name: /repo-of-buildbox/ })).toBeTruthy();

    // A URL, or an unknown id, selects nothing.
    for (const param of ['https://evil.example', URL_A, 'h-unknown']) {
      act(() => agentd.hosts.select(param));
      expect(agentd.hosts.selected()?.id).toBe(b.id);
    }
  });
});

describe('Settings › Agentd hosts', () => {
  function showSettings(agentd = hostedAgentd({ transportFor: fakeClients().transportFor })) {
    render(wrap(<StudioInShell agentd={agentd} route={{ kind: 'settings' }} navigate={vi.fn()} />));
    return agentd;
  }

  it('adds a host, takes a pasted token under a warning, and removes it', async () => {
    const agentd = showSettings();
    expect(await screen.findByText('No agentd hosts')).toBeTruthy();
    expect(screen.getByText('Development sign-in')).toBeTruthy();

    const form = screen.getByRole('form', { name: 'Add an agentd host' });
    fireEvent.change(within(form).getByLabelText('Name'), { target: { value: 'devbox' } });
    fireEvent.change(within(form).getByLabelText('URL'), {
      target: { value: 'http://devbox.example.com' },
    });
    fireEvent.click(within(form).getByRole('button', { name: 'Add host' }));
    expect(await within(form).findByText(/https:\/\/ URL/)).toBeTruthy();
    expect(agentd.hosts.registry.list()).toEqual([]);

    fireEvent.change(within(form).getByLabelText('URL'), { target: { value: `${URL_A}/` } });
    fireEvent.click(within(form).getByRole('button', { name: 'Add host' }));
    const row = await screen.findByRole('listitem', { name: 'devbox' });
    expect(within(row).getByText(URL_A)).toBeTruthy();
    expect(within(row).getByText('in use')).toBeTruthy();
    expect(within(row).getByText('no token')).toBeTruthy();

    fireEvent.change(within(row).getByLabelText('Access token for devbox'), {
      target: { value: 'pasted' },
    });
    fireEvent.click(within(row).getByRole('button', { name: 'Set token' }));
    expect(await within(row).findByText('signed in')).toBeTruthy();
    expect(agentd.reach()).toEqual({ state: 'ready' });

    fireEvent.click(within(row).getByRole('button', { name: 'Remove' }));
    expect(await screen.findByText('No agentd hosts')).toBeTruthy();
    expect(agentd.reach()).toEqual({ state: 'no-host' });
  });

  it('works when storage is blocked', async () => {
    const getItem = vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => {
      throw new Error('SecurityError');
    });
    const setItem = vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
      throw new Error('QuotaExceededError');
    });
    try {
      const agentd = showSettings();
      const form = await screen.findByRole('form', { name: 'Add an agentd host' });
      fireEvent.change(within(form).getByLabelText('URL'), { target: { value: URL_A } });
      fireEvent.click(within(form).getByRole('button', { name: 'Add host' }));
      expect(await screen.findByRole('listitem', { name: /devbox/ })).toBeTruthy();
      expect(agentd.hosts.registry.list()).toHaveLength(1);
    } finally {
      getItem.mockRestore();
      setItem.mockRestore();
    }
  });
});
