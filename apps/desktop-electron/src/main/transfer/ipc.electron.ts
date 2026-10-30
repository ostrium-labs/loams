// Transfers over IPC (design §85 §12, DQ1 Task 6): the file and save dialogs,
// the handlers, and the change broadcast to the console window. Keys live in
// the encrypted vault (`transfer:<serverId>`); no handler returns a token.
import { randomUUID } from "node:crypto";
import { join } from "node:path";
import {
	BrowserWindow,
	dialog,
	ipcMain,
	type OpenDialogOptions,
	type SaveDialogOptions,
} from "electron";
import {
	type IpcResult,
	TRANSFER_CH,
	type TransferServerSettings,
	type TransferSnapshot,
	type TransferTarget,
} from "../../shared/contracts";
import type { Vault } from "../factory/vault";
import { assertTrustedSender } from "../security/policy";
import type { ServerRegistry } from "../servers/registry";
import { getMainWindow } from "../shell/main-window";
import { type TransferKeys, TransferService } from "./service";
import { TransferSupervisor } from "./supervisor";

/** The vault as the transfer service's key store. */
export function vaultKeys(vault: Vault): TransferKeys {
	return {
		get persistent() {
			return vault.persistent;
		},
		get(serverId) {
			const e = vault.get(`transfer:${serverId}`);
			const token = e?.fields.token?.reveal();
			if (!e || !token) return null;
			const settings: TransferServerSettings = {};
			const mode = e.fields.mode?.reveal();
			if (mode === "auto" || mode === "h3" || mode === "h2") settings.mode = mode;
			const pin = e.fields.pin?.reveal();
			if (pin) settings.pin = pin;
			const ca = e.fields.caFile?.reveal();
			if (ca) settings.caFile = ca;
			return { token, settings };
		},
		set(serverId, token, settings) {
			const fields: Record<string, string> = { token };
			if (settings.mode) fields.mode = settings.mode;
			if (settings.pin) fields.pin = settings.pin;
			if (settings.caFile) fields.caFile = settings.caFile;
			vault.set(`transfer:${serverId}`, "", fields);
		},
	};
}

export interface TransferIpcDeps {
	userData: string;
	binary: () => string | null;
	spawn: typeof import("node:child_process").spawn;
	vault: Vault;
	registry: ServerRegistry;
}

const isTarget = (t: unknown): t is TransferTarget =>
	typeof t === "object" &&
	t !== null &&
	typeof (t as TransferTarget).serverId === "string" &&
	typeof (t as TransferTarget).ns === "string";

const bad = <T>(message: string): IpcResult<T> => ({
	ok: false,
	code: "bad_request",
	message,
});

