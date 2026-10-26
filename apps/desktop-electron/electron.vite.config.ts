// Adapted from dataelement/dsh-desktop (MIT), electron.vite.config.ts.
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { defineConfig, externalizeDepsPlugin } from "electron-vite";

// Main and preload only: the renderer is the console build (web/apps/console).
const def = (k: string, fallback = "") => ({
	[`process.env.${k}`]: JSON.stringify(process.env[k] || fallback),
});

// The update public key is pinned in update-pubkey.txt (the first non-comment line); the
// LOAMS_UPDATE_PUBKEY environment variable overrides it for local test builds only.
const pinnedUpdateKey = (): string =>
	readFileSync(resolve(__dirname, "update-pubkey.txt"), "utf8")
		.split(/\r?\n/)
		.map((l) => l.trim())
		.find((l) => l && !l.startsWith("#")) ?? "";

export default defineConfig({
	main: {
		define: {
			...def("LOAMS_UPDATE_PUBKEY", pinnedUpdateKey()),
			...def("LOAMS_UPDATE_FEED"),
		},
		build: {
			rollupOptions: {
				// `pg`'s optional native binding. Bundled, Vite replaces the missing optional
				// peer with a stub that throws at load in development builds ("Could not
				// resolve "pg-native" imported by "pg""). External, it stays a lazy `require`
				// that `pg` only reaches for `pg.native` and catches when it is absent.
				external: ["pg-native"],
			},
		},
		// The workspace adapters are raw TypeScript: bundle them (and cordis, zod)
		// so the packaged app does not need the workspace.
		plugins: [
			externalizeDepsPlugin({
				exclude: [
					"cordis",
					"zod",
					// The local shim answers the console API over Connect (API2 Task 10).
					"@bufbuild/protobuf",
					"@loams/proto",
					// Main watches the daemon for Studio's tray and notifications (SD1 Task 28).
					"@connectrpc/connect",
					"@connectrpc/connect-web",
					"@loams-core/http",
					"@loams-core/host",
					// SQL drivers: bundled so the packaged app needs no node_modules.
					"pg",
					"pg-cursor",
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
