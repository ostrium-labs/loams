import { ipcMain, session } from "electron";
import { CH } from "../../shared/contracts";
import { assertTrustedSender } from "../security/policy";
import { getMainWindow } from "../shell/main-window";
import { clearSessionCookies, serverHandlers } from "./handlers";
import type { ServerRegistry } from "./registry";

export function registerServerIpc(registry: ServerRegistry): void {
	const h = serverHandlers(registry, {
		clearCookies: (origin) =>
			clearSessionCookies(session.defaultSession.cookies, origin),
		reload: () => getMainWindow()?.webContents.reloadIgnoringCache(),
	});
	ipcMain.handle(CH.serversList, (e) => {
		assertTrustedSender(e);
		return h.list();
	});
	ipcMain.handle(CH.serversAdd, (e, entry: unknown) => {
		assertTrustedSender(e);
		return h.add(entry);
	});
	ipcMain.handle(CH.serversRemove, (e, id: unknown) => {
		assertTrustedSender(e);
		return h.remove(id);
	});
	ipcMain.handle(CH.serversActivate, (e, id: unknown) => {
		assertTrustedSender(e);
		return h.activate(id);
	});
}
