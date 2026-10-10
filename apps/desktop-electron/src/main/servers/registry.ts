import { randomUUID } from "node:crypto";
import {
	existsSync,
	mkdirSync,
	readFileSync,
	renameSync,
	writeFileSync,
} from "node:fs";
import { dirname } from "node:path";
import type { IpcResult, ServerEntry } from "../../shared/contracts";

export const LOCAL_ID = "local";
export const DEMO_URL = "http://127.0.0.1:8084";

const LOOPBACK = new Set(["localhost", "127.0.0.1", "[::1]"]);

interface FileV1 {
	v: 1;
	servers: ServerEntry[];
	activeId: string;
}

const fail = <T>(code: string, message: string): IpcResult<T> => ({
	ok: false,
	code,
	message,
});

export class ServerRegistry {
	private remotes: ServerEntry[] = [];
	private activeId = LOCAL_ID;
	private localUrl = "";
	private readonly demo: ServerEntry | null;

	constructor(
		private readonly file: string,
		opts: { devDemo: boolean },
	) {
		this.demo = opts.devDemo
			? { id: "demo", name: "Demo", kind: "demo", url: DEMO_URL }
			: null;
		this.load();
	}

	private builtins(): ServerEntry[] {
		const local: ServerEntry = {
			id: LOCAL_ID,
			name: "Local engine",
			kind: "local",
			url: this.localUrl,
		};
		return this.demo ? [local, this.demo] : [local];
	}

	list(): { servers: ServerEntry[]; activeId: string } {
		return {
			servers: [...this.builtins(), ...this.remotes].map((s) => ({ ...s })),
			activeId: this.activeId,
		};
	}

	active(): ServerEntry {
		const all = this.list().servers;
		return all.find((s) => s.id === this.activeId) ?? (all[0] as ServerEntry);
	}

	/** Called by the engine supervisor: url when ready, '' otherwise. Not persisted. */
	setLocalUrl(url: string): void {
		this.localUrl = url;
	}

	add(e: Omit<ServerEntry, "id">): IpcResult<ServerEntry> {
		let u: URL;
		try {
			u = new URL(e.url);
		} catch {
			return fail("invalid_url", "not a valid URL");
		}
		if (u.protocol !== "http:" && u.protocol !== "https:")
			return fail("invalid_url", "only http and https URLs are allowed");
		if (u.protocol === "http:" && !LOOPBACK.has(u.hostname))
			return fail("insecure_url", "plain http is only allowed for loopback");
		const entry: ServerEntry = {
			id: randomUUID(),
			name: e.name.trim() || u.host,
			kind: "remote",
			url: u.origin,
		};
		this.remotes.push(entry);
		this.persist();
		return { ok: true, value: { ...entry } };
	}

	remove(id: string): IpcResult<void> {
		if (id === LOCAL_ID)
			return fail("not_removable", "the local engine cannot be removed");
		const i = this.remotes.findIndex((s) => s.id === id);
		if (i < 0) return fail("not_found", "no such server");
		this.remotes.splice(i, 1);
		if (this.activeId === id) this.activeId = LOCAL_ID;
		this.persist();
		return { ok: true, value: undefined };
	}

	activate(id: string): IpcResult<void> {
		if (!this.list().servers.some((s) => s.id === id))
			return fail("not_found", "no such server");
		this.activeId = id;
		this.persist();
		return { ok: true, value: undefined };
	}

	private load(): void {
		if (!existsSync(this.file)) return;
		try {
			const raw = JSON.parse(
				readFileSync(this.file, "utf8"),
			) as Partial<FileV1>;
			if (raw.v !== 1 || !Array.isArray(raw.servers)) throw new Error("shape");
			this.remotes = raw.servers.filter(
				(s) =>
					s &&
					s.kind === "remote" &&
					typeof s.id === "string" &&
					typeof s.url === "string" &&
					/^https?:\/\//.test(s.url),
			);
			this.activeId =
				typeof raw.activeId === "string" ? raw.activeId : LOCAL_ID;
			if (!this.list().servers.some((s) => s.id === this.activeId))
				this.activeId = LOCAL_ID;
		} catch {
			try {
				renameSync(this.file, `${this.file}.corrupt-${Date.now()}`);
			} catch {
				/* best effort */
			}
			this.remotes = [];
			this.activeId = LOCAL_ID;
		}
	}

	private persist(): void {
		const data: FileV1 = {
			v: 1,
			servers: this.remotes,
			activeId: this.activeId,
		};
		mkdirSync(dirname(this.file), { recursive: true });
		const tmp = `${this.file}.tmp`;
		writeFileSync(tmp, JSON.stringify(data, null, 2), "utf8");
		renameSync(tmp, this.file);
	}
}
