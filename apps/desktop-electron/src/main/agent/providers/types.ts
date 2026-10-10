// Provider-neutral shapes for the agent loop (D675). No electron, no SDK.
import type { ChatMessage, ChatPart } from "../../../shared/contracts";

export type Msg = Pick<ChatMessage, "role" | "content">;
export type Part = ChatPart;

/** What a provider sees of a tool. */
export interface ProviderTool {
	name: string;
	description: string;
	schema: Record<string, unknown>;
}

export type ProviderStop =
	| "end_turn"
	| "tool_use"
	| "max_tokens"
	| "refusal"
	| "other";

/**
 * A streamed event. `index` orders the blocks of one assistant message, so the
 * loop can rebuild it exactly (thinking blocks are echoed back unchanged).
 */
export type ProviderEvent =
	| { type: "text"; index: number; text: string }
	| { type: "thinking"; index: number; text: string }
	| { type: "signature"; index: number; signature: string }
	| { type: "redacted_thinking"; index: number; data: string }
	/** `input` is undefined when the streamed arguments were not valid JSON. */
	| {
			type: "tool_use";
			index: number;
			id: string;
			name: string;
			input: unknown;
			raw: string;
	  }
	/** A server-side model fallback: blocks before it that are not text are discarded. */
	| { type: "fallback"; index: number; from?: string; to?: string }
	/** The model serving this call, as the provider reports it. */
	| { type: "model"; model: string }
	/** Totals for this call so far (each usage event replaces the previous one). */
	| { type: "usage"; inputTokens: number; outputTokens: number }
	| { type: "stop"; reason: ProviderStop };

export interface TurnRequest {
	model: string;
	system: string;
	messages: Msg[];
	tools: ProviderTool[];
	signal: AbortSignal;
}

export interface Provider {
	id: string;
	streamTurn(req: TurnRequest): AsyncIterable<ProviderEvent>;
}

/** An HTTP or stream error from a provider. Its message may quote the server; the loop redacts it. */
export class ProviderError extends Error {
	constructor(
		readonly status: number,
		message: string,
	) {
		super(message);
		this.name = "ProviderError";
	}
}

export type FetchLike = (url: string, init: RequestInit) => Promise<Response>;

/** Reads an error response into a ProviderError (body capped, JSON `error.message` preferred). */
export async function httpError(res: Response): Promise<ProviderError> {
	let text = "";
	try {
		text = (await res.text()).slice(0, 2000);
	} catch {
		// keep the status only
	}
	let message = text;
	try {
		const j = JSON.parse(text) as {
			error?: { message?: unknown } | string;
			message?: unknown;
		};
		const m =
			typeof j.error === "string"
				? j.error
				: typeof j.error?.message === "string"
					? j.error.message
					: typeof j.message === "string"
						? j.message
						: undefined;
		if (m) message = m;
	} catch {
		// not JSON
	}
	return new ProviderError(
		res.status,
		`HTTP ${res.status}${message ? `: ${message.slice(0, 500)}` : ""}`,
	);
}
