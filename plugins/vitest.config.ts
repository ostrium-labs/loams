import { defineConfig } from "vite-plus";

export default defineConfig({
  test: {
    // Vitest v4 compatibility: preserve mock call history.
    // Remove after tests no longer rely on calls from setup or earlier tests.
    // https://viteplus.dev/guide/vitest-v5#remove-unneeded-compatibility-settings
    // https://vitest.dev/guide/migration/#clearmocks-is-enabled-by-default
    clearMocks: false,
    watch: false,
    include: [
      "packages/*/tests/**/*.spec.ts",
      "packages/*/tests/**/*.spec.tsx",
      "packages/*/tests/**/*.test.ts",
      // Apps carry tests too (the Connect bridge lives in apps/server). They
      // were silently skipped before this glob existed: `vitest run <path>`
      // exits 1 with "No test files found" rather than reporting a failure.
      "apps/*/tests/**/*.spec.ts",
      "apps/*/tests/**/*.test.ts",
    ],
    globals: true,
    environment: "node",
    coverage: {
      provider: "v8",
      include: ["packages/*/src/**/*.ts", "apps/*/src/**/*.ts"],
      exclude: ["**/index.ts", "**/*.d.ts", "**/gen/**"],
    },
    testTimeout: 10000,
  },
});
