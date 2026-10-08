// Pure application-menu template (ids and roles); menu.electron.ts maps it to Electron.
export const DOCS_URL = "https://loams.dev/docs";

export interface MenuNode {
	id?: string;
	label?: string;
	role?: string;
	type?: "separator";
	submenu?: MenuNode[];
}

export function menuModel(o: { platform: string; dev: boolean }): MenuNode[] {
	const sep: MenuNode = { type: "separator" };
	const mac = o.platform === "darwin";
	const menus: MenuNode[] = [];
	if (mac) {
		menus.push({
			label: "Loams Desktop",
			submenu: [
				{ id: "about", label: "About Loams Desktop" },
				sep,
				{ role: "hide" },
				{ role: "hideOthers" },
				{ role: "unhide" },
				sep,
				{ id: "quit", label: "Quit Loams Desktop" },
			],
		});
	} else {
		menus.push({
			label: "File",
			submenu: [{ id: "quit", label: "Quit" }],
		});
	}
	menus.push({
		label: "Edit",
		submenu: [
			{ role: "undo" },
			{ role: "redo" },
			sep,
			{ role: "cut" },
			{ role: "copy" },
			{ role: "paste" },
			{ role: "selectAll" },
		],
	});
	const view: MenuNode[] = [
		{ role: "resetZoom" },
		{ role: "zoomIn" },
		{ role: "zoomOut" },
		sep,
		{ role: "togglefullscreen" },
	];
	if (o.dev) view.unshift({ role: "reload" }, { role: "toggleDevTools" }, sep);
	menus.push({ label: "View", submenu: view });
	menus.push({
		label: "Window",
		submenu: [{ role: "minimize" }, { role: "zoom" }, { role: "close" }],
	});
	menus.push({
		label: "Help",
		submenu: [
			{ id: "docs", label: "Documentation" },
			{ id: "logs", label: "Open logs folder" },
			...(mac ? [] : [sep, { id: "about", label: "About Loams Desktop" }]),
		],
	});
	return menus;
}
