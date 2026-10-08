import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import {
	ANTHROPIC_DEFAULT_MODEL,
	createAnthropicProvider,
	toAnthropicMessages,
} from "../src/main/agent/providers/anthropic";
import {
	createOpenAIProvider,
	toOpenAIMessages,
} from "../src/main/agent/providers/openai";
import { readSse } from "../src/main/agent/providers/sse";
import type { ProviderEvent } from "../src/main/agent/providers/types";
import { Secret } from "../src/main/factory/vault";

const fixture = (name: string) =>
	readFileSync(join(__dirname, "fixtures", "agent", name), "utf8");

/** A body that delivers `text` in chunks of `size` bytes (to split lines and JSON). */
function chunked(text: string, size: number): ReadableStream<Uint8Array> {
	const bytes = new TextEncoder().encode(text);
	let at = 0;
	return new ReadableStream({
		pull(c) {
			if (at >= bytes.length) return c.close();
			c.enqueue(bytes.slice(at, at + size));
			at += size;
		},
	});
}

interface Captured {
	url: string;
	init: RequestInit;
}
function sseFetch(text: string, size = 7, status = 200) {
	const calls: Captured[] = [];
	const fetch = async (url: string, init: RequestInit) => {
		calls.push({ url, init });
		return new Response(chunked(text, size), {
			status,
			headers: { "content-type": "text/event-stream" },
		});
	};
	return { calls, fetch };
}

async function collect(
	it: AsyncIterable<ProviderEvent>,
): Promise<ProviderEvent[]> {
	const out: ProviderEvent[] = [];
	for await (const e of it) out.push(e);
	return out;
}

const req = (model: string) => ({
	model,
	system: "sys",
	messages: [
		{ role: "user" as const, content: [{ type: "text" as const, text: "hi" }] },
	],
	tools: [
		{
			name: "collections_list",
			description: "d",
			schema: { type: "object", properties: {} },
		},
	],
	signal: new AbortController().signal,
});

describe("sse", () => {
	it("parses_crlf_split_and_multiline_data", async () => {
		const text =
			"event: a\r\ndata: one\r\ndata: two\r\n\r\n: comment\n\ndata: x\n\ndata: tail";
		for (const size of [1, 2, 3, 64]) {
			const msgs = [];
			for await (const m of readSse(chunked(text, size))) msgs.push(m);
			expect(msgs).toEqual([
				{ event: "a", data: "one\ntwo" },
				{ event: "message", data: "x" },
				{ event: "message", data: "tail" },
			]);
		}
	});
});

describe("sse limits", () => {
	it("multi_byte_split_at_chunk_size_1", async () => {
		const text = 'data: {"t":"é😀中"}\n\n';
		const msgs = [];
		for await (const m of readSse(chunked(text, 1))) msgs.push(m);
		expect(msgs).toEqual([{ event: "message", data: '{"t":"é😀中"}' }]);
	});

	it("over_long_line_is_an_error", async () => {
		const long = `data: ${"x".repeat(200)}`;
		const read = async () => {
			for await (const _ of readSse(chunked(long, 16), 100)) {
				// drain
			}
		};
		await expect(read()).rejects.toThrow(/longer than 100/);
		const many = "data: aaaaaaaaaa\n".repeat(20);
		const readMany = async () => {
			for await (const _ of readSse(chunked(many, 7), 100)) {
				// drain
			}
		};
		await expect(readMany()).rejects.toThrow(/longer than 100/);
	});
});

