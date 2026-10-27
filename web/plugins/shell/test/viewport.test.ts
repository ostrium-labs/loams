// @vitest-environment node
// Review Focus 4: a page sizes itself to the main region, never against the viewport or the
// old 56px header, now that a 44px top bar, an inspector and a drawer share the window. A bare
// page (sign-in, setup) may fill the viewport with a `viewport-ok: bare page` marker.

import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vite-plus/test';

const WEB = fileURLToPath(new URL('../../..', import.meta.url));
const ROOTS = ['plugins', 'packages/studio-ui', 'packages/admin-ui', 'packages/desktop-ui'].map(
  (d) => join(WEB, d),
);
const SKIP = new Set(['node_modules', 'dist', 'gen', 'test', 'tests']);
const VIEWPORT = /\b100(?:s|d|l)?vh\b|var\(--top,\s*56px\)/;

function* files(dir: string): Generator<string> {
  for (const name of readdirSync(dir)) {
    if (SKIP.has(name) || name.startsWith('.')) continue;
    const p = join(dir, name);
    if (statSync(p).isDirectory()) yield* files(p);
    else if (/\.(tsx?|css)$/.test(name) && !/\.(test|spec)\./.test(name)) yield p;
  }
}

describe('pages fill the main region (Review Focus 4)', () => {
  it('no page sizes itself against the viewport', () => {
    const hits: string[] = [];
    for (const root of ROOTS) {
      for (const path of files(root)) {
        if (path.includes(`${join('plugins', 'shell')}`)) continue;
        readFileSync(path, 'utf8')
          .split('\n')
          .forEach((line, i) => {
            if (VIEWPORT.test(line) && !line.includes('viewport-ok:'))
              hits.push(`${relative(WEB, path)}:${i + 1}`);
          });
      }
    }
    expect(hits).toEqual([]);
  });
});
