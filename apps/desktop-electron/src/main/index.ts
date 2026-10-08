import { join } from "node:path";
import { app, BrowserWindow, session } from "electron";
import type { ServerEntry } from "../shared/contracts";
import { appPaths } from "./app-paths";
import {
	installAppProtocol,
	registerAppScheme,
} from "./protocol/handler.electron";
import { secureWindow } from "./security/install.electron";
import { initSingleInstance } from "./shell/single-instance.electron";

registerAppScheme();

// Temporary until the server registry lands (Task 4).
function activeServer(): ServerEntry {
	return {
		id: "demo",
		name: "Demo",
		kind: "demo",
		url: process.env.LOAMS_DESKTOP_SERVER ?? "http://127.0.0.1:8084",
	};
}

function createWindow(): BrowserWindow {
	const win = new BrowserWindow({
		width: 1280,
		height: 800,
		show: false,
		title: "Loams Desktop",
		webPreferences: {
			preload: join(__dirname, "../preload/index.cjs"),
			contextIsolation: true,
			sandbox: true,
			nodeIntegration: false,
			webSecurity: true,
			webviewTag: false,
		},
	});
	secureWindow(win);
	win.once("ready-to-show", () => win.show());
	void win.loadURL("loams-app://console/ui/cordis.html");
	return win;
}

initSingleInstance({
	getWindow: () => BrowserWindow.getAllWindows()[0],
	onPrimary: () => {
		void app.whenReady().then(async () => {
			app.setAppUserModelId("dev.loams.desktop");
			installAppProtocol(session.defaultSession, {
				distRoot: (await appPaths()).consoleDist,
				activeServer,
				localShim: () => null, // Task 6
			});
			createWindow();
			app.on("activate", () => {
				if (BrowserWindow.getAllWindows().length === 0) createWindow();
			});
		});
		app.on("window-all-closed", () => {
			if (process.platform !== "darwin") app.quit();
		});
	},
});
