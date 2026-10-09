import { spawn } from "node:child_process";
import { EventEmitter } from "node:events";
import {
	cpSync,
	existsSync,
	mkdirSync,
	readFileSync,
	rmSync,
	writeFileSync,
} from "node:fs";
import { dirname, join } from "node:path";
import type {
	IpcResult,
	LiveStoreChoice,
	StackErrorCode,
	StackId,
	StackState,
} from "../../shared/contracts";
import { RotatingLog } from "../engine/log-rotate";
import type { ComposeRuntime } from "./runtime";

export interface StackDef {
	/** The shipped stack: a directory under deploy/ in dev, under resources/stacks when packaged. */
	source: string;
	/**
	 * The per-user copy under the stacks run dir (userData/stacks). Kept when `source`
	 * is renamed, so users keep their copied files (postgres: still "neon", D823).
	 */
	dir: string;
	/** Compose services that must all be running for the stack to count as running. */
	required: string[];
	/** Host ports the compose file publishes (from each compose.yaml). */
	ports: Record<string, number>;
}

/** Ports and services read from deploy/{loams-postgres-dev,wesql,tikv}/compose.yaml. */
export const STACKS: Record<StackId, StackDef> = {
	postgres: {
		source: "loams-postgres-dev",
		dir: "neon",
		required: [
			"rustfs",
			"storage_broker",
			"pageserver",
			"safekeeper1",
			"compute1",
		],
		ports: {
			rustfs: 9000,
			pageserver: 9898,
			safekeeper1: 7676,
			compute1: 55433,
		},
	},
	wesql: {
		source: "wesql",
		dir: "wesql",
		required: ["rustfs", "wesql"],
		ports: { rustfs: 9000, wesql: 13306 },
	},
	tikv: {
		source: "tikv",
		dir: "tikv",
		required: ["pd", "tikv"],
		ports: { pd: 19379, tikv: 20160 },
	},
};

export const STACK_IDS = Object.keys(STACKS) as StackId[];

/** Where a stack ships: `sourceDir` is deploy/ in dev and resources/stacks when packaged. */
export function stackSource(sourceDir: string, id: StackId): string {
	return join(sourceDir, STACKS[id].source);
}

/** PD client address of the tikv stack; the engine's `--live-pd` while it runs. */
export const TIKV_PD_ADDR = `127.0.0.1:${STACKS.tikv.ports.pd}`;

/** The postgres and wesql stacks both publish rustfs on this host port. */
const SHARED_PORT_GROUP: StackId[] = ["postgres", "wesql"];

export const COMMAND_TIMEOUT_MS = 5 * 60 * 1000;
export const PROBE_TIMEOUT_MS = 3000;

/**
 * Running containers are not a usable TiKV: PD must report every member healthy and
 * at least one store must be Up, or the engine's `--live-pd` would fail on a cold start.
 */
export async function tikvReady(
	fetchFn: (url: string) => Promise<Response>,
): Promise<boolean> {
	const get = async (path: string): Promise<unknown> => {
		const r = await fetchFn(`http://${TIKV_PD_ADDR}${path}`);
		if (!r.ok) throw new Error(`status ${r.status}`);
		return r.json();
	};
	try {
		const health = await get("/pd/api/v1/health");
		if (
			!Array.isArray(health) ||
			health.length === 0 ||
			!health.every((m) => (m as { health?: unknown })?.health === true)
		)
			return false;
		const stores = (await get("/pd/api/v1/stores")) as {
			stores?: { store?: { state_name?: unknown } }[];
		};
		return (stores?.stores ?? []).some((s) => s?.store?.state_name === "Up");
	} catch {
		return false;
	}
}
/**
 * The Postgres major version the postgres stack runs: deploy/loams-postgres-dev pins
 * compute-node-v17 (PG2 Task 2, R2.2).
 */
export const PG_MAJOR = 17;

/** The postgres stack's pageserver management API. */
export const PAGESERVER_ADDR = `127.0.0.1:${STACKS.postgres.ports.pageserver}`;

/** A stack whose containers run but cannot be used, and why. */
export interface StackProblem {
	code: StackErrorCode;
	message: string;
}

/** What `ready` answers: usable, not yet (still starting), or a problem. */
export type Readiness = boolean | StackProblem;

export const RESET_POSTGRES_LABEL = "Reset local Postgres data";

/**
 * The postgres stack is usable once its pageserver answers and every timeline it
 * holds is of the compute's major version. Data made by an older Loams (Postgres 16)
 * cannot be read by compute-node-v17, so it is `pg_major_mismatch`, not "starting".
 */
