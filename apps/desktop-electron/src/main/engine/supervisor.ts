// Adapted from dataelement/dsh-desktop (MIT), src/main/runtime/harness-runtime.ts.
import type { ChildProcess, spawn as spawnFn } from "node:child_process";
import { EventEmitter } from "node:events";
import type { EngineState } from "../../shared/contracts";
import { engineArgs } from "./binary";
import { RotatingLog } from "./log-rotate";
import { reservePorts } from "./ports";

export interface SupervisorDeps {
	spawn: typeof spawnFn;
	binary: () => string | null;
	dataDir: string;
	logFile: string;
	fetch: typeof fetch;
	now: () => number;
	sleep: (ms: number) => Promise<void>;
	/** Whether the binary was built with the optional `live` feature. Default: false. */
	liveSupported?: (bin: string) => Promise<boolean>;
	livePd?: string | null;
	platform?: NodeJS.Platform;
	env?: NodeJS.ProcessEnv;
	graceMs?: number;
	pollMs?: number;
}

const BACKOFF_S = [1, 2, 4, 8, 16, 30];
const MAX_RESTARTS = 5;
const WINDOW_MS = 10 * 60 * 1000;
const SECRET_NAME = /(^LOAMS_.*_TOKEN$)|(_SECRET$)|(_API_KEY$)/i;

export function scrubEnv(env: NodeJS.ProcessEnv): NodeJS.ProcessEnv {
	const out: NodeJS.ProcessEnv = {};
	for (const [k, v] of Object.entries(env))
		if (!SECRET_NAME.test(k)) out[k] = v;
	return out;
}

export class EngineSupervisor extends EventEmitter {
	private cur: EngineState = { phase: "stopped" };
	private child: ChildProcess | null = null;
	private generation = 0;
	private restarts: number[] = [];
	private livePd: string | null;
	private readonly log: RotatingLog;
	private stopping: Promise<void> | null = null;

	constructor(private readonly deps: SupervisorDeps) {
		super();
		this.livePd = deps.livePd ?? null;
		this.log = new RotatingLog(deps.logFile);
	}

	state(): EngineState {
		return this.cur;
	}

	private set(s: EngineState): void {
		this.cur = s;
		this.emit("state", s);
	}

	private fail(reason: string): void {
		this.log.write(`[supervisor] failed: ${reason}\n`);
		this.set({ phase: "failed", reason, logPath: this.deps.logFile });
	}

	start(): void {
		if (this.cur.phase === "starting" || this.cur.phase === "ready") return;
		this.restarts = [];
		const gen = ++this.generation;
		void this.guardedRun(gen, 1);
	}

	/** Any unexpected throw becomes `failed` (if still current), never an unhandled rejection. */
	private async guardedRun(gen: number, attempt: number): Promise<void> {
		try {
			await this.run(gen, attempt);
		} catch (e) {
			if (gen !== this.generation) return;
			const child = this.child;
			this.generation++;
			this.child = null;
			if (child) await this.kill(child).catch(() => {});
			this.fail(`unexpected error: ${(e as Error).message}`);
		}
	}

	async stop(): Promise<void> {
		this.generation++;
		const child = this.child;
		if (!child) {
			if (this.cur.phase !== "stopped") this.set({ phase: "stopped" });
			return;
		}
		this.stopping ??= this.kill(child).finally(() => {
			this.stopping = null;
		});
		await this.stopping;
		this.child = null;
		this.set({ phase: "stopped" });
	}

	/** Changes the live PD endpoint; restarts a running engine so the flags take effect. */
	async setLivePd(pd: string | null): Promise<void> {
		if (pd === this.livePd) return;
		this.livePd = pd;
		if (this.cur.phase === "starting" || this.cur.phase === "ready") {
			await this.stop();
			this.start();
		}
	}

	private async kill(child: ChildProcess): Promise<void> {
		const exited = new Promise<void>((r) => {
			if (child.exitCode !== null || child.signalCode !== null) r();
			else child.once("exit", () => r());
		});
		const win = (this.deps.platform ?? process.platform) === "win32";
		child.kill("SIGTERM");
		const grace = this.deps.sleep(this.deps.graceMs ?? 5000).then(() => false);
		const done = await Promise.race([exited.then(() => true), grace]);
		if (done) return;
		if (win && child.pid !== undefined) {
			this.deps.spawn("taskkill", ["/pid", String(child.pid), "/T", "/F"], {
				stdio: "ignore",
			});
		} else child.kill("SIGKILL");
		await exited;
	}

