// The thread's Preview tab (SD1 Task 26; §53 §5.8): the dev servers a daemon's
// `PreviewService.WatchPreviews` lists (an in-memory Connect router), opened in the
// desktop's view, in a sandboxed frame through the remote listener (Q806), or, where
// the agentd serves no previews, the port-forward command.

import { create } from '@bufbuild/protobuf';
import { createRouterTransport } from '@connectrpc/connect';
import { AgentdTransportProvider } from '@loams/console-host/query';
import {
  PreviewServerSchema,
  PreviewService,
  PreviewSnapshotSchema,
  ProjectService,
} from '@loams/proto/agentd';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeAll, describe, expect, it, vi } from 'vite-plus/test';
import {
  type DesktopPreviewApi,
  type DesktopPreviewState,
  desktopPreview,
  PreviewUnsupported,
  remotePreview,
  remotePreviewOpener,
  trimTrailingSlashes,
} from '../src/preview/access.js';
import { PreviewTab } from '../src/preview/PreviewTab.js';
import { frameSandbox } from '../src/preview/RemoteFrame.js';
import { wrap } from './fakes.js';

beforeAll(() => {
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver;
});
afterEach(cleanup);

const TICKET = `/preview/pt_${'a'.repeat(64)}/`;

function fakePreviews() {
  const calls: string[] = [];
  const transport = createRouterTransport(({ service }) => {
    service(PreviewService, {
      async *watchPreviews({ chatId }, ctx) {
        calls.push(`WatchPreviews:${chatId}`);
        yield {
          snapshot: create(PreviewSnapshotSchema, {
            proxyPort: 7331,
            projectName: 'my-app',
            services: [
              create(PreviewServerSchema, {
                id: 's2',
                name: 'web',
                hostname: 'dev.my-app.localhost',
                port: 5173,
                pid: 42,
                projectName: 'my-app',
                startedAtMs: 1n,
              }),
              create(PreviewServerSchema, {
                id: 's1',
                name: 'api',
                hostname: 'dev.my-app-api.localhost',
                port: 3000,
                pid: 43,
                projectName: 'my-app',
                startedAtMs: 1n,
              }),
              // Not the proxy's shape: never offered.
              create(PreviewServerSchema, {
                id: 's3',
                name: 'odd',
                hostname: 'evil.example.com',
                port: 1,
              }),
            ],
          }),
        };
        await new Promise<void>((resolve) => ctx.signal.addEventListener('abort', () => resolve()));
      },
    });
    service(ProjectService, {
      listProjectActions: ({ spaceId }) => {
        calls.push(`ListProjectActions:${spaceId}`);
        return { snapshot: { actions: [{ id: 'dev', name: 'Dev server' }] } };
      },
      runProjectAction: ({ spaceId, chatId, actionId }) => {
        calls.push(`RunProjectAction:${spaceId}:${chatId}:${actionId}`);
        return {
          run: { actionName: 'Dev server', terminal: { id: 't1', cwd: '/w', shell: 'sh' } },
        };
      },
    });
  });
  return { transport, calls };
}

function fakeView() {
  const listeners = new Set<(s: DesktopPreviewState) => void>();
  const view = {
    show: vi.fn(async (_url: string, _rect: object) => ({ ok: true as const, value: undefined })),
    hide: vi.fn(async () => undefined),
    reload: vi.fn(async () => undefined),
    back: vi.fn(async () => undefined),
    forward: vi.fn(async () => undefined),
    onState: (cb: (s: DesktopPreviewState) => void) => {
      listeners.add(cb);
      return () => listeners.delete(cb);
    },
  } satisfies DesktopPreviewApi;
  return {
    view,
    emit: (s: DesktopPreviewState) => {
      for (const l of listeners) l(s);
    },
  };
}

function show(props: Partial<Parameters<typeof PreviewTab>[0]> = {}) {
  const daemon = fakePreviews();
  render(
    wrap(
      <AgentdTransportProvider transport={daemon.transport}>
        <PreviewTab threadId="chat-7" {...props} />
      </AgentdTransportProvider>,
    ),
  );
  return daemon;
}

