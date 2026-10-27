// vendored_headers_present (SD1 Task 1; design §53 §9.2, D913).
import { describe, expect, it } from 'vite-plus/test';
import { listFiles, loadProvenance, PACKAGE_DIR, readPackageFile } from './provenance-data';

const MARK = '// Portions of this file are from T3 Code (https://github.com/pingdotgg/t3code),';

describe('vendored_headers_present', () => {
  const prov = loadProvenance();

  it('records at least one vendored file', () => {
    expect(prov.files.length).toBeGreaterThan(0);
  });

  it.each(prov.files.map((row) => [row.path, row] as const))(
    '%s starts with the §53 §9.2 header naming its upstream path and commit',
    (_path, row) => {
      const text = readPackageFile(row.path);
      const lines = text.split('\n');
      expect(lines[0]).toBe(MARK);
      expect(lines[1]).toBe(
        `// ${row.upstreamPath} at commit ${row.commit.slice(0, 8)}, Copyright (c) 2026 T3 Tools Inc., MIT licence`,
      );
      expect(lines[2]).toBe(
        '// (see web/packages/studio-ui/LICENSE.t3code). Modified by The Loams Authors:',
      );
      expect(lines[3]).toMatch(/^\/\/ .+\. Loams' changes are Apache-2\.0\.$/);
    },
  );

  it('every file under src/ that carries the header has a provenance row', () => {
    const rows = new Set(prov.files.map((row) => row.path));
    const headed = listFiles(`${PACKAGE_DIR}/src`)
      .map((path) => `src/${path}`)
      .filter((path) => readPackageFile(path).includes('Portions of this file are from T3 Code'));
    expect(headed.filter((path) => !rows.has(path))).toEqual([]);
  });

  it('LICENSE.t3code is the MIT text with T3 Tools Inc.’s notice', () => {
    const licence = readPackageFile('LICENSE.t3code');
    expect(licence.startsWith('MIT License\n\nCopyright (c) 2026 T3 Tools Inc.\n')).toBe(true);
    expect(licence).toContain(
      'The above copyright notice and this permission notice shall be included',
    );
  });
});
