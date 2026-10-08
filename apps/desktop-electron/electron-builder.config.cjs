// electron-builder configuration for Loams Desktop (plan AP1e Task 15).
// Run through `pnpm run package --linux|--mac|--win`; scripts/fetch-engine.mjs must have run first.
const feed = process.env.LOAMS_UPDATE_FEED;
const hasWindowsSigning = Boolean(process.env.WINDOWS_SIGN_KEYSTORE);
// The release workflow (desktop-electron-release.yml) sets these to "-unsigned" when the matching
// signing secrets are absent, so an unsigned file never carries a signed-looking name. Empty otherwise.
// The suffix is part of the file name that latest*.yml records, so manifests stay consistent.
const sfx = (name) => process.env[name] ?? "";
const named = (suffix) => `loams-desktop-\${version}-\${os}-\${arch}${suffix}.\${ext}`;

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
	appImage: { artifactName: named(sfx("LOAMS_SUFFIX_GPG")) },
	deb: {
		artifactName: named(sfx("LOAMS_SUFFIX_GPG")),
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
	rpm: { packageName: "loams-desktop", artifactName: named(sfx("LOAMS_SUFFIX_RPM")) },
	pacman: { packageName: "loams-desktop", artifactName: named(sfx("LOAMS_SUFFIX_GPG")) },
	// differentialPackage off: SignPath rewrites the installer after the build, which would leave a stale
	// .blockmap; without one electron-updater downloads the full installer.
	nsis: {
		oneClick: false,
		allowToChangeInstallationDirectory: true,
		differentialPackage: false,
		artifactName: named(sfx("LOAMS_SUFFIX_WIN")),
	},
	// The runtime feed is set by setFeedURL from the build-time LOAMS_UPDATE_FEED constant;
	// this publish block only emits latest*.yml next to the artifacts when a feed is configured.
	publish: feed ? [{ provider: "generic", url: feed }] : null,
};
