import type { BrowserWindow } from "electron";

let mainWindow: BrowserWindow | undefined;

export function setMainWindow(w: BrowserWindow | undefined): void {
	mainWindow = w;
}

/** The single main window, or undefined if none/destroyed. */
export function getMainWindow(): BrowserWindow | undefined {
	return mainWindow && !mainWindow.isDestroyed() ? mainWindow : undefined;
}
