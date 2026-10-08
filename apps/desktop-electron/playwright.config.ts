import { defineConfig } from "@playwright/test";

// Electron smoke (Task 16). Run `pnpm build` and build the console first.
// LOAMS_BIN: a built `loams` (real-engine variant); unset or LOAMS_E2E_ENGINE=fake uses the fake engine.
// LOAMS_E2E_ELECTRON_ARGS: extra electron args, e.g. "--no-sandbox".
export default defineConfig({
	testDir: "test/e2e",
	testMatch: "*.spec.ts",
	timeout: 180_000,
	expect: { timeout: 15_000 },
	workers: 1,
	fullyParallel: false,
	retries: 0,
	reporter: [["list"]],
	outputDir: "test-results",
});