describe('Preview tab', () => {
  it('preview_lists_discovered_servers', async () => {
    const { view } = fakeView();
    const daemon = show({ preview: desktopPreview(view) });
    await screen.findByText('Local servers');
    const rows = [...document.querySelectorAll('[data-preview-server]')].map((r) =>
      r.getAttribute('data-preview-server'),
    );
    // Sorted by name; the hostname that is not the proxy's is left out.
    expect(rows).toEqual(['dev.my-app-api', 'dev.my-app']);
    expect(screen.getByText('dev.my-app.localhost')).toBeTruthy();
    expect(daemon.calls).toContain('WatchPreviews:chat-7');

    // Desktop: the loopback URL goes to main's view, placed over the surface.
    fireEvent.click(screen.getByText('web · my-app'));
    await waitFor(() => expect(view.show).toHaveBeenCalled());
    const [url, rect] = view.show.mock.calls[0] as unknown as [string, object];
    expect(url).toBe('http://dev.my-app.localhost:7331/');
    expect(Object.keys(rect).sort()).toEqual(['height', 'width', 'x', 'y']);
    expect(document.querySelector('[data-preview-surface="desktop"]')).toBeTruthy();
    expect(document.querySelector('iframe')).toBeNull();
    expect((screen.getByLabelText('Preview address') as HTMLInputElement).value).toBe(
      'http://dev.my-app.localhost:7331/',
    );

    // Closing returns to the list and hides the native view.
    fireEvent.click(screen.getByLabelText('Close preview'));
    await screen.findByText('Local servers');
    expect(view.hide).toHaveBeenCalled();
  });

  it('desktop_url_bar_opens_loopback_only', async () => {
    const { view } = fakeView();
    show({ preview: desktopPreview(view) });
    await screen.findByText('Local servers');
    const input = screen.getByLabelText('Preview address');
    fireEvent.focus(input);
    fireEvent.change(input, { target: { value: 'https://example.com/' } });
    fireEvent.submit(input.closest('form') as HTMLFormElement);
    await screen.findByText(/Only addresses on the daemon/);
    expect(view.show).not.toHaveBeenCalled();
    fireEvent.change(input, { target: { value: 'localhost:5173/app' } });
    fireEvent.submit(input.closest('form') as HTMLFormElement);
    await waitFor(() => expect(view.show).toHaveBeenCalled());
    expect(view.show.mock.calls[0]?.[0]).toBe('http://localhost:5173/app');
  });

  it('desktop_shows_unreachable_on_load_failure', async () => {
    const { view, emit } = fakeView();
    show({ preview: desktopPreview(view) });
    fireEvent.click(await screen.findByText('web · my-app'));
    await waitFor(() => expect(view.show).toHaveBeenCalled());
    emit({
      url: 'http://dev.my-app.localhost:7331/',
      canGoBack: false,
      canGoForward: false,
      loading: false,
      error: {
        code: -102,
        description: 'ERR_CONNECTION_REFUSED',
        url: 'http://dev.my-app.localhost:7331/',
      },
    });
    await screen.findByText(/can.t be reached/);
    expect(screen.getByText(/Connection refused/)).toBeTruthy();
    fireEvent.click(screen.getByText('Reload'));
    expect(view.reload).toHaveBeenCalled();
  });

  it('web_frames_the_remote_preview_sandboxed', async () => {
    const open = vi.fn(async (name: string) => ({
      url: `https://agentd.example.com${TICKET}`,
      expiresAtMs: Date.now() + 60_000,
      name,
    }));
    show({ preview: remotePreview(open) });
    fireEvent.click(await screen.findByText('api · my-app'));
    const frame = (await waitFor(() => {
      const f = document.querySelector('iframe');
      if (!f) throw new Error('no frame yet');
      return f;
    })) as HTMLIFrameElement;
    expect(open).toHaveBeenCalledWith('dev.my-app-api');
    expect(frame.getAttribute('src')).toBe(`https://agentd.example.com${TICKET}`);
    const sandbox = frame.getAttribute('sandbox') ?? '';
    expect(sandbox).toContain('allow-scripts');
    expect(sandbox).toContain('allow-same-origin');
    expect(sandbox).not.toMatch(/allow-top-navigation|allow-popups-to-escape-sandbox/);
    expect(frame.getAttribute('allow')).toBe('');
    expect(frame.getAttribute('referrerpolicy')).toBe('no-referrer');
    // The URL bar shows the server's own address, not the ticket.
    expect((screen.getByLabelText('Preview address') as HTMLInputElement).value).toBe(
      'http://dev.my-app-api.localhost:7331/',
    );
  });

  it('web_without_preview_shows_forward_hint', async () => {
    // No preview host at all (a daemon without Q806).
    show();
    await screen.findByText('Local servers');
    expect(document.querySelector('[data-preview-forward-hint]')).toBeTruthy();
    expect(screen.getByText('ssh -N -L 7331:localhost:7331 <agentd host>')).toBeTruthy();
    cleanup();

    // A remote host that answers that it serves none: the hint replaces the frame.
    show({
      preview: remotePreview(async () => {
        throw new PreviewUnsupported();
      }),
    });
    expect(document.querySelector('[data-preview-forward-hint]')).toBeNull();
    fireEvent.click(await screen.findByText('web · my-app'));
    await waitFor(() => expect(document.querySelector('[data-preview-forward-hint]')).toBeTruthy());
    expect(document.querySelector('iframe')).toBeNull();
  });

  it('starts_the_dev_action', async () => {
    const daemon = show({ spaceId: 'space-1' });
    fireEvent.click(await screen.findByText('Dev server'));
    await waitFor(() => expect(daemon.calls).toContain('RunProjectAction:space-1:chat-7:dev'));
  });
});

