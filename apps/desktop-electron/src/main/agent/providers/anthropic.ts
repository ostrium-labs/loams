// Anthropic Messages over raw HTTP + SSE (D675; no SDK dependency in the desktop's main process).
import type { Secret } from "../../factory/vault";
import { readSse } from "./sse";
import {
	type FetchLike,
	httpError,
	type Msg,
	type Provider,
	ProviderError,
	type ProviderEvent,
	type ProviderStop,
	type TurnRequest,
} from "./types";

export const ANTHROPIC_BASE_URL = "https://api.anthropic.com";
export const ANTHROPIC_DEFAULT_MODEL = "claude-sonnet-5-5";
export const ANTHROPIC_VERSION = "2023-06-01";
const MAX_TOKENS = 32_000;

/** Models that take `thinking: {type: "adaptive"}` (current generation). */
const ADAPTIVE =
	/^claude-(fable|mythos|opus|sonnet|haiku)-5(-\d+)?$|^claude-(opus-4-[678]|sonnet-4-6)$/;
/** Models the server-side `fallbacks: "default"` form accepts. */
const FALLBACK_MODELS = new Set([
	"claude-fable-5-1",
	"claude-opus-5-5",
	"claude-opus-5",
	"claude-sonnet-5-5",
]);
const FALLBACK_BETA = "server-side-fallback-2026-07-01";

export interface AnthropicOptions {
	baseUrl?: string;
	apiKey: Secret;
	fetch: FetchLike;
}

type Block = Record<string, unknown>;

/** Stored messages as Messages API content blocks. */
export function toAnthropicMessages(messages: Msg[]): Block[] {
	const out: Block[] = [];
	for (const m of messages) {
		const content: Block[] = [];
		for (const p of m.content) {
			if (p.type === "text") {
				if (p.text) content.push({ type: "text", text: p.text });
			} else if (p.type === "thinking") {
				// Echoed unchanged; a thinking part from another provider has no signature and is dropped.
				if (m.role === "assistant" && p.signature)
					content.push({
						type: "thinking",
						thinking: p.text,
						signature: p.signature,
					});
			} else if (p.type === "redacted_thinking") {
				if (m.role === "assistant")
					content.push({ type: "redacted_thinking", data: p.data });
			} else if (p.type === "tool_use") {
				if (m.role === "assistant")
					content.push({
						type: "tool_use",
						id: p.id,
						name: p.name,
						input: p.input ?? {},
					});
			} else if (p.type === "tool_result") {
				if (m.role === "user")
					content.push({
						type: "tool_result",
						tool_use_id: p.toolUseId,
						content: p.text,
						...(p.isError ? { is_error: true } : {}),
					});
			}
		}
		if (content.length > 0) out.push({ role: m.role, content });
	}
	return out;
}

function stopOf(reason: unknown): ProviderStop {
	switch (reason) {
		case "end_turn":
		case "stop_sequence":
			return "end_turn";
		case "tool_use":
			return "tool_use";
		case "max_tokens":
			return "max_tokens";
		case "refusal":
			return "refusal";
		default:
			return "other";
	}
}

const num = (v: unknown): number => (typeof v === "number" ? v : 0);

