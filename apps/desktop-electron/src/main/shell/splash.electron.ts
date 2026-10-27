import { join } from "node:path";
import { app, BrowserWindow } from "electron";
import { secureWindow } from "../security/install.electron";
import { CHROME } from "./window-chrome";

export function splashPath(): string {
	return app.isPackaged
		? join(process.resourcesPath, "splash", "splash.html")
		: join(app.getAppPath(), "build", "splash.html");
}

/** A frameless splash while the local stack starts (D1084; T3 Code's connecting splash window). */
export function showSplash(o: { dark: boolean }): { close(): void } {
	const splash = new BrowserWindow({
		width: 360,
		height: 280,
		frame: false,
		resizable: false,
		minimizable: false,
		maximizable: false,
		fullscreenable: false,
		center: true,
		show: false,
		// The console canvas in each theme, before the splash's page paints (T15-2).
		backgroundColor: o.dark ? CHROME.dark.canvas : CHROME.light.canvas,
		title: "Loams Desktop",
		webPreferences: { contextIsolation: true, nodeIntegration: false, sandbox: true },
	});
	// The splash is a local page: no window.open, no navigation, no permissions.
	secureWindow(splash);
	splash.once("ready-to-show", () => {
		if (!splash.isDestroyed()) splash.show();
	});
	void splash.loadFile(splashPath());
	return {
		close: () => {
			if (!splash.isDestroyed()) splash.close();
		},
	};
}
