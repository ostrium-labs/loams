// Update orchestration without Electron imports, so the state machine is unit-tested
// with a fake electron-updater (D661, D677).
import type { LoamsDesktopApi } from "../../shared/contracts";
import {
	downloadedFileMatches,
	type InfoLike,
	matchesPinned,
	type Pinned,
	parsePinned,
} from "./pin";
import { compareVersions, type UpdateMode } from "./policy";

export type State = Awaited<ReturnType<LoamsDesktopApi["update"]["state"]>>;

export interface UpdaterLike {
	autoDownload: boolean;
	autoInstallOnAppQuit: boolean;
	checkForUpdates(): Promise<{
		isUpdateAvailable: boolean;
		updateInfo: InfoLike;
	} | null>;
	downloadUpdate(): Promise<unknown>;
	quitAndInstall(silent?: boolean, forceRunAfter?: boolean): void;
	on(event: "download-progress", cb: (p: { percent: number }) => void): unknown;
	on(
		event: "update-downloaded",
		cb: (e: { version: string; downloadedFile: string }) => void,
	): unknown;
	on(event: "error", cb: (e: Error) => void): unknown;
}

export interface ControllerDeps {
	mode: UpdateMode;
	appVersion: string;
	updater: UpdaterLike;
	/** Fetches the manifest and its signature; rejects on transport failure. */
	fetchFeed: () => Promise<{ yml: Uint8Array; sig: Uint8Array }>;
	verify: (yml: Uint8Array, sigB64: string) => Promise<boolean>;
	hashFile: (file: string) => Promise<string>;
	deleteFile: (file: string) => Promise<void>;
	openRelease: (version: string) => Promise<void>;
	prepareToInstall?: () => Promise<void>;
	restartEngine?: () => Promise<void>;
	onState: (s: State) => void;
	log?: (...a: unknown[]) => void;
	/** Injectable timer for the install watchdog. */
	schedule?: (fn: () => void, ms: number) => () => void;
}

/** quitAndInstall in electron-updater 6.8.9 never throws: it emits "error" and returns. */
export const INSTALL_WATCHDOG_MS = 10_000;

export interface Controller {
	state(): State;
	check(): Promise<void>;
	download(): Promise<void>;
	install(): Promise<void>;
	/** True once a downloaded file passed the pinned-hash check and is waiting to be installed. */
	hasVerifiedDownload(): boolean;
	/** Quit-time install (silent, no relaunch). Resolves true when quitAndInstall was invoked. */
	installOnQuit(): Promise<boolean>;
}

export function createController(d: ControllerDeps): Controller {
	const log = d.log ?? (() => undefined);
	let state: State = { phase: d.mode === "disabled" ? "disabled" : "idle" };
	let pinned: Pinned | null = null;
	let verifiedDownload = false;
	let checking = false;
	let installing = false;
	const schedule =
		d.schedule ??
		((fn, ms) => {
			const t = setTimeout(fn, ms);
			t.unref?.();
			return () => clearTimeout(t);
		});

	const installFailed = async (detail?: unknown): Promise<void> => {
		if (!installing) return;
		installing = false;
		verifiedDownload = false;
		await d.restartEngine?.().catch(() => undefined);
		fail("update_failed", detail);
	};

	const set = (s: State): void => {
		state = d.mode === "disabled" ? s : { ...s, mode: d.mode };
		d.onState(state);
	};
	const fail = (code: string, detail?: unknown): void => {
		if (detail !== undefined) log("update error", code, detail);
		set({ phase: "error", message: code });
	};

	if (d.mode === "self") {
		d.updater.autoDownload = false;
		d.updater.autoInstallOnAppQuit = false;
		d.updater.on("download-progress", (p) => {
			if (state.phase !== "downloading") return;
			set({ phase: "downloading", version: state.version, percent: p.percent });
		});
		d.updater.on("update-downloaded", (ev) => {
			void (async () => {
				const ok =
					pinned !== null &&
					ev.version === pinned.version &&
					(await downloadedFileMatches(pinned, ev.downloadedFile, d.hashFile));
				if (!ok) {
					verifiedDownload = false;
					d.updater.autoInstallOnAppQuit = false;
					await d.deleteFile(ev.downloadedFile).catch(() => undefined);
					fail("download_hash_mismatch");
					return;
				}
				// autoInstallOnAppQuit stays false: electron-updater registers its quit
				// handler synchronously, before this hash finishes. We install on quit ourselves.
				verifiedDownload = true;
				set({ phase: "ready", version: ev.version });
			})();
		});
		d.updater.on("error", (e) => {
			if (installing) void installFailed(e);
			else fail("update_failed", e);
		});
	}

	return {
		state: () => state,
		async check() {
			if (d.mode === "disabled" || checking) return;
			if (state.phase === "downloading" || state.phase === "ready") return;
			checking = true;
			set({ phase: "checking" });
			try {
				let files: { yml: Uint8Array; sig: Uint8Array };
				try {
					files = await d.fetchFeed();
				} catch (e) {
					fail("feed_unreachable", e);
					return;
				}
				const sig = new TextDecoder().decode(files.sig);
				const p = (await d.verify(files.yml, sig))
					? parsePinned(files.yml)
					: null;
				if (!p) {
					fail("unsigned_manifest");
					return;
				}
				if (compareVersions(p.version, d.appVersion) <= 0) {
					set({ phase: "idle" });
					return;
				}
				if (d.mode === "manual") {
					pinned = p;
					set({ phase: "available", version: p.version });
					return;
				}
				// electron-updater re-fetches the yml itself: what it resolved must equal what we verified.
				const r = await d.updater.checkForUpdates();
				if (!r?.isUpdateAvailable) {
					set({ phase: "idle" });
					return;
				}
				if (!matchesPinned(r.updateInfo, p)) {
					fail("unsigned_manifest");
					return;
				}
				pinned = p;
				set({ phase: "available", version: p.version });
			} catch (e) {
				fail("update_failed", e);
			} finally {
				checking = false;
			}
		},
		async download() {
			if (d.mode === "disabled" || state.phase !== "available") return;
			const version = state.version;
			if (!version || !pinned) return;
			if (d.mode === "manual") {
				await d.openRelease(version);
				return;
			}
			verifiedDownload = false;
			d.updater.autoInstallOnAppQuit = false;
			set({ phase: "downloading", version, percent: 0 });
			try {
				await d.updater.downloadUpdate();
			} catch (e) {
				fail("update_failed", e);
			}
		},
		hasVerifiedDownload: () => verifiedDownload && state.phase === "ready",
		async install() {
			await runInstall(false, true);
		},
		installOnQuit: () => runInstall(true, false),
	};

	async function runInstall(
		silent: boolean,
		runAfter: boolean,
	): Promise<boolean> {
		if (
			d.mode !== "self" ||
			state.phase !== "ready" ||
			!verifiedDownload ||
			installing
		)
			return false;
		installing = true;
		try {
			await d.prepareToInstall?.();
		} catch (e) {
			installing = false;
			fail("update_failed", e);
			return false;
		}
		// If the app has not quit shortly after, the install did not start.
		schedule(() => void installFailed("install watchdog"), INSTALL_WATCHDOG_MS);
		try {
			d.updater.quitAndInstall(silent, runAfter);
		} catch (e) {
			await installFailed(e);
			return false;
		}
		return true;
	}
}
