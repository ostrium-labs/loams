// The transfer sidecar's supervisor (design §85 §12, D2420): starts
// `loams-transfer agent --state <userData>/transfers` on first use, writes
// JSON-lines commands to its stdin, parses its events, and restarts it with
// backoff like the engine supervisor. No electron import.
import type { ChildProcess, spawn as spawnFn } from "node:child_process";
import { EventEmitter } from "node:events";
import { createInterface } from "node:readline";
import type { TransferAgentState } from "../../shared/contracts";
import { parseEvent } from "./model";

export interface TransferSupervisorDeps {
	spawn: typeof spawnFn;
	/** The sidecar binary, or null when this build does not bundle it. */
	binary: () => string | null;
	stateDir: string;
	sleep: (ms: number) => Promise<void>;
	now: () => number;
	env?: NodeJS.ProcessEnv;
}

const BACKOFF_S = [1, 2, 4, 8, 16, 30];
const MAX_RESTARTS = 5;
const WINDOW_MS = 10 * 60 * 1000;
export const NOT_BUNDLED =
	"The transfer sidecar (loams-transfer) is not bundled with this build.";

/**
 * Emits `event` (an AgentEvent) for each parsed line and `phase` (a
 * TransferAgentState). Commands carry tokens, so nothing here logs them.
 */
export class TransferSupervisor extends EventEmitter {
	private child: ChildProcess | null = null;
	private ready: Promise<void> | null = null;
	private restarts: number[] = [];
	private stopping = false;
	private cur: TransferAgentState = { phase: "stopped" };

	constructor(private readonly deps: TransferSupervisorDeps) {
		super();
	}

	phase(): TransferAgentState {
		return this.cur;
	}

	private setPhase(p: TransferAgentState): void {
		this.cur = p;
		this.emit("phase", p);
	}

	/** Starts the sidecar if it is not running; resolves once it is ready. */
	ensure(): Promise<void> {
		if (this.ready) return this.ready;
		this.stopping = false;
		const p = this.start();
		this.ready = p;
		p.catch(() => {
			if (this.ready === p) this.ready = null;
		});
		return p;
	}

	private start(): Promise<void> {
		const bin = this.deps.binary();
		if (!bin) {
			this.setPhase({ phase: "failed", reason: NOT_BUNDLED });
			return Promise.reject(new Error(NOT_BUNDLED));
		}
		this.setPhase({ phase: "starting" });
		const child = this.deps.spawn(
			bin,
			["agent", "--state", this.deps.stateDir],
			{
				stdio: ["pipe", "pipe", "pipe"],
				env: this.deps.env ?? process.env,
				windowsHide: true,
			},
		);
		this.child = child;
		return new Promise<void>((resolve, reject) => {
			let settled = false;
			let tail = "";
			if (child.stdout) {
				createInterface({ input: child.stdout }).on("line", (line) => {
					const e = parseEvent(line);
					if (!e) return;
					if (e.event === "ready" && !settled) {
						settled = true;
						this.setPhase({ phase: "ready", version: e.version });
						resolve();
					}
					this.emit("event", e);
				});
			}
			// The sidecar logs to stderr; keep a tail for the failure message.
			child.stderr?.on("data", (b: Buffer) => {
				tail = (tail + b.toString("utf8")).slice(-2000);
			});
			child.on("error", (err) => {
				if (!settled) {
					settled = true;
					reject(err);
				}
			});
			child.on("exit", (code, signal) => {
				if (this.child === child) this.child = null;
				this.ready = null;
				const last = tail.trim().split("\n").pop();
				const reason = `the transfer sidecar exited (${signal ?? code})${last ? `: ${last}` : ""}`;
				if (!settled) {
					settled = true;
					reject(new Error(reason));
				}
				if (this.stopping) {
					this.setPhase({ phase: "stopped" });
					return;
				}
				void this.restart(reason);
			});
		});
	}

	private async restart(reason: string): Promise<void> {
		const now = this.deps.now();
		this.restarts = this.restarts.filter((t) => now - t < WINDOW_MS);
		if (this.restarts.length >= MAX_RESTARTS) {
			this.setPhase({ phase: "failed", reason });
			return;
		}
		const wait =
			BACKOFF_S[Math.min(this.restarts.length, BACKOFF_S.length - 1)] ?? 30;
		this.restarts.push(now);
		this.setPhase({ phase: "starting" });
		await this.deps.sleep(wait * 1000);
		if (this.stopping) return;
		try {
			await this.ensure();
		} catch {
			// A failed start exits again, which restarts or ends in "failed".
		}
	}

	/** Sends one command, starting the sidecar first if needed. */
	async send(cmd: Record<string, unknown>): Promise<void> {
		await this.ensure();
		const stdin = this.child?.stdin;
		if (!stdin) throw new Error("the transfer sidecar is not running");
		stdin.write(`${JSON.stringify(cmd)}\n`);
	}

	/** Stops the sidecar; unfinished transfers resume on its next start. */
	stop(): void {
		this.stopping = true;
		const c = this.child;
		if (!c) return;
		try {
			c.stdin?.write(`${JSON.stringify({ op: "shutdown" })}\n`);
		} catch {
			// The pipe may already be closed.
		}
		const t = setTimeout(() => {
			if (c.exitCode === null && c.signalCode === null) c.kill("SIGTERM");
		}, 2000);
		t.unref?.();
	}
}
