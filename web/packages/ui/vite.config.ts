import { defineConfig } from 'vite-plus';

export default defineConfig({
  test: { environment: 'jsdom' },
  // `vp pack` (tsdown) replaces tsup (design 79 §5.1, plan VV1 item 4). Peer
  // dependencies and their subpaths (react, react/jsx-runtime) stay external by
  // default. `fixedExtension: false` keeps `dist/index.js` and `dist/index.d.ts`,
  // the paths `publishConfig.exports` names; tsdown's default is `.mjs`/`.d.mts`
  // (VV0 Task 1).
  pack: {
    entry: ['src/index.ts'],
    format: ['esm'],
    dts: true,
    clean: true,
    fixedExtension: false,
  },
});