describe('remote preview tickets', () => {
  // Issue #1105 (CodeQL js/polynomial-redos): the base URL's trailing slashes are
  // trimmed in one pass; `/\/+$/` took quadratic time on a long run of `/`.
  it('trims the base URL in linear time', () => {
    expect(trimTrailingSlashes('https://a.example///')).toBe('https://a.example');
    expect(trimTrailingSlashes('https://a.example/x')).toBe('https://a.example/x');
    expect(trimTrailingSlashes('///')).toBe('');
    const start = performance.now();
    trimTrailingSlashes(`https://a.example${'/'.repeat(200_000)}x`);
    expect(performance.now() - start).toBeLessThan(1_000);
  });

  it('asks with the bearer token and no cookies, and accepts a ticket path only', async () => {
    const fetch = vi.fn(
      async () => new Response(JSON.stringify({ base: TICKET, expiresAtMs: 9 }), { status: 200 }),
    );
    const open = remotePreviewOpener({
      baseUrl: 'https://agentd.example.com:8443/',
      getToken: () => 'tok',
      fetch: fetch as unknown as typeof globalThis.fetch,
    });
    await expect(open('dev.my-app')).resolves.toEqual({
      url: `https://agentd.example.com:8443${TICKET}`,
      expiresAtMs: 9,
    });
    const [url, init] = fetch.mock.calls[0] as unknown as [string, RequestInit];
    expect(url).toBe('https://agentd.example.com:8443/preview/-/ticket');
    expect(init.method).toBe('POST');
    expect((init.headers as Record<string, string>).authorization).toBe('Bearer tok');
    expect(init.credentials).toBe('omit');
    expect(init.redirect).toBe('error');
    expect(JSON.parse(String(init.body))).toEqual({ preview: 'dev.my-app' });

    for (const base of ['https://evil.example/', '/preview/dev.my-app/', '//x/preview/pt_1/']) {
      fetch.mockResolvedValueOnce(new Response(JSON.stringify({ base }), { status: 200 }));
      await expect(open('dev.my-app')).rejects.toThrow(/unexpected/);
    }
    fetch.mockResolvedValueOnce(new Response('{}', { status: 501 }));
    await expect(open('dev.my-app')).rejects.toBeInstanceOf(PreviewUnsupported);
    await expect(
      remotePreviewOpener({ baseUrl: 'https://a.example', getToken: () => undefined })('a.b'),
    ).rejects.toThrow(/token/);
  });

  it('never gives a frame on the console origin its own origin', () => {
    expect(
      frameSandbox('https://agentd.example.com/preview/x/', 'https://console.example.com'),
    ).toContain('allow-same-origin');
    expect(
      frameSandbox('https://console.example.com/preview/x/', 'https://console.example.com'),
    ).not.toContain('allow-same-origin');
  });
});
