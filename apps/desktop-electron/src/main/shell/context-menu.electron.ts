// Adapted from dataelement/dsh-desktop (MIT), src/main/context-menu.ts.
// Loams changes (CF1 Task 14, D1084): no locale, the platform for Look Up, and Look Up's action.
import { type BrowserWindow, clipboard, Menu, shell } from "electron";
import { buildContextMenuTemplate } from "./context-menu-template";

/**
 * The native context menu: cut, copy and paste in a field, links, images, and Look Up on
 * macOS. A page that opens its own menu (Studio's thread rows) calls `preventDefault()` on
 * `contextmenu`, and Chromium then raises no `context-menu` here, so its menu stays.
 */
export function installContextMenu(window: BrowserWindow): void {
	window.webContents.on("context-menu", (_event, params) => {
		const template = buildContextMenuTemplate(params, process.platform, {
			openLink: (url) => void shell.openExternal(url),
			copyLink: (url) => clipboard.writeText(url),
			copyImage: () => {
				if (!window.isDestroyed()) window.webContents.copyImageAt(params.x, params.y);
			},
			lookUp: () => {
				if (!window.isDestroyed()) window.webContents.showDefinitionForSelection();
			},
		});
		if (template.length === 0 || window.isDestroyed()) return;
		Menu.buildFromTemplate(template).popup({ window });
	});
}
