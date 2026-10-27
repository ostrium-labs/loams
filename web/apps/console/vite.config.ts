import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import tailwindcss from '@tailwindcss/vite';
import react from '@vitejs/plugin-react';
import { configDefaults, defineConfig, type Plugin } from 'vite-plus';

// The engine serves the build at /ui (design §19 §3). In development the
// console API comes from loams-apps-mock (`cargo run -p loams-apps-mock`,
// port 8084), which serves the console API (loams.auth.v1, loams.admin.v1) and
// the app protos over Connect on one listener, unless LOAMS_API points at an
// engine.
const api = process.env.LOAMS_API ?? 'http://127.0.0.1:8084';

// Where the console is mounted. The engine serves it at /ui/ (design §19 §3);
// a standalone dev server can serve it at the root with LOAMS_UI_BASE=/.
const base = process.env.LOAMS_UI_BASE ?? '/ui/';

const require = createRequire(import.meta.url);

/**
 * Static files the cordis console serves beside itself (AP1a Task 6): the
 * sandbox frame and its runtime, and the sample third-party plugin. In dev
 * they are served from their packages; in a build they are emitted as assets.
 */
const SANDBOX_FILES: Record<string, string> = {
  'sandbox/frame.html': require.resolve('@loams/plugin-sandbox/frame.html'),
  'sandbox/runtime.js': require.resolve('@loams/plugin-sandbox/runtime.js'),
  'plugins/hello/client.js': require.resolve('@loams/example-plugin-hello/client'),
};

/**
 * Attribution shipped beside the bundle (SD1 T1-7): the root NOTICE and T3 Code's
 * MIT licence, since the console bundles code vendored into @loams/studio-ui and
 * @loams/ui (both packages carry the same LICENSE.t3code; the ui test checks they match).
 */
const ATTRIBUTION_FILES: Record<string, string> = {
  NOTICE: fileURLToPath(new URL('../../../NOTICE', import.meta.url)),
  'LICENSE.t3code': fileURLToPath(
    new URL('../../packages/studio-ui/LICENSE.t3code', import.meta.url),
  ),
};

function sandboxAssets(base: string): Plugin {
  return {
    name: 'loams-sandbox-assets',
    configureServer(server) {
      server.middlewares.use((req, res, next) => {
        const path = req.url?.split(/[?#]/)[0] ?? '';
        const file = path.startsWith(base) ? SANDBOX_FILES[path.slice(base.length)] : undefined;
        if (!file) return next();
        res.setHeader(
          'content-type',
          file.endsWith('.html') ? 'text/html; charset=utf-8' : 'text/javascript; charset=utf-8',
        );
        res.end(readFileSync(file));
      });
    },
    generateBundle() {
      for (const [fileName, file] of Object.entries({ ...SANDBOX_FILES, ...ATTRIBUTION_FILES })) {
        this.emitFile({ type: 'asset', fileName, source: readFileSync(file) });
      }
    },
  };
}

/**
 * The desktop preview (`?desktop`) reads the connector catalog the
 * desktop app ships, apps/desktop-electron/resources/connectors.json, which is
 * generated and not checked in. The dev server builds it on start when it is
 * missing (about half a second), so the Connectors page works without a manual
 * step; if that fails, the page explains how to build it.
 */
function connectorCatalog(): Plugin {
  const desktop = fileURLToPath(new URL('../../../apps/desktop-electron/', import.meta.url));
  return {
    name: 'loams-connector-catalog',
    apply: 'serve',
    configResolved(config) {
      if (config.mode === 'test') return;
      if (existsSync(`${desktop}resources/connectors.json`)) return;
      try {
        execFileSync(process.execPath, [`${desktop}scripts/connectors-catalog.mjs`], {
          cwd: desktop,
          stdio: 'ignore',
          timeout: 30_000,
        });
      } catch (e) {
        config.logger.warn(`connectors.json was not generated: ${(e as Error).message}`);
      }
    },
  };
}

/**
 * The console's CSP, in production builds only (Vite's dev server injects
 * inline scripts): no `unsafe-eval` and no remote source (§37 §6.3).
 * `wasm-unsafe-eval` lets WebAssembly compile (and nothing else): the code
 * viewers highlight with Shiki's WASM engine (@pierre/diffs `shiki-wasm`), and
 * without it every diff and file view rendered empty in a production build.
 */
const CSP =
  "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; " +
  "img-src 'self' data: blob:; font-src 'self'; connect-src 'self'; frame-src 'self'; " +
  "object-src 'none'; base-uri 'none'; form-action 'self'";

function cspMeta(): Plugin {
  return {
    name: 'loams-csp',
    apply: 'build',
    transformIndexHtml: {
      order: 'post',
      handler(html, ctx) {
        if (!/(?:^|\/)(?:index|cordis)\.html$/.test(ctx.filename)) return html;
        return html.replace(
          '<meta charset="utf-8" />',
          `<meta charset="utf-8" />\n    <meta http-equiv="Content-Security-Policy" content="${CSP}" />`,
        );
      },
    },
  };
}

export default defineConfig({
  base,
  // The server the dev server proxies to, for copy that shows a client where to connect.
  define: { 'import.meta.env.LOAMS_DEV_SERVER': JSON.stringify(api) },
  plugins: [react(), tailwindcss(), sandboxAssets(base), cspMeta(), connectorCatalog()],
  server: {
    port: 5173,
    proxy: {
      '/api': api,
      '/v1': api,
      // The durable API (promises and schedules) on the engine's main port (B12).
      '/durable/': api,
      // The app protos over Connect (`/loams.<package>.<Service>/<Method>`), as the
      // desktop shell proxies them: the `?desktop` preview's pages read them here.
      '^/loams\\.[a-z0-9_.]+\\.[A-Za-z0-9]+/': api,
      '/grpc.health.': api,
      '/.well-known': api,
      '/health': api,
      '/ready': api,
    },
  },
  build: {
    outDir: 'dist',
    // Everything is bundled: an air-gapped console loads nothing remote.
    assetsInlineLimit: 0,
    sourcemap: true,
    rollupOptions: {
      // The console (index.html), and cordis.html, its former address, which moves
      // to the root on load (API2 Task 10).
      input: { main: 'index.html', cordis: 'cordis.html' },
    },
  },
  test: {
    environment: 'jsdom',
    // Deployment tests use node:test and run in the package test command.
    // So do the browser tests (AP1d Task 4, D638), which need a real Chrome and
    // run under `test:browser`. Under jsdom `document.modelContext` does not
    // exist at all, which is the whole reason that harness is separate.
    // Studio's end-to-end specs are Playwright's (SD1 Task 29, `test:e2e`).
    exclude: [...configDefaults.exclude, 'deploy/**', 'browser/**', 'test/e2e/**'],
  },
});
