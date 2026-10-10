// The agent loop (D675): stream a provider turn, run the tools it asks for
// (write tools only after the user's approval), feed the results back, and
// stop at end_turn or at a budget. Pure: no electron, everything injected.
import type {
	ChatApproval,
	ChatEvent,
	ChatMessage,
	ChatPart,
	ChatRecord,
	ChatStopReason,
} from "../../shared/contracts";
import { redact } from "../factory/host";
import type { Provider, ProviderStop } from "./providers/types";
import { resultText, type ToolRegistry, truncate } from "./tools";

export interface Budgets {
	iterations: number;
	wallClockMs: number;
	tokens: number;
}
export const BUDGETS: Budgets = {
	iterations: 25,
	wallClockMs: 10 * 60_000,
	tokens: 200_000,
};

export const DENIED = "The user denied this action.";
export const CUT_OFF =
	"Not run: the answer hit the output limit (max_tokens) before this call was complete. Try a smaller step.";
export const CANCELLED = "Not run: the turn was stopped.";

export interface PendingCall {
	callId: string;
	tool: string;
	args: unknown;
	risk: "read" | "write";
}

export interface TurnDeps {
	provider: Provider;
	model: string;
	tools: ToolRegistry;
	system: string;
	emit(e: ChatEvent): void;
	save(chat: ChatRecord): void;
	/** Resolves with the user's decision; it is raced against `signal`. */
	askApproval(call: PendingCall): Promise<ChatApproval>;
	/** Aborted by cancel(). */
	signal: AbortSignal;
	/** Every form of every secret that must not reach an event, the store or an error. */
	secrets(): string[];
	now(): number;
	budgets?: Partial<Budgets>;
}

/** Replaces every secret form inside a JSON-serialisable value. */
export function scrub<T>(value: T, secrets: readonly string[]): T {
	if (secrets.length === 0) return value;
	const json = JSON.stringify(value);
	if (json === undefined) return value;
	const clean = redact(json, secrets);
	return clean === json ? value : (JSON.parse(clean) as T);
}

/**
 * A stopwatch for the wall-clock budget. It is paused while a write call waits for the
 * user's approval (controller ruling, Task 28 fix round 1): user think time does not count
 * against the 10 minutes, so stepping away does not lose the turn. Cancel works at any time.
 */
