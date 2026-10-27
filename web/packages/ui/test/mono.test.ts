// @vitest-environment node
// D1075: monospace is for code, ids and SQL only, never for labels. Outside @loams/ui's own
// sources, a mono face (`font-mono`, `--loams-font-mono`, `loams-mono`) may appear only on a
// line that says why with `mono-ok: <code|id|sql|path|hash|number>`, or on a <code>, <pre>,
// <kbd> or <samp> element. Inside @loams/ui, the label primitives use the UI face. A file
// vendored `as-is` (its provenance row pins its hash) cannot carry a marker, so it is skipped.

import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vite-plus/test';

const WEB = fileURLToPath(new URL('../../..', import.meta.url));
const ROOTS = ['plugins', 'packages', 'apps/console/src'].map((d) => join(WEB, d));
const UI_SRC = join(WEB, 'packages/ui/src');
const SKIP = new Set(['node_modules', 'dist', 'gen', 'test', 'tests', '__tests__']);
const SOURCE = /\.(css|tsx?|jsx?)$/;
const MONO = /\bfont-mono\b|--loams-font-mono|\bloams-mono\b/;
const MARKED = /mono-ok:\s*(code|id|sql|path|hash|number)\b/;
// The mono face must be on the code element's own opening tag, not elsewhere on the line.
const ELEMENT = /<(code|pre|kbd|samp)\b[^>]*(\bfont-mono\b|--loams-font-mono|\bloams-mono\b)/;

// Files copied unchanged from upstream: `provenance.test.ts` pins their hash (T3-1).
const AS_IS = new Set(
  ['packages/studio-ui', 'packages/ui'].flatMap((pkg) => {
    const rows: { path: string; verdict: string }[] = JSON.parse(
      readFileSync(join(WEB, pkg, 'provenance.json'), 'utf8'),
    ).files;
    return rows.filter((r) => r.verdict === 'as-is').map((r) => join(WEB, pkg, r.path));
  }),
);

function* files(dir: string): Generator<string> {
  for (const name of readdirSync(dir)) {
    if (SKIP.has(name) || name.startsWith('.')) continue;
    const path = join(dir, name);
    if (statSync(path).isDirectory()) yield* files(path);
    else if (SOURCE.test(name) && !/\.(test|spec)\./.test(name)) yield path;
  }
}

function monoHits(): string[] {
  const hits: string[] = [];
  for (const root of ROOTS) {
    for (const path of files(root)) {
      if (!relative(UI_SRC, path).startsWith('..') || AS_IS.has(path)) continue;
      readFileSync(path, 'utf8')
        .split('\n')
        .forEach((line, i) => {
          if (MONO.test(line) && !MARKED.test(line) && !ELEMENT.test(line)) {
            hits.push(`${relative(WEB, path)}:${i + 1}`);
          }
        });
    }
  }
  return hits;
}

describe('monospace (D1075)', () => {
  it('the scan catches unmarked use and allows marked use', () => {
    expect(MONO.test("className='font-mono text-faint'")).toBe(true);
    expect(MARKED.test("className={/* mono-ok: id */ 'font-mono'}")).toBe(true);
    expect(MARKED.test("className={/* mono-ok: because */ 'font-mono'}")).toBe(false);
    expect(ELEMENT.test('<code className="font-mono">x</code>')).toBe(true);
    expect(ELEMENT.test('<span className="font-mono">Label</span><code>x</code>')).toBe(false);
  });

  it('labels outside @loams/ui do not use monospace', () => {
    expect(monoHits()).toEqual([]);
  });

  it("@loams/ui's label primitives use the UI face", () => {
    const css = readFileSync(join(UI_SRC, 'styles.css'), 'utf8');
    for (const selector of ['.loams-status {', '.loams-table th {', '.loams-stat > span {']) {
      const at = css.indexOf(selector);
      expect(at, selector).toBeGreaterThan(-1);
      expect(css.slice(at, css.indexOf('}', at)), selector).not.toContain('--loams-font-mono');
    }
    // Badge paints itself with utilities since CF1 Task 4 (its .loams-badge rule is gone).
    expect(css).not.toContain('.loams-badge {');
    expect(readFileSync(join(UI_SRC, 'badge.tsx'), 'utf8')).not.toMatch(MONO);
    const snippet = css.slice(css.indexOf('.loams-snippet {'));
    expect(snippet.slice(0, snippet.indexOf('}'))).toContain('--loams-font-mono');
  });
});