describe("anthropic provider", () => {
	it("anthropic_sse_parsing", async () => {
		const { calls, fetch } = sseFetch(fixture("anthropic-tool-use.sse"), 5);
		const p = createAnthropicProvider({
			apiKey: new Secret("sk-ant-test"),
			fetch,
		});
		const events = await collect(p.streamTurn(req(ANTHROPIC_DEFAULT_MODEL)));

		const text = events
			.flatMap((e) => (e.type === "text" ? [e.text] : []))
			.join("");
		expect(text).toBe("Let me check.");
		const thinking = events
			.flatMap((e) => (e.type === "thinking" ? [e.text] : []))
			.join("");
		expect(thinking).toBe("The user wants collections; list them.");
		expect(events).toContainEqual({
			type: "signature",
			index: 0,
			signature: "EqQBCkYIBxgCKkA=",
		});
		expect(events).toContainEqual({
			type: "tool_use",
			index: 2,
			id: "toolu_01",
			name: "collections_list",
			input: { namespace: "default" },
			raw: '{"namespace": "default"}',
		});
		expect(events.at(-1)).toEqual({ type: "stop", reason: "tool_use" });
		const usage = events.filter((e) => e.type === "usage").at(-1);
		expect(usage).toEqual({
			type: "usage",
			inputTokens: 912,
			outputTokens: 57,
		});

		// The request.
		expect(calls[0]?.url).toBe("https://api.anthropic.com/v1/messages");
		const h = calls[0]?.init.headers as Record<string, string>;
		expect(h["anthropic-version"]).toBe("2023-06-01");
		expect(h["x-api-key"]).toBe("sk-ant-test");
		const body = JSON.parse(String(calls[0]?.init.body));
		expect(body).toMatchObject({
			model: "claude-sonnet-5-5",
			stream: true,
			thinking: { type: "adaptive", display: "summarized" },
			// Prompt caching: system, the last tool, and the automatic breakpoint.
			system: [
				{ type: "text", text: "sys", cache_control: { type: "ephemeral" } },
			],
			cache_control: { type: "ephemeral" },
		});
		expect(body.tools[0]).toMatchObject({
			name: "collections_list",
			input_schema: { type: "object" },
			eager_input_streaming: true,
			cache_control: { type: "ephemeral" },
		});
		// The refusal fallback is opt-in: off by default.
		expect(body.fallbacks).toBeUndefined();
		expect(h["anthropic-beta"]).toBeUndefined();
		expect(events).toContainEqual({
			type: "model",
			model: "claude-sonnet-5-5",
		});
	});

	it("fallback_opt_in_sends_beta_and_reports_models", async () => {
		const ev = (o: unknown) => `data: ${JSON.stringify(o)}\n\n`;
		const sse =
			ev({
				type: "message_start",
				message: { model: "claude-sonnet-5-5", usage: { input_tokens: 3 } },
			}) +
			ev({
				type: "content_block_start",
				index: 0,
				content_block: { type: "text", text: "" },
			}) +
			ev({
				type: "content_block_delta",
				index: 0,
				delta: { type: "text_delta", text: "partial" },
			}) +
			ev({
				type: "content_block_start",
				index: 1,
				content_block: {
					type: "fallback",
					from: { model: "claude-sonnet-5-5" },
					to: { model: "claude-opus-4-8" },
				},
			}) +
			ev({ type: "content_block_stop", index: 1 }) +
			ev({
				type: "message_delta",
				delta: { stop_reason: "end_turn" },
				usage: { output_tokens: 4 },
			});
		const { calls, fetch } = sseFetch(sse);
		const p = createAnthropicProvider({
			apiKey: new Secret("sk-ant-test"),
			fetch,
			fallback: true,
		});
		const events = await collect(p.streamTurn(req("claude-sonnet-5-5")));
		const body = JSON.parse(String(calls[0]?.init.body));
		expect(body.fallbacks).toBe("default");
		expect(
			(calls[0]?.init.headers as Record<string, string> | undefined)?.[
				"anthropic-beta"
			],
		).toBe("server-side-fallback-2026-07-01");
		expect(events).toContainEqual({
			type: "fallback",
			index: 1,
			from: "claude-sonnet-5-5",
			to: "claude-opus-4-8",
		});
		// Opted in but on a model the fallback does not cover: nothing is sent.
		const other = sseFetch(sse);
		await collect(
			createAnthropicProvider({
				apiKey: new Secret("k"),
				fetch: other.fetch,
				fallback: true,
			}).streamTurn(req("claude-haiku-4-5")),
		);
		expect(
			JSON.parse(String(other.calls[0]?.init.body)).fallbacks,
		).toBeUndefined();
	});

	it("custom_base_url_sends_plain_request", async () => {
		const { calls, fetch } = sseFetch(fixture("anthropic-tool-use.sse"));
		const p = createAnthropicProvider({
			baseUrl: "https://proxy.example/",
			apiKey: new Secret("k"),
			fetch,
		});
		await collect(p.streamTurn(req("claude-haiku-4-5")));
		expect(calls[0]?.url).toBe("https://proxy.example/v1/messages");
		const body = JSON.parse(String(calls[0]?.init.body));
		expect(body.thinking).toBeUndefined();
		expect(body.fallbacks).toBeUndefined();
		expect(body.cache_control).toBeUndefined();
		expect(body.tools[0].eager_input_streaming).toBeUndefined();
	});

	it("http_error_becomes_provider_error", async () => {
		const fetch = async () =>
			new Response(
				JSON.stringify({
					type: "error",
					error: { type: "authentication_error", message: "invalid x-api-key" },
				}),
				{ status: 401 },
			);
		const p = createAnthropicProvider({ apiKey: new Secret("k"), fetch });
		await expect(collect(p.streamTurn(req("m")))).rejects.toThrow(
			"HTTP 401: invalid x-api-key",
		);
	});

	it("stream_error_event_throws", async () => {
		const { fetch } = sseFetch(
			'event: error\ndata: {"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}\n\n',
		);
		const p = createAnthropicProvider({ apiKey: new Secret("k"), fetch });
		await expect(collect(p.streamTurn(req("m")))).rejects.toThrow(
			"overloaded_error: Overloaded",
		);
	});

	it("messages_echo_signed_thinking_and_tool_results", () => {
		const out = toAnthropicMessages([
			{ role: "user", content: [{ type: "text", text: "q" }] },
			{
				role: "assistant",
				content: [
					{ type: "thinking", text: "t", signature: "sig" },
					{ type: "thinking", text: "from deepseek" },
					{ type: "tool_use", id: "u1", name: "x", input: { a: 1 } },
				],
			},
			{
				role: "user",
				content: [
					{ type: "tool_result", toolUseId: "u1", text: "r", isError: true },
				],
			},
		]);
		expect(out).toEqual([
			{ role: "user", content: [{ type: "text", text: "q" }] },
			{
				role: "assistant",
				content: [
					{ type: "thinking", thinking: "t", signature: "sig" },
					{ type: "tool_use", id: "u1", name: "x", input: { a: 1 } },
				],
			},
			{
				role: "user",
				content: [
					{
						type: "tool_result",
						tool_use_id: "u1",
						content: "r",
						is_error: true,
					},
				],
			},
		]);
	});
});

