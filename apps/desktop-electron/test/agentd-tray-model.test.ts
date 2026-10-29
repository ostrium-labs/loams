import { describe, expect, it } from "vite-plus/test";
import {
	agentdBadge,
	agentdStatusLabel,
	agentdTrayItems,
} from "../src/main/shell/agentd-tray-model";
import type { AgentdStatus } from "../src/shared/contracts";

const running = (over: Partial<Extract<AgentdStatus, { state: "running" }>> = {}): AgentdStatus => ({
	state: "running",
	mode: "child",
	version: "0.2.1",
	protocol: 1,
	pid: 7,
	startedAtMs: 0,
	dataDir: "/data/agentd",
	draining: false,
	runningTurns: 0,
	pendingApprovals: 0,
	...over,
});

describe("agentd tray model", () => {
	it("tray_model_counts_from_daemon", () => {
		const s = running({ runningTurns: 2, pendingApprovals: 1 });
		expect(agentdStatusLabel(s)).toBe(
			"Background agents: running · 2 turns · 1 approval",
		);
		expect(agentdBadge(s)).toBe(1);
	});

	it("stopped_and_unavailable_labels", () => {
		expect(agentdStatusLabel({ state: "stopped", dataDir: "/d" })).toBe(
			"Background agents: stopped",
		);
		expect(agentdStatusLabel({ state: "unavailable", reason: "x" })).toBe(
			"Background agents: unavailable",
		);
		expect(agentdBadge({ state: "stopped", dataDir: "/d" })).toBe(0);
	});

	it("stop_enabled_only_while_running", () => {
		const stop = (s: AgentdStatus) =>
			agentdTrayItems(s).find((i) => i.id === "agentd-stop")?.enabled;
		expect(stop(running())).toBe(true);
		expect(stop(running({ draining: true }))).toBe(false);
		expect(stop({ state: "stopped", dataDir: "/d" })).toBe(false);
	});

	it("quit_hint_in_service_mode", () => {
		const quit = (s: AgentdStatus) =>
			agentdTrayItems(s).find((i) => i.id === "quit")?.label;
		expect(quit(running({ mode: "service" }))).toBe(
			"Quit Loams Desktop (agents keep running)",
		);
		expect(quit(running({ mode: "child" }))).toBe("Quit Loams Desktop");
	});
});
