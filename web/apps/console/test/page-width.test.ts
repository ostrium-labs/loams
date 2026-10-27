// @vitest-environment node
// Pages use the card's width (owner feedback 2026-10-17, design 59; CF1 ruling T7-6
// amended): no page is held to the old 1180px measure, and the wide cap is centred.
// jsdom does not lay out, so the contract is checked in the stylesheet.

import { describe, expect, it } from 'vite-plus/test';

// vitest returns '' for CSS `?raw` imports and the tsconfig has no Node types: read the
// stylesheet through node:fs by a variable specifier (as tailwind.test.ts does).
const fsId = 'node:fs';
const { readFileSync } = (await import(/* @vite-ignore */ fsId)) as {
  readFileSync(url: URL, enc: 'utf8'): string;
};
const css = readFileSync(new URL('../src/cordis/console.css', import.meta.url), 'utf8');

/** The declarations of the first rule whose selector is exactly `selector`. */
function rule(selector: string): string {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const match = css.match(new RegExp(`(?:^|\\n)${escaped}\\s*\\{([^}]*)\\}`));
  if (!match?.[1]) throw new Error(`no rule for ${selector}`);
  return match[1];
}

describe('page width', () => {
  it('a page fills the card up to 1600px, centred', () => {
    const main = rule('.lc-main > :not(.lc-home)');
    expect(main).toMatch(/max-width:\s*1600px/);
    expect(main).toMatch(/margin-inline:\s*auto/);
  });

  it('no rule holds a page to 1180px', () => {
    expect(rule('.lc-page')).not.toMatch(/max-width/);
    expect(css).not.toMatch(/max-width:\s*1180px/);
  });
});
