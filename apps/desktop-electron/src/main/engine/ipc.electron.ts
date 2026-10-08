import { dirname } from "node:path";
import { ipcMain, shell } from "electron";
import { CH, type EngineState } from "../../shared/contracts";
import { assertTrustedSender } from "../security/policy";
import { getMainWindow } from "../shell/main-window";
import type { EngineSupervisor } from "./supervisor";

export function registerEngineIpc(
	sup: EngineSupervisor,
	logFile: string,
): void {
	ipcMain.handle(CH.engineState, (e) => {
		assertTrustedSender(e);
		return sup.state();
	});
	ipcMain.handle(CH.engineStart, (e) => {
		assertTrustedSender(e);
		sup.start();
	});
	ipcMain.handle(CH.engineStop, async (e) => {
		assertTrustedSender(e);
		await sup.stop();
	});
	ipcMain.handle(CH.engineLogs, async (e) => {
		assertTrustedSender(e);
		await shell.openPath(dirname(logFile));
	});
	sup.on("state", (s: EngineState) => {
		getMainWindow()?.webContents.send(CH.engineEvent, s);
	});
}
