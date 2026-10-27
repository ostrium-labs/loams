// no_effect_imports (SD1 Task 1; design §53 §9.1, Global Constraints "No Effect").
// Scans @loams/studio-ui and @loams/plugin-studio (once it exists) for imports of Effect, T3
// Code's own packages and the dependencies Studio replaces or refuses.

import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vite-plus/test';
import { listFiles, PACKAGE_DIR, readPackageFile, WEB_DIR } from './provenance-data';

const FORBIDDEN: readonly RegExp[] = [
  /^effect(\/|$)/,
  /^@effect\//,
  /^@effect-atom\//,
  /^@t3tools\//,
  /^@legendapp\/list(\/|$)/,
  /^@tanstack\/react-router(\/|$)/,
  /^heic-to(\/|$)/,
];

const SOURCE = /\.(?:[cm]?[jt]sx?)$/;
const SPECIFIER =
  /(?:\bfrom\s*|\bimport\s*\(\s*|\bimport\s+|\brequire\s*\(\s*|\bexport\s*\*\s*from\s*)(['"])([^'"\n]+)\1/g;

function forbiddenSpecifiers(text: string): string[] {
  const hits: string[] = [];
  for (const match of text.matchAll(SPECIFIER)) {
    const spec = match[2] ?? '';
    if (FORBIDDEN.some((re) => re.test(spec))) hits.push(spec);
  }
  return hits;
}

const ROOTS = [PACKAGE_DIR, join(WEB_DIR, 'plugins', 'studio')];
const THIS_FILE = 'test/imports.test.ts';

describe('no_effect_imports', () => {
  it('the scanner catches each forbidden form', () => {
    const planted = [
      "import { Effect } from 'effect';",
      'import * as Schema from "effect/Schema";',
      "import { useAtom } from '@effect/atom-react';",
      "export * from '@effect-atom/atom';",
      "const c = await import('@t3tools/contracts');",
      "import '@legendapp/list';",
      "const r = require('@tanstack/react-router');",
      "import heic from 'heic-to';",
    ].join('\n');
    expect(forbiddenSpecifiers(planted)).toHaveLength(8);
    expect(
      forbiddenSpecifiers("import { x } from 'effective';\nimport '@tanstack/react-query';"),
    ).toEqual([]);
  });

  it('no source file imports Effect, T3 Code packages, @legendapp/list, TanStack Router or heic-to', () => {
    const hits: string[] = [];
    for (const root of ROOTS) {
      for (const rel of listFiles(root)) {
        if (!SOURCE.test(rel) || (root === PACKAGE_DIR && rel === THIS_FILE)) continue;
        const text = readFileSync(join(root, rel), 'utf8');
        for (const spec of forbiddenSpecifiers(text)) hits.push(`${join(root, rel)}: ${spec}`);
      }
    }
    expect(hits).toEqual([]);
  });

  it('no package.json depends on them', () => {
    const hits: string[] = [];
    for (const root of ROOTS) {
      let manifest: Record<string, Record<string, string> | undefined>;
      try {
        manifest = JSON.parse(readFileSync(join(root, 'package.json'), 'utf8'));
      } catch {
        continue;
      }
      for (const field of [
        'dependencies',
        'devDependencies',
        'peerDependencies',
        'optionalDependencies',
      ]) {
        for (const name of Object.keys(manifest[field] ?? {})) {
          if (FORBIDDEN.some((re) => re.test(name)))
            hits.push(`${root}/package.json ${field}: ${name}`);
        }
      }
    }
    expect(hits).toEqual([]);
    expect(readPackageFile('package.json')).toContain('"license": "Apache-2.0 AND MIT"');
  });
});
