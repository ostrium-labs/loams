import { spawn } from "node:child_process";
import { EventEmitter } from "node:events";
import { appendFileSync, mkdirSync } from "node:fs";
import { join } from "node:path";
import type { IpcResult, StackId, StackState } from "../../shared/contracts";
import type { ComposeRuntime } from "./runtime";

export interface StackDef {
	/** Directory under the stacks dir (deploy/ in dev, resources/stacks when packaged). */
	dir: string;
	/** Compose services that must all be running for the stack to count as running. */
	required: string[];
	/** Host ports the compose file publishes (from each compose.yaml). */
	ports: Record<string, number>;
}

/** Ports and services read from deploy/{neon,wesql,tikv}/compose.yaml. */
export const STACKS: Record<StackId, StackDef> = {
	postgres: {
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
		dir: "wesql",
		required: ["rustfs", "wesql"],
		ports: { rustfs: 9000, wesql: 13306 },
	},
	tikv: {
		dir: "tikv",
		required: ["pd", "tikv"],
		ports: { pd: 19379, tikv: 20160 },
	},
};

export const STACK_IDS = Object.keys(STACKS) as StackId[];

/** PD client address of the tikv stack; the engine's `--live-pd` while it runs. */
export const TIKV_PD_ADDR = `127.0.0.1:${STACKS.tikv.ports.pd}`;

/** The postgres and wesql stacks both publish rustfs on this host port. */
const SHARED_PORT_GROUP: StackId[] = ["postgres", "wesql"];

export const COMMAND_TIMEOUT_MS = 5 * 60 * 1000;
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
export type RunFn = (
	bin: string,
	args: string[],
	opts: { timeoutMs: number; logFile: string },
) => Promise<RunResult>;

/** Spawns with an argument array (no shell); appends all output to the log file. */
export const runCommand: RunFn = (bin, args, { timeoutMs, logFile }) =>
	new Promise((resolve) => {
		const log = (s: string | Buffer) => {
			try {
				appendFileSync(logFile, s);
			} catch {
				/* logging is best effort */
			}
		};
		log(`\n$ ${bin} ${args.join(" ")}\n`);
		let stdout = "";
		let child: ReturnType<typeof spawn>;
		try {
			child = spawn(bin, args, {
				stdio: ["ignore", "pipe", "pipe"],
				windowsHide: true,
			});
		} catch (e) {
			log(`spawn error: ${(e as Error).message}\n`);
			resolve({ code: -1, stdout: "" });
			return;
		}
		const timer = setTimeout(() => {
			log(`timed out after ${timeoutMs} ms\n`);
			child.kill("SIGKILL");
		}, timeoutMs);
		child.stdout?.on("data", (c: Buffer) => {
			stdout += c.toString();
			log(c);
		});
		child.stderr?.on("data", (c: Buffer) => log(c));
		child.once("error", (e) => {
			log(`spawn error: ${e.message}\n`);
			clearTimeout(timer);
			resolve({ code: -1, stdout });
		});
		child.once("close", (code) => {
			clearTimeout(timer);
			resolve({ code: code ?? -1, stdout });
		});
	});

export interface StackManagerDeps {
	runtime: ComposeRuntime | null;
	stacksDir: string;
	logsDir: string;
	run?: RunFn;
	timeoutMs?: number;
}

export class StackManager extends EventEmitter {
	private readonly cur = new Map<StackId, StackState>();
	private readonly busy = new Set<StackId>();
	/** Stacks whose last start/stop failed; the error stays until a ps shows them up or the next command. */
	private readonly failed = new Set<StackId>();
	private readonly run: RunFn;

	constructor(private readonly deps: StackManagerDeps) {
		super();
		this.run = deps.run ?? runCommand;
	}

	private logFile(id: StackId): string {
		const dir = join(this.deps.logsDir, "stacks");
		try {
			mkdirSync(dir, { recursive: true });
		} catch {
			/* surfaced by the command failing to log; not fatal */
		}
		return join(dir, `${id}.log`);
	}

