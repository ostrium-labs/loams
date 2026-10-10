import { describe, expect, it } from "vitest";
import {
	fromValueJson,
	liveTools,
	toValueJson,
} from "../src/main/agent-tools/live";

const tool = (name: string) => {
	const t = liveTools.find((x) => x.name === name);
	if (!t) throw new Error(name);
	return t;
};

function fake(reply: unknown, status = 200) {
	const calls: { url: string; body: Record<string, unknown> }[] = [];
	const f = (async (url: string, init: RequestInit) => {
		calls.push({ url, body: JSON.parse(String(init.body)) });
		return new Response(JSON.stringify(reply), { status });
	}) as unknown as typeof fetch;
	return { calls, ctx: { fetch: f, liveUrl: "http://127.0.0.1:1/" } };
}

describe("live agent tools", () => {
	it("declares two read tools and one write tool", () => {
		expect(liveTools.map((t) => [t.name, t.access])).toEqual([
			["live_tables", "read"],
			["live_query", "read"],
			["live_mutate", "write"],
		]);
	});
	it("round-trips values", () => {
		const v = { a: 1, b: 1.5, c: "x", d: [true, null], e: { f: 2 } };
		expect(fromValueJson(toValueJson(v))).toEqual(v);
		expect(toValueJson(3)).toEqual({ int64Value: "3" });
	});
	it("keeps big ints and bytes exact in the canonical forms", () => {
		for (const text of [
			"9223372036854775807",
			"-9223372036854775808",
			"9007199254740993",
		]) {
			const wire = toValueJson({ $int64: text });
			expect(wire).toEqual({ int64Value: text });
			expect(fromValueJson(wire)).toEqual({ $int64: text });
		}
		expect(fromValueJson({ int64Value: "5" })).toBe(5);
		expect(toValueJson({ $bytes: "AQID" })).toEqual({ bytesValue: "AQID" });
		expect(fromValueJson({ bytesValue: "AQID" })).toEqual({ $bytes: "AQID" });
		expect(() => toValueJson({ $int64: "9223372036854775808" })).toThrow(
			/int64 range/,
		);
		expect(() => toValueJson({ $int64: "1.5" })).toThrow(/decimal/);
		expect(tool("live_mutate").description).toContain("$int64");
	});
	it("live_tables queries _system:tables", async () => {
		const { calls, ctx } = fake({
			ts: "5",
			result: toValueJson([{ name: "t", id: 10, indexes: [] }]),
		});
		const out = await tool("live_tables").run({}, ctx);
		expect(calls[0]?.url).toBe(
			"http://127.0.0.1:1/loams.live.v1.LiveService/Query",
		);
		expect(calls[0]?.body.function).toBe("_system:tables");
		expect(out).toEqual([{ name: "t", id: 10, indexes: [] }]);
	});
	it("live_query passes the filter and needs a table", async () => {
		const { calls, ctx } = fake({ ts: "5", result: toValueJson([]) });
		await tool("live_query").run({ table: "m", eq: ["a"], limit: 5 }, ctx);
		expect(fromValueJson(calls[0]?.body.args)).toEqual({
			table: "m",
			eq: ["a"],
			limit: 5,
		});
		await expect(tool("live_query").run({}, ctx)).rejects.toThrow(/table/);
	});
	it("live_mutate always sends an idempotency key", async () => {
		const { calls, ctx } = fake({ commitTs: "9", result: toValueJson("id1") });
		const out = (await tool("live_mutate").run(
			{ action: "insert", table: "m", fields: { x: 1 } },
			ctx,
		)) as { idempotencyKey: string };
		expect(calls[0]?.body.function).toBe("_system:insert");
		expect(calls[0]?.body.idempotencyKey).toBe(out.idempotencyKey);
		expect(out.idempotencyKey.length).toBeGreaterThan(10);
		await tool("live_mutate").run(
			{ action: "delete", id: "d1", idempotencyKey: "k1" },
			ctx,
		);
		expect(calls[1]?.body.idempotencyKey).toBe("k1");
		await expect(
			tool("live_mutate").run({ action: "drop" }, ctx),
		).rejects.toThrow(/action/);
	});
	it("reports a failed call with the server's message", async () => {
		const { ctx } = fake({ code: "unavailable", message: "no live" }, 503);
		await expect(tool("live_tables").run({}, ctx)).rejects.toThrow(
			/503.*no live/,
		);
	});
});
