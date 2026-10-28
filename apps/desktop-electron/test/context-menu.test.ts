import { describe, expect, it, vi } from "vite-plus/test";
import {
	buildContextMenuTemplate,
	type ContextMenuState,
} from "../src/main/shell/context-menu-template";

const base: ContextMenuState = {
	isEditable: false,
	selectionText: "",
	linkURL: "",
	hasImageContents: false,
	editFlags: {
		canUndo: false,
		canRedo: false,
		canCut: false,
		canCopy: false,
		canPaste: false,
		canSelectAll: true,
	},
};
const actions = { openLink: vi.fn(), copyLink: vi.fn(), copyImage: vi.fn(), lookUp: vi.fn() };
const labels = (t: ReturnType<typeof buildContextMenuTemplate>) =>
	t.filter((i) => i.type !== "separator").map((i) => i.label);

describe("the context menu (D1084)", () => {
	it("offers cut, copy and paste in a field", () => {
		const t = buildContextMenuTemplate(
			{
				...base,
				isEditable: true,
				editFlags: { ...base.editFlags, canCut: true, canCopy: true, canPaste: true },
			},
			"linux",
			actions,
		);
		expect(labels(t)).toEqual(["Undo", "Redo", "Cut", "Copy", "Paste", "Select All"]);
	});

	it("offers Look Up for a selection on macOS only", () => {
		const sel = { ...base, selectionText: "branching" };
		expect(labels(buildContextMenuTemplate(sel, "darwin", actions))).toEqual([
			"Look Up “branching”",
			"Copy",
			"Select All",
		]);
		expect(labels(buildContextMenuTemplate(sel, "linux", actions))).toEqual(["Copy", "Select All"]);
	});

	it("shortens a long selection in Look Up's label", () => {
		const sel = { ...base, selectionText: `  ${"x".repeat(300)}  ` };
		const [first] = labels(buildContextMenuTemplate(sel, "darwin", actions));
		expect(first).toBe(`Look Up “${"x".repeat(40)}…”`);
	});

	it("opens only external web links in the browser", () => {
		expect(
			labels(buildContextMenuTemplate({ ...base, linkURL: "https://loams.dev" }, "linux", actions))[0],
		).toBe("Open Link in Browser");
		expect(
			labels(buildContextMenuTemplate({ ...base, linkURL: "http://127.0.0.1:8080" }, "linux", actions))[0],
		).toBe("Copy Link Address");
		expect(
			labels(buildContextMenuTemplate({ ...base, linkURL: "file:///etc/passwd" }, "linux", actions))[0],
		).toBe("Copy Link Address");
	});

	it("runs the action for the link it was opened on", () => {
		const t = buildContextMenuTemplate({ ...base, linkURL: "https://loams.dev/docs" }, "linux", actions);
		(t[0]?.click as () => void)();
		expect(actions.openLink).toHaveBeenCalledWith("https://loams.dev/docs");
	});
});