export async function postgresReady(
	fetchFn: (url: string) => Promise<Response>,
): Promise<Readiness> {
	const get = async (path: string): Promise<unknown> => {
		const r = await fetchFn(`http://${PAGESERVER_ADDR}${path}`);
		if (!r.ok) throw new Error(`status ${r.status}`);
		return r.json();
	};
	let found: { tenant: string; timeline: string; version: unknown }[];
	try {
		const tenants = (await get("/v1/tenant")) as { id?: unknown }[];
		found = [];
		for (const t of Array.isArray(tenants) ? tenants : []) {
			const tenant = String(t?.id ?? "");
			if (!/^[0-9a-f]{32}$/.test(tenant)) continue;
			const timelines = (await get(`/v1/tenant/${tenant}/timeline`)) as {
				timeline_id?: unknown;
				pg_version?: unknown;
			}[];
			for (const tl of Array.isArray(timelines) ? timelines : [])
				found.push({
					tenant,
					timeline: String(tl?.timeline_id ?? ""),
					version: tl?.pg_version,
				});
		}
	} catch {
		return false;
	}
	const other = found.filter((f) => f.version !== PG_MAJOR);
	if (other.length === 0) return true;
	const versions = [...new Set(other.map((f) => String(f.version)))].join(", ");
	return {
		code: "pg_major_mismatch",
		message:
			`Your local Postgres data was made with Postgres ${versions}, and this version of ` +
			`Loams runs Postgres ${PG_MAJOR}, which cannot open it. Use "${RESET_POSTGRES_LABEL}" ` +
			`to delete the local data and start again, or keep it by staying on the older Loams.`,
	};
}

export const POLL_MS = 5000;

export function composeArgs(
	rt: ComposeRuntime,
	id: StackId,
	stacksDir: string,
	action: string[],
): string[] {
	return [
		...rt.args,
		"-p",
		`loams-desktop-${id}`,
		"-f",
		join(stacksDir, STACKS[id].dir, "compose.yaml"),
		...action,
	];
}

const VERSION_MARKER = ".loams-stack-version";

/**
 * Copies a stack directory (compose.yaml plus the configs it bind-mounts) from the
 * read-only app resources to a writable per-user copy, so no bind mount points into
 * resourcesPath (which an update replaces under a running stack). The copy is redone
 * when `version` changes or `force` is set; data lives in named volumes, not here.
 * Returns true when it copied.
 */
export function syncStackDir(
	from: string,
	to: string,
	version: string,
	force = false,
): boolean {
	let have: string | undefined;
	try {
		have = readFileSync(join(to, VERSION_MARKER), "utf8").trim();
	} catch {
		have = undefined;
	}
	if (!force && have === version && existsSync(join(to, "compose.yaml")))
		return false;
	rmSync(to, { recursive: true, force: true });
	mkdirSync(dirname(to), { recursive: true });
	cpSync(from, to, { recursive: true });
	writeFileSync(join(to, VERSION_MARKER), version);
	return true;
}

export interface ServiceRow {
	name: string;
	state: string;
	ports: string[];
}

type Obj = Record<string, unknown>;

function portsOf(row: Obj): string[] {
	const out: string[] = [];
	// docker compose: Publishers [{URL, TargetPort, PublishedPort, Protocol}]
	if (Array.isArray(row.Publishers)) {
		for (const p of row.Publishers as Obj[]) {
			if (!p.PublishedPort) continue;
			out.push(`${p.URL || "0.0.0.0"}:${p.PublishedPort}->${p.TargetPort}`);
		}
	}
	// podman ps: Ports [{host_ip, host_port, container_port, protocol}]
	if (Array.isArray(row.Ports)) {
		for (const p of row.Ports as Obj[]) {
			if (!p.host_port) continue;
			out.push(`${p.host_ip || "0.0.0.0"}:${p.host_port}->${p.container_port}`);
		}
	}
	return out;
}

function nameOf(row: Obj): string {
	if (typeof row.Service === "string") return row.Service;
	const labels = row.Labels as Obj | undefined;
	const svc = labels?.["com.docker.compose.service"];
	if (typeof svc === "string") return svc;
	if (Array.isArray(row.Names) && typeof row.Names[0] === "string")
		return row.Names[0];
	return typeof row.Name === "string" ? row.Name : "";
}

