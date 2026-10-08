import { writeFile } from "node:fs/promises";
import { dialog, ipcMain } from "electron";
import { CH } from "../../shared/contracts";
import { assertTrustedSender } from "../security/policy";
import { getMainWindow } from "../shell/main-window";
import type { ConnectorCatalog } from "./catalog";
import { saveYaml } from "./save";

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
	ipcMain.handle(CH.connectorsSaveYaml, (e, name: unknown, text: unknown) => {
		assertTrustedSender(e);
		return saveYaml(
			{
				pick: async (defaultName) => {
					const opts = {
						defaultPath: defaultName,
						filters: [{ name: "YAML", extensions: ["yaml", "yml"] }],
					};
					const w = getMainWindow();
					const r = w
						? await dialog.showSaveDialog(w, opts)
						: await dialog.showSaveDialog(opts);
					return r.canceled ? undefined : r.filePath;
				},
				write: (path, data) => writeFile(path, data, "utf8"),
			},
			name,
			text,
		);
	});
}
