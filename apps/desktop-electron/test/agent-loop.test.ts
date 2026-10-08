import { mkdtempSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import {
	CANCELLED,
	CUT_OFF,
	DENIED,
	runTurn,
	type TurnDeps,
} from "../src/main/agent/loop";
import { ProviderConfigs } from "../src/main/agent/providers/presets";
import type {
	Provider,
	ProviderEvent,
	TurnRequest,
} from "../src/main/agent/providers/types";
import { ChatService } from "../src/main/agent/service";
import { ChatStore } from "../src/main/agent/store";
import {
	MAX_RESULT_CHARS,
	type ToolDef,
	ToolRegistry,
} from "../src/main/agent/tools";
import { Vault, type VaultCrypto } from "../src/main/factory/vault";
import type {
	ChatApproval,
	ChatEvent,
	ChatRecord,
} from "../src/shared/contracts";

type Script = (
	req: TurnRequest,
	n: number,
) => ProviderEvent[] | AsyncIterable<ProviderEvent>;

/** A provider that answers call n with script(n). */
function scripted(script: Script, seen: TurnRequest[] = []): Provider {
	let n = 0;
	return {
		id: "fake",
		async *streamTurn(req) {
			seen.push(
				structuredClone({
					...req,
					signal: undefined,
				}) as unknown as TurnRequest,
			);
			yield* script(req, n++);
		},
	};
}

const toolUse = (id: string, name: string, input: unknown): ProviderEvent[] => [
	{ type: "tool_use", index: 1, id, name, input, raw: JSON.stringify(input) },
	{ type: "usage", inputTokens: 10, outputTokens: 5 },
	{ type: "stop", reason: "tool_use" },
];
const say = (text: string): ProviderEvent[] => [
	{ type: "text", index: 0, text },
	{ type: "usage", inputTokens: 10, outputTokens: 5 },
	{ type: "stop", reason: "end_turn" },
];

function registry(runs: string[] = []): ToolRegistry {
	const r = new ToolRegistry();
	const tool = (name: string, risk: "read" | "write"): ToolDef => ({
		name,
		description: name,
		risk,
		schema: {
			type: "object",
			properties: { n: { type: "integer" } },
			additionalProperties: false,
		},
		run: async (_ctx, args) => {
			runs.push(`${name}:${JSON.stringify(args)}`);
			return `${name} ran`;
		},
	});
	r.register([
		tool("read_thing", "read"),
		tool("write_a", "write"),
		tool("write_b", "write"),
		{
			name: "big",
			description: "big",
			risk: "read",
			schema: { type: "object" },
			run: async () => "x".repeat(50_000),
		},
		{
			name: "slow",
			description: "slow",
			risk: "read",
			schema: { type: "object" },
			run: (ctx) =>
				new Promise((_, reject) =>
					ctx.signal.addEventListener("abort", () =>
						reject(new Error("aborted")),
					),
				),
		},
	]);
	return r;
}

const newChat = (id = "c_test0001"): ChatRecord => ({
	id,
	title: "",
	createdAt: 0,
	updatedAt: 0,
	provider: "anthropic",
	model: "m",
	alwaysAllow: [],
	messages: [],
});

function deps(over: Partial<TurnDeps> & { provider: Provider }): TurnDeps & {
	events: ChatEvent[];
	saved: ChatRecord[];
} {
	const events: ChatEvent[] = [];
	const saved: ChatRecord[] = [];
	return {
		model: "m",
		tools: registry(),
		system: "sys",
		emit: (e) => events.push(e),
		save: (c) => saved.push(structuredClone(c)),
		askApproval: async () => "once" as ChatApproval,
		signal: new AbortController().signal,
		secrets: () => [],
		now: () => 1000,
		events,
		saved,
		...over,
	};
}

const tick = () => new Promise((r) => setTimeout(r, 0));

describe("agent loop", () => {
	it("runs_read_tool_and_finishes", async () => {
		const runs: string[] = [];
		const seen: TurnRequest[] = [];
		const d = deps({
			provider: scripted(
				(_r, n) =>
					n === 0 ? toolUse("u1", "read_thing", { n: 1 }) : say("done"),
				seen,
			),
			tools: registry(runs),
		});
		const chat = newChat();
		expect(await runTurn(chat, "hello", d)).toBe("end_turn");
		expect(runs).toEqual(['read_thing:{"n":1}']);
		expect(d.events.find((e) => e.kind === "tool_call")).toMatchObject({
			tool: "read_thing",
			risk: "read",
			needsApproval: false,
		});
		expect(d.events.at(-1)).toEqual({
			kind: "done",
			chatId: chat.id,
			stop: "end_turn",
			usage: { inputTokens: 20, outputTokens: 10 },
		});
		// The second call saw the tool result.
		expect(seen[1]?.messages.at(-1)).toEqual({
			role: "user",
			content: [
				{ type: "tool_result", toolUseId: "u1", text: "read_thing ran" },
			],
		});
		expect(chat.title).toBe("hello");
		expect(chat.messages.at(-1)).toMatchObject({
			role: "assistant",
			stop: "end_turn",
		});
	});

	it("write_tool_waits_for_approval", async () => {
		const runs: string[] = [];
		let decide: ((d: ChatApproval) => void) | undefined;
		const d = deps({
			provider: scripted((_r, n) =>
				n === 0 ? toolUse("u1", "write_a", { n: 2 }) : say("ok"),
			),
			tools: registry(runs),
			askApproval: () => new Promise((r) => (decide = r)),
		});
		const turn = runTurn(newChat(), "do it", d);
		for (let i = 0; i < 10; i++) await tick();
		expect(d.events.find((e) => e.kind === "tool_call")).toMatchObject({
			callId: "u1",
			tool: "write_a",
			risk: "write",
			needsApproval: true,
		});
		expect(runs).toEqual([]);
		expect(decide).toBeDefined();
		decide?.("once");
		expect(await turn).toBe("end_turn");
		expect(runs).toEqual(['write_a:{"n":2}']);
	});

	it("deny_returns_denial_result", async () => {
		const runs: string[] = [];
		const seen: TurnRequest[] = [];
		const d = deps({
			provider: scripted(
				(_r, n) => (n === 0 ? toolUse("u1", "write_a", {}) : say("ok")),
				seen,
			),
			tools: registry(runs),
			askApproval: async () => "deny",
		});
		await runTurn(newChat(), "do it", d);
		expect(runs).toEqual([]);
		expect(d.events.find((e) => e.kind === "tool_result")).toMatchObject({
			ok: false,
			text: DENIED,
		});
		expect(seen[1]?.messages.at(-1)?.content).toEqual([
			{
				type: "tool_result",
				toolUseId: "u1",
				text: "The user denied this action.",
				isError: true,
			},
		]);
	});

	it("iteration_cap_stops", async () => {
		const d = deps({
			provider: scripted((_r, n) => toolUse(`u${n}`, "read_thing", {})),
			budgets: { iterations: 3 },
		});
		const chat = newChat();
		expect(await runTurn(chat, "loop", d)).toBe("iteration_cap");
		expect(d.events.filter((e) => e.kind === "tool_call")).toHaveLength(3);
		expect(d.events.at(-1)).toMatchObject({
			kind: "done",
			stop: "iteration_cap",
		});
		// The default cap is 25.
		const d25 = deps({
			provider: scripted((_r, n) => toolUse(`u${n}`, "read_thing", {})),
		});
		expect(await runTurn(newChat(), "loop", d25)).toBe("iteration_cap");
		expect(d25.events.filter((e) => e.kind === "tool_call")).toHaveLength(25);
	});

	it("token_budget_stops_before_running_tools", async () => {
		const runs: string[] = [];
		const d = deps({
			provider: scripted(() => [
				{
					type: "tool_use",
					index: 0,
					id: "u1",
					name: "read_thing",
					input: {},
					raw: "{}",
				},
				{ type: "usage", inputTokens: 150_000, outputTokens: 60_000 },
				{ type: "stop", reason: "tool_use" },
			]),
			tools: registry(runs),
		});
		const chat = newChat();
		expect(await runTurn(chat, "q", d)).toBe("token_budget");
		expect(runs).toEqual([]);
		// The unanswered call is closed so the next turn is valid.
		expect(chat.messages.at(-1)?.content[0]).toMatchObject({
			type: "tool_result",
			toolUseId: "u1",
		});
	});

	it("token_budget_counts_incremental_input", async () => {
		// Each call re-sends a 90k history that grows by 1k: billed input sums past 200k
		// after three calls, but the budget counts 90k + 1k + 1k ... plus outputs.
		const d = deps({
			provider: scripted((_r, n) =>
				n < 5
					? [
							{
								type: "tool_use",
								index: 0,
								id: `u${n}`,
								name: "read_thing",
								input: {},
								raw: "{}",
							},
							{
								type: "usage",
								inputTokens: 90_000 + n * 1_000,
								outputTokens: 100,
							},
							{ type: "stop", reason: "tool_use" },
						]
					: say("done"),
			),
		});
		expect(await runTurn(newChat(), "q", d)).toBe("end_turn");
		expect(d.events.at(-1)).toMatchObject({
			usage: { inputTokens: 460_000 + 10, outputTokens: 505 },
		});
	});

	it("max_tokens_tool_calls_are_not_run", async () => {
		const runs: string[] = [];
		const seen: TurnRequest[] = [];
		const d = deps({
			provider: scripted(
				(_r, n) =>
					n === 0
						? [
								{
									type: "tool_use",
									index: 0,
									id: "u1",
									name: "write_a",
									input: { n: 1 },
									raw: "",
								},
								{ type: "stop", reason: "max_tokens" },
							]
						: say("smaller steps"),
				seen,
			),
			tools: registry(runs),
			askApproval: () => {
				throw new Error("must not ask");
			},
		});
		expect(await runTurn(newChat(), "q", d)).toBe("end_turn");
		expect(runs).toEqual([]);
		expect(seen[1]?.messages.at(-1)?.content).toEqual([
			{ type: "tool_result", toolUseId: "u1", text: CUT_OFF, isError: true },
		]);
	});

	it("fallback_model_is_recorded_and_announced", async () => {
		const d = deps({
			provider: scripted(() => [
				{ type: "model", model: "claude-sonnet-5-5" },
				{ type: "text", index: 0, text: "a" },
				{
					type: "fallback",
					index: 1,
					from: "claude-sonnet-5-5",
					to: "claude-opus-4-8",
				},
				{ type: "text", index: 2, text: "b" },
				{ type: "stop", reason: "end_turn" },
			]),
			model: "claude-sonnet-5-5",
		});
		const chat = newChat();
		await runTurn(chat, "q", d);
		expect(d.events.filter((e) => e.kind === "model")).toEqual([
			{
				kind: "model",
				chatId: chat.id,
				model: "claude-opus-4-8",
				fallbackFrom: "claude-sonnet-5-5",
			},
		]);
		expect(chat.messages.at(-1)).toMatchObject({
			model: "claude-opus-4-8",
			fallbackFrom: "claude-sonnet-5-5",
		});
		// Sticky routing: another model named up front is announced; a longer own id is not.
		const sticky = deps({
			provider: scripted(() => [
				{ type: "model", model: "claude-opus-4-8" },
				...say("x"),
			]),
			model: "claude-sonnet-5-5",
		});
		await runTurn(newChat(), "q", sticky);
		expect(sticky.events.filter((e) => e.kind === "model")).toHaveLength(1);
		const own = deps({
			provider: scripted(() => [
				{ type: "model", model: "llama3.1:latest" },
				...say("x"),
			]),
			model: "llama3.1",
		});
		await runTurn(newChat(), "q", own);
		expect(own.events.filter((e) => e.kind === "model")).toHaveLength(0);
	});

	it("wall_clock_budget_stops", async () => {
		const d = deps({
			provider: scripted(() => toolUse("u1", "slow", {})),
			budgets: { wallClockMs: 30 },
			now: Date.now,
		});
		expect(await runTurn(newChat(), "q", d)).toBe("wall_clock_budget");
	});

	it("cancel_aborts", async () => {
		const ctl = new AbortController();
		let providerSignal: AbortSignal | undefined;
		const d = deps({
			provider: {
				id: "hang",
				async *streamTurn(req) {
					providerSignal = req.signal;
					yield { type: "text", index: 0, text: "partial" };
					await new Promise((_, reject) =>
						req.signal.addEventListener("abort", () =>
							reject(new Error("aborted")),
						),
					);
				},
			},
			signal: ctl.signal,
		});
		const chat = newChat();
		const turn = runTurn(chat, "q", d);
		for (let i = 0; i < 5; i++) await tick();
		ctl.abort();
		expect(await turn).toBe("cancelled");
		expect(providerSignal?.aborted).toBe(true);
		expect(d.events.at(-1)).toMatchObject({ kind: "done", stop: "cancelled" });
		expect(chat.messages.at(-1)).toMatchObject({
			role: "assistant",
			content: [{ type: "text", text: "partial" }],
			stop: "cancelled",
		});
	});

	it("cancel_aborts_pending_tool_and_approval", async () => {
		const ctl = new AbortController();
		const d = deps({
			provider: scripted(() => [
				{
					type: "tool_use",
					index: 0,
					id: "u1",
					name: "write_a",
					input: {},
					raw: "{}",
				},
				{
					type: "tool_use",
					index: 1,
					id: "u2",
					name: "read_thing",
					input: {},
					raw: "{}",
				},
				{ type: "stop", reason: "tool_use" },
			]),
			askApproval: () => new Promise(() => undefined),
			signal: ctl.signal,
		});
		const chat = newChat();
		const turn = runTurn(chat, "q", d);
		for (let i = 0; i < 10; i++) await tick();
		ctl.abort();
		expect(await turn).toBe("cancelled");
		expect(chat.messages.at(-1)?.content).toEqual([
			{ type: "tool_result", toolUseId: "u1", text: CANCELLED, isError: true },
			{ type: "tool_result", toolUseId: "u2", text: CANCELLED, isError: true },
		]);
	});

	it("tool_results_truncated_to_20k", async () => {
		const seen: TurnRequest[] = [];
		const d = deps({
			provider: scripted(
				(_r, n) => (n === 0 ? toolUse("u1", "big", {}) : say("ok")),
				seen,
			),
		});
		await runTurn(newChat(), "q", d);
		const part = seen[1]?.messages.at(-1)?.content[0] as { text: string };
		expect(part.text.length).toBeLessThanOrEqual(MAX_RESULT_CHARS);
		expect(part.text).toMatch(/\[truncated: \d+ more characters\]$/);
	});

	it("invalid_arguments_are_answered_not_run", async () => {
		const runs: string[] = [];
		const d = deps({
			provider: scripted((_r, n) =>
				n === 0
					? [
							{
								type: "tool_use",
								index: 0,
								id: "u1",
								name: "write_a",
								input: { n: "x" },
								raw: "",
							},
							{
								type: "tool_use",
								index: 1,
								id: "u2",
								name: "read_thing",
								input: undefined,
								raw: '{"n":',
							},
							{
								type: "tool_use",
								index: 2,
								id: "u3",
								name: "nope",
								input: {},
								raw: "{}",
							},
							{ type: "stop", reason: "tool_use" },
						]
					: say("ok"),
			),
			tools: registry(runs),
			askApproval: () => {
				throw new Error("must not ask");
			},
		});
		await runTurn(newChat(), "q", d);
		expect(runs).toEqual([]);
		const results = d.events.filter((e) => e.kind === "tool_result");
		expect(results.map((r) => r.kind === "tool_result" && r.ok)).toEqual([
			false,
			false,
			false,
		]);
		expect(d.events.find((e) => e.kind === "tool_call")).toMatchObject({
			needsApproval: false,
		});
	});

	it("llm_error_is_reported_and_redacted", async () => {
		const d = deps({
			provider: {
				id: "x",
				// biome-ignore lint/correctness/useYield: throws before yielding
				async *streamTurn() {
					throw new Error("HTTP 401: bad key sk-secret-123");
				},
			},
			secrets: () => ["sk-secret-123"],
		});
		expect(await runTurn(newChat(), "q", d)).toBe("llm_error");
		expect(d.events).toContainEqual({
			kind: "error",
			chatId: "c_test0001",
			message: "HTTP 401: bad key [redacted]",
		});
	});

	it("fallback_drops_earlier_non_text_blocks", async () => {
		const runs: string[] = [];
		const d = deps({
			provider: scripted((_r, n) =>
				n === 0
					? [
							{ type: "thinking", index: 0, text: "t" },
							{ type: "text", index: 1, text: "partial " },
							{
								type: "tool_use",
								index: 2,
								id: "u1",
								name: "read_thing",
								input: {},
								raw: "{}",
							},
							{ type: "fallback", index: 3 },
							{ type: "text", index: 4, text: "rescued" },
							{ type: "stop", reason: "end_turn" },
						]
					: say("never"),
			),
			tools: registry(runs),
		});
		const chat = newChat();
		await runTurn(chat, "q", d);
		expect(runs).toEqual([]);
		expect(chat.messages.at(-1)?.content).toEqual([
			{ type: "text", text: "partial " },
			{ type: "text", text: "rescued" },
		]);
	});
});

// ---- the service: approvals across chats, keys ----

const xor: VaultCrypto = {
	available: () => true,
	encrypt: (s) => Buffer.from(Buffer.from(s).map((b) => b ^ 0x5a)),
	decrypt: (b) => Buffer.from(Buffer.from(b).map((x) => x ^ 0x5a)).toString(),
};

function service(
	fetchImpl: (url: string, init: RequestInit) => Promise<Response>,
	runs: string[] = [],
) {
	const dir = mkdtempSync(join(tmpdir(), "agent-"));
	const events: ChatEvent[] = [];
	const configs = new ProviderConfigs(
		join(dir, "agent", "providers.json"),
		new Vault(join(dir, "credentials.bin"), xor),
		fetchImpl,
	);
	const store = new ChatStore(join(dir, "chats"));
	const svc = new ChatService({
		store,
		configs,
		tools: registry(runs),
		emit: (e) => events.push(e),
		now: Date.now,
	});
	return { dir, events, svc, configs, store };
}

/** An Anthropic-shaped SSE body for one tool_use (or plain text when tool is undefined). */
function anthropicSse(tool?: { id: string; name: string }): string {
	const ev = (o: unknown) => `data: ${JSON.stringify(o)}\n\n`;
	const head = ev({
		type: "message_start",
		message: { usage: { input_tokens: 5, output_tokens: 1 } },
	});
	if (!tool)
		return (
			head +
			ev({
				type: "content_block_start",
				index: 0,
				content_block: { type: "text", text: "" },
			}) +
			ev({
				type: "content_block_delta",
				index: 0,
				delta: { type: "text_delta", text: "fine" },
			}) +
			ev({ type: "content_block_stop", index: 0 }) +
			ev({
				type: "message_delta",
				delta: { stop_reason: "end_turn" },
				usage: { output_tokens: 3 },
			})
		);
	return (
		head +
		ev({
			type: "content_block_start",
			index: 0,
			content_block: {
				type: "tool_use",
				id: tool.id,
				name: tool.name,
				input: {},
			},
		}) +
		ev({
			type: "content_block_delta",
			index: 0,
			delta: { type: "input_json_delta", partial_json: "{}" },
		}) +
		ev({ type: "content_block_stop", index: 0 }) +
		ev({
			type: "message_delta",
			delta: { stop_reason: "tool_use" },
			usage: { output_tokens: 3 },
		})
	);
}

async function until(cond: () => boolean): Promise<void> {
	for (let i = 0; i < 200 && !cond(); i++)
		await new Promise((r) => setTimeout(r, 2));
	expect(cond()).toBe(true);
}

describe("chat service", () => {
	it("always_allow_scoped_to_chat_and_tool", async () => {
		const runs: string[] = [];
		// Each request answers with a tool_use of the tool named in the user's last text, then text.
		let call = 0;
		const fetchImpl = async (_url: string, init: RequestInit) => {
			const body = JSON.parse(String(init.body)) as {
				messages: {
					role: string;
					content: { type: string; text?: string }[];
				}[];
			};
			const last = body.messages.at(-1);
			const text = last?.content.find((c) => c.type === "text")?.text;
			const isToolResult = last?.content.some((c) => c.type === "tool_result");
			call++;
			return new Response(
				isToolResult || !text
					? anthropicSse()
					: anthropicSse({ id: `u${call}`, name: text }),
				{ status: 200 },
			);
		};
		const { svc, events, configs } = service(fetchImpl, runs);
		expect(
			configs.configure("anthropic", {
				model: "claude-sonnet-5-5",
				apiKey: "sk-ant-xyz",
			}).ok,
		).toBe(true);
		const a = await svc.create({ provider: "anthropic" });
		const b = await svc.create({ provider: "anthropic" });
		if (!a.ok || !b.ok) throw new Error("create failed");

		const waitApproval = async (chatId: string, tool: string) => {
			await until(() =>
				events.some(
					(e) =>
						e.kind === "tool_call" &&
						e.chatId === chatId &&
						e.tool === tool &&
						e.needsApproval,
				),
			);
			const e = events
				.filter((x) => x.kind === "tool_call" && x.chatId === chatId)
				.at(-1);
			return e?.kind === "tool_call" ? e.callId : "";
		};
		const turnDone = (chatId: string, n: number) =>
			until(
				() =>
					events.filter((e) => e.kind === "done" && e.chatId === chatId)
						.length === n,
			);

		// Chat A: "always" for write_a.
		expect((await svc.send(a.value.id, "write_a", {})).ok).toBe(true);
		const id1 = await waitApproval(a.value.id, "write_a");
		const view = await svc.get(a.value.id);
		expect(view.ok && view.value.pending).toHaveLength(1);
		expect(svc.approve(a.value.id, id1, "always").ok).toBe(true);
		await turnDone(a.value.id, 1);

		// Same chat, same tool: runs without asking.
		events.length = 0;
		expect((await svc.send(a.value.id, "write_a", {})).ok).toBe(true);
		await turnDone(a.value.id, 1);
		expect(events.find((e) => e.kind === "tool_call")).toMatchObject({
			tool: "write_a",
			needsApproval: false,
		});

		// Same chat, another write tool: asks.
		events.length = 0;
		await svc.send(a.value.id, "write_b", {});
		const id2 = await waitApproval(a.value.id, "write_b");
		svc.approve(a.value.id, id2, "once");
		await turnDone(a.value.id, 1);

		// Another chat, same tool: asks.
		await svc.send(b.value.id, "write_a", {});
		const id3 = await waitApproval(b.value.id, "write_a");
		svc.approve(b.value.id, id3, "deny");
		await turnDone(b.value.id, 1);

		expect(runs).toEqual(["write_a:{}", "write_a:{}", "write_b:{}"]);
		const stored = await svc.get(a.value.id);
		expect(stored.ok && stored.value.alwaysAllow).toEqual(["write_a"]);
		const storedB = await svc.get(b.value.id);
		expect(storedB.ok && storedB.value.alwaysAllow).toEqual([]);
	});

	it("key_never_in_events_or_store", async () => {
		const KEY = "sk-ant-SECRET-0123456789";
		let n = 0;
		// The server echoes the key back in an error (as a misbehaving proxy might), then in a stream error.
		const fetchImpl = async (_url: string, init: RequestInit) => {
			const key = (init.headers as Record<string, string>)["x-api-key"];
			n++;
			if (n === 1)
				return new Response(
					JSON.stringify({ error: { message: `invalid x-api-key ${key}` } }),
					{ status: 401 },
				);
			return new Response(
				`data: ${JSON.stringify({ type: "error", error: { type: "x", message: `bad ${key} ${encodeURIComponent(key ?? "")}` } })}\n\n`,
				{ status: 200 },
			);
		};
		const { svc, events, dir, configs, store } = service(fetchImpl);
		const r = svc.configureProvider("anthropic", {
			model: "claude-sonnet-5-5",
			apiKey: KEY,
		});
		expect(r.ok).toBe(true);
		expect(JSON.stringify(r)).not.toContain(KEY);
		expect(JSON.stringify(svc.providers())).not.toContain(KEY);
		expect(svc.providers().find((p) => p.id === "anthropic")).toMatchObject({
			hasKey: true,
			configured: true,
		});
		const c = await svc.create({});
		if (!c.ok) throw new Error("create");
		await svc.send(c.value.id, `my key is ${KEY}?`, {});
		await until(() => events.some((e) => e.kind === "done"));
		await svc.send(c.value.id, "again", {});
		await until(() => events.filter((e) => e.kind === "done").length === 2);

		await store.flush();
		const all = JSON.stringify(events);
		expect(all).toContain("[redacted]");
		expect(all).not.toContain(KEY);
		expect(all).not.toContain(encodeURIComponent(KEY));
		for (const f of readdirSync(join(dir, "chats")))
			expect(readFileSync(join(dir, "chats", f), "utf8")).not.toContain(KEY);
		expect(
			readFileSync(join(dir, "agent", "providers.json"), "utf8"),
		).not.toContain(KEY);
		expect(readFileSync(join(dir, "credentials.bin"), "utf8")).not.toContain(
			KEY,
		);
		const view = await svc.get(c.value.id);
		expect(JSON.stringify(view)).not.toContain(KEY);
		expect(configs.secrets()).toEqual([KEY]);
	});

	it("send_validates_and_refuses_unconfigured", async () => {
		const { svc } = service(async () => new Response(""));
		const c = await svc.create({ provider: "deepseek" });
		if (!c.ok) throw new Error("create");
		expect(await svc.send(c.value.id, "hi", {})).toMatchObject({
			ok: false,
			code: "unconfigured",
		});
		expect(await svc.send("../etc", "hi", {})).toMatchObject({
			ok: false,
			code: "not_found",
		});
		expect(await svc.send(c.value.id, "", {})).toMatchObject({ ok: false });
		expect(svc.approve(c.value.id, "x", "maybe")).toMatchObject({ ok: false });
		expect(
			svc.configureProvider("ollama", {
				model: "llama3.1",
				baseUrl: "http://10.0.0.5:11434/v1",
			}),
		).toMatchObject({
			ok: false,
			code: "invalid_url",
		});
		expect(
			svc.configureProvider("ollama", { model: "llama3.1" }),
		).toMatchObject({
			ok: true,
			value: { configured: true, hasKey: false },
		});
	});

	it("key_is_bound_to_its_origin", async () => {
		const KEY = "sk-ant-bound-0123456789";
		const urls: string[] = [];
		const keys: (string | undefined)[] = [];
		const fetchImpl = async (url: string, init: RequestInit) => {
			urls.push(url);
			keys.push((init.headers as Record<string, string>)["x-api-key"]);
			return new Response(anthropicSse(), { status: 200 });
		};
		const { svc, configs, dir } = service(fetchImpl);
		expect(
			configs.configure("anthropic", { model: "m", apiKey: "short" }),
		).toMatchObject({
			ok: false,
			code: "bad_key",
		});
		expect(configs.configure("anthropic", { model: "m", apiKey: KEY }).ok).toBe(
			true,
		);
		// Same origin, other path: the key stays.
		expect(
			configs.configure("anthropic", {
				model: "m",
				baseUrl: "https://api.anthropic.com/",
			}),
		).toMatchObject({ ok: true, value: { hasKey: true } });
		// Redirect attempt: another origin without a new key clears the key.
		expect(
			configs.configure("anthropic", {
				model: "m",
				baseUrl: "https://evil.example",
			}),
		).toMatchObject({ ok: false, code: "key_required" });
		expect(configs.info("anthropic")).toMatchObject({
			hasKey: false,
			configured: false,
		});
		const c = await svc.create({ provider: "anthropic" });
		if (!c.ok) throw new Error("create");
		expect(await svc.send(c.value.id, "hi", {})).toMatchObject({
			code: "unconfigured",
		});
		expect(urls).toEqual([]);

		// A providers.json edited behind the app's back cannot redirect a stored key either.
		expect(
			configs.configure("anthropic", {
				model: "m",
				baseUrl: "https://api.anthropic.com",
				apiKey: KEY,
			}).ok,
		).toBe(true);
		writeFileSync(
			join(dir, "agent", "providers.json"),
			JSON.stringify({
				anthropic: { baseUrl: "https://evil.example", model: "m" },
			}),
		);
		const edited = new ProviderConfigs(
			join(dir, "agent", "providers.json"),
			new Vault(join(dir, "credentials.bin"), xor),
			fetchImpl,
		);
		expect(edited.info("anthropic").hasKey).toBe(false);
		expect("error" in edited.create("anthropic")).toBe(true);
		expect(keys).toEqual([]);
	});

	it("fallback_is_opt_in_per_provider", async () => {
		const bodies: Record<string, unknown>[] = [];
		const fetchImpl = async (_u: string, init: RequestInit) => {
			bodies.push(JSON.parse(String(init.body)));
			return new Response(anthropicSse(), { status: 200 });
		};
		const { svc, configs, events } = service(fetchImpl);
		configs.configure("anthropic", {
			model: "claude-sonnet-5-5",
			apiKey: "sk-ant-0123456789",
		});
		expect(configs.info("anthropic").fallback).toBe(false);
		const c = await svc.create({ provider: "anthropic" });
		if (!c.ok) throw new Error("create");
		await svc.send(c.value.id, "one", {});
		await until(() => events.filter((e) => e.kind === "done").length === 1);
		expect(bodies[0]?.fallbacks).toBeUndefined();
		expect(
			configs.configure("anthropic", {
				model: "claude-sonnet-5-5",
				fallback: true,
			}),
		).toMatchObject({ ok: true, value: { fallback: true, hasKey: true } });
		await svc.send(c.value.id, "two", {});
		await until(() => events.filter((e) => e.kind === "done").length === 2);
		expect(bodies[1]?.fallbacks).toBe("default");
		// The setting survives a reconfigure that does not mention it.
		expect(
			configs.configure("anthropic", { model: "claude-sonnet-5-5" }),
		).toMatchObject({
			value: { fallback: true },
		});
		expect(configs.info("ollama").fallback).toBeUndefined();
	});

	it("cancel_resolves_and_remove_deletes", async () => {
		const fetchImpl = async (_u: string, init: RequestInit) =>
			new Promise<Response>((_, reject) =>
				init.signal?.addEventListener("abort", () =>
					reject(new Error("aborted")),
				),
			);
		const { svc, events } = service(fetchImpl);
		svc.configureProvider("ollama", { model: "llama3.1" });
		const c = await svc.create({ provider: "ollama" });
		if (!c.ok) throw new Error("create");
		expect((await svc.send(c.value.id, "hi", {})).ok).toBe(true);
		expect(await svc.send(c.value.id, "again", {})).toMatchObject({
			ok: false,
			code: "busy",
		});
		await svc.cancel(c.value.id);
		expect(events.at(-1)).toMatchObject({ kind: "done", stop: "cancelled" });
		expect((await svc.list()).map((x) => x.id)).toEqual([c.value.id]);
		expect((await svc.remove(c.value.id)).ok).toBe(true);
		expect(await svc.list()).toEqual([]);
	});
});
