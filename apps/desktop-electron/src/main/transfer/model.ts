// The Transfers model (design §85 §12, DQ1 Task 6): the sidecar's JSON-lines
// events folded into one view of every job. No electron import, so it is
// unit-testable; supervisor.ts feeds it and ipc.electron.ts serves it.
import type {
	TransferAgentState,
	TransferJob,
	TransferProgress,
	TransferSnapshot,
} from "../../shared/contracts";

/** Log lines kept per run view; the whole log is the sidecar's mirror file. */
export const LOG_LINES = 2000;

/** An upload an earlier session left unfinished (the sidecar's state). */
export interface UnfinishedUpload {
	tid: string;
	path: string;
	name: string;
	size: number;
	server: string;
	ns: string;
}

export type AgentEvent =
	| {
			event: "ready";
			version: string;
			install_id: string;
			unfinished_uploads: UnfinishedUpload[];
	  }
	| { event: "progress"; job: string; progress: TransferProgress }
	| {
			event: "state";
			job: string;
			state: "started" | "paused" | "done" | "failed" | "cancelled";
			resumed?: boolean;
			code?: string;
			error?: string;
			transfer?: { tid: string };
	  }
	| {
			event: "log";
			job: string;
			rid: string;
			offset: number;
			len?: number;
			text: string;
	  }
	| { event: "reply"; id: string; [k: string]: unknown }
	| { event: "error"; error: string };

const isObj = (v: unknown): v is Record<string, unknown> =>
	typeof v === "object" && v !== null && !Array.isArray(v);

/** Parses one stdout line; anything that is not a known event is dropped. */
export function parseEvent(line: string): AgentEvent | null {
	let v: unknown;
	try {
		v = JSON.parse(line);
	} catch {
		return null;
	}
	if (!isObj(v) || typeof v.event !== "string") return null;
	const ok = (() => {
		switch (v.event) {
			case "ready":
				return typeof v.version === "string" && Array.isArray(v.unfinished_uploads);
			case "progress":
				return typeof v.job === "string" && isObj(v.progress);
			case "state":
				return typeof v.job === "string" && typeof v.state === "string";
			case "log":
				return (
					typeof v.job === "string" &&
					typeof v.text === "string" &&
					typeof v.offset === "number"
				);
			case "reply":
				return typeof v.id === "string";
			case "error":
				return typeof v.error === "string";
			default:
				return false;
		}
	})();
	return ok ? (v as AgentEvent) : null;
}

/** Folds events into jobs. */
export class TransfersModel {
	private jobs = new Map<string, TransferJob>();
	private agent: TransferAgentState = { phase: "stopped" };

	snapshot(): TransferSnapshot {
		return {
			agent: { ...this.agent },
			jobs: [...this.jobs.values()].map((j) => ({ ...j, log: [...j.log] })),
		};
	}

	setAgent(a: TransferAgentState): void {
		this.agent = a;
	}

	/** A job the user just asked for. */
	add(job: TransferJob): void {
		this.jobs.set(job.id, job);
	}

	get(id: string): TransferJob | undefined {
		return this.jobs.get(id);
	}

	remove(id: string): void {
		this.jobs.delete(id);
	}

	/** Applies one event; returns whether the view changed. */
	apply(e: AgentEvent): boolean {
		switch (e.event) {
			case "ready": {
				this.agent = { phase: "ready", version: e.version };
				// Uploads an earlier session left show as paused and resumable.
				for (const u of e.unfinished_uploads) {
					if ([...this.jobs.values()].some((j) => j.tid === u.tid)) continue;
					const id = `resume-${u.tid}`;
					this.jobs.set(id, {
						id,
						kind: "upload",
						tid: u.tid,
						name: u.name,
						path: u.path,
						server: u.server,
						ns: u.ns,
						state: "paused",
						resumed: true,
						progress: null,
						error: null,
						log: [],
					});
				}
				return true;
			}
			case "progress": {
				const j = this.jobs.get(e.job);
				if (!j) return false;
				j.progress = e.progress;
				if (e.progress.tid) j.tid = e.progress.tid;
				if (j.state === "queued") j.state = "running";
				return true;
			}
			case "state": {
				const j = this.jobs.get(e.job);
				if (!j) return false;
				j.state = e.state === "started" ? "running" : e.state;
				if (e.resumed !== undefined) j.resumed = e.resumed;
				if (e.transfer?.tid) j.tid = e.transfer.tid;
				j.error =
					e.state === "failed"
						? { code: e.code ?? "error", message: e.error ?? "" }
						: null;
				return true;
			}
			case "log": {
				const j = this.jobs.get(e.job);
				if (!j) return false;
				const lines = e.text.split("\n");
				// The first piece continues the last line held.
				if (j.log.length > 0) {
					j.log[j.log.length - 1] += lines.shift() ?? "";
				}
				j.log.push(...lines);
				if (j.log.length > LOG_LINES) j.log.splice(0, j.log.length - LOG_LINES);
				j.logOffset = e.offset + (e.len ?? new TextEncoder().encode(e.text).length);
				return true;
			}
			default:
				return false;
		}
	}
}
