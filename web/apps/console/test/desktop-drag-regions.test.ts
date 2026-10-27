// The desktop top bar is the window's drag region (D1078), and a control inside a drag
// region swallows the click. This scans the real desktop console's top bar, in Studio on
// a thread (Studio's git tools, chips and the mode switch) and in Console, and checks
// that every element a person can click or focus there is covered by a `no-drag` rule
// in console.css, and that the bar itself is the drag region.

import type { LoamsDesktopApi } from '@loams/desktop/contracts';
import { waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it } from 'vite-plus/test';
import { startDesktop } from '../src/cordis/desktop.js';
import { createFakeDesktop } from '../src/cordis/fake-desktop.js';

// The tsconfig has no Node types, so node:fs, node:path and node:url load by a variable
// specifier (as tailwind.test.ts does); jsdom's URL is not Node's, so the path is resolved
// from the file's own path.
const ids = ['node:fs', 'node:path', 'node:url'] as const;
const [{ readFileSync }, { dirname, join }, { fileURLToPath }] = (await Promise.all(
  ids.map((id) => import(/* @vite-ignore */ id)),
)) as [
  { readFileSync(path: string, enc: 'utf8'): string },
  { dirname(p: string): string; join(...p: string[]): string },
  { fileURLToPath(url: string): string },
];
const css = readFileSync(
  join(dirname(fileURLToPath(import.meta.url)), '../src/cordis/console.css'),
  'utf8',
);

/** Splits a selector list on its top-level commas (not those inside `:is(…)`). */
function splitList(list: string): string[] {
  const parts: string[] = [];
  let depth = 0;
  let start = 0;
  for (let i = 0; i < list.length; i++) {
    const c = list[i];
    if (c === '(') depth++;
    else if (c === ')') depth--;
    else if (c === ',' && depth === 0) {
      parts.push(list.slice(start, i));
      start = i + 1;
    }
  }
  parts.push(list.slice(start));
  return parts.map((p) => p.replace(/\s+/g, ' ').trim()).filter(Boolean);
}

/** Every `.lc-topbar …` selector of a rule that sets `-webkit-app-region: no-drag`. */
function noDragSelectors(source: string): string[] {
  const out: string[] = [];
  const plain = source.replace(/\/\*[\s\S]*?\*\//g, '');
  for (const match of plain.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
    const [, selector = '', body = ''] = match;
    if (!/-webkit-app-region:\s*no-drag/.test(body)) continue;
    out.push(...splitList(selector).filter((s) => s.startsWith('.lc-topbar ')));
  }
  return out;
}

/** What a person can click or focus. */
const INTERACTIVE = [
  'button',
  'a[href]',
  'input',
  'select',
  'textarea',
  'summary',
  'label',
  '[role="button"]',
  '[role="combobox"]',
  '[role="tab"]',
  '[role="switch"]',
  '[role="checkbox"]',
  '[role="radio"]',
  '[role="menuitem"]',
  '[role="option"]',
  '[role="link"]',
  '[contenteditable="true"]',
  '[tabindex]:not([tabindex="-1"])',
].join(', ');

let dispose: (() => Promise<void>) | undefined;
beforeEach(() => {
  localStorage.clear();
  window.location.hash = '';
  // Base UI's scroll area asks for its running animations; jsdom has none.
  Element.prototype.getAnimations ??= () => [];
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
  window.matchMedia ??= ((query: string) => ({
    matches: query.includes('min-width'),
    media: query,
    addEventListener: () => {},
    removeEventListener: () => {},
  })) as unknown as typeof window.matchMedia;
});
afterEach(async () => {
  await dispose?.();
  dispose = undefined;
  document.body.innerHTML = '';
  window.location.hash = '';
  localStorage.clear();
});

async function start() {
  const api: LoamsDesktopApi = createFakeDesktop();
  api.shell.onCommand = () => () => {};
  const root = document.createElement('div');
  document.body.append(root);
  const handle = await startDesktop(api, root);
  dispose = handle.dispose;
}

/** Each interactive element in the top bar that no `no-drag` rule covers. */
function uncovered(selectors: string[]): string[] {
  const bar = document.querySelector('.lc-topbar');
  if (!bar) throw new Error('no top bar');
  const covered = selectors.join(', ');
  return [...bar.querySelectorAll<HTMLElement>(INTERACTIVE)]
    .filter((el) => !el.matches(covered))
    .map(
      (el) =>
        `<${el.tagName.toLowerCase()} ${el.getAttribute('aria-label') ?? el.textContent?.trim()}>`,
    );
}

describe('the desktop top bar', () => {
  const selectors = noDragSelectors(css);

  it('is the drag region, and its rules name the controls', () => {
    expect(css).toMatch(/\.lc-topbar \{[^}]*-webkit-app-region: drag;/);
    expect(selectors.length).toBeGreaterThan(0);
    // The rules themselves: a bare control inside the bar is covered.
    const bar = document.createElement('header');
    bar.className = 'lc-topbar';
    bar.innerHTML =
      '<button>b</button><a href="#">a</a><input /><span role="tab" tabindex="0">t</span>' +
      '<span role="switch" tabindex="0">s</span><div tabindex="0">f</div><label>l</label>';
    document.body.append(bar);
    expect(uncovered(selectors)).toEqual([]);
    bar.remove();
  });

  it('every control in Studio on a thread and in Console is no-drag', async () => {
    window.location.hash = '#/studio/t/demo-login';
    await start();
    // Studio's git tools join the bar once the thread's checkout is read.
    await waitFor(
      () => expect(document.querySelector('.lc-topbar .lc-studio-tools')).not.toBeNull(),
      { timeout: 15_000 },
    );
    expect(document.querySelectorAll('.lc-topbar button').length).toBeGreaterThan(5);
    expect(uncovered(selectors)).toEqual([]);

    window.location.hash = '#/';
    await waitFor(() => expect(document.querySelector('.lc-topbar .lc-studio-tools')).toBeNull(), {
      timeout: 15_000,
    });
    expect(uncovered(selectors)).toEqual([]);
  }, 40_000);
});
