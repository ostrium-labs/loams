// Demo mode (`?demo`): the way into console.loams.dev for someone who has no
// engine and no tailnet, and the address the gate screens and the demo header use
// to turn it on and off.
//
// The boot below is the wiring main.tsx uses on that address: the in-browser mock
// of the app protos, the sample plugin catalog (catalog/samples.yml), and the
// header that says the data is sample data. There is no engine anywhere in it,
// which is the property worth holding: what ships as the public way in must not
// need one.

import type { ConsoleHandle } from '@loams/console-host';
import { parsePatch, THIRD_PARTY_FLAG } from '@loams/console-host';
import { createMockTransport } from '@loams/console-host/testing';
import { createWebPlatform } from '@loams/platform-web';
import { cleanup, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vite-plus/test';
import samplesYml from '../catalog/samples.yml?raw';
import { startConsole } from '../src/cordis/start.js';
import { DEMO_PARAM, demoHref, demoRequested } from '../src/hosted/demo.js';
import {
  DEMO_PACKAGE,
  DemoHeader,
  demoManifest,
  demoPatch,
  demoPlugin,
} from '../src/hosted/edition.js';
import { ConnectScreen } from '../src/hosted/screens.js';

let dispose: (() => Promise<void>) | undefined;
afterEach(async () => {
  cleanup();
  await dispose?.();
  dispose = undefined;
  document.body.innerHTML = '';
});

describe('the demo address', () => {
  it('is the `demo` query parameter, whatever it carries', () => {
    expect(DEMO_PARAM).toBe('demo');
    for (const search of ['?demo', '?demo=1', '?a=b&demo=0']) {
      expect(demoRequested(search)).toBe(true);
    }
    for (const search of ['', '?desktop', '?demonstrate=1']) {
      expect(demoRequested(search)).toBe(false);
    }
  });

  it('turns on and off without losing the rest of the address', () => {
    // Other parameters survive, through `URLSearchParams`: a valueless one comes back
    // as `name=`. Every real address this acts on is the bare page, or carries `demo`.
    const at = { pathname: '/ui/', search: '?desktop', hash: '#/studio' };
    expect(demoHref(true, at)).toBe('/ui/?desktop=&demo=1#/studio');
    expect(demoHref(false, { pathname: '/ui/', search: '?demo=1&desktop', hash: '' })).toBe(
      '/ui/?desktop=',
    );
    // The bare address, on and off: never `?` with nothing after it.
    expect(demoHref(true, { pathname: '/', search: '', hash: '' })).toBe('/?demo=1');
    expect(demoHref(false, { pathname: '/', search: '', hash: '' })).toBe('/');
  });
});

describe('the gate screens', () => {
  it('offer demo mode, so the first screen is not a dead end', () => {
    render(
      <ConnectScreen
        modes={['tailscale']}
        initialMode="tailscale"
        onConnect={() => {}}
        check={async () => ({ ok: false, kind: 'unreachable', message: 'down' })}
      />,
    );
    const link = screen.getByRole('link', { name: 'Open the console on sample data' });
    expect(link.getAttribute('href')).toBe('/?demo=1');
  });
});

describe('the demo header', () => {
  it('says the data is sample data, and links back out of demo mode', () => {
    render(<DemoHeader />);
    expect(screen.getByRole('status').textContent).toBe('Demo data');
    const leave = screen.getByRole('link', { name: 'Connect your own engine' });
    expect(leave.getAttribute('href')).toBe('/');
  });
});

describe('the demo edition', () => {
  it('boots with no engine at all, and shows the sample header', async () => {
    const root = document.createElement('div');
    document.body.append(root);
    const handle: ConsoleHandle = await startConsole({
      platform: createWebPlatform({
        transport: createMockTransport({ features: { [THIRD_PARTY_FLAG]: true } }),
      }),
      root,
      base: '/ui/',
      patches: [parsePatch(samplesYml), demoPatch],
      extraModules: { [DEMO_PACKAGE]: async () => demoPlugin() },
      extraManifests: [demoManifest],
      grant: (manifest) => manifest.permissions,
    });
    dispose = handle.dispose;

    await waitFor(() => expect(root.textContent).toContain('Demo data'));
    expect(root.querySelector('.lc-hosted-demo-on')?.textContent).toBe('Demo data');
    expect(root.textContent).toContain('Sample data, in this browser');
    // The way out, and none of the hosted header's engine furniture.
    expect(
      [...root.querySelectorAll('a')].map((a) => a.textContent).includes('Connect your own engine'),
    ).toBe(true);
    expect(root.querySelector('.lc-hosted-demo input')).toBeNull();
    expect(Object.fromEntries(handle.plugins().map((p) => [p.id, p.status]))).toMatchObject({
      'console-demo': 'active',
    });
  }, 20_000); // boots the whole catalog, as the hosted edition's test does
});
