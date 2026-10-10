// Pure update decisions (D661, D677): no Electron imports so they are unit-tested.

export type UpdateMode = "disabled" | "self" | "manual";

export interface ModeInput {
	packaged: boolean;
	feed: string;
	pubkeyHex: string;
	platform: NodeJS.Platform;
	/** `process.env.APPIMAGE`: set only when running from an AppImage. */
	appImage: string | undefined;
}

export const FIRST_CHECK_MS = 15_000;
export const FIRST_CHECK_JITTER_MS = 30_000;
export const CHECK_INTERVAL_MS = 6 * 60 * 60 * 1000;

/**
 * "disabled": no feed, no key, or an unpackaged app.
 * "self": electron-updater downloads and installs (Windows, Linux AppImage).
 * "manual": the app only offers "Download vX" and opens the release page
 * (macOS: unsigned app, D677 amendment; Linux deb/rpm/pacman installs).
 */
export function decideUpdateMode(i: ModeInput): UpdateMode {
	if (!i.packaged) return "disabled";
	if (!/^https:\/\/|^http:\/\/(localhost|127\.0\.0\.1)[:/]/.test(i.feed))
		return "disabled";
	if (!/^[0-9a-fA-F]{64}$/.test(i.pubkeyHex)) return "disabled";
	if (i.platform === "win32") return "self";
	if (i.platform === "linux") return i.appImage ? "self" : "manual";
	return "manual";
}

export function channelFile(platform: NodeJS.Platform, arch: string): string {
	if (platform === "win32") return "latest.yml";
	if (platform === "darwin") return "latest-mac.yml";
	return arch === "arm64" ? "latest-linux-arm64.yml" : "latest-linux.yml";
}

export function parseManifestVersion(yml: string): string | null {
	const m = /^version:\s*['"]?([^'"\s#]+)['"]?\s*$/m.exec(yml);
	return m ? (m[1] ?? null) : null;
}

/** Minimal semver ordering: numeric core, a prerelease sorts before its release. */
export function compareVersions(a: string, b: string): number {
	const split = (v: string) => {
		const noBuild = v.replace(/^v/, "").split("+")[0] ?? "";
		const [core = "", pre] = noBuild.split(/-(.+)/);
		return { n: core.split(".").map((x) => Number.parseInt(x, 10) || 0), pre };
	};
	const x = split(a);
	const y = split(b);
	for (let i = 0; i < 3; i++) {
		const d = (x.n[i] ?? 0) - (y.n[i] ?? 0);
		if (d !== 0) return d;
	}
	if (x.pre === y.pre) return 0;
	if (x.pre === undefined) return 1;
	if (y.pre === undefined) return -1;
	return x.pre < y.pre ? -1 : 1;
}

/** `rand` in [0, 1). */
export function firstCheckDelayMs(rand: number): number {
	return FIRST_CHECK_MS + Math.floor(rand * FIRST_CHECK_JITTER_MS);
}

export function releaseUrl(version: string): string {
	return `https://github.com/ostrium-labs/loams/releases/tag/v${version}`;
}
