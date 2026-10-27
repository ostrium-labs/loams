import { describe, expect, it } from "vite-plus/test";
import {
	EMPTY_STUDIO,
	notificationContent,
	notificationRoute,
	type StudioNotice,
	type StudioSnapshot,
	shouldNotify,
	studioCounts,
	studioTrayRoute,
} from "../src/main/shell/studio-tray-model";
import { trayModel } from "../src/main/shell/tray-model";
import type { EngineState, ServerEntry } from "../src/shared/contracts";

const engine: EngineState = { phase: "stopped" };
const server = {
	id: "local",
	name: "This computer",
	kind: "local",
} as ServerEntry;

function snapshot(o: Partial<StudioSnapshot> = {}): StudioSnapshot {
	return {
		connected: true,
		threads: new Map([
			["c1", "Fix the login redirect"],
			["c2", "Refactor ingest"],
			["c3", "Write the release notes"],
		]),
		working: new Set(["c2", "c3"]),
		approvals: [
			{ id: "a1", chatId: "c1" },
			{ id: "a2", chatId: "c1" },
			{ id: "a3", chatId: "c2" },
		],
		...o,
	};
}

describe("the tray's Studio block (SD1 Task 28)", () => {
	it("tray_counts_running_and_approvals", () => {
		const s = snapshot();
		expect(studioCounts(s)).toEqual({ running: 2, approvals: 3 });
		const m = trayModel(engine, 0, server, s);
		expect(m.tooltip).toContain("Studio: 2 running, 3 approvals");
		const ids = m.items.map((i) => i.id);
		// After "Open", before the engine: the status line, then the threads that need the
		// person first (approvals), then the running ones, then "Open Studio".
		expect(ids.slice(0, 6)).toEqual([
			"open",
			"studio-status",
			"studio-thread:c1",
			"studio-thread:c2",
			"studio-thread:c3",
			"studio-open",
		]);
		const label = (id: string) => m.items.find((i) => i.id === id)?.label;
		expect(label("studio-status")).toBe("Studio: 2 running · 3 approvals");
		expect(label("studio-thread:c1")).toBe(
			"Fix the login redirect: 2 approvals",
		);
		expect(label("studio-thread:c2")).toBe("Refactor ingest: 1 approval");
		expect(label("studio-thread:c3")).toBe("Write the release notes: running");
		expect(m.items.find((i) => i.id === "studio-status")?.enabled).toBe(false);
	});

	it("shows no block without a daemon, and an idle line with nothing running", () => {
		expect(trayModel(engine, 0, server).items.map((i) => i.id)).not.toContain(
			"studio-status",
		);
		expect(
			trayModel(engine, 0, server, EMPTY_STUDIO).items.map((i) => i.id),
		).not.toContain("studio-status");
		const idle = trayModel(
			engine,
			0,
			server,
			snapshot({ working: new Set(), approvals: [] }),
		);
		expect(idle.items.find((i) => i.id === "studio-status")?.label).toBe(
			"Studio: idle",
		);
		expect(idle.items.some((i) => i.id.startsWith("studio-thread:"))).toBe(
			false,
		);
	});

	it("caps the thread rows and cleans their titles", () => {
		const threads = new Map<string, string>();
		const working = new Set<string>();
		for (let i = 0; i < 9; i++) {
			threads.set(`t${i}`, i === 0 ? `bad\u0007\ntitle ${"x".repeat(80)}` : "");
			working.add(`t${i}`);
		}
		const m = trayModel(
			engine,
			0,
			server,
			snapshot({ threads, working, approvals: [] }),
		);
		const rows = m.items.filter((i) => i.id.startsWith("studio-thread:"));
		expect(rows).toHaveLength(5);
		expect(rows[0]?.label).toMatch(/^bad title x+…: running$/);
		expect(rows[0]?.label.length).toBeLessThan(80);
		expect(rows[1]?.label).toBe("Untitled thread: running");
	});

	it("routes tray rows to Studio, and only safe ids", () => {
		expect(studioTrayRoute("studio-thread:c_1")).toBe("/studio/t/c_1");
		expect(studioTrayRoute("studio-open")).toBe("/studio");
		expect(studioTrayRoute("studio-thread:../x")).toBeUndefined();
		expect(studioTrayRoute("studio-thread:")).toBeUndefined();
		expect(studioTrayRoute("engine")).toBeUndefined();
	});
});

describe("OS notifications from the daemon (SD1 Task 28, §50 §5.5)", () => {
	const notice = (o: Partial<StudioNotice> = {}): StudioNotice => ({
		id: "n1",
		kind: "turn_done",
		chatId: "c1",
		title: "Fix the login redirect",
		createdAtMs: 2_000,
		...o,
	});

	it("notification_opens_studio_link", () => {
		expect(notificationRoute(notice())).toBe("/studio/t/c1");
		expect(
			notificationRoute(
				notice({ kind: "group_ready", chatId: undefined, groupId: "g1" }),
			),
		).toBe("/studio/g/g1");
		expect(notificationRoute(notice({ chatId: "../etc" }))).toBe("/studio");
		expect(notificationRoute(notice({ chatId: undefined }))).toBe("/studio");
	});

	it("words each kind and keeps the daemon's title as plain text", () => {
		expect(notificationContent(notice())).toEqual({
			title: "Turn finished",
			body: "Fix the login redirect",
		});
		expect(notificationContent(notice({ kind: "approval" })).title).toBe(
			"Approval needed",
		);
		expect(notificationContent(notice({ kind: "question" })).title).toBe(
			"Question waiting",
		);
		expect(notificationContent(notice({ kind: "error" })).title).toBe(
			"Turn failed",
		);
		expect(notificationContent(notice({ kind: "group_ready" })).title).toBe(
			"Parallel runs finished",
		);
		expect(notificationContent(notice({ kind: "check_failed" })).title).toBe(
			"Check failed",
		);
		expect(
			notificationContent(notice({ title: `a\u0000b${"y".repeat(400)}` })).body
				.length,
		).toBe(200);
	});

	it("follows the person's choices, skips old ones and a focused window", () => {
		const prefs = { turnDone: true, approval: true };
		const at = { focused: false, since: 1_000 };
		expect(shouldNotify(notice(), prefs, at)).toBe(true);
		expect(shouldNotify(notice(), { ...prefs, turnDone: false }, at)).toBe(
			false,
		);
		expect(
			shouldNotify(
				notice({ kind: "error" }),
				{ ...prefs, turnDone: false },
				at,
			),
		).toBe(false);
		expect(
			shouldNotify(
				notice({ kind: "approval" }),
				{ ...prefs, approval: false },
				at,
			),
		).toBe(false);
		expect(
			shouldNotify(
				notice({ kind: "question" }),
				{ ...prefs, turnDone: false },
				at,
			),
		).toBe(true);
		// The daemon keeps its last 200: those from before this watch are not news.
		expect(shouldNotify(notice({ createdAtMs: 500 }), prefs, at)).toBe(false);
		// The window in front shows them already.
		expect(shouldNotify(notice(), prefs, { ...at, focused: true })).toBe(false);
	});
});
