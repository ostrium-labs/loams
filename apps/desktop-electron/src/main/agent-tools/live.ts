// Agent tools for Loams Live: pure definitions, no electron, no dependency on
// the agent loop. A tool registry takes `liveTools`, passes `{ fetch, liveUrl }`
// and gets JSON back. The calls are Connect unary over JSON on the engine's
// Live listener (`<liveUrl>/loams.live.v1.LiveService/<Rpc>`).

export type Json =
	| null
	| boolean
	| number
	| string
	| Json[]
	| { [k: string]: Json };

export interface LiveToolContext {
	fetch: typeof fetch;
	/** The engine's Live listener, e.g. http://127.0.0.1:8081 (no trailing slash needed). */
	liveUrl: string;
}

export interface LiveTool {
	name: string;
	description: string;
	/** `write` tools change data and need the user's approval. */
	access: "read" | "write";
	/** JSON Schema of the input object. */
	inputSchema: Record<string, unknown>;
	run(input: unknown, ctx: LiveToolContext): Promise<Json>;
}

/** A plain JSON value as a proto3-JSON `loams.live.v1.Value`. */
export function toValueJson(x: unknown): Json {
	if (x === null || x === undefined) return { nullValue: {} };
	if (typeof x === "boolean") return { boolValue: x };
	if (typeof x === "string") return { stringValue: x };
	if (typeof x === "number")
		return Number.isSafeInteger(x)
			? { int64Value: String(x) }
			: { doubleValue: x };
	if (Array.isArray(x)) return { arrayValue: { values: x.map(toValueJson) } };
	if (typeof x === "object")
		return {
			objectValue: {
				fields: Object.fromEntries(
					Object.entries(x).map(([k, v]) => [k, toValueJson(v)]),
				),
			},
		};
	throw new Error(`cannot encode a ${typeof x} as a Live value`);
}

/** The inverse of `toValueJson`. An int64 outside the safe range stays a string. */
export function fromValueJson(v: unknown): Json {
	if (!v || typeof v !== "object") return null;
	const o = v as Record<string, unknown>;
	if ("int64Value" in o) {
		const n = Number(o.int64Value);
		return Number.isSafeInteger(n) ? n : String(o.int64Value);
	}
	for (const k of ["doubleValue", "boolValue", "stringValue"])
		if (k in o) return o[k] as Json;
	if ("bytesValue" in o) return { $bytes: String(o.bytesValue) };
	const arr = (o.arrayValue as { values?: unknown[] } | undefined)?.values;
	if ("arrayValue" in o) return (arr ?? []).map(fromValueJson);
	const obj = (
		o.objectValue as { fields?: Record<string, unknown> } | undefined
	)?.fields;
	if ("objectValue" in o)
		return Object.fromEntries(
			Object.entries(obj ?? {}).map(([k, x]) => [k, fromValueJson(x)]),
		);
	return null;
}

async function call(
	ctx: LiveToolContext,
	rpc: "Query" | "Mutate",
	body: Json,
): Promise<{ result: Json; ts: string }> {
	const base = ctx.liveUrl.replace(/\/+$/, "");
	const res = await ctx.fetch(`${base}/loams.live.v1.LiveService/${rpc}`, {
		method: "POST",
		headers: {
			"content-type": "application/json",
			"connect-protocol-version": "1",
		},
		body: JSON.stringify(body),
	});
	const text = await res.text();
	let json: Record<string, unknown> = {};
	try {
		json = text ? (JSON.parse(text) as Record<string, unknown>) : {};
	} catch {
		// not JSON: reported below by status
	}
	if (!res.ok) {
		const msg =
			typeof json.message === "string" ? json.message : text.slice(0, 200);
		throw new Error(`Live ${rpc} failed (${res.status}): ${msg}`);
	}
	return {
		result: fromValueJson(json.result),
		ts: String(json.ts ?? json.commitTs ?? ""),
	};
}

const obj = (input: unknown): Record<string, unknown> => {
	if (!input || typeof input !== "object" || Array.isArray(input))
		throw new Error("the input is an object");
	return input as Record<string, unknown>;
};
const str = (o: Record<string, unknown>, k: string): string => {
	const v = o[k];
	if (typeof v !== "string" || !v)
		throw new Error(`'${k}' is a non-empty string`);
	return v;
};

export const MUTATIONS = ["insert", "patch", "delete"] as const;

export const liveTools: LiveTool[] = [
	{
		name: "live_tables",
		description:
			"List the Loams Live tables and their indexes (name, id, indexes with their fields).",
		access: "read",
		inputSchema: {
			type: "object",
			properties: {},
			additionalProperties: false,
		},
		async run(_input, ctx) {
			const r = await call(ctx, "Query", {
				function: "_system:tables",
				args: toValueJson({}),
			});
			return r.result;
		},
	},
	{
		name: "live_query",
		description:
			"Read documents of a Loams Live table, optionally through an index with an equality prefix (`eq`), newest or oldest first.",
		access: "read",
		inputSchema: {
			type: "object",
			properties: {
				table: { type: "string" },
				index: { type: "string", description: "Defaults to by_creation_time." },
				eq: {
					type: "array",
					description: "Values for the index's leading fields.",
				},
				order: { type: "string", enum: ["asc", "desc"] },
				limit: { type: "integer", minimum: 1, maximum: 1000 },
			},
			required: ["table"],
			additionalProperties: false,
		},
		async run(input, ctx) {
			const o = obj(input);
			const args: Record<string, unknown> = { table: str(o, "table") };
			for (const k of ["index", "eq", "order", "limit"])
				if (o[k] !== undefined) args[k] = o[k];
			const r = await call(ctx, "Query", {
				function: "_system:query",
				args: toValueJson(args),
			});
			return { ts: r.ts, documents: r.result };
		},
	},
	{
		name: "live_mutate",
		description:
			"Insert, patch or delete one Loams Live document. Always sent with an idempotency key, so a retry does not apply twice. Changes data: needs approval.",
		access: "write",
		inputSchema: {
			type: "object",
			properties: {
				action: { type: "string", enum: [...MUTATIONS] },
				table: { type: "string", description: "For insert." },
				id: { type: "string", description: "For patch and delete." },
				fields: { type: "object", description: "For insert and patch." },
				idempotencyKey: {
					type: "string",
					description:
						"Optional; generated when absent. Reuse it to retry safely.",
				},
			},
			required: ["action"],
			additionalProperties: false,
		},
		async run(input, ctx) {
			const o = obj(input);
			const action = str(o, "action");
			const fields = o.fields ?? {};
			if (
				typeof fields !== "object" ||
				Array.isArray(fields) ||
				fields === null
			)
				throw new Error("'fields' is an object");
			let args: Record<string, unknown>;
			if (action === "insert") args = { table: str(o, "table"), fields };
			else if (action === "patch") args = { id: str(o, "id"), fields };
			else if (action === "delete") args = { id: str(o, "id") };
			else throw new Error(`'action' is one of ${MUTATIONS.join(", ")}`);
			const key =
				typeof o.idempotencyKey === "string" && o.idempotencyKey
					? o.idempotencyKey
					: crypto.randomUUID();
			const r = await call(ctx, "Mutate", {
				function: `_system:${action}`,
				args: toValueJson(args),
				idempotencyKey: key,
			});
			return { commitTs: r.ts, result: r.result, idempotencyKey: key };
		},
	},
];