/** Parses `ps --format json`: NDJSON (docker compose v2.21+) or one JSON array (older docker, podman). */
export function parsePs(stdout: string): ServiceRow[] {
	const text = stdout.trim();
	if (!text) return [];
	let rows: Obj[];
	if (text.startsWith("[")) rows = JSON.parse(text) as Obj[];
	else
		rows = text
			.split("\n")
			.map((l) => l.trim())
			.filter(Boolean)
			.map((l) => JSON.parse(l) as Obj);
	return rows
		.map((r) => ({
			name: nameOf(r),
			state: String(r.State ?? "").toLowerCase(),
			ports: portsOf(r),
		}))
		.filter((r) => r.name);
}

export function stateFromRows(id: StackId, rows: ServiceRow[]): StackState {
	const up = rows.filter((r) => r.state === "running");
	if (up.length === 0) return { phase: "stopped" };
	const all = STACKS[id].required.every((n) => up.some((r) => r.name === n));
	return all ? { phase: "running", services: rows } : { phase: "starting" };
}

export interface RunResult {
	code: number;
	stdout: string;
}
export interface RunOpts {
	timeoutMs: number;
	log: (chunk: string | Uint8Array) => void;
	/** Poll calls: log only when the command fails. */
	quiet?: boolean;
}
export type RunFn = (
	bin: string,
	args: string[],
	opts: RunOpts,
) => Promise<RunResult>;

/** Spawns with an argument array (no shell); output goes to `log`. */
export const runCommand: RunFn = (bin, args, { timeoutMs, log, quiet }) =>
	new Promise((resolve) => {
		let pending = "";
		const out = (s: string | Uint8Array) => {
			if (quiet) pending += s.toString();
			else log(s);
		};
		const header = `\n$ ${bin} ${args.join(" ")}\n`;
		if (!quiet) log(header);
		const finish = (code: number, stdout: string) => {
			if (quiet && code !== 0) log(header + pending);
			resolve({ code, stdout });
		};
		let stdout = "";
		let child: ReturnType<typeof spawn>;
		try {
			child = spawn(bin, args, {
				stdio: ["ignore", "pipe", "pipe"],
				windowsHide: true,
			});
		} catch (e) {
			out(`spawn error: ${(e as Error).message}\n`);
			finish(-1, "");
			return;
		}
		const timer = setTimeout(() => {
			out(`timed out after ${timeoutMs} ms\n`);
			child.kill("SIGKILL");
		}, timeoutMs);
		child.stdout?.on("data", (c: Buffer) => {
			stdout += c.toString();
			out(c);
		});
		child.stderr?.on("data", (c: Buffer) => out(c));
		child.once("error", (e) => {
			out(`spawn error: ${e.message}\n`);
			clearTimeout(timer);
			finish(-1, stdout);
		});
		child.once("close", (code) => {
			clearTimeout(timer);
			finish(code ?? -1, stdout);
		});
	});

export interface StackManagerDeps {
	/** The runtime, or an async resolver called lazily on first use (never at startup). */
	runtime: ComposeRuntime | null | (() => Promise<ComposeRuntime | null>);
	/** Where compose runs: one subdirectory per stack (userData/stacks when packaged). */
	stacksDir: string;
	/**
	 * The shipped stacks (resources/stacks, or deploy/ in dev). When set, each stack is
	 * copied to `stacksDir` before its first command (see syncStackDir).
	 */
	sourceDir?: string;
	/** The app version; a new version refreshes the copies. */
	version?: string;
	/** Re-copy on every launch (dev: the sources change without a version bump). */
	alwaysCopy?: boolean;
	/**
	 * Service-level readiness once every container runs (tikv: PD health and a store Up;
	 * postgres: the pageserver answers and its timelines are Postgres 17). A problem
	 * makes the stack an error with its code.
	 */
	ready?: (id: StackId) => Promise<Readiness>;
	logsDir: string;
	run?: RunFn;
	timeoutMs?: number;
}

export class StackManager extends EventEmitter {
	private readonly cur = new Map<StackId, StackState>();
	private readonly busy = new Set<StackId>();
	/** Stacks whose last start/stop failed; the error stays until a ps shows them up or the next command. */
	private readonly failed = new Set<StackId>();
	/** One lock per start/stop: shared by postgres and wesql (same host port), per id for tikv. */
	private readonly locks = new Set<string>();
	private readonly logs = new Map<StackId, RotatingLog>();
	private readonly run: RunFn;
	private rt: Promise<ComposeRuntime | null> | undefined;
	private readonly synced = new Set<StackId>();

	constructor(private readonly deps: StackManagerDeps) {
		super();
		this.run = deps.run ?? runCommand;
	}

	private runtime(): Promise<ComposeRuntime | null> {
		this.rt ??= Promise.resolve(
			typeof this.deps.runtime === "function"
				? this.deps.runtime()
				: this.deps.runtime,
		).catch(() => null);
		return this.rt;
	}