	private set(id: StackId, s: StackState): void {
		if (JSON.stringify(this.cur.get(id)) === JSON.stringify(s)) return;
		this.cur.set(id, s);
		this.emit("state", id, s);
	}

	private exec(id: StackId, action: string[]): Promise<RunResult> {
		const rt = this.deps.runtime as ComposeRuntime;
		return this.run(rt.bin, composeArgs(rt, id, this.deps.stacksDir, action), {
			timeoutMs: this.deps.timeoutMs ?? COMMAND_TIMEOUT_MS,
			logFile: this.logFile(id),
		});
	}

	/** Queries the runtime for the current state (on demand and from the poller). */
	async state(id: StackId): Promise<StackState> {
		if (!this.deps.runtime) {
			const s: StackState = {
				phase: "unavailable",
				reason: "no_container_runtime",
			};
			this.set(id, s);
			return s;
		}
		if (this.busy.has(id)) return this.cur.get(id) ?? { phase: "starting" };
		const r = await this.exec(id, ["ps", "--format", "json"]);
		if (this.busy.has(id)) return this.cur.get(id) ?? { phase: "starting" };
		let s: StackState;
		if (r.code !== 0)
			s = {
				phase: "error",
				message: `${this.deps.runtime.bin} compose ps failed (exit ${r.code}); see stacks/${id}.log`,
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
		}
		if (this.failed.has(id) && s.phase === "stopped") s = this.cur.get(id) ?? s;
		else this.failed.delete(id);
		this.set(id, s);
		return s;
	}

	async start(id: StackId): Promise<IpcResult<void>> {
		if (!this.deps.runtime) return unavailable();
		if (this.busy.has(id))
			return { ok: false, code: "busy", message: `${id} is already changing` };
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
		this.busy.add(id);
		this.failed.delete(id);
		this.set(id, { phase: "starting" });
		try {
			const r = await this.exec(id, ["up", "-d"]);
			if (r.code !== 0) {
				const message = `compose up failed (exit ${r.code}); see stacks/${id}.log`;
				this.failed.add(id);
				this.set(id, { phase: "error", message });
				return { ok: false, code: "compose_failed", message };
			}
		} finally {
			this.busy.delete(id);
		}
		await this.state(id);
		return { ok: true, value: undefined };
	}

	async stop(id: StackId): Promise<IpcResult<void>> {
		if (!this.deps.runtime) return unavailable();
		if (this.busy.has(id))
			return { ok: false, code: "busy", message: `${id} is already changing` };
		this.busy.add(id);
		this.failed.delete(id);
		try {
			const r = await this.exec(id, ["down"]);
			if (r.code !== 0) {
				const message = `compose down failed (exit ${r.code}); see stacks/${id}.log`;
				this.failed.add(id);
				this.set(id, { phase: "error", message });
				return { ok: false, code: "compose_failed", message };
			}
		} finally {
			this.busy.delete(id);
		}
		await this.state(id);
		return { ok: true, value: undefined };
	}

	/** Refreshes every stack once. */
	async poll(): Promise<void> {
		await Promise.all(STACK_IDS.map((id) => this.state(id)));
	}
}

function unavailable(): IpcResult<void> {
	return {
		ok: false,
		code: "unavailable",
		message: "no_container_runtime",
	};
}

/** The engine runs with Live while the tikv stack is up, and without it once it is down. */
export function bindLiveToTikv(
	manager: EventEmitter,
	engine: { setLivePd(pd: string | null): Promise<void> },
): void {
	manager.on("state", (id: StackId, s: StackState) => {
		if (id !== "tikv") return;
		if (s.phase === "running") void engine.setLivePd(TIKV_PD_ADDR);
		else if (s.phase === "stopped") void engine.setLivePd(null);
	});
}
