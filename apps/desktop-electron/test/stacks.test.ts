import { EventEmitter } from "node:events";
import { existsSync, mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { resolveRuntime } from "../src/main/stacks/ipc.electron";
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

const LOGS = () => mkdtempSync(join(tmpdir(), "stacks-test-"));
function manager(
	runtime: ReturnType<typeof detectRuntime>,
	run: RunFn,
	logsDir = LOGS(),
) {
	return new StackManager({ runtime, stacksDir: "/res/stacks", logsDir, run });
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
		// One stopped observation is not enough; the next poll confirms it.
		expect(pds).toEqual([TIKV_PD_ADDR]);
		await m.state("tikv");
		expect(pds).toEqual([TIKV_PD_ADDR, null]);
		// Other stacks and non-terminal phases never touch the engine.
		const em = new EventEmitter();
		bindLiveToTikv(em, engine);
		em.emit("observed", "wesql", { phase: "running", services: [] });
		em.emit("observed", "tikv", { phase: "starting" });
		em.emit("observed", "tikv", { phase: "error", message: "x" });
		expect(pds).toHaveLength(2);
		// stopped, running, stopped never reaches two consecutive stops
		const pds2: (string | null)[] = [];
		const em2 = new EventEmitter();
		bindLiveToTikv(em2, { setLivePd: async (p) => void pds2.push(p) });
		const run = { phase: "running", services: [] };
		for (const s of [{ phase: "stopped" }, run, { phase: "stopped" }])
			em2.emit("observed", "tikv", s);
		expect(pds2).toEqual([TIKV_PD_ADDR]);
		// a rejecting setLivePd is caught
		const errs: unknown[] = [];
		const em3 = new EventEmitter();
		bindLiveToTikv(
			em3,
			{ setLivePd: () => Promise.reject(new Error("boom")) },
			(e) => errs.push(e),
		);
		em3.emit("observed", "tikv", run);
		await new Promise((r) => setTimeout(r, 5));
		expect(errs).toHaveLength(1);
	});

	it("poll_ps_is_not_logged_on_success", async () => {
		const dir = LOGS();
		const m = manager(
			{ bin: "docker", args: ["compose"] },
			async (_b, args, o) => {
				const ps = args.includes("ps");
				expect(o.quiet).toBe(ps);
				if (!ps) o.log("up output\n");
				return { code: 0, stdout: "" };
			},
			dir,
		);
		await m.state("tikv");
		await m.state("tikv");
		const f = join(dir, "stacks", "tikv.log");
		expect(existsSync(f)).toBe(false);
		await m.start("tikv");
		expect(readFileSync(f, "utf8")).toContain("up output");
	});

	it("ps_nonzero_and_bad_json_are_error_state", async () => {
		const bad = manager({ bin: "docker", args: ["compose"] }, async () => ({
			code: 2,
			stdout: "",
		}));
		expect(await bad.state("tikv")).toMatchObject({ phase: "error" });
		const junk = manager({ bin: "docker", args: ["compose"] }, async () => ({
			code: 0,
			stdout: "{not json",
		}));
		expect(await junk.state("tikv")).toMatchObject({ phase: "error" });
	});

	it("concurrent_postgres_and_wesql_start_one_refused", async () => {
		let release: () => void = () => {};
		const gate = new Promise<void>((r) => {
			release = r;
		});
		const ups: string[] = [];
		const m = manager(
			{ bin: "docker", args: ["compose"] },
			async (_b, args) => {
				if (args.includes("up")) {
					ups.push(args.join(" "));
					await gate;
				}
				return { code: 0, stdout: "" };
			},
		);
		const a = m.start("postgres");
		const b = m.start("wesql");
		expect(await b).toMatchObject({ ok: false });
		release();
		expect(await a).toMatchObject({ ok: true });
		expect(ups).toHaveLength(1);
		// the refusal released nothing it did not hold, and tikv is independent
		expect(await m.start("wesql")).toMatchObject({ ok: true });
	});

	it("double_start_same_id_runs_once", async () => {
		let release: () => void = () => {};
		const gate = new Promise<void>((r) => {
			release = r;
		});
		let ups = 0;
		const m = manager(
			{ bin: "docker", args: ["compose"] },
			async (_b, args) => {
				if (args.includes("up")) {
					ups++;
					await gate;
				}
				return { code: 0, stdout: "" };
			},
		);
		const a = m.start("tikv");
		const b = m.start("tikv");
		expect(await b).toMatchObject({ ok: false, code: "busy" });
		release();
		await a;
		expect(ups).toBe(1);
	});

	it("runtime_probe_is_async_ordered_and_lazy", async () => {
		const probed: string[] = [];
		const works = async (rt: { bin: string }) => {
			probed.push(rt.bin);
			return rt.bin === "podman";
		};
		const rt = await resolveRuntime(have("docker", "podman"), works);
		expect(rt).toEqual({ bin: "podman", args: ["compose"] });
		expect(probed).toEqual(["docker", "podman"]);
		expect(await resolveRuntime(have("docker"), async () => false)).toBeNull();
		// the manager does not call the resolver until first use
		let calls = 0;
		const m = new StackManager({
			runtime: async () => {
				calls++;
				return null;
			},
			stacksDir: "/r",
			logsDir: LOGS(),
		});
		expect(calls).toBe(0);
		await m.state("tikv");
		await m.state("wesql");
		expect(calls).toBe(1);
	});
});