	private log(id: StackId): RotatingLog {
		let l = this.logs.get(id);
		if (!l) {
			l = new RotatingLog(join(this.deps.logsDir, "stacks", `${id}.log`));
			this.logs.set(id, l);
		}
		return l;
	}

	private set(id: StackId, s: StackState): void {
		this.emit("observed", id, s);
		if (JSON.stringify(this.cur.get(id)) === JSON.stringify(s)) return;
		this.cur.set(id, s);
		this.emit("state", id, s);
	}

	/** Copies the stack out of the app resources once per launch (no-op without sourceDir). */
	private prepare(id: StackId): void {
		const src = this.deps.sourceDir;
		if (!src || this.synced.has(id)) return;
		syncStackDir(
			stackSource(src, id),
			join(this.deps.stacksDir, STACKS[id].dir),
			this.deps.version ?? "0",
			this.deps.alwaysCopy === true,
		);
		this.synced.add(id);
	}

	private exec(
		rt: ComposeRuntime,
		id: StackId,
		action: string[],
		quiet = false,
	): Promise<RunResult> {
		const log = this.log(id);
		try {
			this.prepare(id);
		} catch (e) {
			const msg = `cannot copy the ${id} stack: ${(e as Error).message}\n`;
			log.write(msg);
			return Promise.resolve({ code: -1, stdout: "" });
		}
		return this.run(rt.bin, composeArgs(rt, id, this.deps.stacksDir, action), {
			timeoutMs: this.deps.timeoutMs ?? COMMAND_TIMEOUT_MS,
			log: (c) => log.write(c),
			quiet,
		});
	}

	/** Queries the runtime for the current state (on demand and from the poller). */
	async state(id: StackId): Promise<StackState> {
		const rt = await this.runtime();
		if (!rt) {
			const s: StackState = {
				phase: "unavailable",
				reason: "no_container_runtime",
			};
			this.set(id, s);
			return s;
		}
		if (this.busy.has(id)) return this.cur.get(id) ?? { phase: "starting" };
		const r = await this.exec(rt, id, ["ps", "--format", "json"], true);
		if (this.busy.has(id)) return this.cur.get(id) ?? { phase: "starting" };
		let s: StackState;
		if (r.code !== 0)
			s = {
				phase: "error",
				message: `${rt.bin} compose ps failed (exit ${r.code}); see stacks/${id}.log`,
			};
		else {
			try {
				s = stateFromRows(id, parsePs(r.stdout));
			} catch (e) {
				s = {
					phase: "error",
					message: `unreadable ps output: ${(e as Error).message}`,
				};
			}
			// Running containers are reported as starting until the service answers,
			// and as an error when it answers that it cannot be used.
			if (s.phase === "running" && this.deps.ready) {
				const r = await this.deps.ready(id).catch(() => false);
				if (r === false) s = { phase: "starting" };
				else if (r !== true)
					s = { phase: "error", code: r.code, message: r.message };
			}
		}
		if (this.failed.has(id) && s.phase === "stopped") s = this.cur.get(id) ?? s;
		else this.failed.delete(id);
		this.set(id, s);
		return s;
	}

	async start(id: StackId): Promise<IpcResult<void>> {
		const rt = await this.runtime();
		if (!rt) return unavailable();
		const key = lockKey(id);
		if (this.locks.has(key))
			return { ok: false, code: "busy", message: `${id} is already changing` };
		this.locks.add(key);
		this.busy.add(id);
		try {
			if (SHARED_PORT_GROUP.includes(id)) {
				for (const other of SHARED_PORT_GROUP) {
					if (other === id) continue;
					const o = await this.state(other);
					if (o.phase === "running" || o.phase === "starting")
						return {
							ok: false,
							code: "port_conflict",
							message: `the ${other} stack already uses host port ${STACKS[id].ports.rustfs}; stop it first`,
						};
				}
			}
			return await this.doStart(rt, id);
		} finally {
			this.busy.delete(id);
			this.locks.delete(key);
		}
	}

	private async doStart(
		rt: ComposeRuntime,
		id: StackId,
	): Promise<IpcResult<void>> {
		this.failed.delete(id);
		this.set(id, { phase: "starting" });
		const r = await this.exec(rt, id, ["up", "-d"]);
		if (r.code !== 0) {
			const message = `compose up failed (exit ${r.code}); see stacks/${id}.log`;
			this.failed.add(id);
			this.set(id, { phase: "error", message });
			return { ok: false, code: "compose_failed", message };
		}
		this.busy.delete(id);
		await this.state(id);
		return { ok: true, value: undefined };
	}

