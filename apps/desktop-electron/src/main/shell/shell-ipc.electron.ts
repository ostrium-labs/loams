import { app, clipboard, ipcMain, Notification, shell } from "electron";
import { CH, type IpcResult } from "../../shared/contracts";
import { isSafeRoute } from "../../shared/deeplink";
import { assertTrustedSender, navigationDecision } from "../security/policy";

export interface ShellIpcDeps {
	onBadge: (n: number) => void;
	/** Notification click: focus the window. */
	onNotifyClick: (route?: string) => void;
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
