// Adapted from dataelement/dsh-desktop (MIT), src/main/update/update-manager.ts.
import { app, BrowserWindow, ipcMain, powerMonitor, shell } from "electron";
import { autoUpdater } from "electron-updater";
import { CH, type LoamsDesktopApi } from "../../shared/contracts";
import { assertTrustedSender } from "../security/policy";
import { verifyManifest } from "./manifest";
import {
	CHECK_INTERVAL_MS,
	channelFile,
	compareVersions,
	decideUpdateMode,
	firstCheckDelayMs,
	parseManifestVersion,
	releaseUrl,
	type UpdateMode,
} from "./policy";
import { UPDATE_FEED, UPDATE_PUBKEY_HEX } from "./pubkey";

type State = Awaited<ReturnType<LoamsDesktopApi["update"]["state"]>>;

const MAX_MANIFEST_BYTES = 256 * 1024;

export interface UpdaterHandle {
	state(): State;
	stop(): void;
}

async function fetchBytes(url: string): Promise<Uint8Array> {
	const res = await fetch(url, { redirect: "follow" });
	if (!res.ok) throw new Error(`${res.status} ${url}`);
	const buf = new Uint8Array(await res.arrayBuffer());
	if (buf.length > MAX_MANIFEST_BYTES) throw new Error("manifest too large");
	return buf;
}

class Unsigned extends Error {}

/** Fetch `<feed>/<channelFile>` and its `.sig`; throws Unsigned unless the signature verifies. */
async function fetchVerifiedVersion(feed: string): Promise<string> {
	const file = channelFile(process.platform, process.arch);
	const base = feed.replace(/\/+$/, "");
	const [yml, sig] = await Promise.all([
		fetchBytes(`${base}/${file}`),
		fetchBytes(`${base}/${file}.sig`).catch(() => new Uint8Array()),
	]);
	if (
		!(await verifyManifest(
			yml,
			new TextDecoder().decode(sig),
			UPDATE_PUBKEY_HEX,
		))
	)
		throw new Unsigned("unsigned_manifest");
	const v = parseManifestVersion(new TextDecoder().decode(yml));
	if (!v) throw new Error("manifest has no version");
	return v;
}

export function startUpdater(opts: {
	/** Called before quitAndInstall so the engine can stop. */
	prepareToInstall?: () => Promise<void>;
}): UpdaterHandle {
	const mode: UpdateMode = decideUpdateMode({
		packaged: app.isPackaged,
		feed: UPDATE_FEED,
		pubkeyHex: UPDATE_PUBKEY_HEX,
		platform: process.platform,
		appImage: process.env.APPIMAGE,
	});
	let state: State = { phase: mode === "disabled" ? "disabled" : "idle" };
	if (mode !== "disabled") state.mode = mode;
	let checking = false;
	let timers: NodeJS.Timeout[] = [];

	const set = (s: State): void => {
		state = mode === "disabled" ? s : { ...s, mode };
		for (const w of BrowserWindow.getAllWindows())
			if (!w.isDestroyed()) w.webContents.send(CH.updateState, state);
	};
	const fail = (e: unknown): void =>
		set({
			phase: "error",
			message: e instanceof Unsigned ? "unsigned_manifest" : errMsg(e),
		});

	const check = async (): Promise<void> => {
		if (mode === "disabled" || checking) return;
		if (state.phase === "downloading" || state.phase === "ready") return;
		checking = true;
		set({ phase: "checking" });
		try {
			// The manifest is verified first in every mode: an unsigned feed is never offered.
			const version = await fetchVerifiedVersion(UPDATE_FEED);
			if (compareVersions(version, app.getVersion()) <= 0) {
				set({ phase: "idle" });
				return;
			}
			if (mode === "self") await autoUpdater.checkForUpdates();
			else set({ phase: "available", version });
		} catch (e) {
			fail(e);
		} finally {
			checking = false;
		}
	};

	const download = async (): Promise<void> => {
		if (mode === "disabled" || state.phase !== "available") return;
		const version = state.version;
		if (!version) return;
		if (mode === "manual") {
			// D677: only an https release page is ever opened.
			await shell.openExternal(releaseUrl(version));
			return;
		}
		try {
			await fetchVerifiedVersion(UPDATE_FEED); // re-verify right before downloading
			await autoUpdater.downloadUpdate();
		} catch (e) {
			fail(e);
		}
	};

	if (mode === "self") {
		autoUpdater.setFeedURL({ provider: "generic", url: UPDATE_FEED });
		autoUpdater.autoDownload = false;
		autoUpdater.autoInstallOnAppQuit = false;
		autoUpdater.allowDowngrade = false;
		autoUpdater.allowPrerelease = false;
		autoUpdater.on("update-available", (info) =>
			set({ phase: "available", version: info.version }),
		);
		autoUpdater.on("update-not-available", () => set({ phase: "idle" }));
		autoUpdater.on("download-progress", (p) =>
			set({ phase: "downloading", version: state.version, percent: p.percent }),
		);
		autoUpdater.on("update-downloaded", (info) => {
			autoUpdater.autoInstallOnAppQuit = true;
			set({ phase: "ready", version: info.version });
		});
		autoUpdater.on("error", fail);
	}

	const install = async (): Promise<void> => {
		if (mode !== "self" || state.phase !== "ready") return;
		try {
			await opts.prepareToInstall?.();
			autoUpdater.quitAndInstall(false, true);
		} catch (e) {
			fail(e);
		}
	};

	ipcMain.handle(CH.updateState, (e) => {
		assertTrustedSender(e);
		return state;
	});
	ipcMain.handle(CH.updateCheck, async (e) => {
		assertTrustedSender(e);
		await check();
	});
	ipcMain.handle(CH.updateDownload, async (e) => {
		assertTrustedSender(e);
		await download();
	});
	ipcMain.handle(CH.updateInstall, async (e) => {
		assertTrustedSender(e);
		await install();
	});

	if (mode !== "disabled") {
		timers.push(
			setTimeout(() => void check(), firstCheckDelayMs(Math.random())),
			setInterval(() => void check(), CHECK_INTERVAL_MS),
		);
		powerMonitor.on("resume", () => void check());
	}
	return {
		state: () => state,
		stop: () => {
			for (const t of timers) {
				clearTimeout(t);
				clearInterval(t);
			}
			timers = [];
		},
	};
}

function errMsg(e: unknown): string {
	return e instanceof Error ? e.message : String(e);
}
