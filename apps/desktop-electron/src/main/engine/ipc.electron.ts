import { dirname } from "node:path";
import { ipcMain, shell } from "electron";
import {
	CH,
	type EngineState,
	type IpcResult,
	type LiveStoreChoice,
} from "../../shared/contracts";
import { assertTrustedSender } from "../security/policy";
import { getMainWindow } from "../shell/main-window";
import type { EngineSupervisor } from "./supervisor";

/** The user's Live store choice, read and saved by the main process (ruling T23-8). */
export interface LiveStoreSetting {
	get(): LiveStoreChoice;
	set(choice: LiveStoreChoice): void;
}

export function registerEngineIpc(
	sup: EngineSupervisor,
	logFile: string,
	liveStore: LiveStoreSetting,
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
	ipcMain.handle(CH.engineLiveStore, (e) => {
		assertTrustedSender(e);
		return liveStore.get();
	});
	ipcMain.handle(
		CH.engineSetLiveStore,
		(e, choice: unknown): IpcResult<void> => {
			assertTrustedSender(e);
			if (choice !== "embedded" && choice !== "tikv-stack")
				return {
					ok: false,
					code: "invalid_argument",
					message: "live store must be embedded or tikv-stack",
				};
			try {
				liveStore.set(choice);
				return { ok: true, value: undefined };
			} catch (err) {
				return {
					ok: false,
					code: "io_error",
					message: (err as Error).message,
				};
			}
		},
	);
	sup.on("state", (s: EngineState) => {
		getMainWindow()?.webContents.send(CH.engineEvent, s);
	});
}
