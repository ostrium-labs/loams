import { EventEmitter } from "node:events";
import {
	existsSync,
	mkdirSync,
	mkdtempSync,
	readFileSync,
	writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import {
	LIVE_STORE_KEY,
	readLiveStore,
	readSetting,
	writeSetting,
} from "../src/main/settings";
import { resolveRuntime } from "../src/main/stacks/ipc.electron";
import { detectRuntime } from "../src/main/stacks/runtime";
import {
	bindLiveToTikv,
	composeArgs,
	LIVE_TIKV_UNAVAILABLE,
	parsePs,
	type RunFn,
	StackManager,
	syncStackDir,
	TIKV_PD_ADDR,
	tikvReady,
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
		bindLiveToTikv(m, engine, () => "tikv-stack");
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
		bindLiveToTikv(em, engine, () => "tikv-stack");
		em.emit("observed", "wesql", { phase: "running", services: [] });
		em.emit("observed", "tikv", { phase: "starting" });
		em.emit("observed", "tikv", { phase: "error", message: "x" });
		expect(pds).toHaveLength(2);
		// stopped, running, stopped never reaches two consecutive stops
		const pds2: (string | null)[] = [];
		const em2 = new EventEmitter();
		bindLiveToTikv(
			em2,
			{ setLivePd: async (p) => void pds2.push(p) },
			() => "tikv-stack",
		);
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
			() => "tikv-stack",
			(e) => errs.push(e),
		);
		em3.emit("observed", "tikv", run);
		await new Promise((r) => setTimeout(r, 5));
		expect(errs).toHaveLength(1);
	});

	it("live_uses_tikv_only_when_chosen_and_the_stack_runs", async () => {
		// Ruling T23-8: the desktop never switches Live's store implicitly.
		const pds: (string | null)[] = [];
		const notices: (string | null)[] = [];
		const engine = {
			setLivePd: async (p: string | null) => void pds.push(p),
			setLiveNotice: (n: string | null) => void notices.push(n),
		};
		let choice: "embedded" | "tikv-stack" = "embedded";
		const em = new EventEmitter();
		const live = bindLiveToTikv(em, engine, () => choice);
		const run = { phase: "running", services: [] };
		const stopped = { phase: "stopped" };
		// A running stack alone means nothing.
		em.emit("observed", "tikv", run);
		expect(pds.filter((p) => p !== null)).toEqual([]);
		expect(notices.at(-1)).toBeNull();
		// Chosen and running: TiKV.
		choice = "tikv-stack";
		live.refresh();
		expect(pds.at(-1)).toBe(TIKV_PD_ADDR);
		expect(notices.at(-1)).toBeNull();
		// Chosen but the stack is down: embedded, with the notice.
		em.emit("observed", "tikv", stopped);
		em.emit("observed", "tikv", stopped);
		expect(pds.at(-1)).toBeNull();
		expect(notices.at(-1)).toBe(LIVE_TIKV_UNAVAILABLE);
		expect(LIVE_TIKV_UNAVAILABLE).toBe(
			"Live on TiKV is unavailable; showing local data.",
		);
		// Back up: TiKV again, no notice.
		em.emit("observed", "tikv", run);
		expect(pds.at(-1)).toBe(TIKV_PD_ADDR);
		expect(notices.at(-1)).toBeNull();
		// Switched back to embedded: no PD whatever the stack does.
		choice = "embedded";
		live.refresh();
		expect(pds.at(-1)).toBeNull();
		em.emit("observed", "tikv", run);
		expect(pds.at(-1)).toBeNull();
		expect(notices.at(-1)).toBeNull();
	});

	it("live_store_choice_persists_in_settings", () => {
		const dir = mkdtempSync(join(tmpdir(), "settings-"));
		const file = join(dir, "settings.json");
		expect(readLiveStore(file)).toBe("embedded");
		writeSetting(file, "engine.autoStart", false);
		writeSetting(file, LIVE_STORE_KEY, "tikv-stack");
		expect(readLiveStore(file)).toBe("tikv-stack");
		expect(readSetting(file, "engine.autoStart", true)).toBe(false);
		writeSetting(file, LIVE_STORE_KEY, "nonsense");
		expect(readLiveStore(file)).toBe("embedded");
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

describe("stack directory copy (I7)", () => {
	const src = () => {
		const root = mkdtempSync(join(tmpdir(), "stack-src-"));
		mkdirSync(join(root, "tikv"));
		writeFileSync(join(root, "tikv", "compose.yaml"), "v1");
		writeFileSync(join(root, "tikv", "pd.toml"), "pd");
		return root;
	};

	it("copies_once_and_overwrites_on_version_change", () => {
		const from = join(src(), "tikv");
		const to = join(mkdtempSync(join(tmpdir(), "stack-run-")), "tikv");
		expect(syncStackDir(from, to, "1.0.0")).toBe(true);
		expect(readFileSync(join(to, "compose.yaml"), "utf8")).toBe("v1");
		expect(readFileSync(join(to, "pd.toml"), "utf8")).toBe("pd");
		writeFileSync(join(from, "compose.yaml"), "v2");
		expect(syncStackDir(from, to, "1.0.0")).toBe(false);
		expect(readFileSync(join(to, "compose.yaml"), "utf8")).toBe("v1");
		expect(syncStackDir(from, to, "1.1.0")).toBe(true);
		expect(readFileSync(join(to, "compose.yaml"), "utf8")).toBe("v2");
		// A stale file from the old version does not survive the overwrite.
		writeFileSync(join(to, "old.toml"), "x");
		writeFileSync(join(from, "compose.yaml"), "v3");
		expect(syncStackDir(from, to, "1.1.0", true)).toBe(true);
		expect(existsSync(join(to, "old.toml"))).toBe(false);
		expect(readFileSync(join(to, "compose.yaml"), "utf8")).toBe("v3");
	});

	it("compose_runs_from_the_copy_not_resources", async () => {
		const resources = src();
		const runDir = join(mkdtempSync(join(tmpdir(), "stack-run-")), "stacks");
		const seen: string[][] = [];
		const run: RunFn = async (_bin, args) => {
			seen.push(args);
			// The compose file must exist where compose is pointed at.
			const f = args[args.indexOf("-f") + 1] as string;
			expect(readFileSync(f, "utf8")).toBe("v1");
			return { code: 0, stdout: "" };
		};
		const m = new StackManager({
			runtime: { bin: "docker", args: ["compose"] },
			sourceDir: resources,
			stacksDir: runDir,
			version: "1.0.0",
			logsDir: LOGS(),
			run,
		});
		await m.start("tikv");
		expect(seen.length).toBeGreaterThan(0);
		for (const a of seen) {
			const f = a[a.indexOf("-f") + 1] as string;
			expect(f).toBe(join(runDir, "tikv", "compose.yaml"));
			expect(f.startsWith(resources)).toBe(false);
		}
	});
});

describe("tikv readiness", () => {
	const json = (v: unknown, status = 200) =>
		new Response(JSON.stringify(v), { status });
	const pdFetch =
		(health: unknown, stores: unknown) =>
		async (url: string): Promise<Response> => {
			if (url.endsWith("/pd/api/v1/health")) return json(health);
			if (url.endsWith("/pd/api/v1/stores")) return json(stores);
			return json({}, 404);
		};
	const healthy = [{ name: "pd", health: true }];
	const upStore = { count: 1, stores: [{ store: { state_name: "Up" } }] };

	it("ready_only_with_healthy_pd_and_an_up_store", async () => {
		expect(await tikvReady(pdFetch(healthy, upStore))).toBe(true);
		expect(
			await tikvReady(pdFetch([{ name: "pd", health: false }], upStore)),
		).toBe(false);
		expect(await tikvReady(pdFetch(healthy, { count: 0, stores: [] }))).toBe(
			false,
		);
		expect(
			await tikvReady(
				pdFetch(healthy, {
					count: 1,
					stores: [{ store: { state_name: "Offline" } }],
				}),
			),
		).toBe(false);
		expect(
			await tikvReady(async () => {
				throw new Error("ECONNREFUSED");
			}),
		).toBe(false);
	});

	it("probes_the_pd_client_address", async () => {
		const urls: string[] = [];
		await tikvReady(async (u) => {
			urls.push(u);
			return json([]);
		});
		expect(urls[0]).toBe(`http://${TIKV_PD_ADDR}/pd/api/v1/health`);
	});

	it("containers_running_is_starting_until_the_probe_passes", async () => {
		let ready = false;
		const m = new StackManager({
			runtime: { bin: "docker", args: ["compose"] },
			stacksDir: "/r",
			logsDir: LOGS(),
			run: async () => ({ code: 0, stdout: DOCKER_NDJSON }),
			ready: async (id) => id !== "tikv" || ready,
		});
		expect(await m.state("tikv")).toEqual({ phase: "starting" });
		ready = true;
		expect(await m.state("tikv")).toMatchObject({ phase: "running" });
	});
});
