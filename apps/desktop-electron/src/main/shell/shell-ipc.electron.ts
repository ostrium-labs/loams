import {
	app,
	BrowserWindow,
	clipboard,
	dialog,
	ipcMain,
	Notification,
	type OpenDialogOptions,
	shell,
} from "electron";
import { CH, type IpcResult } from "../../shared/contracts";
import { isSafeRoute } from "../../shared/deeplink";
import { assertTrustedSender, navigationDecision } from "../security/policy";

export interface ShellIpcDeps {
	onBadge: (n: number) => void;
	/** Notification click: focus the window. */
	onNotifyClick: (route?: string) => void;
	/** The console has painted (`shell:ready`): the splash gate shows the window (D1084). */
	onReady: () => void;
}

export function registerShellIpc(deps: ShellIpcDeps): void {
	ipcMain.handle(
		CH.shellOpenExternal,
		async (e, url: unknown): Promise<IpcResult<void>> => {
			assertTrustedSender(e);
			if (typeof url !== "string" || navigationDecision("", url) !== "external")
				return {
					ok: false,
					code: "invalid_url",
					message: "only http and https URLs can be opened",
				};
			await shell.openExternal(url);
			return { ok: true, value: undefined };
		},
	);
	ipcMain.handle(
		CH.shellNotify,
		(e, title: unknown, body: unknown, route: unknown) => {
			assertTrustedSender(e);
			if (typeof title !== "string" || !Notification.isSupported()) return;
			const n = new Notification({
				title: title.slice(0, 200),
				body: typeof body === "string" ? body.slice(0, 1000) : undefined,
			});
			const target = isSafeRoute(route) ? route : undefined;
			n.on("click", () => deps.onNotifyClick(target));
			n.show();
		},
	);
	ipcMain.handle(CH.shellClipboard, (e, text: unknown) => {
		assertTrustedSender(e);
		if (typeof text === "string") clipboard.writeText(text.slice(0, 1_000_000));
	});
	ipcMain.handle(CH.shellPickFolder, async (e, title: unknown): Promise<string | null> => {
		assertTrustedSender(e);
		const opts: OpenDialogOptions = {
			title: typeof title === "string" ? title.slice(0, 120) : "Choose a folder",
			properties: ["openDirectory", "createDirectory"],
		};
		const w = BrowserWindow.fromWebContents(e.sender);
		const r = w ? await dialog.showOpenDialog(w, opts) : await dialog.showOpenDialog(opts);
		return r.canceled ? null : (r.filePaths[0] ?? null);
	});
	ipcMain.handle(CH.shellReady, (e) => {
		assertTrustedSender(e);
		deps.onReady();
	});
	ipcMain.handle(CH.shellBadge, (e, count: unknown) => {
		assertTrustedSender(e);
		const n =
			typeof count === "number" && Number.isFinite(count)
				? Math.max(0, Math.floor(count))
				: 0;
		app.setBadgeCount(n);
		deps.onBadge(n);
	});
}
