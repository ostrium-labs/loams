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
		build: {
			rollupOptions: {
				// `pg`'s optional native binding. Bundled, Vite replaces the missing optional
				// peer with a stub that throws at load in development builds ("Could not
				// resolve "pg-native" imported by "pg""). External, it stays a lazy `require`
				// that `pg` only reaches for `pg.native` and catches when it is absent.
				external: ["pg-native"],
			},
		},
		// Bundle these (cordis, zod, the protos) so the packaged app does not need the
		// workspace. The factory's adapters run in loams-agentd (SF1 amendment F1).
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
					// SQL drivers: bundled so the packaged app needs no node_modules.
					"pg",
					"pg-cursor",
					"mysql2",
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
