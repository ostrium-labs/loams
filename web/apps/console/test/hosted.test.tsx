// The hosted console: the engine URL and health check, the "Connect your engine"
// screen, and the browser edition's catalog (real pages by default, sample pages
// only under "Demo data") booted over the in-memory mock.

import type { ConsoleHandle } from '@loams/console-host';
import { composeCatalog, parseCatalog } from '@loams/console-host';
import { createMockTransport } from '@loams/console-host/testing';
import { createWebPlatform } from '@loams/platform-web';
import { pageEntries } from '@loams/plugin-shell';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vite-plus/test';
import baseCatalog from '../catalog/base.yml?raw';
import { startConsole } from '../src/cordis/start.js';
import {
  HOSTED_PACKAGE,
  HostedHeader,
  type HostedSession,
  hostedManifest,
  hostedPatch,
  hostedPlugin,
  webManifests,
  webModules,
  webPatches,
} from '../src/hosted/edition.js';
import {
  checkHealth,
  type EngineStatus,
  fetchIdentity,
  HEALTH_POLL,
  parseEngineUrl,
  savedEngine,
  savedMode,
  saveEngine,
  saveMode,
  watchHealth,
} from '../src/hosted/engine.js';
import { ConfigErrorScreen, ConnectScreen } from '../src/hosted/screens.js';
import { redirectUri, startHosted } from '../src/hosted/start.js';
import { AUTH_MODES, parseRuntimeConfig } from '../src/runtime-config.js';

/** A /health watcher that never reports, for header tests about something else. */
const idle = () => () => {};

let dispose: (() => Promise<void>) | undefined;
afterEach(async () => {
  cleanup();
  await dispose?.();
  dispose = undefined;
  document.body.innerHTML = '';
  window.location.hash = '';
});

describe('engine URL', () => {
  it('takes a bare tailnet name, an https URL or a loopback http URL', () => {
    expect(parseEngineUrl('loams.tail1.ts.net', 'https:')).toEqual({
      ok: true,
      origin: 'https://loams.tail1.ts.net',
    });
    expect(parseEngineUrl(' https://loams.tail1.ts.net/ui/ ', 'https:')).toEqual({
      ok: true,
      origin: 'https://loams.tail1.ts.net',
    });
    expect(parseEngineUrl('http://127.0.0.1:8080', 'https:')).toEqual({
      ok: true,
      origin: 'http://127.0.0.1:8080',
    });
  });

  it('refuses an empty value, a non-http scheme and mixed content', () => {
    expect(parseEngineUrl('', 'https:').ok).toBe(false);
    expect(parseEngineUrl('ftp://x', 'https:').ok).toBe(false);
    const mixed = parseEngineUrl('http://loams.lan:8080', 'https:');
    expect(mixed.ok).toBe(false);
    expect(mixed.ok ? '' : mixed.problem).toMatch(/tailscale serve/);
    // A console served over http (the Compose console on a LAN) may use one.
    expect(parseEngineUrl('http://loams.lan:8080', 'http:').ok).toBe(true);
  });

  it('remembers the engine in localStorage and forgets it', () => {
    saveEngine('https://loams.tail1.ts.net');
    expect(savedEngine()).toBe('https://loams.tail1.ts.net');
    saveEngine(undefined);
    expect(savedEngine()).toBeUndefined();
  });

  it('checks /health without credentials and tells the failures apart', async () => {
    const seen: [string, RequestInit | undefined][] = [];
    const ok = vi.fn(async (u: RequestInfo | URL, init?: RequestInit) => {
      seen.push([String(u), init]);
      return new Response('ok');
    });
    expect(await checkHealth('https://e.ts.net', ok as typeof fetch)).toMatchObject({
      ok: true,
      status: 200,
    });
    expect(seen[0]?.[0]).toBe('https://e.ts.net/health');
    expect(seen[0]?.[1]).toMatchObject({ mode: 'cors', credentials: 'omit' });

    const down = (async () => new Response('', { status: 503 })) as typeof fetch;
    expect(await checkHealth('https://e.ts.net', down)).toMatchObject({
      kind: 'http',
      status: 503,
    });

    const offline = (async () => {
      throw new TypeError('Failed to fetch');
    }) as typeof fetch;
    expect(await checkHealth('https://e.ts.net', offline)).toMatchObject({
      ok: false,
      kind: 'unreachable',
    });

    const stalled = ((_u: RequestInfo | URL, init?: RequestInit) =>
      new Promise((_r, reject) =>
        init?.signal?.addEventListener('abort', () => reject(new Error('aborted'))),
      )) as typeof fetch;
    expect(await checkHealth('https://e.ts.net', stalled, 10)).toMatchObject({ kind: 'timeout' });
  });

  it('the redirect URI is the bare origin, or the .html page it was opened on', () => {
    expect(redirectUri({ origin: 'https://console.loams.dev', pathname: '/projects/x' })).toBe(
      'https://console.loams.dev/',
    );
    expect(redirectUri({ origin: 'http://localhost:5173', pathname: '/ui/cordis.html' })).toBe(
      'http://localhost:5173/ui/cordis.html',
    );
  });
});

