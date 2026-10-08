// The agent tools whose services live in the desktop today (D675): the data
// plane (collections, search, SQL, streams, links), durable promises, the
// connector catalog and the factory's read-only ops. Each calls the same
// main-process service the pages use; nothing goes through the renderer.
import { randomUUID } from "node:crypto";
import type {
	ConnectorSummary,
	FactoryAppId,
	FactoryQuery,
	IpcResult,
} from "../../shared/contracts";
import { assertReadOnlySql } from "./sql-guard";
import type { ToolContext, ToolDef } from "./tools";

export interface BuiltinServices {
	/**
	 * A request to the console origin (`loams-app://console<path>`): the protocol
	 * handler routes it to the active server, or `/durable/` to the local engine's
	 * durable listener, exactly as for the pages.
	 */
	api(path: string, init: RequestInit): Promise<Response>;
	connectors(): ConnectorSummary[];
	factoryQuery(q: FactoryQuery): Promise<IpcResult<unknown>>;
	/** app -> its allowlisted read-only op names. */
	factoryOps: Partial<Record<FactoryAppId, string[]>>;
	now(): number;
}

/** Durable envelope version (same as the Durable page, plugins/durable/src/envelope.ts). */
export const ENVELOPE_VERSION = "2026-04-01";
const REQUEST_TIMEOUT_MS = 30_000;
const SQL_ROWS = 200;

const ns = {
	type: "string",
	minLength: 1,
	maxLength: 256,
	// Letters, digits, ".", "_" and "-", and not "." or ".." (or any dot-only name).
	pattern: "^(?!\\.+$)[A-Za-z0-9._-]+$",
	description: "The namespace (the active namespace from the context, if any).",
} as const;

function timed(signal: AbortSignal): AbortSignal {
	return AbortSignal.any([signal, AbortSignal.timeout(REQUEST_TIMEOUT_MS)]);
}

async function jsonOf(res: Response): Promise<Record<string, unknown>> {
	const text = await res.text();
	let body: Record<string, unknown> = {};
	try {
		body = text ? (JSON.parse(text) as Record<string, unknown>) : {};
	} catch {
		if (!res.ok) throw new Error(`HTTP ${res.status}: ${text.slice(0, 300)}`);
		throw new Error(`HTTP ${res.status}: the answer is not JSON`);
	}
	if (!res.ok) {
		const msg = body.message ?? body.error ?? res.statusText;
		throw new Error(`HTTP ${res.status}: ${String(msg).slice(0, 500)}`);
	}
	return body;
}

function nsPath(namespace: unknown, rest: string): string {
	return `/v1/namespaces/${encodeURIComponent(String(namespace))}${rest}`;
}

/** Base64 payload -> its text when it is UTF-8 (pretty JSON when JSON). */
function decodeValue(v: unknown): unknown {
	const data = (v as { data?: unknown } | undefined)?.data;
	if (typeof data !== "string" || !data) return null;
	try {
		const text = new TextDecoder("utf-8", { fatal: true }).decode(
			Buffer.from(data, "base64"),
		);
		try {
			return JSON.parse(text);
		} catch {
			return text;
		}
	} catch {
		return { base64: data };
	}
}

