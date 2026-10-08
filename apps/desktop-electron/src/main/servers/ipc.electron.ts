import { ipcMain } from "electron";
import { CH, type ServerEntry } from "../../shared/contracts";
import { assertTrustedSender } from "../security/policy";
import { getMainWindow } from "../shell/main-window";
import type { ServerRegistry } from "./registry";

export function registerServerIpc(registry: ServerRegistry): void {
	ipcMain.handle(CH.serversList, (e) => {
		assertTrustedSender(e);
		return registry.list();
	});
	ipcMain.handle(CH.serversAdd, (e, entry: Omit<ServerEntry, "id">) => {
		assertTrustedSender(e);
		return registry.add(entry);
	});
	ipcMain.handle(CH.serversRemove, (e, id: string) => {
		assertTrustedSender(e);
		return registry.remove(id);
	});
	ipcMain.handle(CH.serversActivate, (e, id: string) => {
		assertTrustedSender(e);
		const r = registry.activate(id);
		if (r.ok) getMainWindow()?.webContents.reloadIgnoringCache();
		return r;
	});
}
