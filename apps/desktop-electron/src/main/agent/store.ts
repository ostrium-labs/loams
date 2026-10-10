// Chats on disk (D675): one JSON file per chat, `<userData>/chats/<id>.json`,
// written atomically (temp file + rename). Never uploaded anywhere.
//
// Off the main thread's critical path: all I/O is async, the summary list is an
// in-memory index built once (lazily) from the directory, and writes for one chat
// are queued so they land in order without blocking the caller.
import { randomBytes } from "node:crypto";
import * as nodeFs from "node:fs/promises";
import { join } from "node:path";
import type { ChatRecord, ChatSummary } from "../../shared/contracts";

const ID = /^c_[a-z0-9]{8,40}$/;

export const isChatId = (v: unknown): v is string =>
	typeof v === "string" && ID.test(v);

export function newChatId(): string {
	return `c_${Date.now().toString(36)}${randomBytes(6).toString("hex")}`;
}

export const chatSummary = (c: ChatRecord): ChatSummary => ({
	id: c.id,
	title: c.title,
	createdAt: c.createdAt,
	updatedAt: c.updatedAt,
	provider: c.provider,
	model: c.model,
});

/** The fs calls the store makes (injectable for tests). */
export interface StoreFs {
	readFile(path: string, enc: "utf8"): Promise<string>;
	readdir(path: string): Promise<string[]>;
	writeFile(path: string, data: string, opts: { mode: number }): Promise<void>;
	rename(from: string, to: string): Promise<void>;
	rm(path: string, opts: { force: boolean }): Promise<void>;
	mkdir(path: string, opts: { recursive: boolean }): Promise<unknown>;
}

export class ChatStore {
	#index: Map<string, ChatSummary> | undefined;
	#loading: Promise<Map<string, ChatSummary>> | undefined;
	readonly #queues = new Map<string, Promise<void>>();
	/** Summaries written by this process; they win over what a concurrent load read. */
	readonly #written = new Map<string, ChatSummary>();
	readonly #removed = new Set<string>();
	#tmpSeq = 0;

	constructor(
		private readonly dir: string,
		private readonly fs: StoreFs = nodeFs as unknown as StoreFs,
	) {}

	#file(id: string): string {
		if (!isChatId(id)) throw new Error("bad chat id");
		return join(this.dir, `${id}.json`);
	}

	async #read(id: string): Promise<ChatRecord | undefined> {
		try {
			const c = JSON.parse(
				await this.fs.readFile(this.#file(id), "utf8"),
			) as ChatRecord;
			return c && c.id === id && Array.isArray(c.messages) ? c : undefined;
		} catch {
			return undefined;
		}
	}

	/** Builds the summary index once; later calls reuse it. */
	#load(): Promise<Map<string, ChatSummary>> {
		if (this.#index) return Promise.resolve(this.#index);
		this.#loading ??= (async () => {
			const index = new Map<string, ChatSummary>();
			let names: string[] = [];
			try {
				names = await this.fs.readdir(this.dir);
			} catch {
				// no chats yet
			}
			const ids = names
				.filter((n) => n.endsWith(".json"))
				.map((n) => n.slice(0, -5))
				.filter(isChatId);
			const chats = await Promise.all(ids.map((id) => this.#read(id)));
			for (const c of chats) if (c) index.set(c.id, chatSummary(c));
			for (const [id, s] of this.#written) index.set(id, s);
			for (const id of this.#removed) index.delete(id);
			this.#index = index;
			return index;
		})();
		return this.#loading;
	}

	get(id: string): Promise<ChatRecord | undefined> {
		if (!isChatId(id)) return Promise.resolve(undefined);
		// A queued write is the newest version: read after it lands.
		const pending = this.#queues.get(id);
		return pending ? pending.then(() => this.#read(id)) : this.#read(id);
	}

	/**
	 * Serialises `chat` now (later mutations do not leak into this write) and writes it
	 * in the background, in order with this chat's other writes. The returned promise
	 * settles when the file is in place.
	 */
	save(chat: ChatRecord): Promise<void> {
		const file = this.#file(chat.id);
		// A late write from a turn that was stopped must not bring a removed chat back.
		if (this.#removed.has(chat.id)) return Promise.resolve();
		const data = JSON.stringify(chat);
		const summary = chatSummary(chat);
		this.#written.set(chat.id, summary);
		this.#index?.set(chat.id, summary);
		const prev = this.#queues.get(chat.id) ?? Promise.resolve();
		const next = prev
			.catch(() => undefined)
			.then(async () => {
				await this.fs.mkdir(this.dir, { recursive: true });
				const tmp = `${file}.${process.pid}.${++this.#tmpSeq}.tmp`;
				await this.fs.writeFile(tmp, data, { mode: 0o600 });
				await this.fs.rename(tmp, file);
			});
		this.#queues.set(chat.id, next);
		void next
			.finally(() => {
				if (this.#queues.get(chat.id) === next) this.#queues.delete(chat.id);
			})
			.catch(() => undefined);
		return next;
	}

	async remove(id: string): Promise<void> {
		const file = this.#file(id);
		this.#removed.add(id);
		await (this.#queues.get(id) ?? Promise.resolve()).catch(() => undefined);
		await this.fs.rm(file, { force: true });
		this.#written.delete(id);
		this.#index?.delete(id);
	}

	/** Newest first (by last update), from the in-memory index. */
	async list(): Promise<ChatSummary[]> {
		const index = await this.#load();
		return [...index.values()].sort((a, b) => b.updatedAt - a.updatedAt);
	}

	/** Resolves when every queued write has landed (quit). */
	async flush(): Promise<void> {
		await Promise.all(
			[...this.#queues.values()].map((p) => p.catch(() => undefined)),
		);
	}
}
