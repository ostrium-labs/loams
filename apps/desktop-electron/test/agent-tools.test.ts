import { mkdtempSync, readdirSync, writeFileSync } from "node:fs";
import * as fsp from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import {
	type BuiltinServices,
	builtinTools,
} from "../src/main/agent/builtin-tools";
import { readOnlyViolation } from "../src/main/agent/sql-guard";
import { ChatStore, newChatId, type StoreFs } from "../src/main/agent/store";
import {
	resultText,
	type ToolDef,
	ToolRegistry,
} from "../src/main/agent/tools";
import type { ChatRecord } from "../src/shared/contracts";

interface Call {
	path: string;
	init: RequestInit;
}

function services(answer: (c: Call) => unknown, status = 200) {
	const calls: Call[] = [];
	const s: BuiltinServices = {
		api: async (path, init) => {
			calls.push({ path, init });
			return new Response(JSON.stringify(answer({ path, init })), { status });
		},
		connectors: () => [
			{
				id: "kafka",
				name: "Apache Kafka",
				category: "streaming",
				status: "beta",
				runtime: { kind: "native", ref: "x" },
				source: ["streaming"],
				sink: ["streaming"],
				modes: [],
				auth: ["sasl"],
				licence: "Apache-2.0",
				stub: false,
			},
			{
				id: "s3",
				name: "Amazon S3",
				category: "storage",
				status: "planned",
				runtime: { kind: "native", ref: "y" },
				source: null,
				sink: ["batch"],
				modes: [],
				auth: [],
				licence: "Apache-2.0",
				stub: true,
			},
		],
		factoryQuery: async (q) =>
			q.op === "list_issues"
				? { ok: true, value: [{ id: 1 }] }
				: { ok: false, code: "unknown_op", message: "Unknown operation" },
		factoryOps: { forgejo: ["list_issues"] },
		now: () => 1_000,
	};
	const tools = new Map(builtinTools(s).map((t) => [t.name, t]));
	const run = (name: string, args: Record<string, unknown>) =>
		(tools.get(name) as ToolDef).run(
			{ signal: new AbortController().signal, chatId: "c" },
			args,
		);
	return { calls, tools, run };
}

describe("sql guard", () => {
	it("sql_tools_read_only", async () => {
		const ok = [
			"SELECT * FROM docs",
			"select count(*) from docs;",
			"WITH t AS (SELECT 1) SELECT * FROM t",
			"EXPLAIN SELECT 1",
			"SHOW TABLES",
			"select replace(title, 'a', 'b') from docs",
			"select 'insert into x' as s, \"delete\" from docs -- drop table docs",
			"select t.update from docs t",
			"/* update */ select 1",
		];
		const bad = [
			"INSERT INTO docs VALUES (1)",
			"update docs set a = 1",
			"DELETE FROM docs",
			"drop table docs",
			"select 1; drop table docs",
			"WITH x AS (DELETE FROM docs RETURNING *) SELECT * FROM x",
			"SELECT * INTO copy FROM docs",
			"SELECT * FROM docs FOR UPDATE",
			"EXPLAIN ANALYZE DELETE FROM docs",
			"EXPLAIN ANALYZE SELECT 1",
			"/* x */ explain analyse select 1",
			"EXPLAIN (BUFFERS, ANALYZE) SELECT 1",
			"select 'unterminated",
			"CALL proc()",
			"SET x = 1",
			"",
			"   ;",
		];
		for (const q of ok) expect(readOnlyViolation(q), q).toBeUndefined();
		for (const q of bad) expect(readOnlyViolation(q), q).toBeDefined();

		// The tool refuses before anything reaches the server.
		const { calls, run } = services(() => ({ columns: [], rows: [] }));
		await expect(
			run("sql_query", { namespace: "default", sql: "DELETE FROM docs" }),
		).rejects.toThrow(/Refused/);
		expect(calls).toHaveLength(0);
		const out = await run("sql_query", {
			namespace: "default",
			sql: "select 1",
		});
		expect(calls[0]?.path).toBe("/v1/namespaces/default/sql");
		expect(JSON.parse(String(calls[0]?.init.body))).toEqual({
			query: "select 1",
		});
		expect(out).toContain("(0 rows)");
	});
});