class Clock {
	#left: number;
	#since = 0;
	#timer: ReturnType<typeof setTimeout> | undefined;
	expired = false;
	constructor(
		ms: number,
		private readonly now: () => number,
		private readonly onExpire: () => void,
	) {
		this.#left = ms;
	}
	start(): void {
		if (this.#timer || this.expired) return;
		this.#since = this.now();
		this.#timer = setTimeout(
			() => {
				this.expired = true;
				this.onExpire();
			},
			Math.max(0, this.#left),
		);
	}
	pause(): void {
		if (!this.#timer) return;
		clearTimeout(this.#timer);
		this.#timer = undefined;
		this.#left -= this.now() - this.#since;
	}
	stop(): void {
		this.pause();
	}
}

const abortable = <T>(p: Promise<T>, signal: AbortSignal): Promise<T> =>
	new Promise<T>((resolve, reject) => {
		if (signal.aborted) return reject(new Error("aborted"));
		const onAbort = () => reject(new Error("aborted"));
		signal.addEventListener("abort", onAbort, { once: true });
		p.then(
			(v) => {
				signal.removeEventListener("abort", onAbort);
				resolve(v);
			},
			(e) => {
				signal.removeEventListener("abort", onAbort);
				reject(e);
			},
		);
	});

/** tool_use ids in the last assistant message that no later message answers. */
function dangling(chat: ChatRecord): string[] {
	const msgs = chat.messages;
	let a = msgs.length - 1;
	while (a >= 0 && msgs[a]?.role !== "assistant") a--;
	if (a < 0) return [];
	const asked = (msgs[a] as ChatMessage).content.flatMap((p) =>
		p.type === "tool_use" ? [p.id] : [],
	);
	const answered = new Set(
		msgs
			.slice(a + 1)
			.flatMap((m) =>
				m.content.flatMap((p) =>
					p.type === "tool_result" ? [p.toolUseId] : [],
				),
			),
	);
	return asked.filter((id) => !answered.has(id));
}

/** Runs one user turn to completion. Resolves with the stop reason; never rejects. */
export async function runTurn(
	chat: ChatRecord,
	userText: string,
	deps: TurnDeps,
): Promise<ChatStopReason> {
	const budgets = { ...BUDGETS, ...deps.budgets };
	const chatId = chat.id;
	const secrets = () => deps.secrets();
	const emit = (e: ChatEvent) => deps.emit(scrub(e, secrets()));
	// Secrets are scrubbed where content enters the chat (the user's text, the model's
	// parts, tool results), so a save never has to scan the whole history.
	const save = () => {
		chat.updatedAt = deps.now();
		deps.save(chat);
	};
	/** Billed totals, reported in `done`. */
	const usage = { inputTokens: 0, outputTokens: 0 };
	/**
	 * The token budget counts what this turn adds: every output token, and each call's
	 * input only by how much it grew over the previous call's (the first call counts in
	 * full). Re-sent history is not counted again on every iteration, so 200k is roughly
	 * "context plus everything generated", not "billed input summed over iterations".
	 */
	let budgetUsed = 0;
	let prevInput = 0;

	const ctl = new AbortController();
	const onCancel = () => ctl.abort();
	if (deps.signal.aborted) ctl.abort();
	else deps.signal.addEventListener("abort", onCancel, { once: true });
	const clock = new Clock(budgets.wallClockMs, deps.now, () => ctl.abort());
	const signal = ctl.signal;

	// The user's message. A turn that stopped after tool results leaves a user
	// message last; the text joins it so roles keep alternating.
	userText = redact(userText, secrets());
	const last = chat.messages[chat.messages.length - 1];
	if (last?.role === "user") {
		last.content.push({ type: "text", text: userText });
		last.at = deps.now();
	} else
		chat.messages.push({
			role: "user",
			content: [{ type: "text", text: userText }],
			at: deps.now(),
		});
	// The stop reason is recorded only on this turn's own last assistant message.
	const turnStart = chat.messages.length - 1;
	if (!chat.title)
		chat.title = userText.replace(/\s+/g, " ").trim().slice(0, 60);
	save();

	const finish = (stop: ChatStopReason, why = CANCELLED): ChatStopReason => {
		clock.stop();
		deps.signal.removeEventListener("abort", onCancel);
		const open: ChatPart[] = dangling(chat).map((id) => ({
			type: "tool_result",
			toolUseId: id,
			text: why,
			isError: true,
		}));
		const tail = chat.messages[chat.messages.length - 1];
		if (open.length > 0 && tail?.role === "user") tail.content.push(...open);
		else if (open.length > 0)
			chat.messages.push({ role: "user", content: open, at: deps.now() });
		for (let i = chat.messages.length - 1; i > turnStart; i--) {
			const m = chat.messages[i] as ChatMessage;
			if (m.role === "assistant") {
				m.stop = stop;
				break;
			}
		}
		save();
		emit({ kind: "done", chatId, stop, usage: { ...usage } });
		return stop;
	};
	const stopOnAbort = (): ChatStopReason =>
		finish(clock.expired ? "wall_clock_budget" : "cancelled");

	clock.start();
	for (let iteration = 0; ; iteration++) {
		if (signal.aborted) return stopOnAbort();
		if (iteration >= budgets.iterations) return finish("iteration_cap");
		if (budgetUsed >= budgets.tokens) return finish("token_budget");

		// ---- one provider call ----
		let served: string | undefined;
		let fallbackFrom: string | undefined;
		const blocks = new Map<number, ChatPart>();
		let callUsage = { inputTokens: 0, outputTokens: 0 };
		let stop: ProviderStop = "end_turn";
		let failed: string | undefined;
		try {
			for await (const ev of deps.provider.streamTurn({
				model: deps.model,
				system: deps.system,
				messages: chat.messages.map((m) => ({
					role: m.role,
					content: m.content,
				})),
				tools: deps.tools.list().map((t) => ({
					name: t.name,
					description: t.description,
					schema: t.schema,
				})),
				signal,
			})) {
				if (signal.aborted) break;
				switch (ev.type) {
					case "text": {
						const b = blocks.get(ev.index);
						if (b?.type === "text") b.text += ev.text;
						else blocks.set(ev.index, { type: "text", text: ev.text });
						if (ev.text) emit({ kind: "delta", chatId, text: ev.text });
						break;
					}
					case "thinking": {
						const b = blocks.get(ev.index);
						if (b?.type === "thinking") b.text += ev.text;
						else blocks.set(ev.index, { type: "thinking", text: ev.text });
						if (ev.text) emit({ kind: "thinking", chatId, text: ev.text });
						break;
					}
					case "signature": {
						const b = blocks.get(ev.index);
						if (b?.type === "thinking")
							b.signature = (b.signature ?? "") + ev.signature;
						break;
					}
					case "redacted_thinking":
						blocks.set(ev.index, { type: "redacted_thinking", data: ev.data });
						break;
					case "tool_use":
						blocks.set(ev.index, {
							type: "tool_use",
							id: ev.id,
							name: ev.name,
							// Invalid streamed JSON is kept as the raw text so the call can be answered.
							input:
								ev.input === undefined ? { _invalid_json: ev.raw } : ev.input,
						});
						break;
					case "fallback":
						// Echo rules after a server-side fallback: drop the non-text blocks before it.
						for (const [i, b] of blocks)
							if (i < ev.index && b.type !== "text") blocks.delete(i);
						fallbackFrom = ev.from ?? served ?? deps.model;
						if (ev.to) served = ev.to;
						if (served)
							emit({ kind: "model", chatId, model: served, fallbackFrom });
						break;
					case "model":
						served = ev.model;
						// Sticky routing after an earlier fallback names another model up front.
						// A provider's own longer id ("llama3.1:latest", a dated id) is not a switch.
						if (!ev.model.startsWith(deps.model))
							emit({ kind: "model", chatId, model: ev.model });
						break;
					case "usage":
						callUsage = {
							inputTokens: ev.inputTokens,
							outputTokens: ev.outputTokens,
						};
						break;
					case "stop":
						stop = ev.reason;
						break;
				}
			}
		} catch (e) {
			if (!signal.aborted)
				failed = redact(
					e instanceof Error ? e.message : String(e),
					secrets(),
				).slice(0, 1000);
		}
		usage.inputTokens += callUsage.inputTokens;
		usage.outputTokens += callUsage.outputTokens;
		budgetUsed +=
			callUsage.outputTokens + Math.max(0, callUsage.inputTokens - prevInput);
		if (callUsage.inputTokens > 0) prevInput = callUsage.inputTokens;

		const parts = scrub(
			[...blocks.entries()]
				.sort((a, b) => a[0] - b[0])
				.map(([, p]) => p)
				// A text block that never got text adds nothing.
				.filter((p) => p.type !== "text" || p.text.length > 0),
			secrets(),
		);
		if (parts.length > 0) {
			chat.messages.push({
				role: "assistant",
				content: parts,
				at: deps.now(),
				...(served ? { model: served } : {}),
				...(fallbackFrom ? { fallbackFrom } : {}),
			});
			save();
		}
		if (signal.aborted) return stopOnAbort();
		if (failed !== undefined) {
			emit({ kind: "error", chatId, message: failed });
			return finish("llm_error", "Not run: the model call failed.");
		}
		if (stop === "refusal") {
			emit({
				kind: "error",
				chatId,
				message: "The model declined this request.",
			});
			return finish("llm_error", "Not run: the model declined.");
		}

		const calls = parts.flatMap((p) => (p.type === "tool_use" ? [p] : []));
		if (calls.length === 0) return finish("end_turn");
		if (budgetUsed >= budgets.tokens)
			return finish("token_budget", "Not run: the token budget is used up.");
		if (stop === "max_tokens") {
			// The calls may be cut short: answer them without running anything and let the
			// model try again in smaller steps.
			const cut: ChatPart[] = calls.map((c) => ({
				type: "tool_result",
				toolUseId: c.id,
				text: CUT_OFF,
				isError: true,
			}));
			for (const c of calls) {
				emit({
					kind: "tool_call",
					chatId,
					callId: c.id,
					tool: c.name,
					args: c.input,
					risk: deps.tools.get(c.name)?.risk ?? "read",
					needsApproval: false,
				});
				emit({
					kind: "tool_result",
					chatId,
					callId: c.id,
					ok: false,
					text: CUT_OFF,
				});
			}
			chat.messages.push({ role: "user", content: cut, at: deps.now() });
			save();
			continue;
		}

		// ---- the tools, in order ----
		const results: ChatPart[] = [];
		for (const call of calls) {
			if (signal.aborted) break;
			const def = deps.tools.get(call.name);
			const risk = def?.risk ?? "read";
			const answer = (ok: boolean, text: string) => {
				const t = truncate(redact(text, secrets()));
				results.push({
					type: "tool_result",
					toolUseId: call.id,
					text: t,
					...(ok ? {} : { isError: true }),
				});
				emit({ kind: "tool_result", chatId, callId: call.id, ok, text: t });
			};
			const invalid = !def
				? undefined
				: call.input &&
						typeof call.input === "object" &&
						"_invalid_json" in call.input
					? "the arguments were not valid JSON"
					: deps.tools.check(call.name, call.input);
			const needsApproval =
				def !== undefined &&
				!invalid &&
				risk === "write" &&
				!chat.alwaysAllow.includes(call.name);
			emit({
				kind: "tool_call",
				chatId,
				callId: call.id,
				tool: call.name,
				args: call.input,
				risk,
				needsApproval,
			});
			if (!def) {
				answer(false, `Unknown tool ${call.name}.`);
				continue;
			}
			if (invalid) {
				answer(false, `Invalid arguments: ${invalid}.`);
				continue;
			}
			if (needsApproval) {
				clock.pause();
				let decision: ChatApproval;
				try {
					decision = await abortable(
						deps.askApproval({
							callId: call.id,
							tool: call.name,
							args: call.input,
							risk,
						}),
						signal,
					);
				} catch {
					break;
				}
				clock.start();
				if (decision === "deny") {
					answer(false, DENIED);
					continue;
				}
				if (decision === "always" && !chat.alwaysAllow.includes(call.name)) {
					chat.alwaysAllow.push(call.name);
					save();
				}
			}
			try {
				const value = await abortable(
					def.run({ signal, chatId }, call.input as Record<string, unknown>),
					signal,
				);
				if (signal.aborted) break;
				answer(true, resultText(value));
			} catch (e) {
				if (signal.aborted) break;
				answer(false, `Error: ${e instanceof Error ? e.message : String(e)}`);
			}
		}
		if (results.length > 0) {
			chat.messages.push({ role: "user", content: results, at: deps.now() });
			save();
		}
		if (signal.aborted) return stopOnAbort();
	}
}
