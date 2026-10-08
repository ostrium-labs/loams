import { describe, expect, it } from "vitest";
import { menuModel } from "../src/main/shell/menu-model";
import {
	badgeLabel,
	closeAction,
	trayModel,
} from "../src/main/shell/tray-model";
import type { EngineState, ServerEntry } from "../src/shared/contracts";

const server: ServerEntry = {
	id: "local",
	name: "Local engine",
	kind: "local",
	url: "",
};
const ready: EngineState = {
	phase: "ready",
	url: "http://127.0.0.1:1",
	esUrl: "",
	flightUrl: "",
	durableUrl: "",
	pid: 1,
};

describe("tray-model", () => {
	it("tray_model_each_engine_phase", () => {
		const phases: [EngineState, string, boolean][] = [
			[{ phase: "stopped" }, "Engine: stopped (start)", true],
			[{ phase: "starting", attempt: 1 }, "Engine: starting", false],
			[ready, "Engine: ready (stop)", true],
			[
				{ phase: "failed", reason: "x", logPath: "/l" },
				"Engine: failed (start)",
				true,
			],
		];
		for (const [st, label, enabled] of phases) {
			const m = trayModel(st, 0, server);
			expect(m.items.map((i) => i.id)).toEqual([
				"open",
				"engine",
				"approvals",
				"servers",
				"quit",
			]);
			const e = m.items[1];
			expect(e?.label).toBe(label);
			expect(e?.enabled).toBe(enabled);
			expect(m.tooltip).toContain(`engine ${st.phase}`);
		}
	});
	it("approvals_item_and_servers", () => {
		const m = trayModel(ready, 3, server);
		expect(m.items[2]).toEqual({
			id: "approvals",
			label: "Pending approvals (3)",
			enabled: true,
		});
		expect(m.items[3]?.label).toContain("Local engine");
		expect(m.tooltip).toContain("3 pending approvals");
		expect(trayModel(ready, 0, server).items[2]?.enabled).toBe(false);
	});
	it("badge_label", () => {
		expect(badgeLabel(0)).toBe("");
		expect(badgeLabel(-4)).toBe("");
		expect(badgeLabel(Number.NaN)).toBe("");
		expect(badgeLabel(7)).toBe("7");
		expect(badgeLabel(99)).toBe("99");
		expect(badgeLabel(100)).toBe("99+");
	});
	it("close_behaviour", () => {
		const base = {
			platform: "linux",
			hasTray: true,
			closeToTray: true,
			quitting: false,
		};
		expect(closeAction(base)).toBe("hide");
		expect(closeAction({ ...base, platform: "win32" })).toBe("hide");
		expect(closeAction({ ...base, hasTray: false })).toBe("close");
		expect(closeAction({ ...base, closeToTray: false })).toBe("close");
		expect(closeAction({ ...base, quitting: true })).toBe("close");
		expect(closeAction({ ...base, platform: "darwin", hasTray: false })).toBe(
			"hide",
		);
	});
});

describe("menu-model", () => {
	const ids = (n: ReturnType<typeof menuModel>) => JSON.stringify(n);
	it("menu_has_help_items_and_dev_only_reload", () => {
		for (const platform of ["linux", "darwin", "win32"]) {
			const prod = ids(menuModel({ platform, dev: false }));
			expect(prod).toContain('"id":"docs"');
			expect(prod).toContain('"id":"logs"');
			expect(prod).toContain('"id":"about"');
			expect(prod).not.toContain('"role":"reload"');
			expect(ids(menuModel({ platform, dev: true }))).toContain(
				'"role":"reload"',
			);
		}
		expect(menuModel({ platform: "darwin", dev: false })[0]?.label).toBe(
			"Loams Desktop",
		);
		expect(
			menuModel({ platform: "linux", dev: false }).map((m) => m.label),
		).toEqual(["File", "Edit", "View", "Window", "Help"]);
	});
});
