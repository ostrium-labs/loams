import { describe, expect, it } from "vite-plus/test";
import {
	ISSUES_URL,
	MENU_COMMANDS,
	type MenuNode,
	menuCommand,
	menuModel,
} from "../src/main/shell/menu-model";

function flat(nodes: MenuNode[]): MenuNode[] {
	return nodes.flatMap((n) => [n, ...flat(n.submenu ?? [])]);
}

function item(platform: string, id: string): MenuNode | undefined {
	return flat(menuModel({ platform, dev: false })).find((n) => n.id === id);
}

describe("menu model (D1084)", () => {
	it("has App on macOS, then File, Edit, View, Window and Help", () => {
		expect(menuModel({ platform: "darwin", dev: false }).map((m) => m.label)).toEqual([
			"Loams Desktop",
			"File",
			"Edit",
			"View",
			"Window",
			"Help",
		]);
		expect(menuModel({ platform: "linux", dev: false }).map((m) => m.label)).toEqual([
			"File",
			"Edit",
			"View",
			"Window",
			"Help",
		]);
	});

	it.each([
		["new-thread", "CmdOrCtrl+N", "chat.new"],
		["palette", "CmdOrCtrl+K", "palette.toggle"],
		["toggle-sidebar", "CmdOrCtrl+B", "sidebar.toggle"],
		["toggle-inspector", "CmdOrCtrl+I", "inspector.toggle"],
		["toggle-drawer", "CmdOrCtrl+J", "drawer.toggle"],
		["back", "CmdOrCtrl+[", "navigation.back"],
		["forward", "CmdOrCtrl+]", "navigation.forward"],
		["next-thread", "Ctrl+Tab", "thread.next"],
		["reopen-thread", "CmdOrCtrl+Shift+T", "thread.reopen"],
		["settings", "CmdOrCtrl+,", "settings.open"],
		["mode-toggle", "CmdOrCtrl+Shift+M", "mode.toggle"],
	])("%s shows %s and the renderer runs %s (the shell owns the chord)", (id, accelerator, command) => {
		for (const platform of ["linux", "darwin", "win32"]) {
			expect(item(platform, id)).toMatchObject({ accelerator, registerAccelerator: false });
		}
		expect(menuCommand(id)).toBe(command);
	});

	it("has the app's own items, handled in main", () => {
		for (const id of ["check-updates", "restart-server", "show-logs", "report-issue"]) {
			for (const platform of ["linux", "darwin", "win32"])
				expect(item(platform, id), `${platform} ${id}`).toBeTruthy();
			expect(menuCommand(id)).toBeUndefined();
		}
		expect(ISSUES_URL).toBe("https://github.com/ostrium-labs/loams/issues/new/choose");
	});

	it("switches modes from View", () => {
		for (const platform of ["linux", "darwin", "win32"]) {
			expect(item(platform, "mode-studio")).toMatchObject({ label: "Studio" });
			expect(item(platform, "mode-console")).toMatchObject({ label: "Console" });
		}
		expect(menuCommand("mode-studio")).toBe("mode.studio");
		expect(menuCommand("mode-console")).toBe("mode.console");
	});

	it("registers no accelerator itself, so a chord never runs twice", () => {
		for (const platform of ["linux", "darwin", "win32"]) {
			for (const n of flat(menuModel({ platform, dev: true }))) {
				if (n.accelerator) expect(n.registerAccelerator, `${platform} ${n.id}`).toBe(false);
			}
		}
	});

	it("forwards only the renderer's commands; quit, docs and about stay main's", () => {
		for (const id of ["quit", "docs", "about", "check-updates"]) {
			expect(item("linux", id), id).toBeDefined();
			expect(menuCommand(id)).toBeUndefined();
		}
		expect(Object.keys(MENU_COMMANDS)).toHaveLength(13);
	});

	it("keeps Settings and Check for Updates in the App menu on macOS", () => {
		const app = menuModel({ platform: "darwin", dev: false })[0]?.submenu ?? [];
		expect(app.map((n) => n.id).filter(Boolean)).toEqual([
			"about",
			"check-updates",
			"settings",
			"quit",
		]);
		expect(item("darwin", "settings")?.label).toBe("Settings…");
		expect(item("linux", "settings")?.label).toBe("Settings");
	});
});
