import { EventEmitter } from "node:events";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { detectRuntime } from "../src/main/stacks/runtime";
import {
	bindLiveToTikv,
	composeArgs,
	parsePs,
	type RunFn,
	StackManager,
	TIKV_PD_ADDR,
} from "../src/main/stacks/stacks";
import type { StackState } from "../src/shared/contracts";

const have =
	(...bins: string[]) =>
	(b: string) =>
		bins.includes(b) ? `/usr/bin/${b}` : null;

const DOCKER_NDJSON = [
	{
		Service: "pd",
		State: "running",
		Publishers: [
			{
				URL: "127.0.0.1",
				TargetPort: 19379,
				PublishedPort: 19379,
				Protocol: "tcp",
			},
		],
	},
	{ Service: "tikv", State: "running", Publishers: [] },
]
	.map((r) => JSON.stringify(r))
	.join("\n");

const PODMAN_ARRAY = JSON.stringify([
	{
		Names: ["loams-desktop-tikv_pd_1"],
		State: "running",
		Labels: { "com.docker.compose.service": "pd" },
		Ports: [
			{
				host_ip: "127.0.0.1",
				host_port: 19379,
				container_port: 19379,
				protocol: "tcp",
			},
		],
	},
	{
		Names: ["loams-desktop-tikv_tikv_1"],
		State: "Running",
		Labels: { "com.docker.compose.service": "tikv" },
		Ports: null,
	},
]);

function manager(runtime: ReturnType<typeof detectRuntime>, run: RunFn) {
	return new StackManager({
		runtime,
		stacksDir: "/res/stacks",
		logsDir: mkdtempSync(join(tmpdir(), "stacks-test-")),
		run,
	});
}

describe("stacks", () => {
	it("detect_runtime_order", () => {
		expect(detectRuntime(have("docker", "podman", "docker-compose"))).toEqual({
			bin: "docker",
			args: ["compose"],
		});
		expect(detectRuntime(have("podman", "docker-compose"))).toEqual({
			bin: "podman",
			args: ["compose"],
		});
		expect(detectRuntime(have("docker-compose"))).toEqual({
			bin: "docker-compose",
			args: [],
		});
		expect(detectRuntime(have())).toBeNull();
	});

	it("compose_args_exact", () => {
		const rt = { bin: "podman", args: ["compose"] };
		expect(composeArgs(rt, "tikv", "/res/stacks", ["up", "-d"])).toEqual([
			"compose",
			"-p",
			"loams-desktop-tikv",
			"-f",
			"/res/stacks/tikv/compose.yaml",
			"up",
			"-d",
		]);
		expect(
			composeArgs({ bin: "docker-compose", args: [] }, "postgres", "/r", [
				"down",
			]),
		).toEqual([
			"-p",
			"loams-desktop-postgres",
			"-f",
			"/r/neon/compose.yaml",
			"down",
		]);
		expect(
			composeArgs(rt, "wesql", "/r", ["ps", "--format", "json"]).slice(4),
		).toEqual(["/r/wesql/compose.yaml", "ps", "--format", "json"]);
	});

	it("parses_ps_json", () => {
		expect(parsePs(DOCKER_NDJSON)).toEqual([
			{ name: "pd", state: "running", ports: ["127.0.0.1:19379->19379"] },
			{ name: "tikv", state: "running", ports: [] },
		]);
		expect(parsePs(PODMAN_ARRAY)).toEqual([
			{ name: "pd", state: "running", ports: ["127.0.0.1:19379->19379"] },
			{ name: "tikv", state: "running", ports: [] },
		]);
		expect(parsePs("")).toEqual([]);
		expect(parsePs("[]")).toEqual([]);
	});

	it("unavailable_without_runtime", async () => {
		let ran = false;
		const m = manager(null, async () => {
			ran = true;
			return { code: 0, stdout: "" };
		});
		expect(await m.state("tikv")).toEqual({
			phase: "unavailable",
			reason: "no_container_runtime",
		});
		const r = await m.start("tikv");
		expect(r.ok).toBe(false);
		expect((await m.stop("tikv")).ok).toBe(false);
		expect(ran).toBe(false);
	});

	it("start_stop_run_exact_commands_and_report_state", async () => {
		const calls: string[][] = [];
		let up = false;
		const m = manager(
			{ bin: "docker", args: ["compose"] },
			async (bin, args) => {
				calls.push([bin, ...args]);
				if (args.includes("up")) up = true;
				if (args.includes("down")) up = false;
				if (args.includes("ps"))
					return { code: 0, stdout: up ? DOCKER_NDJSON : "" };
				return { code: 0, stdout: "" };
			},
		);
		const seen: string[] = [];
		m.on("state", (_id, s: StackState) => seen.push(s.phase));
		expect((await m.start("tikv")).ok).toBe(true);
		expect(calls[0]).toEqual([
			"docker",
			"compose",
			"-p",
			"loams-desktop-tikv",
			"-f",
			"/res/stacks/tikv/compose.yaml",
			"up",
			"-d",
		]);
		expect(await m.state("tikv")).toMatchObject({ phase: "running" });
		expect((await m.stop("tikv")).ok).toBe(true);
		expect(calls.some((c) => c.includes("down"))).toBe(true);
		expect(seen).toEqual(["starting", "running", "stopped"]);
	});

	it("failed_up_is_error_and_postgres_wesql_conflict", async () => {
		const m = manager({ bin: "docker", args: ["compose"] }, async (_b, args) =>
			args.includes("up") ? { code: 1, stdout: "" } : { code: 0, stdout: "" },
		);
		const r = await m.start("wesql");
		expect(r).toMatchObject({ ok: false, code: "compose_failed" });
		expect(await m.state("wesql")).toMatchObject({ phase: "error" });

		const running = JSON.stringify([
			...[
				"rustfs",
				"storage_broker",
				"pageserver",
				"safekeeper1",
				"compute1",
			].map((s) => ({
				Service: s,
				State: "running",
			})),
		]);
		const m2 = manager(
			{ bin: "docker", args: ["compose"] },
			async (_b, args) =>
				args.includes("-f") &&
				args.some((a) => a.includes("/neon/")) &&
				args.includes("ps")
					? { code: 0, stdout: running }
					: { code: 0, stdout: "" },
		);
		expect(await m2.start("wesql")).toMatchObject({
			ok: false,
			code: "port_conflict",
		});
	});

	it("tikv_running_restarts_engine_with_live", async () => {
		const pds: (string | null)[] = [];
		const engine = {
			setLivePd: async (pd: string | null) => {
				pds.push(pd);
			},
		};
		let up = false;
		const m = manager(
			{ bin: "docker", args: ["compose"] },
			async (_b, args) => {
				if (args.includes("up")) up = true;
				if (args.includes("down")) up = false;
				if (args.includes("ps"))
					return { code: 0, stdout: up ? DOCKER_NDJSON : "" };
				return { code: 0, stdout: "" };
			},
		);
		bindLiveToTikv(m, engine);
		await m.start("tikv");
		expect(pds).toEqual([TIKV_PD_ADDR]);
		expect(TIKV_PD_ADDR).toBe("127.0.0.1:19379");
		await m.stop("tikv");
		expect(pds).toEqual([TIKV_PD_ADDR, null]);
		// Other stacks and non-terminal phases never touch the engine.
		const em = new EventEmitter();
		bindLiveToTikv(em, engine);
		em.emit("state", "wesql", { phase: "running", services: [] });
		em.emit("state", "tikv", { phase: "starting" });
		em.emit("state", "tikv", { phase: "error", message: "x" });
		expect(pds).toHaveLength(2);
	});
});