describe('the engine dot (UX-1)', () => {
  afterEach(() => vi.useRealTimers());

  it('polls /health, backs off while the engine is down, and asks again on focus', async () => {
    vi.useFakeTimers();
    const answers = [false, false, true];
    const check = vi.fn(async () =>
      answers.shift() === false
        ? { ok: false as const, kind: 'unreachable' as const, message: 'Failed to fetch' }
        : { ok: true as const, status: 200, ms: 3 },
    );
    const seen: EngineStatus[] = [];
    const target = new EventTarget();
    const stop = watchHealth('https://e', (s) => seen.push(s), check, target);
    expect(seen[0]).toEqual({ state: 'checking' });
    await vi.advanceTimersByTimeAsync(0);
    expect(seen.at(-1)).toMatchObject({ state: 'down', retryInMs: HEALTH_POLL.firstRetryMs });
    await vi.advanceTimersByTimeAsync(HEALTH_POLL.firstRetryMs);
    expect(seen.at(-1)).toMatchObject({ state: 'down', retryInMs: 2 * HEALTH_POLL.firstRetryMs });
    await vi.advanceTimersByTimeAsync(2 * HEALTH_POLL.firstRetryMs);
    expect(seen.at(-1)).toEqual({ state: 'ok', ms: 3 });
    expect(check).toHaveBeenCalledTimes(3);
    target.dispatchEvent(new Event('focus'));
    await vi.advanceTimersByTimeAsync(0);
    expect(check).toHaveBeenCalledTimes(4);
    stop();
    await vi.advanceTimersByTimeAsync(HEALTH_POLL.okMs * 2);
    expect(check).toHaveBeenCalledTimes(4);
  });

  it('caps the backoff', async () => {
    vi.useFakeTimers();
    const seen: EngineStatus[] = [];
    const stop = watchHealth(
      'https://e',
      (s) => seen.push(s),
      async () => ({ ok: false, kind: 'timeout', message: 'No answer within 6 s.' }),
      new EventTarget(),
    );
    await vi.advanceTimersByTimeAsync(10 * HEALTH_POLL.maxRetryMs);
    expect(seen.at(-1)).toMatchObject({ state: 'down', retryInMs: HEALTH_POLL.maxRetryMs });
    stop();
  });

  it('is amber while checking, green when the engine answers, red with the reason when not', () => {
    let report: (s: EngineStatus) => void = () => {};
    const watch = (_origin: string, on: (s: EngineStatus) => void) => {
      report = on;
      return () => {};
    };
    const { container } = render(
      <HostedHeader
        watch={watch}
        session={{
          engine: 'https://loams.tail1.ts.net',
          authMode: 'tailscale',
          user: 'ada@example.com',
          demo: false,
          setDemo: vi.fn(),
          changeEngine: vi.fn(),
        }}
      />,
    );
    const dot = () =>
      container.querySelector('[data-engine-status]')?.getAttribute('data-engine-status');
    const title = () => container.querySelector('.lc-hosted-engine')?.getAttribute('title');
    expect(dot()).toBe('checking');
    expect(title()).toMatch(/^Checking the engine/);
    act(() => report({ state: 'ok', ms: 4 }));
    expect(dot()).toBe('ok');
    expect(title()).toBe('Connected to https://loams.tail1.ts.net. Change engine');
    act(() => report({ state: 'down', message: 'Failed to fetch', retryInMs: 8_000 }));
    expect(dot()).toBe('down');
    expect(title()).toMatch(
      /^Cannot reach the engine at https:\/\/loams.tail1.ts.net\. No answer\./,
    );
    expect(title()).toContain('Trying again in 8 s.');
    expect(screen.getByRole('button', { name: /Cannot reach the engine/ })).toBeTruthy();
  });
});

