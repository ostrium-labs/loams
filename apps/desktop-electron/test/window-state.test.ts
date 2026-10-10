import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import {
	loadWindowState,
	saveWindowState,
} from "../src/main/shell/window-state";

const display = { x: 0, y: 0, width: 1920, height: 1080 };
const file = () => join(mkdtempSync(join(tmpdir(), "ws-")), "w.json");

describe("window state", () => {
	it("round_trips_visible_bounds", () => {
		const f = file();
		saveWindowState(f, {
			x: 100,
			y: 50,
			width: 1000,
			height: 700,
			isMaximized: true,
		});
		expect(loadWindowState(f, [display])).toEqual({
			x: 100,
			y: 50,
			width: 1000,
			height: 700,
			isMaximized: true,
		});
	});
	it("window_state_clamped_to_display", () => {
		const f = file();
		saveWindowState(f, {
			x: 5000,
			y: 4000,
			width: 100,
			height: 100,
			isMaximized: false,
		});
		const s = loadWindowState(f, [display]);
		expect(s.x).toBeUndefined();
		expect(s.y).toBeUndefined();
		expect(s.width).toBeGreaterThanOrEqual(900);
		expect(s.height).toBeGreaterThanOrEqual(600);
	});
	it("missing_or_corrupt_gives_defaults", () => {
		expect(
			loadWindowState(join(tmpdir(), "nope-ws.json"), [display]),
		).toMatchObject({
			width: 1280,
			height: 800,
		});
	});
});
