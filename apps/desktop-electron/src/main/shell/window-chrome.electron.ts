import type { BrowserWindow } from "electron";
import { CH } from "../../shared/contracts";
import type { ChromeSync } from "./window-chrome";

/** The backdrop swap on macOS, and data-fullscreen for the page (design 59, D1083). */
export function installWindowChrome(
	win: BrowserWindow,
	sync: ChromeSync,
): void {
	if (process.platform === "darwin") {
		const apply = (): void => sync.apply();
		win.on("minimize", apply);
		win.on("hide", apply);
		win.on("restore", apply);
		win.on("show", apply);
	}
	const fullscreen = (): void => {
		if (!win.isDestroyed())
			win.webContents.send(CH.shellFullscreen, win.isFullScreen());
	};
	win.webContents.on("did-finish-load", fullscreen);
	win.on("enter-full-screen", fullscreen);
	win.on("leave-full-screen", fullscreen);
}