describe('Connect your engine', () => {
  it('tests the engine, then connects to its origin', async () => {
    const check = vi.fn(async () => ({ ok: true as const, status: 200, ms: 12 }));
    const onConnect = vi.fn();
    render(
      <ConnectScreen modes={AUTH_MODES} initialMode="apikey" check={check} onConnect={onConnect} />,
    );
    expect(screen.queryByText('Preview')).toBeNull();
    expect(screen.getByText(/needs Tailscale running/)).toBeTruthy();
    const connect = screen.getByRole('button', { name: 'Connect' }) as HTMLButtonElement;
    expect(connect.disabled).toBe(true);

    fireEvent.change(screen.getByLabelText('Engine URL'), {
      target: { value: 'loams.tail1.ts.net' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Test connection' }));
    await screen.findByText('The engine answered');
    expect(check).toHaveBeenCalledWith('https://loams.tail1.ts.net');
    // apikey mode: the key is required.
    expect(connect.disabled).toBe(true);
    fireEvent.change(screen.getByLabelText('API key'), { target: { value: ' lk_1 ' } });
    expect(connect.disabled).toBe(false);
    fireEvent.click(connect);
    expect(onConnect).toHaveBeenCalledWith({
      engine: 'https://loams.tail1.ts.net',
      mode: 'apikey',
      apiKey: 'lk_1',
    });
  });

  it('tests a remembered engine at once; a fixed engine cannot be edited', async () => {
    const check = vi.fn(async () => ({ ok: true as const, status: 200, ms: 3 }));
    render(
      <ConnectScreen
        initial="https://loams.tail1.ts.net"
        fixed
        modes={AUTH_MODES}
        initialMode="apikey"
        check={check}
        onConnect={() => {}}
      />,
    );
    await screen.findByText('The engine answered');
    expect(check).toHaveBeenCalledTimes(1);
    expect((screen.getByLabelText('Engine URL') as HTMLInputElement).readOnly).toBe(true);
  });

  it('explains an unreachable tailnet engine and still lets the person connect', async () => {
    const check = vi.fn(async () => ({
      ok: false as const,
      kind: 'unreachable' as const,
      message: 'Failed to fetch.',
    }));
    const onConnect = vi.fn();
    render(
      <ConnectScreen
        modes={['tailscale', 'apikey']}
        initialMode="tailscale"
        check={check}
        onConnect={onConnect}
      />,
    );
    expect(screen.queryByLabelText('API key')).toBeNull();
    fireEvent.change(screen.getByLabelText('Engine URL'), {
      target: { value: 'https://loams.tail1.ts.net' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Test connection' }));
    await screen.findByText('This browser cannot reach the engine');
    expect(screen.getByText(/Tailscale is running on this device/)).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Connect anyway' }));
    expect(onConnect).toHaveBeenCalledWith({
      engine: 'https://loams.tail1.ts.net',
      mode: 'tailscale',
      apiKey: undefined,
    });
  });
});

/** Every page the plugins register (design 59 D1080: the sidebar pins only a few). */
const navLabels = (handle: { slots: ConsoleHandle['slots'] }) =>
  pageEntries(
    handle.slots.entries('shell.nav.section'),
    handle.slots.entries('console.nav'),
    undefined,
  ).map((p) => p.label);

async function boot(demo: boolean) {
  const root = document.createElement('div');
  document.body.append(root);
  const session: HostedSession = {
    engine: 'https://loams.tail1.ts.net',
    authMode: 'oidc',
    user: 'ada@example.com',
    demo,
    setDemo: vi.fn(),
    changeEngine: vi.fn(),
    signOut: vi.fn(),
  };
  const handle = await startConsole({
    platform: createWebPlatform({ transport: createMockTransport({}) }),
    root,
    base: '/ui/',
    patches: [...webPatches(demo), hostedPatch],
    extraModules: { ...webModules, [HOSTED_PACKAGE]: async () => hostedPlugin(session) },
    extraManifests: [...webManifests, hostedManifest],
  });
  dispose = handle.dispose;
  return { root, handle, session };
}

describe('the browser edition', () => {
  it('composes over the base catalog with a module and manifest for every row', () => {
    for (const demo of [false, true]) {
      const rows = composeCatalog(parseCatalog(baseCatalog), ...webPatches(demo), hostedPatch);
      for (const row of rows.filter((r) => !r.disabled)) {
        if (row.name === HOSTED_PACKAGE) continue;
        if (row.name in webModules) {
          expect(webManifests.map((m) => (m as { name: string }).name)).toContain(row.name);
        }
      }
    }
  });

  it('loads only pages the engine serves, with the engine, Demo data and sign-out in the header', async () => {
    const { root, handle, session } = await boot(false);
    await waitFor(() => expect(navLabels(handle)).toContain('Live'));
    const labels = navLabels(handle);
    expect(labels).toEqual(
      expect.arrayContaining(['Live', 'Graph', 'Analytics', 'Streams & Links', 'Status']),
    );
    for (const sample of ['Deployments', 'Loams SQL', 'Metrics', 'Pipelines', 'Git']) {
      expect(labels).not.toContain(sample);
    }
    const status = Object.fromEntries(handle.plugins().map((p) => [p.id, p.status]));
    expect(status).toMatchObject({
      live: 'active',
      graph: 'active',
      house: 'active',
      'data-studio': 'active',
      streams: 'active',
      // B12: over the engine's `/durable/` on its main port.
      durable: 'active',
    });

    expect(root.querySelector('.lc-side .lc-preview')).toBeNull();
    await waitFor(() => expect(root.textContent).toContain('loams.tail1.ts.net'));
    const toggle = root.querySelector('.lc-hosted-demo input') as HTMLInputElement;
    expect(toggle.checked).toBe(false);
    fireEvent.click(toggle);
    expect(session.setDemo).toHaveBeenCalledWith(true);
    expect(root.querySelector('.lc-hosted-user')?.textContent).toBe('ada@example.com');
    const signOut = [...root.querySelectorAll('button')].find((b) => b.textContent === 'Sign out');
    expect(signOut).toBeTruthy();
    if (signOut) fireEvent.click(signOut);
    expect(session.signOut).toHaveBeenCalled();
  }, 20_000); // boots the browser edition's whole catalog, as the test below (Windows: over 5 s)

  it('adds the sample-data pages under Demo data', async () => {
    const { root, handle } = await boot(true);
    await waitFor(() => expect(navLabels(handle)).toContain('Loams SQL'));
    expect(navLabels(handle)).toEqual(
      expect.arrayContaining(['Live', 'Deployments', 'Loams SQL', 'Metrics', 'Pipelines', 'Git']),
    );
    expect(root.textContent).toContain('Sample pages on');
  }, 20_000); // boots the browser edition's whole catalog; a cold macOS runner needs over 5 s
});

describe('sign-in modes', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    saveEngine(undefined);
    saveMode(undefined);
  });

  const json = (status: number, body: unknown) =>
    new Response(JSON.stringify(body), {
      status,
      headers: { 'content-type': 'application/json' },
    });

  it('reads the tailnet identity from the engine session, without credentials', async () => {
    const fetcher = vi.fn(async (_url: RequestInfo | URL, _init?: RequestInit) =>
      json(200, { session: { user: { email: 'ada@example.com', name: 'Ada' } } }),
    );
    expect(await fetchIdentity('https://loams.tail1.ts.net', fetcher)).toEqual({
      ok: true,
      user: 'ada@example.com',
    });
    const [url, init] = fetcher.mock.calls[0] ?? [];
    expect(url).toBe(
      'https://loams.tail1.ts.net/loams.auth.v1.AuthService/GetSession?connect=v1&encoding=json&message=%7B%7D',
    );
    expect(init?.credentials).toBe('omit');
    expect(
      await fetchIdentity('https://e', async () =>
        json(200, { session: { user: { name: 'Ada' } } }),
      ),
    ).toEqual({ ok: true, user: 'Ada' });
    expect(
      await fetchIdentity('https://e', async () =>
        json(200, { session: { user: { id: 'dev-loopback', name: 'Local developer' } } }),
      ),
    ).toEqual({ ok: true, user: 'Local developer', authOff: true });
    const refused = await fetchIdentity('https://e', async () => json(401, {}));
    expect(refused).toEqual({ ok: false, message: expect.stringMatching(/tailnet/) });
    const empty = await fetchIdentity('https://e', async () => json(200, { user: {} }));
    expect(empty.ok).toBe(false);
  });

  it('shows the tailnet identity, or why there is none, and no sign-out', () => {
    const base = {
      engine: 'https://loams.tail1.ts.net',
      authMode: 'tailscale' as const,
      demo: false,
      setDemo: vi.fn(),
    };
    const { container, rerender } = render(
      <HostedHeader watch={idle} session={{ ...base, user: 'ada@example.com' }} />,
    );
    expect(container.querySelector('.lc-hosted-user')?.textContent).toBe(
      'ada@example.com via tailnet',
    );
    expect(screen.queryByRole('button', { name: 'Sign out' })).toBeNull();
    // B20: an engine with sign-in off is not a tailnet login.
    rerender(
      <HostedHeader watch={idle} session={{ ...base, user: 'Local developer', authOff: true }} />,
    );
    expect(container.querySelector('.lc-hosted-user')?.textContent).toBe(
      'Local developer · sign-in off',
    );
    rerender(<HostedHeader watch={idle} session={{ ...base, identityProblem: 'nope' }} />);
    const unknown = container.querySelector('.lc-hosted-user-unknown');
    expect(unknown?.textContent).toBe('Tailnet user unknown');
    expect(unknown?.getAttribute('title')).toBe('nope');
  });

  it('labels an API key session and offers to forget the key', () => {
    const signOut = vi.fn();
    render(
      <HostedHeader
        watch={idle}
        session={{
          engine: 'https://loams.tail1.ts.net',
          authMode: 'apikey',
          demo: false,
          setDemo: vi.fn(),
          signOut,
        }}
      />,
    );
    expect(screen.getByText('API key')).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Forget key' }));
    expect(signOut).toHaveBeenCalled();
  });

  it('offers the modes with Tailscale first, and lists what is coming soon without links', () => {
    const onConnect = vi.fn();
    render(
      <ConnectScreen
        modes={AUTH_MODES}
        initialMode="tailscale"
        check={async () => ({ ok: true, status: 200, ms: 1 })}
        onConnect={onConnect}
      />,
    );
    const radios = screen.getAllByRole('radio') as HTMLInputElement[];
    expect(radios.map((r) => r.value)).toEqual(['tailscale', 'oidc', 'apikey']);
    expect(radios[0]?.checked).toBe(true);
    expect(screen.getByText('Loams account')).toBeTruthy();
    // No API key field until apikey is picked.
    expect(screen.queryByLabelText('API key')).toBeNull();
    fireEvent.click(radios[2] as HTMLInputElement);
    expect(screen.getByLabelText('API key')).toBeTruthy();

    const soon = screen.getByRole('region', { name: 'Coming soon' });
    for (const item of [
      /own Vercel or Cloudflare/,
      /Register your own sign-in client/,
      /Headscale/,
    ]) {
      expect(soon.textContent).toMatch(item);
    }
    expect(soon.querySelectorAll('a')).toHaveLength(0);
    expect(document.body.innerHTML).not.toContain('/register');
  });

  it('says what is wrong with the config', () => {
    render(<ConfigErrorScreen message="bad mode" />);
    expect(screen.getByText('bad mode')).toBeTruthy();
    expect(screen.getByText('This console is not set up yet')).toBeTruthy();
  });

  it('remembers the picked mode, and only a known one', () => {
    saveMode('apikey');
    expect(savedMode()).toBe('apikey');
    localStorage.setItem('loams.auth.mode', 'password');
    expect(savedMode()).toBeUndefined();
  });

  it('stops at the config error screen for an unknown mode, never falling back', async () => {
    const root = document.createElement('div');
    document.body.append(root);
    const start = vi.fn();
    const config = parseRuntimeConfig({ authMode: 'password', engineUrl: 'https://e.ts.net' });
    expect(await startHosted(config, root, start)).toBeUndefined();
    await waitFor(() => expect(root.textContent).toContain('This console is not set up yet'));
    expect(root.textContent).toContain('"password"');
    expect(start).not.toHaveBeenCalled();
  });

  it('first run asks how to sign in, even with a fixed engine', async () => {
    const root = document.createElement('div');
    document.body.append(root);
    const start = vi.fn();
    const config = parseRuntimeConfig({ hosted: true, engineUrl: 'https://loams.tail1.ts.net' });
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => json(200, {})),
    );
    void startHosted(config, root, start);
    await waitFor(() => expect(root.textContent).toContain('Connect your engine'));
    // No client in this config: oidc is not offered; Tailscale is the default.
    const radios = [...root.querySelectorAll('input[type=radio]')] as HTMLInputElement[];
    expect(radios.map((r) => r.value)).toEqual(['tailscale', 'apikey']);
    expect(radios[0]?.checked).toBe(true);
    expect(start).not.toHaveBeenCalled();
  });

  it('asks again for a remembered engine the page cannot reach (plain http)', async () => {
    saveMode('tailscale');
    saveEngine('http://10.0.0.5:8080');
    const root = document.createElement('div');
    document.body.append(root);
    const start = vi.fn();
    const config = parseRuntimeConfig({ hosted: true });
    // An https page, as console.loams.dev is.
    vi.stubGlobal('location', new URL('https://console.loams.dev/'));
    void startHosted(config, root, start);
    await waitFor(() => expect(root.textContent).toContain('Connect your engine'));
    expect((root.querySelector('input[type=text]') as HTMLInputElement).value).toBe('');
    expect(start).not.toHaveBeenCalled();
  });

  it('a remembered engine that is down opens the connect screen, not the console', async () => {
    saveMode('tailscale');
    // The engine is down behind its gateway: /health answers 502.
    const fetcher = vi.fn(async () => json(502, {}));
    vi.stubGlobal('fetch', fetcher);
    const root = document.createElement('div');
    document.body.append(root);
    const start = vi.fn();
    const config = parseRuntimeConfig({
      authMode: 'tailscale',
      engineUrl: 'https://loams.tail1.ts.net',
    });
    void startHosted(config, root, start);
    await waitFor(() => expect(root.textContent).toContain('Connect your engine'));
    expect(fetcher).toHaveBeenCalledWith(
      'https://loams.tail1.ts.net/health',
      expect.objectContaining({ credentials: 'omit' }),
    );
    expect(start).not.toHaveBeenCalled();
  });

  it('tailscale: no sign-in or connect screen, the engine names the user', async () => {
    saveMode('tailscale');
    const fetcher = vi.fn(async () =>
      json(200, { session: { user: { email: 'ada@example.com' } } }),
    );
    vi.stubGlobal('fetch', fetcher);
    const root = document.createElement('div');
    document.body.append(root);
    const start = vi.fn(async () => ({ dispose: async () => {} }) as never);
    const config = parseRuntimeConfig({
      authMode: 'tailscale',
      engineUrl: 'https://loams.tail1.ts.net',
    });
    await startHosted(config, root, start);
    expect(fetcher).toHaveBeenCalledWith(
      'https://loams.tail1.ts.net/loams.auth.v1.AuthService/GetSession?connect=v1&encoding=json&message=%7B%7D',
      expect.objectContaining({ credentials: 'omit' }),
    );
    expect(start).toHaveBeenCalledTimes(1);
    expect(root.textContent).not.toContain('Sign in');
  });
});
