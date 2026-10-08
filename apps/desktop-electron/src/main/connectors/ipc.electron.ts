import { ipcMain } from "electron";
import { CH } from "../../shared/contracts";
import { assertTrustedSender } from "../security/policy";
import type { ConnectorCatalog } from "./catalog";

export function registerConnectorsIpc(catalog: ConnectorCatalog): void {
	ipcMain.handle(CH.connectorsCatalog, (e) => {
		assertTrustedSender(e);
		return catalog.catalog();
	});
	ipcMain.handle(CH.connectorsGet, (e, id: unknown) => {
		assertTrustedSender(e);
		return catalog.get(id);
	});
	ipcMain.handle(CH.connectorsValidate, (e, id: unknown, config: unknown) => {
		assertTrustedSender(e);
		return catalog.validate(id, config);
	});
}