describe("builtin tools", () => {
	it("lists_every_d675_tool_with_risk", () => {
		const { tools } = services(() => ({}));
		expect([...tools.values()].map((t) => [t.name, t.risk])).toEqual([
			["collections_list", "read"],
			["search", "read"],
			["sql_query", "read"],
			["durable_promises_search", "read"],
			["durable_promise_create", "write"],
			["streams_list", "read"],
			["links_list", "read"],
			["connectors_search", "read"],
			["factory_query", "read"],
		]);
		// Every schema compiles and the names are unique.
		const reg = new ToolRegistry();
		reg.register([...tools.values()]);
		// Namespace names: no path tricks.
		for (const bad of ["..", ".", "...", "a/b", "a b", ""])
			expect(reg.check("streams_list", { namespace: bad }), bad).toBeDefined();
		for (const ok of ["default", "my.ns", "a_b-1", ".hidden"])
			expect(reg.check("streams_list", { namespace: ok }), ok).toBeUndefined();
	});

	it("data_plane_tools_call_the_engine_routes", async () => {
		const { calls, run } = services((c) =>
			c.path.endsWith("/collections")
				? { collections: [{ name: "docs" }] }
				: c.path.endsWith("/streams")
					? { streams: [{ name: "s" }] }
					: c.path.endsWith("/links")
						? { links: [{ name: "l" }] }
						: { hits: [] },
		);
		expect(await run("collections_list", { namespace: "my-ns" })).toEqual([
			{ name: "docs" },
		]);
		expect(calls[0]?.path).toBe("/v1/namespaces/my-ns/collections");
		expect(await run("streams_list", { namespace: "a" })).toEqual([
			{ name: "s" },
		]);
		expect(await run("links_list", { namespace: "a" })).toEqual([
			{ name: "l" },
		]);
		await run("search", { namespace: "a", query: { from: "docs", limit: 3 } });
		expect(calls[3]?.path).toBe("/v1/namespaces/a/query");
		expect(calls[3]?.init.method).toBe("POST");
		expect(JSON.parse(String(calls[3]?.init.body))).toEqual({
			from: "docs",
			limit: 3,
		});
	});

	it("http_errors_become_tool_errors", async () => {
		const { run } = services(
			() => ({ error: "not_found", message: "no namespace x" }),
			404,
		);
		await expect(run("collections_list", { namespace: "x" })).rejects.toThrow(
			"HTTP 404: no namespace x",
		);
	});

	it("durable_tools_use_the_envelope", async () => {
		const enc = (v: unknown) =>
			Buffer.from(JSON.stringify(v)).toString("base64");
		const { calls, run } = services((c) => {
			const body = JSON.parse(String(c.init.body));
			if (body.kind === "promise.search")
				return {
					kind: "promise.search",
					head: { corrId: body.head.corrId, status: 200 },
					data: {
						promises: [
							{
								id: "p1",
								state: "pending",
								tags: {},
								param: { data: enc({ a: 1 }) },
								value: {},
								createdAt: 1,
								timeoutAt: 2,
							},
						],
					},
				};
			return {
				kind: "promise.create",
				head: { status: 201 },
				data: {
					promise: {
						id: body.data.id,
						state: "pending",
						param: body.data.param,
						value: {},
						tags: body.data.tags,
					},
				},
			};
		});
		const found = (await run("durable_promises_search", {
			state: "pending",
		})) as { promises: unknown[] };
		expect(found.promises[0]).toMatchObject({
			id: "p1",
			param: { a: 1 },
			value: null,
		});
		expect(calls[0]?.path).toBe("/durable/");
		const sent = JSON.parse(String(calls[0]?.init.body));
		expect(sent).toMatchObject({
			kind: "promise.search",
			head: { version: "2026-04-01" },
			data: { state: "pending", limit: 20 },
		});

		const created = await run("durable_promise_create", {
			id: "p2",
			param: { x: true },
			tags: { k: "v" },
		});
		const body = JSON.parse(String(calls[1]?.init.body));
		expect(body.data).toEqual({
			id: "p2",
			timeoutAt: 1_000 + 3_600_000,
			param: { headers: {}, data: enc({ x: true }) },
			tags: { k: "v" },
		});
		expect(created).toMatchObject({ id: "p2", param: { x: true } });
	});

	it("durable_proxy_refusal_is_an_error", async () => {
		const { run } = services(
			() => ({
				code: "not_available_remote",
				message: "Durable execution is not available on this server.",
			}),
			404,
		);
		await expect(run("durable_promises_search", {})).rejects.toThrow(
			"not available on this server",
		);
	});

	it("connectors_and_factory_tools", async () => {
		const { run, tools } = services(() => ({}));
		const r = (await run("connectors_search", { direction: "source" })) as {
			total: number;
			connectors: { id: string }[];
		};
		expect(r.connectors.map((c) => c.id)).toEqual(["kafka"]);
		const s = (await run("connectors_search", { query: "s3" })) as {
			connectors: { stub?: boolean }[];
		};
		expect(s.connectors[0]?.stub).toBe(true);
		expect(
			await run("factory_query", {
				app: "forgejo",
				op: "list_issues",
				params: { limit: 5 },
			}),
		).toEqual([{ id: 1 }]);
		await expect(
			run("factory_query", { app: "forgejo", op: "delete_repo" }),
		).rejects.toThrow("unknown_op");
		expect(tools.get("factory_query")?.description).toContain(
			"forgejo: list_issues",
		);
	});
});