	private async run(gen: number, attempt: number): Promise<void> {
		const d = this.deps;
		const alive = (): boolean => gen === this.generation;
		this.set({ phase: "starting", attempt });
		const bin = d.binary();
		if (!bin) {
			this.fail("engine binary not found; set LOAMS_BIN or reinstall");
			return;
		}
		let ports: number[];
		let liveOk = false;
		try {
			liveOk = (await d.liveSupported?.(bin)) ?? false;
			const wantLive = liveOk && !!this.livePd;
			ports = await reservePorts(wantLive ? 5 : 4);
		} catch (e) {
			if (!alive()) return;
			this.fail(`could not reserve ports: ${(e as Error).message}`);
			return;
		}
		if (!alive()) return;
		const [http, flight, es, durable, live] = ports as [
			number,
			number,
			number,
			number,
			number | undefined,
		];
		const pd = this.livePd ?? undefined;
		const args = engineArgs({
			dataDir: d.dataDir,
			ports: { http, flight, es, durable, live },
			live: { supported: liveOk, pd },
		});
		const url = `http://127.0.0.1:${http}`;
		let child: ChildProcess;
		try {
			child = d.spawn(bin, args, {
				env: scrubEnv(d.env ?? process.env),
				stdio: ["ignore", "pipe", "pipe"],
				windowsHide: true,
			});
		} catch (e) {
			this.fail(`could not start engine: ${(e as Error).message}`);
			return;
		}
		this.child = child;
		this.log.write(`[supervisor] spawn ${bin} ${args.join(" ")}\n`);
		child.stdout?.on("data", (c: Buffer) => this.log.write(c));
		child.stderr?.on("data", (c: Buffer) => this.log.write(c));
		let exitCode: number | null | undefined;
		const exited = new Promise<void>((r) => {
			child.once("error", (e) => {
				exitCode = -1;
				this.log.write(`[supervisor] spawn error: ${e.message}\n`);
				r();
			});
			child.once("exit", (code) => {
				exitCode = code ?? -1;
				r();
			});
		});

		const win = (d.platform ?? process.platform) === "win32";
		const timeout = win ? 180_000 : 60_000;
		const t0 = d.now();
		let ready = false;
		while (alive() && exitCode === undefined && d.now() - t0 < timeout) {
			try {
				const r = await d.fetch(
					`${url}/loams.instance.v1.InstanceService/GetInstance`,
					{
						method: "POST",
						headers: { "content-type": "application/json" },
						body: "{}",
						signal: AbortSignal.timeout(2000),
					},
				);
				if (r.ok) {
					ready = true;
					break;
				}
			} catch {
				/* not up yet */
			}
			await Promise.race([d.sleep(d.pollMs ?? 250), exited]);
		}
		if (!alive()) return; // stop() owns the child now
		if (ready) {
			this.set({
				phase: "ready",
				url,
				esUrl: `http://127.0.0.1:${es}`,
				flightUrl: `grpc://127.0.0.1:${flight}`,
				durableUrl: `http://127.0.0.1:${durable}`,
				...(live !== undefined && pd
					? { liveUrl: `http://127.0.0.1:${live}` }
					: {}),
				pid: child.pid ?? 0,
			});
			await exited;
			if (!alive()) return;
		} else if (exitCode === undefined) {
			this.generation++;
			await this.kill(child);
			this.child = null;
			this.fail(`engine did not become ready within ${timeout / 1000}s`);
			return;
		}
		this.child = null;

		// Crashed (before or after ready).
		const t = d.now();
		this.restarts = this.restarts.filter((x) => t - x < WINDOW_MS);
		if (this.restarts.length >= MAX_RESTARTS) {
			this.fail(
				`engine exited (code ${exitCode}) and restarted ${MAX_RESTARTS} times in 10 minutes; giving up`,
			);
			return;
		}
		this.restarts.push(t);
		const delay =
			BACKOFF_S[Math.min(this.restarts.length - 1, BACKOFF_S.length - 1)] ?? 30;
		this.log.write(
			`[supervisor] engine exited (code ${exitCode}); restart in ${delay}s\n`,
		);
		this.set({ phase: "starting", attempt: attempt + 1 });
		await d.sleep(delay * 1000);
		if (!alive()) return;
		await this.run(gen, attempt + 1); // errors reach guardedRun
	}
}
