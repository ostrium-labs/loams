// OpenAI-compatible chat completions over raw HTTP + SSE (DeepSeek, OpenAI, Ollama; D675).
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

export interface OpenAIOptions {
	id: string;
	baseUrl: string;
	/** Undefined for keyless endpoints (Ollama). */
	apiKey?: Secret;
	fetch: FetchLike;
	/** Ask for a final usage chunk (`stream_options.include_usage`). */
	includeUsage?: boolean;
}

type Obj = Record<string, unknown>;

/** Stored messages as chat-completions messages (system first). */
export function toOpenAIMessages(system: string, messages: Msg[]): Obj[] {
	const out: Obj[] = [{ role: "system", content: system }];
	for (const m of messages) {
		if (m.role === "user") {
			// Tool results answer the previous assistant message, so they come first.
			for (const p of m.content)
				if (p.type === "tool_result")
					out.push({
						role: "tool",
						tool_call_id: p.toolUseId,
						content: p.text,
					});
			const text = m.content
				.flatMap((p) => (p.type === "text" ? [p.text] : []))
				.join("\n\n");
			if (text) out.push({ role: "user", content: text });
		} else {
			const text = m.content
				.flatMap((p) => (p.type === "text" ? [p.text] : []))
				.join("");
			const calls = m.content.flatMap((p) =>
				p.type === "tool_use"
					? [
							{
								id: p.id,
								type: "function",
								function: {
									name: p.name,
									arguments: JSON.stringify(p.input ?? {}),
								},
							},
						]
					: [],
			);
			if (!text && calls.length === 0) continue;
			out.push({
				role: "assistant",
				content: text || null,
				...(calls.length > 0 ? { tool_calls: calls } : {}),
			});
		}
	}
	return out;
}

function stopOf(reason: unknown): ProviderStop {
	switch (reason) {
		case "stop":
			return "end_turn";
		case "tool_calls":
		case "function_call":
			return "tool_use";
		case "length":
			return "max_tokens";
		case "content_filter":
			return "refusal";
		default:
			return "other";
	}
}

const num = (v: unknown): number => (typeof v === "number" ? v : 0);

export function createOpenAIProvider(opts: OpenAIOptions): Provider {
	const base = opts.baseUrl.replace(/\/+$/, "");
	return {
		id: opts.id,
		async *streamTurn(req: TurnRequest): AsyncGenerator<ProviderEvent> {
			const body: Obj = {
				model: req.model,
				stream: true,
				messages: toOpenAIMessages(req.system, req.messages),
			};
			if (req.tools.length > 0)
				body.tools = req.tools.map((t) => ({
					type: "function",
					function: {
						name: t.name,
						description: t.description,
						parameters: t.schema,
					},
				}));
			if (opts.includeUsage) body.stream_options = { include_usage: true };
			const headers: Record<string, string> = {
				"content-type": "application/json",
				accept: "text/event-stream",
			};
			if (opts.apiKey) headers.authorization = `Bearer ${opts.apiKey.reveal()}`;
			const res = await opts.fetch(`${base}/chat/completions`, {
				method: "POST",
				headers,
				body: JSON.stringify(body),
				signal: req.signal,
			});
			if (!res.ok) throw await httpError(res);
			if (!res.body) throw new ProviderError(res.status, "empty response");

			// Blocks: 0 reasoning, 1 text, 2+i the i-th tool call seen. Calls are keyed by
			// their `index`, or by `id` for servers that omit it; a delta with neither
			// continues the last call.
			const calls = new Map<
				string,
				{ id: string; name: string; args: string }
			>();
			let lastKey: string | undefined;
			let model: string | undefined;
			let stop: ProviderStop | undefined;
			for await (const msg of readSse(res.body)) {
				if (msg.data.trim() === "[DONE]") break;
				let chunk: Obj;
				try {
					chunk = JSON.parse(msg.data) as Obj;
				} catch {
					continue;
				}
				if (chunk.error) {
					const e = chunk.error as Obj;
					throw new ProviderError(0, String(e.message ?? "stream error"));
				}
				if (!model && typeof chunk.model === "string" && chunk.model) {
					model = chunk.model;
					yield { type: "model", model };
				}
				const usage = chunk.usage as Obj | undefined | null;
				if (usage)
					yield {
						type: "usage",
						inputTokens: num(usage.prompt_tokens),
						outputTokens: num(usage.completion_tokens),
					};
				const choices = Array.isArray(chunk.choices)
					? (chunk.choices as Obj[])
					: [];
				const choice = choices[0];
				if (!choice) continue;
				const delta = (choice.delta ?? {}) as Obj;
				const reasoning = delta.reasoning_content ?? delta.reasoning;
				if (typeof reasoning === "string" && reasoning)
					yield { type: "thinking", index: 0, text: reasoning };
				if (typeof delta.content === "string" && delta.content)
					yield { type: "text", index: 1, text: delta.content };
				if (Array.isArray(delta.tool_calls)) {
					for (const tc of delta.tool_calls as Obj[]) {
						const key =
							typeof tc.index === "number"
								? `i:${tc.index}`
								: typeof tc.id === "string" && tc.id
									? `id:${tc.id}`
									: (lastKey ?? "i:0");
						lastKey = key;
						const fn = (tc.function ?? {}) as Obj;
						const cur = calls.get(key) ?? { id: "", name: "", args: "" };
						if (typeof tc.id === "string" && tc.id) cur.id = tc.id;
						if (typeof fn.name === "string" && fn.name) cur.name = fn.name;
						if (typeof fn.arguments === "string") cur.args += fn.arguments;
						// Some servers (Ollama) send the arguments as an object in one chunk.
						else if (fn.arguments && typeof fn.arguments === "object")
							cur.args = JSON.stringify(fn.arguments);
						calls.set(key, cur);
					}
				}
				if (choice.finish_reason) stop = stopOf(choice.finish_reason);
			}
			// Map order is first-seen order.
			let i = -1;
			for (const c of calls.values()) {
				i++;
				let input: unknown;
				try {
					input = c.args.trim() === "" ? {} : JSON.parse(c.args);
				} catch {
					input = undefined;
				}
				yield {
					type: "tool_use",
					index: 2 + i,
					id: c.id || `call_${i}_${Date.now().toString(36)}`,
					name: c.name,
					input,
					raw: c.args,
				};
			}
			yield {
				type: "stop",
				reason: calls.size > 0 ? "tool_use" : (stop ?? "end_turn"),
			};
		},
	};
}
