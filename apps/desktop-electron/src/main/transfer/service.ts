// Transfers in the desktop (design §85 §12, DQ1 Task 6): the commands the
// pages ask for, turned into sidecar commands with the server's address,
// trust and token, and the sidecar's events folded into the model. No
// electron import: ipc.electron.ts wires dialogs, IPC and the vault.
import { EventEmitter } from "node:events";
import { basename } from "node:path";
import type {
	IpcResult,
	ServerEntry,
	TransferJob,
	TransferRemoteList,
	TransferServerSettings,
	TransferSnapshot,
	TransferTarget,
} from "../../shared/contracts";
import { type AgentEvent, TransfersModel } from "./model";
import type { TransferSupervisor } from "./supervisor";

/** Where per-server settings and tokens live (the encrypted vault). */
export interface TransferKeys {
	get(serverId: string): { token: string; settings: TransferServerSettings } | null;
	set(serverId: string, token: string, settings: TransferServerSettings): void;
	persistent: boolean;
}

export interface TransferServiceDeps {
	supervisor: Pick<TransferSupervisor, "send" | "on" | "phase">;
	servers: () => ServerEntry[];
	keys: TransferKeys;
	newId: () => string;
}

const err = <T>(code: string, message: string): IpcResult<T> => ({
	ok: false,
	code,
	message,
});

/** Emits `change` with a TransferSnapshot whenever the view changes. */
export class TransferService extends EventEmitter {
	readonly model = new TransfersModel();
	private replies = new Map<string, (v: Record<string, unknown>) => void>();

	constructor(private readonly deps: TransferServiceDeps) {
		super();
		deps.supervisor.on("event", (e: AgentEvent) => this.onEvent(e));
		deps.supervisor.on("phase", (p) => {
			this.model.setAgent(p);
			this.changed();
		});
		this.model.setAgent(deps.supervisor.phase());
	}

	snapshot(): TransferSnapshot {
		return this.model.snapshot();
	}

	private changed(): void {
		this.emit("change", this.model.snapshot());
	}

	private onEvent(e: AgentEvent): void {
		if (e.event === "reply") {
			const cb = this.replies.get(e.id);
			if (cb) {
				this.replies.delete(e.id);
				cb(e);
			}
			return;
		}
		if (this.model.apply(e)) this.changed();
	}

	/** The sidecar's view of a server: address, trust, token. */
	private connect(
		target: TransferTarget,
	): IpcResult<{ server: Record<string, unknown>; ns: string; token: string; entry: ServerEntry }> {
		const entry = this.deps.servers().find((s) => s.id === target.serverId);
		if (!entry) return err("not_found", "That server is no longer configured.");
		if (entry.kind === "demo")
			return err("unsupported", "The demo server has no transfers.");
		if (!/^[a-z0-9][a-z0-9_-]{0,62}$/.test(target.ns))
			return err("invalid", "Choose a namespace (lowercase letters, digits, - and _).");
		const k = this.deps.keys.get(entry.id);
		if (!k) return err("no_token", "Add an API key for this server first.");
		const s = k.settings;
		const server: Record<string, unknown> = {
			base: entry.url,
			mode: s.mode ?? "auto",
		};
		if (s.pin) server.pin = s.pin;
		if (s.caFile) server.ca_file = s.caFile;
		return { ok: true, value: { server, ns: target.ns, token: k.token, entry } };
	}

	hasToken(serverId: string): boolean {
		return this.deps.keys.get(serverId) !== null;
	}

	setToken(
		serverId: string,
		token: string,
		settings: TransferServerSettings,
	): IpcResult<void> {
		if (!this.deps.servers().some((s) => s.id === serverId))
			return err("not_found", "That server is no longer configured.");
		if (token.trim().length < 8) return err("invalid", "The API key looks too short.");
		if (settings.pin && !/^[0-9a-f]{64}$/i.test(settings.pin))
			return err("invalid", "A certificate pin is a SHA-256 in hex (64 characters).");
		if (!this.deps.keys.persistent)
			return err(
				"no_keychain",
				"This system has no keychain, so the key cannot be stored safely.",
			);
		this.deps.keys.set(serverId, token.trim(), {
			...settings,
			pin: settings.pin?.toLowerCase(),
		});
		return { ok: true, value: undefined };
	}

	async upload(target: TransferTarget, path: string): Promise<IpcResult<TransferJob>> {
		const c = this.connect(target);
		if (!c.ok) return c;
		const job: TransferJob = {
			id: this.deps.newId(),
			kind: "upload",
			name: basename(path),
			path,
			server: c.value.entry.url,
			serverId: c.value.entry.id,
			ns: target.ns,
			state: "queued",
			resumed: false,
			progress: null,
			error: null,
			log: [],
		};
		this.model.add(job);
		this.changed();
		await this.deps.supervisor.send({
			op: "upload",
			id: job.id,
			server: c.value.server,
			ns: c.value.ns,
			token: c.value.token,
			path,
		});
		return { ok: true, value: job };
	}

	async download(
		target: TransferTarget,
		tid: string,
		name: string,
		dest: string,
	): Promise<IpcResult<TransferJob>> {
		const c = this.connect(target);
		if (!c.ok) return c;
		const job: TransferJob = {
			id: this.deps.newId(),
			kind: "download",
			tid,
			name,
			dest,
			server: c.value.entry.url,
			serverId: c.value.entry.id,
			ns: target.ns,
			state: "queued",
			resumed: false,
			progress: null,
			error: null,
			log: [],
		};
		this.model.add(job);
		this.changed();
		await this.deps.supervisor.send({
			op: "download",
			id: job.id,
			server: c.value.server,
			ns: c.value.ns,
			token: c.value.token,
			tid,
			dest,
		});
		return { ok: true, value: job };
	}

