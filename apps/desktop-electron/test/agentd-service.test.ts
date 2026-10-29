import { describe, expect, it } from "vite-plus/test";
import type { ConnectFn } from "../src/main/agentd/client";
import { createMockAgentdClient, defaultFixture } from "../src/main/agentd/mock";
import { AgentdService } from "../src/main/agentd/service";
import { AgentdError } from "../src/main/agentd/types";
import type { AgentdBackground } from "../src/shared/contracts";

function setup(
	init: {
		bg?: AgentdBackground;
		running?: boolean;
		mode?: "service" | "child";
		stopServiceFails?: number;
	} = {},
) {
	let stopFailures = init.stopServiceFails ?? 0;
	let bg: AgentdBackground = init.bg ?? "ask";
	let up = init.running ?? false;
	const log: string[] = [];
	const client = createMockAgentdClient({ ...defaultFixture(1_000_000), mode: init.mode ?? "child" });
	const connect: ConnectFn = async () => {
		if (!up) throw new AgentdError("unavailable", "down");
		return client;
	};
	const svc = new AgentdService({
		connect,
		userData: "/data",
		agentdDir: "/data/agentd",
		clientVersion: "0.1.0",
		readBackground: () => bg,
		writeBackground: (v) => {
			bg = v;
			log.push(`bg:${v}`);
		},
		launch: async (mode) => {
			log.push(`launch:${mode}`);
			up = true;
		},
		stopService: async () => {
			log.push("service.stop");
			if (stopFailures > 0) {
				stopFailures--;
				throw new Error("systemctl failed");
			}
		},
	});
	return { svc, client, log };
}

describe("agentd service", () => {
	it("stopped_when_no_daemon", async () => {
		const { svc } = setup();
		await svc.attach();
		expect(svc.status()).toEqual({ state: "stopped", dataDir: "/data/agentd" });
		expect((await svc.sessions()).ok).toBe(false);
	});

	it("start_before_consent_runs_a_child", async () => {
		const { svc, log } = setup({ bg: "ask" });
		expect((await svc.start()).ok).toBe(true);
		expect(log).toEqual(["launch:child"]);
		expect(svc.status().state).toBe("running");
	});

	it("start_with_background_on_runs_the_service", async () => {
		const { svc, log } = setup({ bg: "ask" });
		expect(svc.setBackground("on")).toEqual({ ok: true, value: undefined });
		expect((await svc.start()).ok).toBe(true);
		expect(log).toEqual(["bg:on", "launch:service"]);
		expect(svc.status().state).toBe("running");
	});

	it("start_reports_why_launch_failed", async () => {
		const { svc } = setup({ bg: "off" });
		const failing = svc as unknown as { deps: { launch: () => Promise<void> } };
		failing.deps.launch = async () => {
			throw new AgentdError("unavailable", "loams-agentd is not bundled with this build");
		};
		expect(await svc.start()).toMatchObject({
			ok: false,
			code: "launch_failed",
			message: "loams-agentd is not bundled with this build",
		});
	});

	it("status_has_no_token_fields", async () => {
		const { svc } = setup({ running: true });
		await svc.attach();
		await Promise.resolve();
		const s = svc.status();
		expect(s).toMatchObject({ state: "running", version: "0.2.1", dataDir: "/data/agentd" });
		expect(JSON.stringify(s).toLowerCase()).not.toContain("token");
	});

	it("lists_and_reattaches_sessions", async () => {
		const { svc } = setup({ running: true });
		await svc.attach();
		const list = await svc.sessions();
		expect(list.ok && list.value.length).toBe(4);
		expect(await svc.reattach("s_01hq7y")).toEqual({
			ok: true,
			value: { route: "/agent/s_01hq7y" },
		});
		expect(await svc.reattach("s_01hq5k")).toMatchObject({ ok: false, code: "bad_params" });
		expect(await svc.reattach("../etc")).toMatchObject({ ok: false, code: "bad_params" });
	});

	it("stop_requires_confirmation", async () => {
		const { svc, client } = setup({ running: true });
		await svc.attach();
		expect(await svc.stop({})).toMatchObject({ ok: false, code: "consent_required" });
		expect(client.calls).not.toContain("Drain");
	});

	it("stop_child_mode_drains_then_shuts_down", async () => {
		const { svc, client, log } = setup({ running: true, mode: "child" });
		await svc.attach();
		expect((await svc.stop({ confirmed: true })).ok).toBe(true);
		const i = client.calls.indexOf("Drain");
		expect(i).toBeGreaterThanOrEqual(0);
		expect(client.calls.indexOf("Shutdown")).toBeGreaterThan(i);
		expect(log).not.toContain("service.stop");
		expect(svc.status().state).toBe("stopped");
	});

	it("stop_background_agents_drains_first_in_service_mode", async () => {
		const { svc, client, log } = setup({ running: true, mode: "service" });
		await svc.attach();
		expect((await svc.stop({ confirmed: true })).ok).toBe(true);
		expect(client.calls).toContain("Drain");
		expect(client.calls).not.toContain("Shutdown");
		expect(log).toEqual(["service.stop"]);
	});

	it("failed_service_stop_keeps_the_daemon_for_a_retry", async () => {
		const { svc, log } = setup({ running: true, mode: "service", stopServiceFails: 1 });
		await svc.attach();
		expect(await svc.stop({ confirmed: true })).toMatchObject({
			ok: false,
			code: "service_stop_failed",
		});
		expect(svc.status().state).toBe("running");
		expect((await svc.stop({ confirmed: true })).ok).toBe(true);
		expect(log).toEqual(["service.stop", "service.stop"]);
		expect(svc.status().state).toBe("stopped");
	});

	it("status_updates_from_watch", async () => {
		const { svc, client } = setup({ running: true });
		const seen: number[] = [];
		svc.onStatus((s) => {
			if (s.state === "running") seen.push(s.pendingApprovals);
		});
		await svc.attach();
		await Promise.resolve();
		client.setSessions([]);
		expect(seen.at(-1)).toBe(0);
		expect(seen).toContain(1);
	});

	it("has no repository bridge: Studio's projects go through ProjectService (T6-9)", () => {
		const { svc } = setup({ running: true });
		for (const gone of ["repos", "addRepo", "cloneRepo"]) expect(svc).not.toHaveProperty(gone);
	});
});
