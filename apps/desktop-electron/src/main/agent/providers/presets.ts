// Provider presets and their configuration. Base URL and model live in a plain
// JSON file; the API key lives in the D659 vault under `agent:<provider>`.
import { mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";
import type {
	ChatProviderId,
	ChatProviderInfo,
	IpcResult,
} from "../../../shared/contracts";
import type { Secret, Vault } from "../../factory/vault";
import {
	ANTHROPIC_BASE_URL,
	ANTHROPIC_DEFAULT_MODEL,
	createAnthropicProvider,
} from "./anthropic";
import { createOpenAIProvider } from "./openai";
import type { FetchLike, Provider } from "./types";

export interface Preset {
	id: ChatProviderId;
	label: string;
	kind: "anthropic" | "openai";
	baseUrl: string;
	defaultModel: string;
	needsKey: boolean;
	includeUsage: boolean;
}

export const PRESETS: Record<ChatProviderId, Preset> = {
	anthropic: {
		id: "anthropic",
		label: "Anthropic",
		kind: "anthropic",
		baseUrl: ANTHROPIC_BASE_URL,
		defaultModel: ANTHROPIC_DEFAULT_MODEL,
		needsKey: true,
		includeUsage: false,
	},
	deepseek: {
		id: "deepseek",
		label: "DeepSeek",
		kind: "openai",
		baseUrl: "https://api.deepseek.com/v1",
		defaultModel: "deepseek-chat",
		needsKey: true,
		includeUsage: true,
	},
	openai: {
		id: "openai",
		label: "OpenAI",
		kind: "openai",
		baseUrl: "https://api.openai.com/v1",
		defaultModel: "gpt-5",
		needsKey: true,
		includeUsage: true,
	},
	ollama: {
		id: "ollama",
		label: "Ollama",
		kind: "openai",
		baseUrl: "http://127.0.0.1:11434/v1",
		defaultModel: "llama3.1",
		needsKey: false,
		includeUsage: true,
	},
};

export const isProviderId = (v: unknown): v is ChatProviderId =>
	typeof v === "string" && Object.hasOwn(PRESETS, v);

const vaultKey = (id: ChatProviderId) => `agent:${id}` as const;

interface Saved {
	baseUrl?: string;
	model?: string;
}

/** http(s) only, no userinfo; plain http only to loopback (keys must not travel in clear). */
export function checkBaseUrl(raw: string): string | undefined {
	let u: URL;
	try {
		u = new URL(raw);
	} catch {
		return "Enter an http(s) URL";
	}
	if (u.protocol !== "https:" && u.protocol !== "http:")
		return "Enter an http(s) URL";
	if (u.username || u.password)
		return "Put the key in the key field, not in the URL";
	const loopback = ["127.0.0.1", "localhost", "[::1]"].includes(u.hostname);
	if (u.protocol === "http:" && !loopback)
		return "Use https:// (http is allowed for localhost only)";
	return undefined;
}

export class ProviderConfigs {
	#saved: Partial<Record<ChatProviderId, Saved>> = {};

	constructor(
		private readonly file: string,
		private readonly vault: Vault,
		private readonly fetchImpl: FetchLike,
	) {
		try {
			const raw = JSON.parse(readFileSync(file, "utf8")) as Record<
				string,
				Saved
			>;
			for (const [k, v] of Object.entries(raw))
				if (isProviderId(k) && typeof v === "object" && v !== null)
					this.#saved[k] = {
						...(typeof v.baseUrl === "string" ? { baseUrl: v.baseUrl } : {}),
						...(typeof v.model === "string" ? { model: v.model } : {}),
					};
		} catch {
			// first run
		}
	}

	#key(id: ChatProviderId): Secret | undefined {
		const k = this.vault.get(vaultKey(id))?.fields.apiKey;
		return k?.reveal() ? k : undefined;
	}

	/** Every form of every stored key, for redaction. */
	secrets(): string[] {
		return (Object.keys(PRESETS) as ChatProviderId[]).flatMap((id) => {
			const k = this.#key(id)?.reveal();
			return k ? [k] : [];
		});
	}

	info(id: ChatProviderId): ChatProviderInfo {
		const p = PRESETS[id];
		const s = this.#saved[id] ?? {};
		const hasKey = this.#key(id) !== undefined;
		return {
			id,
			label: p.label,
			kind: p.kind,
			baseUrl: s.baseUrl ?? p.baseUrl,
			model: s.model ?? p.defaultModel,
			defaultModel: p.defaultModel,
			needsKey: p.needsKey,
			hasKey,
			configured: hasKey || !p.needsKey,
			persistent: this.vault.persistent,
		};
	}

	list(): ChatProviderInfo[] {
		return (Object.keys(PRESETS) as ChatProviderId[]).map((id) =>
			this.info(id),
		);
	}

	configure(id: unknown, cfg: unknown): IpcResult<ChatProviderInfo> {
		if (!isProviderId(id))
			return {
				ok: false,
				code: "unknown_provider",
				message: "Unknown provider",
			};
		const c = (typeof cfg === "object" && cfg !== null ? cfg : {}) as Record<
			string,
			unknown
		>;
		const model = typeof c.model === "string" ? c.model.trim() : "";
		if (!model || model.length > 200 || /\s/.test(model))
			return { ok: false, code: "bad_model", message: "Enter a model name" };
		const baseUrl =
			typeof c.baseUrl === "string" && c.baseUrl.trim()
				? c.baseUrl.trim()
				: undefined;
		if (baseUrl) {
			const bad = checkBaseUrl(baseUrl);
			if (bad) return { ok: false, code: "invalid_url", message: bad };
		}
		const apiKey = typeof c.apiKey === "string" ? c.apiKey.trim() : "";
		if (apiKey.length > 4096)
			return { ok: false, code: "bad_key", message: "The key is too long" };
		const p = PRESETS[id];
		this.#saved[id] = {
			...(baseUrl && baseUrl !== p.baseUrl ? { baseUrl } : {}),
			...(model !== p.defaultModel ? { model } : {}),
		};
		this.#write();
		if (apiKey) this.vault.set(vaultKey(id), baseUrl ?? p.baseUrl, { apiKey });
		return { ok: true, value: this.info(id) };
	}

	/** A ready provider, or why not. */
	create(id: ChatProviderId): Provider | { error: string } {
		const p = PRESETS[id];
		const info = this.info(id);
		const key = this.#key(id);
		if (p.needsKey && !key)
			return {
				error: `${p.label} has no API key yet. Add one in Settings › Agent.`,
			};
		if (p.kind === "anthropic") {
			if (!key) return { error: "Anthropic needs an API key." };
			return createAnthropicProvider({
				baseUrl: info.baseUrl,
				apiKey: key,
				fetch: this.fetchImpl,
			});
		}
		return createOpenAIProvider({
			id,
			baseUrl: info.baseUrl,
			...(key ? { apiKey: key } : {}),
			fetch: this.fetchImpl,
			includeUsage: p.includeUsage,
		});
	}

	#write(): void {
		mkdirSync(dirname(this.file), { recursive: true });
		const tmp = `${this.file}.tmp`;
		writeFileSync(tmp, JSON.stringify(this.#saved, null, 2));
		renameSync(tmp, this.file);
	}
}