export function registerTransferIpc(deps: TransferIpcDeps): {
	service: TransferService;
	supervisor: TransferSupervisor;
} {
	const supervisor = new TransferSupervisor({
		spawn: deps.spawn,
		binary: deps.binary,
		stateDir: join(deps.userData, "transfers"),
		sleep: (ms) => new Promise((r) => setTimeout(r, ms)),
		now: Date.now,
	});
	const service = new TransferService({
		supervisor,
		servers: () => deps.registry.list().servers,
		keys: vaultKeys(deps.vault),
		newId: randomUUID,
	});
	// Progress arrives up to 4 times a second per job; the window gets the
	// latest snapshot at most that often.
	let pending: TransferSnapshot | null = null;
	let timer: NodeJS.Timeout | null = null;
	service.on("change", (s: TransferSnapshot) => {
		pending = s;
		if (timer) return;
		timer = setTimeout(() => {
			timer = null;
			const w = getMainWindow();
			if (pending && w && !w.isDestroyed())
				w.webContents.send(TRANSFER_CH.event, pending);
			pending = null;
		}, 250);
	});

	const str = (v: unknown): v is string => typeof v === "string" && v.length > 0;
	ipcMain.handle(TRANSFER_CH.snapshot, (e) => {
		assertTrustedSender(e);
		return service.snapshot();
	});
	ipcMain.handle(TRANSFER_CH.hasToken, (e, serverId: unknown) => {
		assertTrustedSender(e);
		return str(serverId) && service.hasToken(serverId);
	});
	ipcMain.handle(
		TRANSFER_CH.setToken,
		(e, serverId: unknown, token: unknown, settings: unknown) => {
			assertTrustedSender(e);
			if (!str(serverId) || !str(token)) return bad("a server and a key are required");
			const s = (typeof settings === "object" && settings !== null
				? settings
				: {}) as TransferServerSettings;
			const clean: TransferServerSettings = {};
			if (s.mode === "auto" || s.mode === "h3" || s.mode === "h2") clean.mode = s.mode;
			if (typeof s.pin === "string" && s.pin) clean.pin = s.pin;
			if (typeof s.caFile === "string" && s.caFile) clean.caFile = s.caFile;
			return service.setToken(serverId, token, clean);
		},
	);
	ipcMain.handle(TRANSFER_CH.upload, async (e, target: unknown) => {
		assertTrustedSender(e);
		if (!isTarget(target)) return bad("a server and namespace are required");
		const opts: OpenDialogOptions = {
			title: "Choose a file to upload",
			properties: ["openFile"],
		};
		const w = BrowserWindow.fromWebContents(e.sender);
		const r = w ? await dialog.showOpenDialog(w, opts) : await dialog.showOpenDialog(opts);
		const path = r.canceled ? undefined : r.filePaths[0];
		if (!path) return { ok: true, value: null };
		return service.upload(target, path);
	});
	ipcMain.handle(
		TRANSFER_CH.download,
		async (e, target: unknown, tid: unknown, name: unknown) => {
			assertTrustedSender(e);
			if (!isTarget(target) || !str(tid)) return bad("a transfer is required");
			// The name is a label from the server: only its last component
			// becomes the suggested file name (D2443).
			const label = str(name) ? (name.split(/[\\/]/).pop() ?? "download") : "download";
			const opts: SaveDialogOptions = { title: "Save the download as", defaultPath: label };
			const w = BrowserWindow.fromWebContents(e.sender);
			const r = w ? await dialog.showSaveDialog(w, opts) : await dialog.showSaveDialog(opts);
			if (r.canceled || !r.filePath) return { ok: true, value: null };
			return service.download(target, tid, label, r.filePath);
		},
	);
	for (const [ch, fn] of [
		[TRANSFER_CH.pause, (id: string) => service.pause(id)],
		[TRANSFER_CH.resume, (id: string) => service.resume(id)],
		[TRANSFER_CH.cancel, (id: string) => service.cancel(id)],
	] as const) {
		ipcMain.handle(ch, (e, id: unknown) => {
			assertTrustedSender(e);
			return str(id) ? fn(id) : bad("a transfer id is required");
		});
	}
	ipcMain.handle(TRANSFER_CH.dismiss, (e, id: unknown) => {
		assertTrustedSender(e);
		if (str(id)) service.dismiss(id);
	});
	ipcMain.handle(TRANSFER_CH.remoteList, (e, target: unknown) => {
		assertTrustedSender(e);
		return isTarget(target) ? service.remoteList(target) : bad("a target is required");
	});
	ipcMain.handle(
		TRANSFER_CH.startRun,
		(e, target: unknown, job: unknown, params: unknown) => {
			assertTrustedSender(e);
			if (!isTarget(target) || !str(job)) return bad("a target and a job are required");
			const p: Record<string, string> = {};
			if (typeof params === "object" && params !== null)
				for (const [k, v] of Object.entries(params))
					if (typeof v === "string") p[k] = v;
			return service.startRun(target, job, p);
		},
	);
	ipcMain.handle(
		TRANSFER_CH.followRun,
		(e, target: unknown, rid: unknown, job: unknown) => {
			assertTrustedSender(e);
			if (!isTarget(target) || !str(rid)) return bad("a target and a run are required");
			return service.followRun(target, rid, str(job) ? job : "run");
		},
	);
	return { service, supervisor };
}
