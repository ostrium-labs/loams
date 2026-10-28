// The preview pieces adapted from T3 Code (SD1 Task 26, inventory (n)).
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeAll, describe, expect, it, vi } from 'vite-plus/test';
import {
  describePreviewError,
  forwardCommand,
  mergeServers,
  openDiscoveredPort,
  PreviewEmptyState,
  PreviewUnreachable,
  parsePreviewUrl,
  previewNameOf,
} from '../src/preview';

beforeAll(() => {
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver;
});
afterEach(cleanup);

const server = (id: string, name: string, hostname: string, port: number) => ({
  id,
  name,
  hostname,
  port,
  pid: 1,
  cwd: '/w',
  projectName: 'app',
  startedAtMs: 0,
});

describe('discovered servers', () => {
  it('keeps the proxy hostnames, sorted, with their proxy URL', () => {
    const merged = mergeServers({
      proxyPort: 7331,
      servers: [
        server('a', 'web', 'dev.app.localhost', 5173),
        server('b', 'api', 'dev.app-api.localhost', 3000),
        server('c', 'x', 'example.com', 80),
        server('d', 'dup', 'dev.app.localhost', 5174),
      ],
    });
    expect(merged.map((s) => [s.previewName, s.url])).toEqual([
      ['dev.app-api', 'http://dev.app-api.localhost:7331/'],
      ['dev.app', 'http://dev.app.localhost:7331/'],
    ]);
  });

  it('reads preview names and URL bar input strictly', () => {
    expect(previewNameOf('dev.app.localhost')).toBe('dev.app');
    expect(previewNameOf('a.b.c.localhost')).toBeUndefined();
    expect(previewNameOf('-a.b.localhost')).toBeUndefined();
    expect(previewNameOf('dev.app.example')).toBeUndefined();
    expect(parsePreviewUrl('localhost:5173')).toBe('http://localhost:5173/');
    expect(parsePreviewUrl(':3000/x')).toBe('http://localhost:3000/x');
    expect(parsePreviewUrl('http://127.0.0.1:8080')).toBe('http://127.0.0.1:8080/');
    expect(parsePreviewUrl('https://example.com')).toBeUndefined();
    expect(parsePreviewUrl('javascript:alert(1)')).toBeUndefined();
    expect(parsePreviewUrl('file:///etc/passwd')).toBeUndefined();
    expect(parsePreviewUrl('http://u:p@localhost/')).toBeUndefined();
    expect(parsePreviewUrl('localhost.evil.example')).toBeUndefined();
  });

  it('resolves a server per reach', async () => {
    const [s] = mergeServers({
      proxyPort: 7331,
      servers: [server('a', 'web', 'dev.app.localhost', 1)],
    });
    if (!s) throw new Error('no server');
    expect(
      await openDiscoveredPort({ server: s, reach: { kind: 'loopback' }, proxyPort: 7331 }),
    ).toEqual({
      kind: 'loopback',
      url: 'http://dev.app.localhost:7331/',
    });
    const open = vi.fn(async () => ({ url: 'https://h/preview/pt_x/', expiresAtMs: 5 }));
    expect(
      await openDiscoveredPort({ server: s, reach: { kind: 'remote', open }, proxyPort: 7331 }),
    ).toEqual({ kind: 'remote', url: 'https://h/preview/pt_x/', expiresAtMs: 5 });
    expect(open).toHaveBeenCalledWith('dev.app');
    expect(forwardCommand(7331, 'box')).toBe('ssh -N -L 7331:localhost:7331 box');
  });
});

describe('preview states', () => {
  it('shows the empty state and the start action', () => {
    const onStart = vi.fn();
    render(
      <PreviewEmptyState
        servers={[]}
        onOpenServer={() => {}}
        startActions={[{ id: 'dev', name: 'Dev server' }]}
        onStart={onStart}
      />,
    );
    expect(screen.getByText('No preview yet')).toBeTruthy();
    fireEvent.click(screen.getByText('Dev server'));
    expect(onStart).toHaveBeenCalledWith({ id: 'dev', name: 'Dev server' });
  });

  it('describes an unreachable page as text', () => {
    const onReload = vi.fn();
    render(
      <PreviewUnreachable
        url="http://dev.app.localhost:7331/<img src=x onerror=alert(1)>"
        code={-102}
        description="ERR_CONNECTION_REFUSED"
        onReload={onReload}
      />,
    );
    expect(screen.getByText('dev.app.localhost:7331')).toBeTruthy();
    expect(document.querySelector('img')).toBeNull();
    fireEvent.click(screen.getByText('Reload'));
    expect(onReload).toHaveBeenCalled();
    expect(describePreviewError('ERR_PREVIEW_NOT_RUNNING')).toMatch(/not running/);
    expect(describePreviewError('')).toBe('Network error');
  });
});
