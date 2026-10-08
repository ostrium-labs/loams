import { composeCatalog, parseCatalog, parsePatch } from '@loams/console-host';
import { describe, expect, it } from 'vitest';
import baseCatalog from '../catalog/base.yml?raw';
import desktopYml from '../catalog/desktop.yml?raw';
import { desktopManifests, desktopModules } from '../src/cordis/desktop.js';

describe('desktop edition', () => {
  it('desktop_catalog_parses and composes over the base catalog', () => {
    const patch = parsePatch(desktopYml);
    const base = parseCatalog(baseCatalog);
    const composed = composeCatalog(base, patch);
    expect(composed.length).toBeGreaterThanOrEqual(base.length);
    // Every row a desktop plugin task adds needs its module and manifest.
    for (const row of patch.flatMap((r) => ('insert' in r ? r.insert : []))) {
      expect(desktopModules).toHaveProperty([row.name]);
    }
    expect(Array.isArray(desktopManifests)).toBe(true);
  });
});