	async pause(id: string): Promise<IpcResult<void>> {
		const j = this.model.get(id);
		if (!j) return err("not_found", "No such transfer.");
		await this.deps.supervisor.send({ op: "pause", id: this.deps.newId(), job: id });
		return { ok: true, value: undefined };
	}

	/** Resumes a paused or failed transfer: the same command again resumes it. */
	async resume(id: string): Promise<IpcResult<TransferJob>> {
		const j = this.model.get(id);
		if (!j) return err("not_found", "No such transfer.");
		const serverId =
			j.serverId ?? this.deps.servers().find((s) => s.url === j.server)?.id;
		if (!serverId) return err("not_found", "That server is no longer configured.");
		const target = { serverId, ns: j.ns };
		this.model.remove(id);
		if (j.kind === "upload" && j.path) return this.upload(target, j.path);
		if (j.kind === "download" && j.tid && j.dest)
			return this.download(target, j.tid, j.name, j.dest);
		return err("unsupported", "This item cannot be resumed.");
	}

	async cancel(id: string): Promise<IpcResult<void>> {
		const j = this.model.get(id);
		if (!j) return err("not_found", "No such transfer.");
		const cmd: Record<string, unknown> = { op: "cancel", id: this.deps.newId(), job: id };
		// An upload's server side is abandoned too (its bytes go after the
		// retention window, Q1513).
		const serverId =
			j.serverId ?? this.deps.servers().find((s) => s.url === j.server)?.id;
		if (j.kind === "upload" && j.tid && serverId) {
			const c = this.connect({ serverId, ns: j.ns });
			if (c.ok) Object.assign(cmd, { server: c.value.server, ns: j.ns, token: c.value.token, tid: j.tid });
		}
		await this.deps.supervisor.send(cmd);
		this.model.remove(id);
		this.changed();
		return { ok: true, value: undefined };
	}

	/** Removes a finished item from the list. */
	dismiss(id: string): void {
		const j = this.model.get(id);
		if (j && (j.state === "done" || j.state === "failed" || j.state === "cancelled")) {
			this.model.remove(id);
			this.changed();
		}
	}

	private request(cmd: Record<string, unknown>, timeoutMs = 30_000): Promise<Record<string, unknown>> {
		const id = this.deps.newId();
		return new Promise((resolve, reject) => {
			const t = setTimeout(() => {
				this.replies.delete(id);
				reject(new Error("the transfer sidecar did not answer"));
			}, timeoutMs);
			this.replies.set(id, (v) => {
				clearTimeout(t);
				resolve(v);
			});
			this.deps.supervisor.send({ ...cmd, id }).catch((e: unknown) => {
				clearTimeout(t);
				this.replies.delete(id);
				reject(e instanceof Error ? e : new Error(String(e)));
			});
		});
	}

	async remoteList(target: TransferTarget): Promise<IpcResult<TransferRemoteList>> {
		const c = this.connect(target);
		if (!c.ok) return c;
		try {
			const r = await this.request({
				op: "remote.list",
				server: c.value.server,
				ns: c.value.ns,
				token: c.value.token,
			});
			if (typeof r.error === "string") return err(String(r.code ?? "error"), r.error);
			return {
				ok: true,
				value: {
					transfers: (r.transfers as TransferRemoteList["transfers"]) ?? [],
					runs: (r.runs as TransferRemoteList["runs"]) ?? [],
				},
			};
		} catch (e) {
			return err("sidecar", e instanceof Error ? e.message : String(e));
		}
	}

	/** Queues a run of `job` and follows its log. */
	async startRun(
		target: TransferTarget,
		job: string,
		params: Record<string, string>,
	): Promise<IpcResult<TransferJob>> {
		const c = this.connect(target);
		if (!c.ok) return c;
		let r: Record<string, unknown>;
		try {
			r = await this.request({
				op: "run.start",
				server: c.value.server,
				ns: c.value.ns,
				token: c.value.token,
				job,
				params,
			});
		} catch (e) {
			return err("sidecar", e instanceof Error ? e.message : String(e));
		}
		if (typeof r.error === "string") return err(String(r.code ?? "error"), r.error);
		const rid = (r.run as { rid?: string } | undefined)?.rid;
		if (!rid) return err("error", "The server did not return a run.");
		return this.followRun(target, rid, job);
	}

	/** Follows a run's log; reconnecting resumes from the last byte received. */
	async followRun(
		target: TransferTarget,
		rid: string,
		job: string,
	): Promise<IpcResult<TransferJob>> {
		const c = this.connect(target);
		if (!c.ok) return c;
		const item: TransferJob = {
			id: this.deps.newId(),
			kind: "run",
			rid,
			name: job,
			server: c.value.entry.url,
			serverId: c.value.entry.id,
			ns: target.ns,
			state: "running",
			resumed: false,
			progress: null,
			error: null,
			log: [],
		};
		this.model.add(item);
		this.changed();
		await this.deps.supervisor.send({
			op: "run.tail",
			id: item.id,
			server: c.value.server,
			ns: c.value.ns,
			token: c.value.token,
			rid,
		});
		return { ok: true, value: item };
	}
}
