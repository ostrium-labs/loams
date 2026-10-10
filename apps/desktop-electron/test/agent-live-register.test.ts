import { describe, expect, it } from "vitest";
import { liveToolDefs } from "../src/main/agent/live-tools";
import { ToolRegistry } from "../src/main/agent/tools";

describe("live tools in the agent registry", () => {
	const calls: { url: string; signal?: AbortSignal | null }[] = [];
	const defs = liveToolDefs({
		fetch: (async (url: string, init: RequestInit) => {
			calls.push({ url, signal: init.signal });
			return new Response(JSON.stringify({ result: { stringValue: "x" } }));
		}) as unknown as typeof fetch,
		liveUrl: "loams-app://console",
	});

	it("live_tools_risk_tags", () => {
		expect(defs.map((d) => [d.name, d.risk])).toEqual([
			["live_tables", "read"],
			["live_query", "read"],
			["live_mutate", "write"],
		]);
	});

	it("registers and validates arguments with the registry", () => {
		const r = new ToolRegistry();
		r.register(defs);
		expect(r.check("live_query", { table: "t", limit: 5 })).toBeUndefined();
		expect(r.check("live_query", { limit: 5 })).toMatch(/table/);
		expect(r.check("live_mutate", { action: "insert" })).toBeUndefined();
	});

	it("runs through the console origin and carries the turn's abort signal", async () => {
		const ctl = new AbortController();
		const q = defs.find((d) => d.name === "live_tables");
		await q?.run({ signal: ctl.signal, chatId: "c_abcdefgh" }, {});
		expect(calls[0]?.url).toBe(
			"loams-app://console/loams.live.v1.LiveService/Query",
		);
		ctl.abort();
		expect(calls[0]?.signal?.aborted).toBe(true);
	});
});
