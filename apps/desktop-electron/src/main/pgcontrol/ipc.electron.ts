// App settings > Postgres control plane (PG2 R9.1). Every value is checked in
// settings.ts; the key file is read only in main, and no handler returns its contents.
import {
	BrowserWindow,
	dialog,
	ipcMain,
	type OpenDialogOptions,
} from "electron";
import { CH, type PgControlSettings } from "../../shared/contracts";
import { assertTrustedSender } from "../security/policy";
import { getMainWindow } from "../shell/main-window";
import {
	pgControlView,
	savePgControlSettings,
	testPgControl,
} from "./settings";

export function registerPgControlIpc(o: { settingsFile: () => string }): void {
	ipcMain.handle(CH.pgControlSettings, (e) => {
		assertTrustedSender(e);
		return pgControlView(o.settingsFile());
	});
	ipcMain.handle(CH.pgControlSave, (e, input: unknown) => {
		assertTrustedSender(e);
		const r = savePgControlSettings(o.settingsFile(), input);
		// The console's Postgres page drops its client and probes again: requests
		// already read the target per call, so nothing needs a restart.
		if (r.ok) broadcast(r.value);
		return r;
	});
	ipcMain.handle(CH.pgControlTest, (e, input: unknown) => {
		assertTrustedSender(e);
		return testPgControl(input, (u, init) => globalThis.fetch(u, init));
	});
	ipcMain.handle(CH.pgControlPickKeyFile, async (e) => {
		assertTrustedSender(e);
		const opts: OpenDialogOptions = {
			title: "Choose the pg-control API key file",
			properties: ["openFile", "showHiddenFiles"],
		};
		const w = BrowserWindow.fromWebContents(e.sender);
		const r = w
			? await dialog.showOpenDialog(w, opts)
			: await dialog.showOpenDialog(opts);
		return r.canceled ? null : (r.filePaths[0] ?? null);
	});
}

/** To the console window only: other windows (factory apps) are not the console's. */
function broadcast(s: PgControlSettings): void {
	const w = getMainWindow();
	if (w && !w.isDestroyed()) w.webContents.send(CH.pgControlEvent, s);
}
