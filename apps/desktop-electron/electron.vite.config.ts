// Adapted from dataelement/dsh-desktop (MIT), electron.vite.config.ts.
import { resolve } from "node:path";
import { defineConfig, externalizeDepsPlugin } from "electron-vite";

// Main and preload only: the renderer is the console build (web/apps/console).
const def = (k: string) => ({
	[`process.env.${k}`]: JSON.stringify(process.env[k] ?? ""),
});

export default defineConfig({
	main: {
		define: { ...def("LOAMS_UPDATE_PUBKEY"), ...def("LOAMS_UPDATE_FEED") },
		// The workspace adapters are raw TypeScript: bundle them (and cordis, zod)
		// so the packaged app does not need the workspace.
		plugins: [
			externalizeDepsPlugin({
				exclude: [
					"cordis",
					"zod",
					"@loams-core/http",
					"@loams-core/host",
					// SQL drivers: bundled so the packaged app needs no node_modules.
					"pg",
					"mysql2",
					...[
						"forgejo",
						"zulip",
						"itsaplan",
						"glitchtip",
						"openpanel",
						"matomo",
						"langfuse",
					].map((a) => `@loams-plugins/plugin-${a}-adapter`),
				],
			}),
		],
	},
	preload: {
		plugins: [externalizeDepsPlugin()],
		build: {
			rollupOptions: {
				input: { index: resolve("src/preload/index.ts") },
				output: { format: "cjs", entryFileNames: "[name].cjs" },
			},
		},
	},
});
