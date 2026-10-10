// The agent's tool registry (D675). A tool is framework-free: a name, a
// description, a risk, a JSON Schema for its arguments and a `run`. Each task
// that owns a service registers its own tools (Task 22: pg_sql, wesql_sql,
// pg_branch_create; Task 24: live_mutate), closing over that service.
import Ajv2020 from "ajv/dist/2020.js";

export type ToolRisk = "read" | "write";

export interface ToolContext {
	/** Aborted when the turn is cancelled or runs out of time. */
	signal: AbortSignal;
	chatId: string;
}

export interface ToolDef {
	/** `^[a-z][a-z0-9_]{0,63}$`, unique. */
	name: string;
	/** What the model reads to decide when to call it. */
	description: string;
	/** `write` calls wait for the user's approval in the panel. */
	risk: ToolRisk;
	/** JSON Schema (2020-12) of the arguments; the root is an object. */
	schema: Record<string, unknown>;
	/** Returns text, or a value that is sent as pretty JSON. Throw to report an error. */
	run(ctx: ToolContext, args: Record<string, unknown>): Promise<unknown>;
}

/** Tool results are cut to this many characters before they reach the model. */
export const MAX_RESULT_CHARS = 20_000;

const NAME = /^[a-z][a-z0-9_]{0,63}$/;

const ajv = new Ajv2020({
	allErrors: true,
	strict: false,
	validateFormats: false,
});

export class ToolRegistry {
	readonly #tools = new Map<
		string,
		{ def: ToolDef; validate: (v: unknown) => string | undefined }
	>();

	/** Adds tools; throws on a bad or duplicate name, or a schema that does not compile. */
	register(defs: readonly ToolDef[]): void {
		for (const def of defs) {
			if (!NAME.test(def.name)) throw new Error(`bad tool name: ${def.name}`);
			if (this.#tools.has(def.name))
				throw new Error(`duplicate tool: ${def.name}`);
			if (def.risk !== "read" && def.risk !== "write")
				throw new Error(`bad risk for ${def.name}`);
			const fn = ajv.compile(def.schema);
			const validate = (v: unknown): string | undefined =>
				fn(v)
					? undefined
					: (fn.errors ?? [])
							.map(
								(e) => `${e.instancePath || "args"} ${e.message ?? "invalid"}`,
							)
							.join("; ");
			this.#tools.set(def.name, { def, validate });
		}
	}

	get(name: string): ToolDef | undefined {
		return this.#tools.get(name)?.def;
	}

	/** Undefined when `args` is valid for the tool. */
	check(name: string, args: unknown): string | undefined {
		const t = this.#tools.get(name);
		if (!t) return `unknown tool ${name}`;
		return t.validate(args);
	}

	list(): ToolDef[] {
		return [...this.#tools.values()].map((t) => t.def);
	}
}

/** The process-wide registry the chat service uses. */
export const TOOLS = new ToolRegistry();

export function registerTools(defs: readonly ToolDef[]): void {
	TOOLS.register(defs);
}

/** A tool's return value as model-facing text, cut to `max` characters. */
export function resultText(value: unknown, max = MAX_RESULT_CHARS): string {
	let text: string;
	if (typeof value === "string") text = value;
	else if (value === undefined) text = "OK";
	else {
		try {
			text = JSON.stringify(value, null, 2) ?? String(value);
		} catch {
			text = String(value);
		}
	}
	return truncate(text, max);
}

export function truncate(text: string, max = MAX_RESULT_CHARS): string {
	if (text.length <= max) return text;
	const note = `\n[truncated: ${text.length - max} more characters]`;
	return text.slice(0, Math.max(0, max - note.length)) + note;
}
