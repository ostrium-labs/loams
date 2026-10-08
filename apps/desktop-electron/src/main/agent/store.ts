// Chats on disk (D675): one JSON file per chat, `<userData>/chats/<id>.json`,
// written atomically (temp file + rename). Never uploaded anywhere.
import { randomBytes } from "node:crypto";
import {
	mkdirSync,
	readdirSync,
	readFileSync,
	renameSync,
	rmSync,
	writeFileSync,
} from "node:fs";
import { join } from "node:path";
import type { ChatRecord, ChatSummary } from "../../shared/contracts";

const ID = /^c_[a-z0-9]{8,40}$/;

export const isChatId = (v: unknown): v is string =>
	typeof v === "string" && ID.test(v);

export function newChatId(): string {
	return `c_${Date.now().toString(36)}${randomBytes(6).toString("hex")}`;
}

const summary = (c: ChatRecord): ChatSummary => ({
	id: c.id,
	title: c.title,
	createdAt: c.createdAt,
	updatedAt: c.updatedAt,
	provider: c.provider,
	model: c.model,
});

export class ChatStore {
	constructor(private readonly dir: string) {}

	#file(id: string): string {
		if (!isChatId(id)) throw new Error("bad chat id");
		return join(this.dir, `${id}.json`);
	}

	get(id: string): ChatRecord | undefined {
		if (!isChatId(id)) return undefined;
		try {
			const c = JSON.parse(readFileSync(this.#file(id), "utf8")) as ChatRecord;
			return c && c.id === id && Array.isArray(c.messages) ? c : undefined;
		} catch {
			return undefined;
		}
	}

	save(chat: ChatRecord): void {
		const file = this.#file(chat.id);
		mkdirSync(this.dir, { recursive: true });
		const tmp = `${file}.${process.pid}.tmp`;
		writeFileSync(tmp, JSON.stringify(chat), { mode: 0o600 });
		renameSync(tmp, file);
	}

	remove(id: string): void {
		rmSync(this.#file(id), { force: true });
	}

	/** Newest first (by last update). */
	list(): ChatSummary[] {
		let names: string[];
		try {
			names = readdirSync(this.dir);
		} catch {
			return [];
		}
		const out: ChatSummary[] = [];
		for (const n of names) {
			if (!n.endsWith(".json")) continue;
			const c = this.get(n.slice(0, -5));
			if (c) out.push(summary(c));
		}
		return out.sort((a, b) => b.updatedAt - a.updatedAt);
	}
}

export { summary as chatSummary };