export function createAnthropicProvider(opts: AnthropicOptions): Provider {
	const base = (opts.baseUrl || ANTHROPIC_BASE_URL).replace(/\/+$/, "");
	const firstParty = base === ANTHROPIC_BASE_URL;
	return {
		id: "anthropic",
		async *streamTurn(req: TurnRequest): AsyncGenerator<ProviderEvent> {
			const fallbacks = firstParty && FALLBACK_MODELS.has(req.model);
			const body: Record<string, unknown> = {
				model: req.model,
				max_tokens: MAX_TOKENS,
				stream: true,
				system: req.system,
				messages: toAnthropicMessages(req.messages),
				tools: req.tools.map((t) => ({
					name: t.name,
					description: t.description,
					input_schema: t.schema,
					// Proxies may reject the field; the loop validates inputs either way.
					...(firstParty ? { eager_input_streaming: true } : {}),
				})),
			};
			if (ADAPTIVE.test(req.model))
				body.thinking = { type: "adaptive", display: "summarized" };
			if (fallbacks) body.fallbacks = "default";
			const headers: Record<string, string> = {
				"content-type": "application/json",
				accept: "text/event-stream",
				"anthropic-version": ANTHROPIC_VERSION,
				"x-api-key": opts.apiKey.reveal(),
			};
			if (fallbacks) headers["anthropic-beta"] = FALLBACK_BETA;
			const res = await opts.fetch(`${base}/v1/messages`, {
				method: "POST",
				headers,
				body: JSON.stringify(body),
				signal: req.signal,
			});
			if (!res.ok) throw await httpError(res);
			if (!res.body) throw new ProviderError(res.status, "empty response");

			let input = 0;
			let output = 0;
			const tools = new Map<
				number,
				{ id: string; name: string; json: string }
			>();
			for await (const msg of readSse(res.body)) {
				let ev: Block;
				try {
					ev = JSON.parse(msg.data) as Block;
				} catch {
					continue;
				}
				const type = ev.type ?? msg.event;
				const index = num(ev.index);
				if (type === "message_start") {
					const u = ((ev.message as Block | undefined)?.usage ?? {}) as Block;
					input =
						num(u.input_tokens) +
						num(u.cache_creation_input_tokens) +
						num(u.cache_read_input_tokens);
					output = num(u.output_tokens);
					yield { type: "usage", inputTokens: input, outputTokens: output };
				} else if (type === "content_block_start") {
					const b = (ev.content_block ?? {}) as Block;
					if (b.type === "tool_use")
						tools.set(index, {
							id: String(b.id ?? ""),
							name: String(b.name ?? ""),
							json: "",
						});
					else if (b.type === "text" && typeof b.text === "string" && b.text)
						yield { type: "text", index, text: b.text };
					else if (b.type === "thinking")
						yield {
							type: "thinking",
							index,
							text: typeof b.thinking === "string" ? b.thinking : "",
						};
					else if (b.type === "redacted_thinking")
						yield {
							type: "redacted_thinking",
							index,
							data: String(b.data ?? ""),
						};
					else if (b.type === "fallback") yield { type: "fallback", index };
				} else if (type === "content_block_delta") {
					const d = (ev.delta ?? {}) as Block;
					if (d.type === "text_delta")
						yield { type: "text", index, text: String(d.text ?? "") };
					else if (d.type === "thinking_delta")
						yield { type: "thinking", index, text: String(d.thinking ?? "") };
					else if (d.type === "signature_delta")
						yield {
							type: "signature",
							index,
							signature: String(d.signature ?? ""),
						};
					else if (d.type === "input_json_delta") {
						const t = tools.get(index);
						if (t) t.json += String(d.partial_json ?? "");
					}
				} else if (type === "content_block_stop") {
					const t = tools.get(index);
					if (t) {
						tools.delete(index);
						let parsed: unknown;
						try {
							parsed = t.json.trim() === "" ? {} : JSON.parse(t.json);
						} catch {
							parsed = undefined;
						}
						yield {
							type: "tool_use",
							index,
							id: t.id,
							name: t.name,
							input: parsed,
							raw: t.json,
						};
					}
				} else if (type === "message_delta") {
					const u = (ev.usage ?? {}) as Block;
					if (typeof u.output_tokens === "number") output = u.output_tokens;
					if (typeof u.input_tokens === "number")
						input =
							num(u.input_tokens) +
							num(u.cache_creation_input_tokens) +
							num(u.cache_read_input_tokens);
					yield { type: "usage", inputTokens: input, outputTokens: output };
					const reason = (ev.delta as Block | undefined)?.stop_reason;
					if (reason !== undefined && reason !== null)
						yield { type: "stop", reason: stopOf(reason) };
				} else if (type === "error") {
					const e = (ev.error ?? {}) as Block;
					throw new ProviderError(
						0,
						`${String(e.type ?? "error")}: ${String(e.message ?? "stream error")}`,
					);
				}
			}
		},
	};
}
