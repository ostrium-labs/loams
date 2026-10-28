import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it, vi } from "vite-plus/test";
import {
	parseNotifyPrefs,
	readNotifyPrefs,
	STUDIO_NOTIFY_KEY,
	writeSetting,
} from "../src/main/settings";
import {
	EDITOR_KEY,
	type EditorLaunch,
	editorLaunch,
	isLocalPath,
	launchEditor,
	readEditor,
} from "../src/main/studio/editor";

describe("Open in editor (SD1 Task 28, Q810)", () => {
	it("open_in_editor_uses_scheme_or_argv_not_shell", async () => {
		const path = "/home/dev/src/loams wt/a#b?c";
		expect(editorLaunch({ editor: "vscode" }, path)).toEqual({
			kind: "url",
			url: "vscode://file/home/dev/src/loams%20wt/a%23b%3Fc",
		});
		expect(
			editorLaunch({ editor: "cursor" }, path, { line: 12, column: 3 }),
		).toEqual({
			kind: "url",
			url: "cursor://file/home/dev/src/loams%20wt/a%23b%3Fc:12:3",
		});
		expect(editorLaunch({ editor: "zed" }, "/srv/x.ts", { line: 4 })).toEqual({
			kind: "url",
			url: "zed://file/srv/x.ts:4",
		});
		expect(
			editorLaunch({ editor: "vscode" }, "C:\\Users\\dev\\src\\app"),
		).toEqual({
			kind: "url",
			url: "vscode://file/C:/Users/dev/src/app",
		});
		// A picked program gets the path as one argument: nothing is parsed by a shell.
		const hostile = "/tmp/$(touch pwned); rm -rf ~ `id` | x";
		const argv = editorLaunch(
			{ editor: "command", command: "/usr/bin/subl" },
			hostile,
		);
		expect(argv).toEqual({
			kind: "argv",
			file: "/usr/bin/subl",
			args: [hostile],
		});
		expect(
			editorLaunch(
				{ editor: "command", command: "/Applications/Nova.app" },
				"/srv/x",
				undefined,
				"darwin",
			),
		).toEqual({
			kind: "argv",
			file: "/usr/bin/open",
			args: ["-a", "/Applications/Nova.app", "/srv/x"],
		});

		const spawn = vi.fn(() => ({ unref: vi.fn(), on: vi.fn() }));
		const openExternal = vi.fn(async () => undefined);
		await launchEditor(argv as EditorLaunch, { spawn, openExternal });
		expect(spawn).toHaveBeenCalledWith("/usr/bin/subl", [hostile], {
			shell: false,
			detached: true,
			stdio: "ignore",
			windowsHide: true,
		});
		expect(openExternal).not.toHaveBeenCalled();
		await launchEditor(
			editorLaunch({ editor: "zed" }, "/srv") as EditorLaunch,
			{
				spawn,
				openExternal,
			},
		);
		expect(openExternal).toHaveBeenCalledWith("zed://file/srv");
	});

	it("takes only an absolute local path, and a command only once one was picked", () => {
		for (const bad of [
			"",
			"relative/path",
			"-rf",
			"/a\nb",
			"/a\u0000b",
			"\\\\server\\share",
			"x".repeat(5000),
		])
			expect(isLocalPath(bad)).toBe(false);
		expect(isLocalPath("/home/dev")).toBe(true);
		expect(isLocalPath("D:/work")).toBe(true);
		expect(editorLaunch({ editor: "vscode" }, "relative")).toBeNull();
		expect(editorLaunch({ editor: "command" }, "/srv")).toBeNull();
		expect(
			editorLaunch({ editor: "command", command: "subl" }, "/srv"),
		).toBeNull();
	});

	it("notification choices round-trip through settings.json, both on by default", () => {
		const dir = mkdtempSync(join(tmpdir(), "loams-studio-"));
		const file = join(dir, "settings.json");
		expect(readNotifyPrefs(file)).toEqual({ turnDone: true, approval: true });
		writeSetting(file, STUDIO_NOTIFY_KEY, { turnDone: false, approval: true });
		expect(readNotifyPrefs(file)).toEqual({ turnDone: false, approval: true });
		expect(parseNotifyPrefs({ turnDone: "no", approval: true })).toBeNull();
		expect(parseNotifyPrefs([true, true])).toBeNull();
	});

	it("reads the choice from settings.json, VS Code by default", () => {
		const read = (v: unknown) => readEditor(() => v);
		expect(read(undefined)).toEqual({ editor: "vscode" });
		expect(read({ editor: "zed" })).toEqual({ editor: "zed" });
		expect(read({ editor: "nano" })).toEqual({ editor: "vscode" });
		expect(read({ editor: "command", command: "/opt/ed/bin/ed" })).toEqual({
			editor: "command",
			command: "/opt/ed/bin/ed",
		});
		// A command entry without an absolute program falls back.
		expect(read({ editor: "command", command: "ed --flag" })).toEqual({
			editor: "vscode",
		});
		expect(EDITOR_KEY).toBe("studio.editor");
	});
});