	async stop(id: StackId): Promise<IpcResult<void>> {
		const rt = await this.runtime();
		if (!rt) return unavailable();
		const key = lockKey(id);
		if (this.locks.has(key))
			return { ok: false, code: "busy", message: `${id} is already changing` };
		this.locks.add(key);
		this.busy.add(id);
		this.failed.delete(id);
		try {
			const r = await this.exec(rt, id, ["down"]);
			if (r.code !== 0) {
				const message = `compose down failed (exit ${r.code}); see stacks/${id}.log`;
				this.failed.add(id);
				this.set(id, { phase: "error", message });
				return { ok: false, code: "compose_failed", message };
			}
			this.busy.delete(id);
		} finally {
			this.busy.delete(id);
			this.locks.delete(key);
		}
		await this.state(id);
		return { ok: true, value: undefined };
	}

	/**
	 * Stops the stack and deletes its named volumes (`compose down -v`): every local
	 * database, branch and timeline of that stack. Only the stack's own compose project
	 * (`loams-desktop-<id>`) is touched. The caller confirms first ({@link confirmReset}).
	 */
	async reset(id: StackId): Promise<IpcResult<void>> {
		const rt = await this.runtime();
		if (!rt) return unavailable();
		const key = lockKey(id);
		if (this.locks.has(key))
			return { ok: false, code: "busy", message: `${id} is already changing` };
		this.locks.add(key);
		this.busy.add(id);
		this.failed.delete(id);
		try {
			const r = await this.exec(rt, id, ["down", "-v"]);
			if (r.code !== 0) {
				const message = `compose down -v failed (exit ${r.code}); see stacks/${id}.log`;
				this.failed.add(id);
				this.set(id, { phase: "error", message });
				return { ok: false, code: "compose_failed", message };
			}
		} finally {
			this.busy.delete(id);
			this.locks.delete(key);
		}
		await this.state(id);
		return { ok: true, value: undefined };
	}

	/** Refreshes every stack once. */
	async poll(): Promise<void> {
		await Promise.all(STACK_IDS.map((id) => this.state(id)));
	}
}

function lockKey(id: StackId): string {
	return SHARED_PORT_GROUP.includes(id) ? "rustfs-9000" : id;
}

function unavailable(): IpcResult<void> {
	return {
		ok: false,
		code: "unavailable",
		message: "no_container_runtime",
	};
}

/**
 * Resets a stack only after `confirm` says yes (the IPC layer asks with a native
 * dialog); a declined reset runs nothing and answers `cancelled`.
 */
export async function confirmReset(
	manager: Pick<StackManager, "reset">,
	id: StackId,
	confirm: (id: StackId) => Promise<boolean>,
): Promise<IpcResult<void>> {
	if (!(await confirm(id)))
		return { ok: false, code: "cancelled", message: "reset cancelled" };
	return manager.reset(id);
}

/** Consecutive polls that must see tikv stopped before the engine drops TiKV. */
export const STOPPED_POLLS = 2;

/** What the Live page and Settings show when the TiKV stack was chosen but is down. */
export const LIVE_TIKV_UNAVAILABLE =
	"Live on TiKV is unavailable; showing local data.";

/**
 * Live's store follows the user's choice, never the stack alone (ruling T23-8): with `tikv-stack`
 * chosen, the engine runs Live on the TiKV stack while it is up, and on its embedded store, with
 * {@link LIVE_TIKV_UNAVAILABLE} as the notice, once the stack has been seen stopped on two
 * consecutive observations (a single missed `ps` must not restart the engine). With `embedded`
 * chosen, a running stack changes nothing. `refresh` re-applies after the choice changes.
 */
export function bindLiveToTikv(
	manager: EventEmitter,
	engine: {
		setLivePd(pd: string | null): Promise<void>;
		setLiveNotice?(notice: string | null): void;
	},
	choice: () => LiveStoreChoice,
	onError: (e: unknown) => void = () => {},
): { refresh(): void } {
	let stopped = 0;
	let running = false;
	const apply = (): void => {
		const tikv = choice() === "tikv-stack";
		engine.setLivePd(tikv && running ? TIKV_PD_ADDR : null).catch(onError);
		engine.setLiveNotice?.(tikv && !running ? LIVE_TIKV_UNAVAILABLE : null);
	};
	manager.on("observed", (id: StackId, s: StackState) => {
		if (id !== "tikv") return;
		if (s.phase === "stopped") {
			if (++stopped >= STOPPED_POLLS) {
				running = false;
				apply();
			}
			return;
		}
		stopped = 0;
		if (s.phase === "running") {
			running = true;
			apply();
		}
	});
	return { refresh: apply };
}
