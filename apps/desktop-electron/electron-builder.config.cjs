// electron-builder configuration for Loams Desktop (plan AP1e Task 15).
// Run through `pnpm run package --linux|--mac|--win`; scripts/fetch-engine.mjs must have run first.
const feed = process.env.LOAMS_UPDATE_FEED;
const hasWindowsSigning = Boolean(process.env.WINDOWS_SIGN_KEYSTORE);

/** @type {import('electron-builder').Configuration} */
module.exports = {
	appId: "dev.loams.desktop",
	productName: "Loams Desktop",
	copyright: "Copyright (c) Loams contributors",
	protocols: [{ name: "Loams", schemes: ["loams"] }],
	asar: true,
	directories: { output: "dist", buildResources: "build" },
	// build/tray-*.png are read at runtime from app.getAppPath()/build (src/main/shell/tray.electron.ts),
	// so they ship inside the asar. App icons in build/ are consumed by electron-builder itself.
	files: ["out/**", "package.json", "build/tray-*.png"],
	// Later tasks add: connectors.json (Task 27) as further entries here.
	extraResources: [
		{ from: "../../web/apps/console/dist", to: "console" },
		{ from: "resources/bin", to: "bin" },
		{ from: "../../deploy/neon", to: "stacks/neon" },
		{ from: "../../deploy/wesql", to: "stacks/wesql" },
		{ from: "../../deploy/tikv", to: "stacks/tikv" },
		{ from: "resources/connectors.json", to: "connectors.json" },
	],
	artifactName: "loams-desktop-${version}-${os}-${arch}.${ext}",
	linux: {
		target: ["AppImage", "deb", "rpm", "pacman"].map((t) => ({ target: t })),
		category: "Development",
		executableName: "loams-desktop",
		icon: "build/icon.png",
		syncDesktopName: true,
		desktop: {
			entry: { MimeType: "x-scheme-handler/loams", StartupWMClass: "loams-desktop" },
		},
		maintainer: "Loams <hello@loams.dev>",
	},
	deb: {
		packageName: "loams-desktop", depends: ["libgtk-3-0", "libnotify4", "libnss3", "libxss1", "libxtst6", "xdg-utils", "libatspi2.0-0", "libuuid1", "libsecret-1-0"] },
	mac: {
		target: ["dmg", "zip"],
		category: "public.app-category.developer-tools",
		icon: "build/icon.icns",
		// Unsigned until the owner supplies Apple credentials (D677): no identity, no hardened
		// runtime, no notarization.
		identity: null,
		hardenedRuntime: false,
		notarize: false,
	},
	win: {
		target: [{ target: "nsis", arch: ["x64"] }],
		icon: "build/icon.ico",
		...(hasWindowsSigning ? { sign: "scripts/windows-sign.cjs" } : {}),
	},
	rpm: { packageName: "loams-desktop" },
	pacman: { packageName: "loams-desktop" },
	nsis: { oneClick: false, allowToChangeInstallationDirectory: true },
	// The runtime feed is set by setFeedURL from the build-time LOAMS_UPDATE_FEED constant;
	// this publish block only emits latest*.yml next to the artifacts when a feed is configured.
	publish: feed ? [{ provider: "generic", url: feed }] : null,
};
