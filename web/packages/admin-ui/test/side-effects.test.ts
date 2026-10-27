// The pages' styles (admin.css) are imported by src/index.ts. A production build skips
// a side-effect-free barrel when pages import only its names, and took admin.css with it:
// the hosted setup screen and the org pages shipped unstyled. index.ts stays listed.
import { describe, expect, it } from 'vite-plus/test';
import pkg from '../package.json';

describe('package.json sideEffects', () => {
  it('keeps the index that imports admin.css', () => {
    expect(pkg.sideEffects).toEqual(expect.arrayContaining(['*.css', './src/index.ts']));
  });
});
