// The browser entry of the console (§37 §5, AP1a): index.html at the base
// path's root (`/ui/`, or `/` with LOAMS_UI_BASE=/). The original React
// console's pages are plugins now (API2 Task 10), and its old addresses
// (`/ui/teams`, `/ui/cordis.html`) move to the hash routes here (legacy.ts).
//
//   pnpm dev                                    → the server the dev server proxies
//                                                 (LOAMS_API, default loams-apps-mock
//                                                 on 127.0.0.1:8084), cookie session
//   pnpm dev, then open /?demo                  → demo mode (an in-browser mock)
//   console.loams.dev/?demo                     → demo mode on a hosted deploy,
//                                                 which is the only way in without a tailnet
//   VITE_LOAMS_APPS_URL=http://127.0.0.1:8084 \
//   VITE_LOAMS_DEV_BEARER=mock-access-usr_omar pnpm dev
//                                               → straight at the mock, with a bearer

// First, so the page never paints in the wrong theme.
import './theme.js';
import '@loams/ui/styles.css';
import '../tailwind.css';
import './console.css';

import { parsePatch, THIRD_PARTY_FLAG } from '@loams/console-host';
import { createMockTransport } from '@loams/console-host/testing';
import { createWebPlatform } from '@loams/platform-web';
import samplesYml from '../../catalog/samples.yml?raw';
import { demoRequested } from '../hosted/demo.js';
import {
  DEMO_PACKAGE,
  demoManifest,
  demoPatch,
  demoPlugin,
  servedPagesPatch,
  webManifests,
  webModules,
} from '../hosted/edition.js';
import { startHosted } from '../hosted/start.js';
import { loadRuntimeConfig } from '../runtime-config.js';
import { startDesktop } from './desktop.js';
import { legacyUrl } from './legacy.js';
import { startConsole } from './start.js';

// An old console address (`/ui/projects/docs`, `/ui/cordis.html?desktop`) becomes
// its hash route before anything reads the location.
const moved = legacyUrl(globalThis.location, import.meta.env.BASE_URL);
if (moved) globalThis.history.replaceState(null, '', moved);

const appsUrl = import.meta.env.DEV
  ? (import.meta.env.VITE_LOAMS_APPS_URL as string | undefined)
  : undefined;
const demo = demoRequested(globalThis.location.search);

const root = document.getElementById('root');
if (root) {
  loadRuntimeConfig(import.meta.env.BASE_URL)
    .then((config) => {
      // Dev preview of the desktop edition in a browser (`?desktop`): a fake
      // bridge from a dev-only module. `import.meta.env.DEV` is false in a
      // production build, so the branch and the dynamic import are dropped.
      if (import.meta.env.DEV && new URLSearchParams(globalThis.location.search).has('desktop')) {
        return import('./fake-desktop.js').then((m) => startDesktop(m.createFakeDesktop(), root));
      }
      // The Electron shell exposes `loamsDesktop` (preload); everything else is the web path.
      if (globalThis.loamsDesktop) return startDesktop(globalThis.loamsDesktop, root);

      // A console that is not served by the engine (a Vercel or Cloudflare deploy, the
      // Compose console; the hosted console.loams.dev is retired): sign in, connect an
      // engine, then boot.
      if (config.hosted && !demo && !appsUrl) return startHosted(config, root, startConsole);

      const platform = createWebPlatform(
        demo
          ? {
              transport: createMockTransport({
                features: { [THIRD_PARTY_FLAG]: true },
              }),
            }
          : {
              baseUrl: appsUrl ?? config.server,
              // The Connect snippets name the server the dev server proxies to.
              serverUrl:
                appsUrl ??
                config.server ??
                (import.meta.env.DEV
                  ? (import.meta.env.LOAMS_DEV_SERVER as string | undefined)
                  : undefined),
              devBearer: import.meta.env.DEV
                ? (import.meta.env.VITE_LOAMS_DEV_BEARER as string | undefined)
                : undefined,
            },
      );

      return startConsole({
        platform,
        root,
        // Demo: the sample third-party plugin (catalog/samples.yml), the mock's own
        // sample data, and the header that says so. Otherwise the pages the engine
        // serves (Data explorer, Streams & Links, Live, Graph, Analytics), as in the
        // hosted edition.
        patches: demo ? [parsePatch(samplesYml), demoPatch] : [servedPagesPatch()],
        ...(demo
          ? {
              extraModules: { [DEMO_PACKAGE]: async () => demoPlugin() },
              extraManifests: [demoManifest],
            }
          : { extraModules: webModules, extraManifests: webManifests }),
        // Demo only: grant a sandboxed plugin what it declares (start.ts).
        grant: demo ? (manifest) => manifest.permissions : undefined,
      });
    })
    .then((handle) => {
      // The hosted path may stop at a sign-in or connect screen, or leave for the issuer.
      if (!handle) return;
      // For debugging in the browser console: the plugin table and the sweep.
      Object.assign(globalThis, { loamsConsole: handle });
      setTimeout(() => {
        const stuck = handle.pending().filter((p) => !p.silent);
        if (stuck.length > 0) console.warn('loams console: plugins still pending', stuck);
      }, 10_000);
    });
}