describe("openai-compatible provider", () => {
	it("openai_sse_tool_call_assembly", async () => {
		for (const size of [3, 11, 4096]) {
			const { calls, fetch } = sseFetch(fixture("openai-tool-call.sse"), size);
			const p = createOpenAIProvider({
				id: "deepseek",
				baseUrl: "https://api.deepseek.com/v1",
				apiKey: new Secret("sk-ds"),
				fetch,
				includeUsage: true,
			});
			const events = await collect(p.streamTurn(req("deepseek-chat")));
			expect(
				events.flatMap((e) => (e.type === "text" ? [e.text] : [])).join(""),
			).toBe("Checking streams.");
			const uses = events.filter((e) => e.type === "tool_use");
			expect(uses).toEqual([
				{
					type: "tool_use",
					index: 2,
					id: "call_a",
					name: "streams_list",
					input: { namespace: "default" },
					raw: '{"namespace": "default"}',
				},
				{
					type: "tool_use",
					index: 3,
					id: "call_b",
					name: "links_list",
					input: { namespace: "prod" },
					raw: '{"namespace": "prod"}',
				},
			]);
			expect(events).toContainEqual({
				type: "usage",
				inputTokens: 400,
				outputTokens: 31,
			});
			expect(events.at(-1)).toEqual({ type: "stop", reason: "tool_use" });
			expect(calls[0]?.url).toBe(
				"https://api.deepseek.com/v1/chat/completions",
			);
			const h = calls[0]?.init.headers as Record<string, string>;
			expect(h.authorization).toBe("Bearer sk-ds");
			const body = JSON.parse(String(calls[0]?.init.body));
			expect(body).toMatchObject({
				model: "deepseek-chat",
				stream: true,
				stream_options: { include_usage: true },
			});
			expect(body.tools[0]).toEqual({
				type: "function",
				function: {
					name: "collections_list",
					description: "d",
					parameters: { type: "object", properties: {} },
				},
			});
		}
	});

	it("tool_calls_without_index_are_keyed_by_id", async () => {
		const d = (o: unknown) => `data: ${JSON.stringify(o)}\n\n`;
		const tc = (calls: unknown[]) =>
			d({
				choices: [
					{ index: 0, delta: { tool_calls: calls }, finish_reason: null },
				],
			});
		const sse =
			tc([
				{
					id: "a",
					function: { name: "streams_list", arguments: '{"namespace":' },
				},
			]) +
			tc([
				{
					id: "b",
					function: { name: "links_list", arguments: '{"namespace":"x"}' },
				},
			]) +
			tc([{ id: "a", function: { arguments: '"y"}' } }]) +
			"data: [DONE]\n\n";
		const { fetch } = sseFetch(sse);
		const p = createOpenAIProvider({
			id: "ollama",
			baseUrl: "http://127.0.0.1:1/v1",
			fetch,
		});
		const uses = (await collect(p.streamTurn(req("m")))).filter(
			(e) => e.type === "tool_use",
		);
		expect(
			uses.map((u) => u.type === "tool_use" && [u.id, u.name, u.input]),
		).toEqual([
			["a", "streams_list", { namespace: "y" }],
			["b", "links_list", { namespace: "x" }],
		]);
	});

	it("keyless_endpoint_sends_no_authorization", async () => {
		const { calls, fetch } = sseFetch("data: [DONE]\n\n");
		const p = createOpenAIProvider({
			id: "ollama",
			baseUrl: "http://127.0.0.1:11434/v1",
			fetch,
		});
		const events = await collect(p.streamTurn(req("llama3.1")));
		expect(events).toEqual([{ type: "stop", reason: "end_turn" }]);
		expect(
			(calls[0]?.init.headers as Record<string, string> | undefined)
				?.authorization,
		).toBeUndefined();
	});

	it("invalid_arguments_yield_undefined_input", async () => {
		const { fetch } = sseFetch(
			'data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"c","function":{"name":"x","arguments":"{\\"a\\":"}}]},"finish_reason":"length"}]}\n\ndata: [DONE]\n\n',
		);
		const p = createOpenAIProvider({
			id: "openai",
			baseUrl: "https://api.openai.com/v1",
			fetch,
		});
		const events = await collect(p.streamTurn(req("m")));
		expect(events.find((e) => e.type === "tool_use")).toMatchObject({
			input: undefined,
			raw: '{"a":',
		});
	});

	it("messages_map_tool_results_to_tool_role", () => {
		expect(
			toOpenAIMessages("sys", [
				{ role: "user", content: [{ type: "text", text: "q" }] },
				{
					role: "assistant",
					content: [
						{ type: "thinking", text: "hidden", signature: "s" },
						{ type: "text", text: "ok" },
						{ type: "tool_use", id: "u1", name: "x", input: { a: 1 } },
					],
				},
				{
					role: "user",
					content: [
						{ type: "tool_result", toolUseId: "u1", text: "r" },
						{ type: "text", text: "next" },
					],
				},
			]),
		).toEqual([
			{ role: "system", content: "sys" },
			{ role: "user", content: "q" },
			{
				role: "assistant",
				content: "ok",
				tool_calls: [
					{
						id: "u1",
						type: "function",
						function: { name: "x", arguments: '{"a":1}' },
					},
				],
			},
			{ role: "tool", tool_call_id: "u1", content: "r" },
			{ role: "user", content: "next" },
		]);
	});
});
