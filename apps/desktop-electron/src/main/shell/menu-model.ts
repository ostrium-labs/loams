// Pure application-menu template (ids and roles); menu.electron.ts maps it to Electron.
import type { ShellCommand } from "../../shared/contracts";

export const DOCS_URL = "https://loams.dev/docs";

export interface MenuNode {
	id?: string;
	label?: string;
	role?: string;
	type?: "separator";
	accelerator?: string;
	/**
	 * False: the accelerator is shown but not registered, so the key press reaches the
	 * renderer, which owns the chord (Mod+Shift+M is the shell's, Mod+N Studio's).
	 */
	registerAccelerator?: boolean;
	submenu?: MenuNode[];
}

/**
 * The menu items the renderer acts on (SD1 Task 28, §53 §4.2, CF1 Task 14): main forwards
 * them on `shell:command`; every other id is main's own (`check-updates`, `restart-server`,
 * `show-logs`, `report-issue`, `docs`, `about`, `quit`).
 */
export const MENU_COMMANDS: Readonly<Record<string, ShellCommand>> = {
	"mode-studio": "mode.studio",
	"mode-console": "mode.console",
	"mode-toggle": "mode.toggle",
	"new-thread": "chat.new",
	palette: "palette.toggle",
	"toggle-sidebar": "sidebar.toggle",
	"toggle-inspector": "inspector.toggle",
	"toggle-drawer": "drawer.toggle",
	back: "navigation.back",
	forward: "navigation.forward",
	"next-thread": "thread.next",
	"reopen-thread": "thread.reopen",
	settings: "settings.open",
};

export const ISSUES_URL = "https://github.com/ostrium-labs/loams/issues/new/choose";

export function menuCommand(id: string): ShellCommand | undefined {
	return Object.hasOwn(MENU_COMMANDS, id) ? MENU_COMMANDS[id] : undefined;
}

/**
 * An item whose chord the renderer's shell owns (D1081): the menu shows the accelerator but
 * does not register it, so one key press never runs twice and editable targets keep theirs.
 */
const r = (id: string, label: string, accelerator: string): MenuNode => ({
	id,
	label,
	accelerator,
	registerAccelerator: false,
});

/** The application menu (D1084): App on macOS, then File, Edit, View, Window and Help. */
export function menuModel(o: { platform: string; dev: boolean }): MenuNode[] {
	const sep: MenuNode = { type: "separator" };
	const mac = o.platform === "darwin";
	const settings = r("settings", mac ? "Settings…" : "Settings", "CmdOrCtrl+,");
	const checkUpdates: MenuNode = { id: "check-updates", label: "Check for Updates…" };
	const newThread = r("new-thread", "New Thread", "CmdOrCtrl+N");
	const menus: MenuNode[] = [];
	if (mac) {
		menus.push({
			label: "Loams Desktop",
			submenu: [
				{ id: "about", label: "About Loams Desktop" },
				checkUpdates,
				sep,
				settings,
				sep,
				{ role: "hide" },
				{ role: "hideOthers" },
				{ role: "unhide" },
				sep,
				{ id: "quit", label: "Quit Loams Desktop" },
			],
		});
		menus.push({ label: "File", submenu: [newThread, sep, { role: "close" }] });
	} else {
		menus.push({
			label: "File",
			submenu: [newThread, sep, settings, sep, { id: "quit", label: "Quit" }],
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
		r("palette", "Command Palette", "CmdOrCtrl+K"),
		sep,
		r("toggle-sidebar", "Toggle Sidebar", "CmdOrCtrl+B"),
		r("toggle-inspector", "Toggle Inspector", "CmdOrCtrl+I"),
		r("toggle-drawer", "Toggle Drawer", "CmdOrCtrl+J"),
		sep,
		r("back", "Back", "CmdOrCtrl+["),
		r("forward", "Forward", "CmdOrCtrl+]"),
		sep,
		// Loams Studio and Console (§53 §4.2): one window, two modes.
		{ id: "mode-studio", label: "Studio" },
		{ id: "mode-console", label: "Console" },
		r("mode-toggle", "Switch Mode", "CmdOrCtrl+Shift+M"),
		sep,
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
		submenu: [
			{ role: "minimize" },
			{ role: "zoom" },
			sep,
			r("next-thread", "Next Thread", "Ctrl+Tab"),
			r("reopen-thread", "Reopen Closed Thread", "CmdOrCtrl+Shift+T"),
			sep,
			{ role: "close" },
		],
	});
	menus.push({
		label: "Help",
		submenu: [
			{ id: "docs", label: "Documentation" },
			{ id: "show-logs", label: "Show Logs" },
			{ id: "restart-server", label: "Restart Local Server" },
			{ id: "report-issue", label: "Report an Issue…" },
			...(mac ? [] : [sep, checkUpdates, { id: "about", label: "About Loams Desktop" }]),
		],
	});
	return menus;
}