export function builtinTools(s: BuiltinServices): ToolDef[] {
	const get = async (ctx: ToolContext, path: string) =>
		jsonOf(await s.api(path, { method: "GET", signal: timed(ctx.signal) }));
	const post = async (ctx: ToolContext, path: string, body: unknown) =>
		jsonOf(
			await s.api(path, {
				method: "POST",
				headers: { "content-type": "application/json" },
				body: JSON.stringify(body),
				signal: timed(ctx.signal),
			}),
		);
	const durable = async (ctx: ToolContext, kind: string, data: unknown) => {
		const res = await s.api("/durable/", {
			method: "POST",
			headers: { "content-type": "application/json" },
			body: JSON.stringify({
				kind,
				head: { corrId: randomUUID(), version: ENVELOPE_VERSION },
				data,
			}),
			signal: timed(ctx.signal),
		});
		let json: Record<string, unknown>;
		try {
			json = (await res.json()) as Record<string, unknown>;
		} catch {
			throw new Error(`durable: unexpected answer (HTTP ${res.status})`);
		}
		const head = json.head as { status?: number } | undefined;
		// The proxy's own refusals (remote server, engine not ready) are {code, message}.
		if (!head)
			throw new Error(
				`durable: ${String(json.message ?? `HTTP ${res.status}`)}`,
			);
		const status = head.status ?? res.status;
		if (status >= 400)
			throw new Error(
				`durable (${status}): ${typeof json.data === "string" ? json.data : JSON.stringify(json.data)}`,
			);
		return (json.data ?? {}) as Record<string, unknown>;
	};
	const promiseView = (p: Record<string, unknown>) => ({
		id: p.id,
		state: p.state,
		tags: p.tags,
		param: decodeValue(p.param),
		value: decodeValue(p.value),
		createdAt: p.createdAt,
		timeoutAt: p.timeoutAt,
		...(p.settledAt ? { settledAt: p.settledAt } : {}),
	});
	const apps = Object.keys(s.factoryOps) as FactoryAppId[];
	const opsText = apps
		.map((a) => `${a}: ${(s.factoryOps[a] ?? []).join(", ")}`)
		.join("; ");

	return [
		{
			name: "collections_list",
			description:
				"List the collections in a namespace of the active Loams server, with their schemas.",
			risk: "read",
			schema: {
				type: "object",
				properties: { namespace: ns },
				required: ["namespace"],
				additionalProperties: false,
			},
			run: async (ctx, a) => {
				const body = await get(ctx, nsPath(a.namespace, "/collections"));
				return body.collections ?? body;
			},
		},
		{
			name: "search",
			description:
				'Search a collection (POST /v1/namespaces/{ns}/query). `query` is the native query body, e.g. {"from": "docs", "retrieve": [{"text": {"field": "body", "query": "refund policy", "k": 10}}], "limit": 10}. Retrievers: {"text": {field, query, k, operator?}} and {"vector": {field, query: number[], k}}; optional "filter", "fuse", "select", "offset".',
			risk: "read",
			schema: {
				type: "object",
				properties: {
					namespace: ns,
					query: { type: "object", description: "The query body." },
				},
				required: ["namespace", "query"],
				additionalProperties: false,
			},
			run: async (ctx, a) => post(ctx, nsPath(a.namespace, "/query"), a.query),
		},
		{
			name: "sql_query",
			description: `Run one read-only SQL statement (SELECT, WITH, EXPLAIN, SHOW) over the collections of a namespace. Writes are refused. At most ${SQL_ROWS} rows are returned.`,
			risk: "read",
			schema: {
				type: "object",
				properties: {
					namespace: ns,
					sql: { type: "string", minLength: 1, maxLength: 20_000 },
				},
				required: ["namespace", "sql"],
				additionalProperties: false,
			},
			run: async (ctx, a) => {
				const sql = String(a.sql);
				assertReadOnlySql(sql);
				const r = (await post(ctx, nsPath(a.namespace, "/sql"), {
					query: sql,
				})) as { columns?: unknown[]; rows?: unknown[][]; truncated?: boolean };
				const rows = Array.isArray(r.rows) ? r.rows : [];
				const cols = (Array.isArray(r.columns) ? r.columns : []).map((c) =>
					typeof c === "object" && c !== null && "name" in c
						? String((c as { name: unknown }).name)
						: String(c),
				);
				const lines = [
					`columns: ${JSON.stringify(cols)}`,
					...rows.slice(0, SQL_ROWS).map((row) => JSON.stringify(row)),
				];
				if (rows.length > SQL_ROWS || r.truncated)
					lines.push(
						`[more rows not shown: ${rows.length > SQL_ROWS ? `${rows.length - SQL_ROWS} cut here` : "the server truncated the result"}]`,
					);
				lines.push(`(${Math.min(rows.length, SQL_ROWS)} rows)`);
				return lines.join("\n");
			},
		},
		{
			name: "durable_promises_search",
			description:
				"Search durable promises on the local engine (Resonate). Filter by state and tags; params and values are decoded.",
			risk: "read",
			schema: {
				type: "object",
				properties: {
					state: {
						type: "string",
						enum: [
							"pending",
							"resolved",
							"rejected",
							"rejected_canceled",
							"rejected_timedout",
						],
					},
					tags: {
						type: "object",
						additionalProperties: { type: "string" },
					},
					limit: { type: "integer", minimum: 1, maximum: 100 },
					cursor: { type: "string" },
				},
				additionalProperties: false,
			},
			run: async (ctx, a) => {
				const d = await durable(ctx, "promise.search", {
					...(a.state ? { state: a.state } : {}),
					...(a.tags ? { tags: a.tags } : {}),
					limit: a.limit ?? 20,
					...(a.cursor ? { cursor: a.cursor } : {}),
				});
				const items = Array.isArray(d.promises)
					? (d.promises as Record<string, unknown>[])
					: [];
				return {
					promises: items.map(promiseView),
					...(d.cursor ? { cursor: d.cursor } : {}),
				};
			},
		},
		{
			name: "durable_promise_create",
			description:
				"Create a durable promise on the local engine. `param` is any JSON value (stored as base64 JSON). Needs the user's approval.",
			risk: "write",
			schema: {
				type: "object",
				properties: {
					id: { type: "string", minLength: 1, maxLength: 512 },
					timeoutMs: {
						type: "integer",
						minimum: 1000,
						maximum: 31 * 24 * 3_600_000,
						description: "How long until it times out (default one hour).",
					},
					param: { description: "Any JSON value." },
					tags: { type: "object", additionalProperties: { type: "string" } },
				},
				required: ["id"],
				additionalProperties: false,
			},
			run: async (ctx, a) => {
				const param =
					a.param === undefined
						? { headers: {}, data: "" }
						: {
								headers: {},
								data: Buffer.from(JSON.stringify(a.param)).toString("base64"),
							};
				const d = await durable(ctx, "promise.create", {
					id: a.id,
					timeoutAt:
						s.now() +
						(typeof a.timeoutMs === "number" ? a.timeoutMs : 3_600_000),
					param,
					tags: a.tags ?? {},
				});
				const p = d.promise as Record<string, unknown> | undefined;
				return p ? promiseView(p) : d;
			},
		},
		{
			name: "streams_list",
			description: "List the streams in a namespace (partitions, retention).",
			risk: "read",
			schema: {
				type: "object",
				properties: { namespace: ns },
				required: ["namespace"],
				additionalProperties: false,
			},
			run: async (ctx, a) => {
				const b = await get(ctx, nsPath(a.namespace, "/streams"));
				return b.streams ?? b;
			},
		},
		{
			name: "links_list",
			description:
				"List the links in a namespace (source stream, target, status, lag).",
			risk: "read",
			schema: {
				type: "object",
				properties: { namespace: ns },
				required: ["namespace"],
				additionalProperties: false,
			},
			run: async (ctx, a) => {
				const b = await get(ctx, nsPath(a.namespace, "/links"));
				return b.links ?? b;
			},
		},
		{
			name: "connectors_search",
			description:
				"Search the bundled connector catalog by text, category and direction (source or sink).",
			risk: "read",
			schema: {
				type: "object",
				properties: {
					query: { type: "string", maxLength: 200 },
					category: { type: "string", maxLength: 100 },
					direction: { type: "string", enum: ["source", "sink"] },
					limit: { type: "integer", minimum: 1, maximum: 100 },
				},
				additionalProperties: false,
			},
			run: async (_ctx, a) => {
				const q =
					typeof a.query === "string" ? a.query.toLowerCase().trim() : "";
				const cat =
					typeof a.category === "string" ? a.category.toLowerCase() : "";
				const hits = s.connectors().filter((c) => {
					if (cat && c.category.toLowerCase() !== cat) return false;
					if (a.direction === "source" && !c.source) return false;
					if (a.direction === "sink" && !c.sink) return false;
					if (!q) return true;
					return `${c.id} ${c.name} ${c.category}`.toLowerCase().includes(q);
				});
				const limit = typeof a.limit === "number" ? a.limit : 25;
				return {
					total: hits.length,
					connectors: hits.slice(0, limit).map((c) => ({
						id: c.id,
						name: c.name,
						category: c.category,
						status: c.status,
						source: c.source,
						sink: c.sink,
						auth: c.auth,
						...(c.stub ? { stub: true } : {}),
					})),
				};
			},
		},
		{
			name: "factory_query",
			description: `Run a read-only operation on a configured Loams Software Factory app. Operations by app: ${opsText || "none configured"}. \`params\` are the op's parameters (e.g. {"limit": 10}).`,
			risk: "read",
			schema: {
				type: "object",
				properties: {
					app: { type: "string", enum: apps.length > 0 ? apps : ["forgejo"] },
					op: { type: "string", minLength: 1, maxLength: 64 },
					params: { type: "object" },
				},
				required: ["app", "op"],
				additionalProperties: false,
			},
			run: async (_ctx, a) => {
				const r = await s.factoryQuery({
					app: a.app as FactoryAppId,
					op: String(a.op),
					params: (a.params as Record<string, unknown> | undefined) ?? {},
				});
				if (!r.ok) throw new Error(`${r.code}: ${r.message}`);
				return r.value;
			},
		},
	];
}
