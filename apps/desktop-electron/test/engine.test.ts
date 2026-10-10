import { spawn } from "node:child_process";
import { EventEmitter } from "node:events";
import { existsSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { PassThrough } from "node:stream";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import {
	engineArgs,
	findEngineBinary,
	helpSupportsLive,
	liveSupportFromHelp,
} from "../src/main/engine/binary";
import { RotatingLog } from "../src/main/engine/log-rotate";
import { reservePorts } from "../src/main/engine/ports";
import { EngineSupervisor } from "../src/main/engine/supervisor";

const FAKE = fileURLToPath(
	new URL("./fixtures/fake-engine.mjs", import.meta.url),
);
const scratch = () => mkdtempSync(join(tmpdir(), "engine-test-"));

class FakeChild extends EventEmitter {
	stdout = new PassThrough();
	stderr = new PassThrough();
	exitCode: number | null = null;
	signalCode: string | null = null;
	pid = 4242;
	signals: string[] = [];
	constructor(private readonly dieOn: string[] = ["SIGTERM", "SIGKILL"]) {
		super();
	}
	kill(sig: string = "SIGTERM"): boolean {
		this.signals.push(sig);
		if (this.dieOn.includes(sig)) this.die(null);
		return true;
	}
	die(code: number | null): void {
		this.exitCode = code ?? 0;
		this.emit("exit", code, null);
	}
}

// biome-ignore lint/suspicious/noExplicitAny: test double
const asSpawn = (f: (...a: any[]) => unknown) => f as unknown as typeof spawn;
const noFetch = (async () => {
	throw new Error("down");
}) as unknown as typeof fetch;

function deps(over: Record<string, unknown> = {}) {
	const dir = scratch();
	return {
		spawn: asSpawn(() => new FakeChild()),
		binary: () => "/bin/engine",
		dataDir: join(dir, "data"),
		logFile: join(dir, "engine.log"),
		fetch: noFetch,
		now: () => 0,
		sleep: () => Promise.resolve(),
		...over,
	} as ConstructorParameters<typeof EngineSupervisor>[0];
}

const until = async (f: () => boolean, ms = 8000) => {
	const t = Date.now();
	while (!f()) {
		if (Date.now() - t > ms) throw new Error("timeout");
		await new Promise((r) => setTimeout(r, 10));
	}
};

describe("engine", () => {
	it("reserves_distinct_ports", async () => {
		const p = await reservePorts(5);
		expect(new Set(p).size).toBe(5);
		for (const x of p) expect(x).toBeGreaterThan(0);
	});

	it("engine_args_exact", () => {
		const ports = { http: 1, flight: 2, es: 3, durable: 4, live: 5 };
		const base = [
			"dev",
			"--data-dir",
			"/d",
			"--listen",
			"127.0.0.1:1",
			"--flight-sql-listen",
			"127.0.0.1:2",
			"--es-listen",
			"127.0.0.1:3",
			"--no-qdrant",
			"--durable-listen",
			"127.0.0.1:4",
		];
		expect(
			engineArgs({ dataDir: "/d", ports, live: { supported: false } }),
		).toEqual(base);
		expect(
			engineArgs({ dataDir: "/d", ports, live: { supported: true } }),
		).toEqual([...base, "--no-live"]);
		expect(
			engineArgs({
				dataDir: "/d",
				ports,
				live: { supported: true, pd: "pd:2379" },
			}),
		).toEqual([
			...base,
			"--live-listen",
			"127.0.0.1:5",
			"--live-pd",
			"pd:2379",
		]);
		// An engine with the embedded store (LV1 Task 23, ruling T23-7): Live
		// with no PD, and `--live-store tikv://…` (not the deprecated
		// `--live-pd`) with one.
		expect(
			engineArgs({
				dataDir: "/d",
				ports,
				live: { supported: true, embedded: true },
			}),
		).toEqual([...base, "--live-listen", "127.0.0.1:5"]);
		expect(
			engineArgs({
				dataDir: "/d",
				ports,
				live: { supported: true, embedded: true, pd: "pd:2379" },
			}),
		).toEqual([
			...base,
			"--live-listen",
			"127.0.0.1:5",
			"--live-store",
			"tikv://pd:2379",
		]);
		expect(helpSupportsLive("  --no-live  disable")).toBe(true);
		expect(helpSupportsLive("--no-durable")).toBe(false);
		expect(liveSupportFromHelp("--no-durable")).toBe("none");
		// The `--live-store` block names tikv:// only in a live-tikv build
		// (ruling T23-9); tikv:// elsewhere in the help does not count.
		const help = (store: string) =>
			[
				"      --durable-store <S>",
				"          Where durable state lives: sqlite:<path>, tikv://<pd>/<ks>",
				"      --live-listen <A>",
				"          Address",
				"      --live-store <LIVE_STORE>",
				`          ${store}`,
				"      --no-live",
				"          Serve no Live",
			].join("\n");
		expect(
			liveSupportFromHelp(help("`embedded` or `tikv://<pd>/<keyspace>`")),
		).toBe("embedded+tikv");
		expect(
			liveSupportFromHelp(
				help("`embedded`, a store (this build has no other)"),
			),
		).toBe("embedded");
		expect(
			liveSupportFromHelp("--live-listen <A>\n --live-pd <P>\n --no-live"),
		).toBe("tikv");
		expect(
			liveSupportFromHelp("--live-listen <A>\n --live-store <S>\n --no-live"),
		).toBe("embedded");
		expect(findEngineBinary(["/a", "/b"], (p) => p === "/b")).toBe("/b");
		expect(findEngineBinary(["/a"], () => false)).toBeNull();
	});

	it("ready_with_fake_engine", async () => {
		const sup = new EngineSupervisor(
			deps({
				spawn: asSpawn((_c: string, a: string[], o: object) =>
					spawn(process.execPath, [FAKE, ...a], o),
				),
				binary: () => process.execPath,
				fetch,
				sleep: (ms: number) => new Promise((r) => setTimeout(r, ms)),
				now: Date.now,
			}),
		);
		sup.start();
		await until(() => sup.state().phase === "ready");
		const s = sup.state();
		if (s.phase !== "ready") throw new Error("not ready");
		expect(s.url).toMatch(/^http:\/\/127\.0\.0\.1:\d+$/);
		expect(s.durableUrl).toMatch(/^http:\/\/127\.0\.0\.1:\d+$/);
		expect(s.liveUrl).toBeUndefined();
		const pid = s.pid;
		await sup.stop();
		expect(sup.state().phase).toBe("stopped");
		expect(() => process.kill(pid, 0)).toThrow();
	});

	it("live_flags_follow_probe_and_pd", async () => {
		const seen: string[][] = [];
		const mk = (supported: boolean, livePd?: string) =>
			new EngineSupervisor(
				deps({
					spawn: asSpawn((_c: string, a: string[]) => {
						seen.push(a);
						return new FakeChild();
					}),
					fetch: (async () => ({ ok: true })) as unknown as typeof fetch,
					liveSupported: async () => supported,
					livePd,
				}),
			);
		const a = mk(true, "pd:2379");
		a.start();
		await until(() => a.state().phase === "ready");
		const s = a.state();
		expect(s.phase === "ready" && s.liveUrl).toMatch(/^http:\/\/127/);
		expect(seen[0]).toContain("--live-pd");
		const b = mk(true);
		b.start();
		await until(() => b.state().phase === "ready");
		expect(seen[1]).toContain("--no-live");
		const c = mk(false, "pd:2379");
		c.start();
		await until(() => c.state().phase === "ready");
		expect(seen[2]?.join(" ")).not.toMatch(/live/);
	});

	it("an_engine_without_live_tikv_ignores_the_pd_with_a_notice", async () => {
		const seen: string[][] = [];
		const sup = new EngineSupervisor(
			deps({
				spawn: asSpawn((_c: string, a: string[]) => {
					seen.push(a);
					return new FakeChild();
				}),
				fetch: (async () => ({ ok: true })) as unknown as typeof fetch,
				liveSupported: async () => "embedded" as const,
				livePd: "127.0.0.1:19379",
			}),
		);
		sup.start();
		await until(() => sup.state().phase === "ready");
		expect(seen[0]).toContain("--live-listen");
		expect(seen[0]?.join(" ")).not.toMatch(/--live-store|--live-pd|19379/);
		const s = sup.state();
		expect(s.phase === "ready" && s.liveUrl).toMatch(/^http:\/\/127/);
		expect(s.phase === "ready" && s.liveNotice).toMatch(/without Live on TiKV/);
		await sup.stop();
	});

	it("a_live_tikv_refusal_restarts_on_the_embedded_store", async () => {
		const seen: string[][] = [];
		const sup = new EngineSupervisor(
			deps({
				spawn: asSpawn((_c: string, a: string[]) => {
					seen.push(a);
					const child = new FakeChild();
					if (seen.length === 1) {
						// The engine refuses the TiKV store and exits.
						setTimeout(() => {
							child.stderr.write(
								"error: invalid value 'tikv://127.0.0.1:19379' for '--live-store <LIVE_STORE>': --live-store tikv:// needs a build with the live-tikv feature\n",
							);
							setTimeout(() => child.die(2), 5);
						}, 5);
					}
					return child;
				}),
				fetch: (async () => {
					if (seen.length < 2) throw new Error("down");
					return { ok: true };
				}) as unknown as typeof fetch,
				sleep: (ms: number) =>
					new Promise((r) => setTimeout(r, Math.min(ms, 5))),
				liveSupported: async () => "embedded+tikv" as const,
				livePd: "127.0.0.1:19379",
			}),
		);
		sup.start();
		await until(() => sup.state().phase === "ready");
		expect(seen).toHaveLength(2);
		expect(seen[0]).toContain("tikv://127.0.0.1:19379");
		expect(seen[1]).toContain("--live-listen");
		expect(seen[1]?.join(" ")).not.toMatch(/--live-store|--live-pd/);
		const s = sup.state();
		expect(s.phase === "ready" && s.liveNotice).toMatch(/without Live on TiKV/);
		// The next start remembers it: no second refusal.
		await sup.stop();
		sup.start();
		await until(() => seen.length === 3 && sup.state().phase === "ready");
		expect(seen[2]?.join(" ")).not.toMatch(/--live-store/);
		await sup.stop();
	});

	it("set_live_notice_updates_a_ready_engine_without_a_restart", async () => {
		const seen: string[][] = [];
		const sup = new EngineSupervisor(
			deps({
				spawn: asSpawn((_c: string, a: string[]) => {
					seen.push(a);
					return new FakeChild();
				}),
				fetch: (async () => ({ ok: true })) as unknown as typeof fetch,
				liveSupported: async () => "embedded+tikv" as const,
			}),
		);
		sup.setLiveNotice("Live on TiKV is unavailable; showing local data.");
		sup.start();
		await until(() => sup.state().phase === "ready");
		let s = sup.state();
		expect(s.phase === "ready" && s.liveNotice).toBe(
			"Live on TiKV is unavailable; showing local data.",
		);
		sup.setLiveNotice(null);
		s = sup.state();
		expect(s.phase === "ready" && s.liveNotice).toBeFalsy();
		expect(seen).toHaveLength(1);
		await sup.stop();
	});

	it("embedded_live_runs_without_a_pd", async () => {
		const seen: string[][] = [];
		const sup = new EngineSupervisor(
			deps({
				spawn: asSpawn((_c: string, a: string[]) => {
					seen.push(a);
					return new FakeChild();
				}),
				fetch: (async () => ({ ok: true })) as unknown as typeof fetch,
				liveSupported: async () => "embedded+tikv" as const,
			}),
		);
		sup.start();
		await until(() => sup.state().phase === "ready");
		const s = sup.state();
		expect(s.phase === "ready" && s.liveUrl).toMatch(/^http:\/\/127/);
		expect(seen[0]).toContain("--live-listen");
		expect(seen[0]).not.toContain("--no-live");
		expect(seen[0]).not.toContain("--live-store");
		// A PD selects TiKV through --live-store, not the deprecated --live-pd.
		await sup.setLivePd("127.0.0.1:19379");
		await until(() => seen.length === 2 && sup.state().phase === "ready");
		expect(seen[1]).toContain("--live-store");
		expect(seen[1]).toContain("tikv://127.0.0.1:19379");
		expect(seen[1]).not.toContain("--live-pd");
		// Back to no PD: embedded Live again.
		await sup.setLivePd(null);
		await until(() => seen.length === 3 && sup.state().phase === "ready");
		expect(seen[2]).toContain("--live-listen");
		expect(seen[2]).not.toContain("--no-live");
		const r = sup.state();
		expect(r.phase === "ready" && r.liveUrl).toMatch(/^http:\/\/127/);
		await sup.stop();
	});

	it("set_live_pd_restarts_engine", async () => {
		const seen: string[][] = [];
		const sup = new EngineSupervisor(
			deps({
				spawn: asSpawn((_c: string, a: string[]) => {
					seen.push(a);
					return new FakeChild();
				}),
				fetch: (async () => ({ ok: true })) as unknown as typeof fetch,
				liveSupported: async () => true,
			}),
		);
		sup.start();
		await until(() => sup.state().phase === "ready");
		expect(seen).toHaveLength(1);
		expect(seen[0]).toContain("--no-live");
		await sup.setLivePd("127.0.0.1:19379");
		await until(() => seen.length === 2 && sup.state().phase === "ready");
		expect(seen[1]).toContain("--live-pd");
		expect(seen[1]).toContain("127.0.0.1:19379");
		await sup.setLivePd("127.0.0.1:19379"); // unchanged: no restart
		expect(seen).toHaveLength(2);
		await sup.setLivePd(null);
		await until(() => seen.length === 3 && sup.state().phase === "ready");
		expect(seen[2]).toContain("--no-live");
		await sup.stop();
		await sup.setLivePd("pd:1"); // stopped engine stays stopped
		expect(sup.state().phase).toBe("stopped");
		expect(seen).toHaveLength(3);
	});

	it("disposed_supervisor_ignores_start_and_restart", async () => {
		const seen: string[][] = [];
		const sup = new EngineSupervisor(
			deps({
				spawn: asSpawn((_c: string, a: string[]) => {
					seen.push(a);
					return new FakeChild();
				}),
				fetch: (async () => ({ ok: true })) as unknown as typeof fetch,
				liveSupported: async () => true,
			}),
		);
		sup.start();
		await until(() => sup.state().phase === "ready");
		// A tikv state change during quit must not bring the engine back.
		const disposing = sup.dispose();
		const restart = sup.setLivePd("127.0.0.1:19379");
		await disposing;
		await restart;
		sup.start();
		await new Promise((r) => setTimeout(r, 30));
		expect(sup.state().phase).toBe("stopped");
		expect(seen).toHaveLength(1);
		await sup.dispose(); // idempotent
	});

	it("missing_binary_is_failed_not_thrown", async () => {
		const sup = new EngineSupervisor(deps({ binary: () => null }));
		sup.start();
		await until(() => sup.state().phase === "failed");
		const s = sup.state();
		expect(s.phase === "failed" && s.reason).toMatch(/not found/);
	});

	it("crash_loop_stops_after_five", async () => {
		let spawns = 0;
		const sleeps: number[] = [];
		const sup = new EngineSupervisor(
			deps({
				spawn: asSpawn(() => {
					spawns++;
					const c = new FakeChild();
					queueMicrotask(() => c.die(1));
					return c;
				}),
				sleep: async (ms: number) => {
					sleeps.push(ms);
				},
			}),
		);
		sup.start();
		await until(() => sup.state().phase === "failed");
		expect(spawns).toBe(6);
		expect(sleeps.filter((s) => s >= 1000)).toEqual([
			1000, 2000, 4000, 8000, 16000,
		]);
	});

	it("stop_kills_after_grace", async () => {
		const child = new FakeChild(["SIGKILL"]);
		let release: () => void = () => {};
		const graceGate = new Promise<void>((r) => {
			release = r;
		});
		const sup = new EngineSupervisor(
			deps({
				spawn: asSpawn(() => child),
				fetch: (async () => ({ ok: true })) as unknown as typeof fetch,
				sleep: (ms: number) => (ms === 5000 ? graceGate : Promise.resolve()),
			}),
		);
		sup.start();
		await until(() => sup.state().phase === "ready");
		const p = sup.stop();
		await until(() => child.signals.length > 0);
		expect(child.signals).toEqual(["SIGTERM"]);
		release();
		await p;
		expect(child.signals).toEqual(["SIGTERM", "SIGKILL"]);
		expect(sup.state().phase).toBe("stopped");
	});

	it("env_scrubbed_of_secrets", async () => {
		let env: NodeJS.ProcessEnv = {};
		const sup = new EngineSupervisor(
			deps({
				spawn: asSpawn(
					(_c: string, _a: string[], o: { env: NodeJS.ProcessEnv }) => {
						env = o.env;
						return new FakeChild();
					},
				),
				fetch: (async () => ({ ok: true })) as unknown as typeof fetch,
				env: {
					PATH: "/usr/bin",
					LOAMS_ADMIN_TOKEN: "x",
					GITHUB_SECRET: "x",
					OPENAI_API_KEY: "x",
					LOAMS_LOG: "debug",
				},
			}),
		);
		sup.start();
		await until(() => sup.state().phase === "ready");
		expect(Object.keys(env).sort()).toEqual(["LOAMS_LOG", "PATH"]);
	});

	it("log_rotates_at_limit", () => {
		const f = join(scratch(), "engine.log");
		const log = new RotatingLog(f, 100, 2);
		for (let i = 0; i < 10; i++) log.write("x".repeat(40));
		expect(existsSync(`${f}.1`)).toBe(true);
		expect(existsSync(`${f}.2`)).toBe(true);
		expect(existsSync(`${f}.3`)).toBe(false);
		expect(readFileSync(f, "utf8").length).toBeLessThanOrEqual(100);
	});

	it("readiness_hung_request_times_out_to_failed", async () => {
		let clock = 0;
		let sawSignal = false;
		const hung = ((_u: string, init?: RequestInit) => {
			sawSignal = !!init?.signal;
			return new Promise((_res, rej) => {
				init?.signal?.addEventListener("abort", () => {
					clock += 61_000; // the deadline passes while the request hangs
					rej(new Error("aborted"));
				});
			});
		}) as unknown as typeof fetch;
		const sup = new EngineSupervisor(deps({ fetch: hung, now: () => clock }));
		sup.start();
		await until(() => sup.state().phase === "failed", 8000);
		expect(sawSignal).toBe(true);
		const st = sup.state();
		expect(st.phase === "failed" && st.reason).toMatch(/did not become ready/);
	}, 15000);

	it("stale_run_cannot_overwrite_new_state", async () => {
		let rejectFirst: (e: Error) => void = () => {};
		let calls = 0;
		const sup = new EngineSupervisor(
			deps({
				liveSupported: () => {
					calls++;
					return calls === 1
						? new Promise<boolean>((_r, rej) => {
								rejectFirst = rej;
							})
						: new Promise<boolean>(() => {});
				},
			}),
		);
		sup.start();
		await until(() => calls === 1);
		await sup.stop();
		sup.start();
		await until(() => calls === 2);
		rejectFirst(new Error("late boom"));
		await new Promise((r) => setTimeout(r, 50));
		expect(sup.state().phase).toBe("starting");
	});

	it("unexpected_throw_in_run_goes_to_failed", async () => {
		const sup = new EngineSupervisor(
			deps({
				binary: () => {
					throw new Error("kaboom");
				},
			}),
		);
		sup.start();
		await until(() => sup.state().phase === "failed");
		const st = sup.state();
		expect(st.phase === "failed" && st.reason).toMatch(/kaboom/);
	});

	it("log_dir_unwritable_degrades", () => {
		const dir = scratch();
		const blocker = join(dir, "file");
		writeFileSync(blocker, "x");
		const log = new RotatingLog(join(blocker, "sub", "engine.log"));
		expect(() => log.write("hello")).not.toThrow();
	});
});
