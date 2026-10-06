
import { fileURLToPath } from "node:url";
import { defineConfig, lazyPlugins } from "vite-plus";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

const coreUiSource = fileURLToPath(new URL("../core/src/ui/index.ts", import.meta.url));
// Resolve the published subpath to its workspace source without assuming where
// pnpm installs symlinks. Deduplication keeps the shell and SPA on one React tree.
const coreUiAlias: Record<string, string> = {
  "@loams-plugins/core/ui": coreUiSource,
};

// https://vitejs.dev/config/
export default defineConfig({
  /*
   * Tailwind v4 is CSS-first: no `tailwind.config.js` anywhere. Tokens live in
   * the `@theme` block at the top of `src/styles.css`, and the component
   * layer is expressed as utilities in the TSX rather than as classes here.
   */
  plugins: lazyPlugins(() => [react(), tailwindcss()]),
  resolve: {
    alias: coreUiAlias,
    dedupe: ["react", "react-dom", "react-router", "@tanstack/react-query"],
  },
  /*
   * The shell owns the client-side routes `/`, `/plugins`, `/console` and
   * `/plugins/:id`, so a hard refresh or a pasted deep link has to be answered
   * with index.html instead of a 404. `spa` is Vite's default; setting it
   * explicitly keeps the history fallback correct if a preset or a future
   * `custom` appType is ever introduced here.
   */
  appType: "spa",
  server: {
    // Include root pnpm's real module paths as well as sibling UI source.
    fs: { allow: [fileURLToPath(new URL("../../../", import.meta.url))] },
    port: 5173,
    proxy: {
      "/api": {
        target: "http://localhost:3001",
        changeOrigin: true,
      },
    },
  },
});
