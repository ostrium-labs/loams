// @vitest-environment node
// no_direct_base_ui_import_outside_ui (SD1 Task 16; docs/design/console-ui-stack.md, decision 2):
// Base UI is wrapped once, in @loams/ui. Plugins, other packages and the console import UI
// from @loams/ui and never from @base-ui/react. Also: the primitives adapted from T3 Code carry
// the §53 §9.2 header naming @loams/ui's own copy of the MIT text, and a provenance row (D912, D913).

import { createHash } from 'node:crypto';
import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs';
import { join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vite-plus/test';

const WEB = fileURLToPath(new URL('../../..', import.meta.url));
const UI = join(WEB, 'packages/ui');
const ROOTS = ['plugins', 'packages', 'apps/console/src', 'apps/console/test'].map((d) =>
  join(WEB, d),
);
const SKIP_DIRS = new Set(['node_modules', 'dist', 'gen']);
const SOURCE = /\.(?:[cm]?[jt]sx?)$/;
const BASE_UI =
  /(?:\bfrom\s*|\bimport\s*\(\s*|\bimport\s+|\brequire\s*\(\s*|\bexport\s*\*\s*from\s*)(['"])@base-ui\/react[^'"]*\1/g;

function* files(dir: string): Generator<string> {
  if (!existsSync(dir)) return;
  for (const name of readdirSync(dir)) {
    if (SKIP_DIRS.has(name) || name.startsWith('.')) continue;
    const path = join(dir, name);
    if (statSync(path).isDirectory()) yield* files(path);
    else yield path;
  }
}

const insideUi = (path: string) => !relative(UI, path).startsWith('..');

describe('no_direct_base_ui_import_outside_ui', () => {
  it('the scan catches each import form', () => {
    const planted = [
      "import { Dialog } from '@base-ui/react/dialog';",
      'import * as B from "@base-ui/react";',
      "const m = await import('@base-ui/react/menu');",
      "export * from '@base-ui/react/tabs';",
    ].join('\n');
    expect(planted.match(BASE_UI)).toHaveLength(4);
    expect("import { Dialog } from '@loams/ui';".match(BASE_UI)).toBeNull();
  });

  it('no source outside @loams/ui imports @base-ui/react', () => {
    const hits: string[] = [];
    for (const root of ROOTS) {
      for (const path of files(root)) {
        if (insideUi(path) || !SOURCE.test(path)) continue;
        for (const m of readFileSync(path, 'utf8').matchAll(BASE_UI)) {
          hits.push(`${relative(WEB, path)}: ${m[0]}`);
        }
      }
    }
    expect(hits).toEqual([]);
  });

  it('no package.json outside @loams/ui depends on @base-ui/react', () => {
    const hits: string[] = [];
    for (const root of ROOTS) {
      for (const path of files(root)) {
        if (insideUi(path) || !path.endsWith('package.json')) continue;
        const manifest = JSON.parse(readFileSync(path, 'utf8'));
        for (const field of ['dependencies', 'devDependencies', 'peerDependencies']) {
          if (manifest[field]?.['@base-ui/react']) hits.push(`${relative(WEB, path)} ${field}`);
        }
      }
    }
    expect(hits).toEqual([]);
  });
});

interface Row {
  path: string;
  upstreamPath: string;
  commit: string;
  upstreamSha256: string;
  verdict: string;
}

describe('primitives adapted from T3 Code', () => {
  const prov = JSON.parse(readFileSync(join(UI, 'provenance.json'), 'utf8')) as {
    upstream: string;
    license: string;
    copyright: string;
    archives: { commit: string; sha256: string }[];
    files: Row[];
  };

  it('names T3 Code, its licence, its copyright and the archive', () => {
    expect(prov.upstream).toBe('https://github.com/pingdotgg/t3code');
    expect(prov.license).toBe('MIT');
    expect(prov.copyright).toBe('Copyright (c) 2026 T3 Tools Inc.');
    const commits = new Set(prov.archives.map((a) => a.commit));
    for (const row of prov.files) expect(commits.has(row.commit)).toBe(true);
  });

  it.each(prov.files.map((row) => [row.path, row] as const))(
    '%s carries the header naming @loams/ui’s LICENSE.t3code',
    (_path, row) => {
      const lines = readFileSync(join(UI, row.path), 'utf8').split('\n');
      expect(lines[0]).toBe(
        '// Portions of this file are from T3 Code (https://github.com/pingdotgg/t3code),',
      );
      expect(lines[1]).toBe(
        `// ${row.upstreamPath} at commit ${row.commit.slice(0, 8)}, Copyright (c) 2026 T3 Tools Inc., MIT licence`,
      );
      expect(lines[2]).toBe(
        '// (see web/packages/ui/LICENSE.t3code). Modified by The Loams Authors:',
      );
      expect(row.upstreamSha256).toMatch(/^[0-9a-f]{64}$/);
      expect(['as-is', 'adapt', 'rewrite']).toContain(row.verdict);
    },
  );

  it('every file under src/ that carries the header has a row', () => {
    const rows = new Set(prov.files.map((r) => r.path));
    const headed = [...files(join(UI, 'src'))]
      .filter((p) => readFileSync(p, 'utf8').includes('Portions of this file are from T3 Code'))
      .map((p) => relative(UI, p));
    expect(headed.filter((p) => !rows.has(p))).toEqual([]);
  });

  it('has a row for every design 59 primitive (D1077)', () => {
    const rows = new Set(prov.files.map((r) => r.upstreamPath));
    const ts = new Set(['dialog-styles', 'sidebarState', 'toastHelpers']);
    const expected = [
      'alert',
      'alert-dialog',
      'autocomplete',
      'badge',
      'button',
      'checkbox',
      'collapsible-section-header',
      'combobox',
      'command',
      'dialog',
      'dialog-styles',
      'empty',
      'group',
      'input',
      'input-group',
      'kbd',
      'menu',
      'middle-truncate',
      'number-field',
      'popover',
      'preview-card',
      'radio-group',
      'scroll-area',
      'select',
      'separator',
      'sheet',
      'sidebar',
      'sidebarState',
      'skeleton',
      'spinner',
      'switch',
      'table',
      'textarea',
      'toast',
      'toastHelpers',
      'toggle-group',
      'tooltip',
    ].map((n) => `apps/web/src/components/ui/${n}.${ts.has(n) ? 'ts' : 'tsx'}`);
    expect(expected.filter((p) => !rows.has(p))).toEqual([]);
  });

  it('ships the MIT text and a NOTICE, and says so in its licence field', () => {
    const licence = readFileSync(join(UI, 'LICENSE.t3code'), 'utf8');
    const studio = readFileSync(join(WEB, 'packages/studio-ui/LICENSE.t3code'), 'utf8');
    const hash = (s: string) => createHash('sha256').update(s).digest('hex');
    expect(hash(licence)).toBe(hash(studio));
    expect(readFileSync(join(UI, 'NOTICE'), 'utf8')).toContain('T3 Tools Inc.');
    const manifest = JSON.parse(readFileSync(join(UI, 'package.json'), 'utf8'));
    expect(manifest.license).toBe('Apache-2.0 AND MIT');
    expect(manifest.files).toEqual(expect.arrayContaining(['LICENSE.t3code', 'NOTICE']));
  });
});