describe("tool registry", () => {
	it("validates_names_duplicates_and_args", () => {
		const r = new ToolRegistry();
		const t: ToolDef = {
			name: "pg_sql",
			description: "d",
			risk: "read",
			schema: {
				type: "object",
				properties: { sql: { type: "string" } },
				required: ["sql"],
			},
			run: async () => "",
		};
		r.register([t]);
		expect(() => r.register([t])).toThrow(/duplicate/);
		expect(() => r.register([{ ...t, name: "Bad-Name" }])).toThrow(
			/bad tool name/,
		);
		expect(r.check("pg_sql", { sql: "x" })).toBeUndefined();
		expect(r.check("pg_sql", {})).toMatch(/sql/);
		expect(r.check("nope", {})).toMatch(/unknown/);
	});

	it("result_text_is_plain_text", () => {
		expect(resultText("<b>x</b>")).toBe("<b>x</b>");
		expect(resultText({ a: 1 })).toBe('{\n  "a": 1\n}');
		expect(resultText(undefined)).toBe("OK");
	});
});

describe("chat store", () => {
	const chat = (id: string, updatedAt: number): ChatRecord => ({
		id,
		title: id,
		createdAt: 0,
		updatedAt,
		provider: "anthropic",
		model: "m",
		alwaysAllow: [],
		messages: [],
	});

	it("saves_atomically_and_lists_newest_first", async () => {
		const dir = mkdtempSync(join(tmpdir(), "chats-"));
		const s = new ChatStore(dir);
		const a = newChatId();
		const b = newChatId();
		await s.save(chat(a, 1));
		await s.save(chat(b, 5));
		writeFileSync(join(dir, "c_garbage00.json"), "{not json");
		expect((await s.list()).map((c) => c.id)).toEqual([b, a]);
		expect(readdirSync(dir).filter((f) => f.endsWith(".tmp"))).toEqual([]);
		expect((await s.get(a))?.updatedAt).toBe(1);
		await s.remove(a);
		expect(await s.get(a)).toBeUndefined();
		expect((await s.list()).map((c) => c.id)).toEqual([b]);
		expect(await s.get("../x")).toBeUndefined();
		expect(() => s.save(chat("../../evil", 1))).toThrow(/bad chat id/);
	});

	it("list_uses_the_index_not_every_file", async () => {
		const dir = mkdtempSync(join(tmpdir(), "chats-"));
		const seed = new ChatStore(dir);
		for (let i = 0; i < 5; i++) await seed.save(chat(newChatId(), i));
		const reads: string[] = [];
		const counting = {
			...fsp,
			readFile: (path: string, enc: "utf8") => {
				reads.push(path);
				return fsp.readFile(path, enc);
			},
			readdir: (path: string) => {
				reads.push(`dir:${path}`);
				return fsp.readdir(path);
			},
		} as unknown as StoreFs;
		const s = new ChatStore(dir, counting);
		expect(await s.list()).toHaveLength(5);
		expect(reads).toHaveLength(6); // one readdir, five files, once
		const c = chat(newChatId(), 99);
		await s.save(c);
		const again = await s.list();
		expect(again).toHaveLength(6);
		expect(again[0]?.id).toBe(c.id);
		expect(reads).toHaveLength(6); // no re-read on later calls
	});

	it("queued_writes_land_in_order_and_snapshot_at_call_time", async () => {
		const dir = mkdtempSync(join(tmpdir(), "chats-"));
		const s = new ChatStore(dir);
		const c = chat(newChatId(), 1);
		void s.save(c);
		c.title = "second";
		c.updatedAt = 2;
		void s.save(c);
		c.title = "not saved";
		await s.flush();
		expect((await s.get(c.id))?.title).toBe("second");
	});
});
