// electron-builder configuration for Loams Desktop (plan AP1e Task 15).
// Run through `pnpm run package --linux|--mac|--win`; scripts/fetch-engine.mjs must have run first.
const feed = process.env.LOAMS_UPDATE_FEED;
const hasWindowsSigning = Boolean(process.env.WINDOWS_SIGN_KEYSTORE);
// The release workflow (desktop-electron-release.yml) sets these to "-unsigned" when the matching
// signing secrets are absent, so an unsigned file never carries a signed-looking name. Empty otherwise.
// The suffix is part of the file name that latest*.yml records, so manifests stay consistent.
const sfx = (name) => process.env[name] ?? "";
const named = (suffix) => `loams-desktop-\${version}-\${os}-\${arch}${suffix}.\${ext}`;

// macOS signing. With a Developer ID certificate (CSC_LINK, or CSC_NAME for one already in the
// keychain) electron-builder signs with it under the hardened runtime, and notarizes when the
// notary credentials are set too (APPLE_API_KEY + APPLE_API_KEY_ID + APPLE_API_ISSUER, or APPLE_ID +
// APPLE_APP_SPECIFIC_PASSWORD + APPLE_TEAM_ID; electron-builder reads them from the environment).
// Without a certificate the app is ad-hoc signed ("-"): no identity and no notarization, but a valid
// signature, which Apple Silicon requires before it runs an arm64 binary at all (an unsigned arm64
// app is reported as "damaged"). Users still clear the quarantine flag once (docs/release/desktop.md).
const hasAppleCertificate = Boolean(process.env.CSC_LINK || process.env.CSC_NAME);
const hasNotaryCredentials = Boolean(
	(process.env.APPLE_API_KEY && process.env.APPLE_API_KEY_ID && process.env.APPLE_API_ISSUER) ||
		(process.env.APPLE_ID && process.env.APPLE_APP_SPECIFIC_PASSWORD && process.env.APPLE_TEAM_ID),
);
// One macOS architecture per run: "arm64" or "x64" (the release builds x64 on an arm64 runner).
// Unset, electron-builder builds for the machine it runs on.
const macArch = process.env.LOAMS_DESKTOP_MAC_ARCH || undefined;
if (macArch && !["arm64", "x64"].includes(macArch)) {
	throw new Error(`LOAMS_DESKTOP_MAC_ARCH must be arm64 or x64, not ${macArch}`);
}
// The release of the Loams product (a vX.Y.Z tag) stamps its own version on the desktop build, so
// the app and its file names match the release they are attached to. Unset: package.json's version.
const version = process.env.LOAMS_DESKTOP_VERSION || undefined;
if (version !== undefined && !/^\d+\.\d+\.\d+(-[0-9A-Za-z.]+)?$/.test(version)) {
	throw new Error(`LOAMS_DESKTOP_VERSION must be X.Y.Z or X.Y.Z-pre, not ${version}`);
}

/** @type {import('electron-builder').Configuration} */
module.exports = {
	appId: "dev.loams.desktop",
	productName: "Loams Desktop",
	copyright: "Copyright (c) Loams contributors",
	protocols: [{ name: "Loams", schemes: ["loams"] }],
	asar: true,
	directories: { output: "dist", buildResources: "build" },
	...(version ? { extraMetadata: { version } } : {}),
	// build/tray-*.png are read at runtime from app.getAppPath()/build (src/main/shell/tray.electron.ts),
	// so they ship inside the asar. App icons in build/ are consumed by electron-builder itself.
	files: ["out/**", "package.json", "build/tray-*.png"],
	// Later tasks add: connectors.json (Task 27) as further entries here.
	extraResources: [
		{ from: "../../web/apps/console/dist", to: "console" },
		{ from: "resources/bin", to: "bin" },
		{ from: "../../deploy/loams-postgres-dev", to: "stacks/loams-postgres-dev" },
		{ from: "../../deploy/wesql", to: "stacks/wesql" },
		{ from: "../../deploy/tikv", to: "stacks/tikv" },
		{ from: "resources/connectors.json", to: "connectors.json" },
		// Attribution (SD1 T1-7, Task 28): the desktop's NOTICE and T3 Code's MIT licence
		// for the vendored Loams Studio code in the console bundle.
		{ from: "NOTICE", to: "NOTICE" },
		{ from: "../../web/packages/studio-ui/LICENSE.t3code", to: "LICENSE.t3code" },
		// dsh-desktop's MIT licence (CF1 Task 15, §10) and the splash with its loaders (D1084).
		{ from: "LICENSE.dsh-desktop", to: "LICENSE.dsh-desktop" },
		{ from: "build/splash.html", to: "splash/splash.html" },
		{ from: "build/loams-loader.gif", to: "splash/loams-loader.gif" },
		{ from: "build/loams-loader-dark.gif", to: "splash/loams-loader-dark.gif" },
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
		target: ["dmg", "zip"].map((target) => (macArch ? { target, arch: [macArch] } : { target })),
		category: "public.app-category.developer-tools",
		icon: "build/icon.icns",
		// Signs as electron-builder would, then rewrites resources/bin/SHA256SUMS from the signed
		// loams-agentd and loams and reseals the bundle (scripts/mac-sign.cjs, release fix B9).
		sign: require("./scripts/mac-sign.cjs"),
		...(hasAppleCertificate
			? {
					hardenedRuntime: true,
					entitlements: "build/entitlements.mac.plist",
					entitlementsInherit: "build/entitlements.mac.plist",
					notarize: hasNotaryCredentials,
				}
			: { identity: "-", hardenedRuntime: false, notarize: false }),
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
