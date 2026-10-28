import { defineConfig, devices } from '@playwright/test';

// Studio end to end on the web edition (SD1 Task 29): the console's dev server
// (the web transport works in development builds only until the deployment's
// CSP names agentd hosts, ruling T26-9), in demo mode (an in-browser console
// API with `studio` on), against a real `loams-agentd serve --remote` that each
// spec starts (test/e2e/fixtures/agentd.ts).
//
// LOAMS_AGENTD_BIN: the daemon (default: the workspace's debug build).
// LOAMS_E2E_CHROMIUM: a Chromium to run instead of Playwright's own download.
// LOAMS_E2E_PORT: the dev server's port (default 5191).
const port = Number(process.env.LOAMS_E2E_PORT ?? 5191);
const executablePath = process.env.LOAMS_E2E_CHROMIUM || undefined;

export default defineConfig({
  testDir: 'test/e2e',
  testMatch: '*.spec.ts',
  timeout: 180_000,
  expect: { timeout: 20_000 },
  // Each spec runs its own daemon; one worker keeps the machine's load (and so
  // the timings) the same in CI and locally.
  workers: 1,
  fullyParallel: false,
  retries: 0,
  forbidOnly: !!process.env.CI,
  reporter: [['list']],
  outputDir: 'test-results',
  use: {
    ...devices['Desktop Chrome'],
    baseURL: `http://127.0.0.1:${port}/`,
    // The daemon's certificate is self-signed for 127.0.0.1.
    ignoreHTTPSErrors: true,
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
    launchOptions: executablePath ? { executablePath } : {},
  },
  webServer: {
    // Vite+ (design 79 D1772): `vite` is aliased to vite-plus-core, which ships no
    // `vite` binary, so the dev server is `vp dev` (the same Vite dev server and flags).
    command: `pnpm exec vp dev --host 127.0.0.1 --port ${port} --strictPort`,
    url: `http://127.0.0.1:${port}/`,
    env: { LOAMS_UI_BASE: '/' },
    reuseExistingServer: !process.env.CI,
    timeout: 120_000,
    stdout: 'ignore',
    stderr: 'pipe',
  },
});
