// The cordis console end to end in jsdom: the real catalog (base.yml), the
// real plugins, the browser platform over the in-memory mock.

import type { ConsoleHandle } from '@loams/console-host';
import { parsePatch, THIRD_PARTY_FLAG } from '@loams/console-host';
import { createMockTransport } from '@loams/console-host/testing';
import { createWebPlatform } from '@loams/platform-web';
import { pageEntries } from '@loams/plugin-shell';
import { waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vite-plus/test';
import samplesYml from '../catalog/samples.yml?raw';
import { startConsole } from '../src/cordis/start.js';

let dispose: (() => Promise<void>) | undefined;
afterEach(async () => {
  await dispose?.();
  dispose = undefined;
  document.body.innerHTML = '';
  window.location.hash = '';
});

async function start(
  features: Record<string, boolean> = {},
  grantDeclared = false,
  samples = false,
) {
  const root = document.createElement('div');
  document.body.append(root);
  const handle = await startConsole({
    platform: createWebPlatform({ transport: createMockTransport({ features }) }),
    root,
    base: '/ui/',
    patches: samples ? [parsePatch(samplesYml)] : undefined,
    grant: grantDeclared ? (manifest) => manifest.permissions : undefined,
  });
  dispose = handle.dispose;
  return { root, handle };
}

/** Every page the plugins register (design 59 D1080: the sidebar pins only a few). */
const navLabels = (handle: { slots: ConsoleHandle['slots'] }) =>
  pageEntries(
    handle.slots.entries('shell.nav.section'),
    handle.slots.entries('console.nav'),
    undefined,
  ).map((p) => p.label);
/**
 * Waits until Studio's plugin is active. Design 59 (W10): every available mode's plugins load
 * at boot, after the boot mode's, so Studio's pages ("Studio settings") join the list a moment
 * after Console's; a list read before that raced the load (it passed on fast runners only).
 */
const studioLoaded = (handle: ConsoleHandle) =>
  waitFor(() => expect(handle.plugins().find((p) => p.id === 'studio')?.status).toBe('active'), {
    timeout: 10_000,
  });
/** The sidebar's pinned Pages. */
const pinnedLabels = (root: HTMLElement) =>
  [...root.querySelectorAll('.lc-pages ul a')].map((a) => a.textContent);

describe('the cordis console', () => {
  it('boots the oss catalog and renders the first-party pages', async () => {
    const { root, handle } = await start();
    await studioLoaded(handle);
    await waitFor(() =>
      expect(navLabels(handle)).toEqual([
        'Status',
        'Namespaces',
        'Deployments',
        'Approvals',
        // MK1 Task 9: one entry per mode (Console's Integrate, Studio's group).
        'The Seed Exchange',
        'The Seed Exchange',
        'Studio settings',
        'Preferences',
      ]),
    );
    // The sidebar pins Q932's defaults that this catalog serves (D1080).
    await waitFor(() => expect(pinnedLabels(root)).toEqual(['Deployments']));
    const status = Object.fromEntries(handle.plugins().map((p) => [p.id, p.status]));
    expect(status).toMatchObject({
      shell: 'active',
      rpc: 'active',
      identity: 'active',
      'stack-status': 'active',
      namespaces: 'active',
      approvals: 'active',
      deployments: 'active',
    });
    // The sample third-party plugin is demo-only (catalog/samples.yml).
    expect(handle.plugins().find((p) => p.id === 'hello')).toBeUndefined();
    await waitFor(() => expect(root.textContent).toContain('Demo instance'));

    window.location.hash = '#/approvals';
    window.dispatchEvent(new HashChangeEvent('hashchange'));
    await waitFor(() =>
      expect(root.textContent).toContain('Drop the collection docs in production'),
    );
    expect(root.querySelector('.lc-chip')?.textContent).toBe('2 approvals waiting');

    window.location.hash = '#/namespaces';
    window.dispatchEvent(new HashChangeEvent('hashchange'));
    await waitFor(() => expect(root.textContent).toContain('search-prod'));
  }, 20_000); // the whole oss catalog boots here; a cold macOS runner needs over 5 s

  it('disabling a plugin removes its page and nav entry without a reload', async () => {
    const { root, handle } = await start();
    // waitFor's 1 s default is too short on a cold macOS runner (as above).
    const slow = { timeout: 10_000 };
    await waitFor(() => expect(navLabels(handle)).toContain('Approvals'), slow);
    await studioLoaded(handle);
    await handle.disable('approvals');
    await waitFor(
      () =>
        expect(navLabels(handle)).toEqual([
          'Status',
          'Namespaces',
          'Deployments',
          'The Seed Exchange',
          'The Seed Exchange',
          'Studio settings',
          'Preferences',
        ]),
      slow,
    );
    expect(root.querySelector('.lc-chip')).toBeNull();
  }, 20_000);

  it('runs the third-party sample in a sandboxed frame when the instance allows it', async () => {
    const { root, handle } = await start({ [THIRD_PARTY_FLAG]: true }, true, true);
    expect(handle.plugins().find((p) => p.id === 'hello')?.tier).toBe('third-party');
    await waitFor(() => expect(navLabels(handle)).toContain('hello'));
    window.location.hash = '#/plugins/hello';
    window.dispatchEvent(new HashChangeEvent('hashchange'));
    const frame = await waitFor(() => {
      const f = root.querySelector('iframe');
      expect(f).toBeTruthy();
      return f as HTMLIFrameElement;
    });
    expect(frame.getAttribute('sandbox')).toBe('allow-scripts');
    expect(frame.getAttribute('src')).toBe('/ui/sandbox/frame.html#plugin=hello');
    expect(root.textContent).toContain('Unverified plugin: @loams/example-plugin-hello');
    expect(handle.plugins().find((p) => p.id === 'hello')?.granted).toEqual(['approvals:read']);
  });
});
