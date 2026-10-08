import { describe, expect, it } from "vitest";
import { ToolRegistry } from "../src/main/agent/tools";
import { sqlAgentTools } from "../src/main/sql/tools";

describe("sql agent tools", () => {
	it("registers_pg_sql_and_wesql_sql_as_read_and_branch_as_write", async () => {
		const calls: unknown[] = [];
		const q = async (sql: string, o?: unknown) => {
			calls.push([sql, o]);
			return {
				columns: [],
				rows: [],
				rowCount: 0,
				truncated: false,
				elapsedMs: 0,
			};
		};
		const reg = new ToolRegistry();
		reg.register(
			sqlAgentTools({
				pg: { query: q } as never,
				wesql: { query: q } as never,
			}),
		);
		expect(reg.get("pg_sql")?.risk).toBe("read");
		expect(reg.get("wesql_sql")?.risk).toBe("read");
		expect(reg.get("pg_branch_create")?.risk).toBe("write");
		expect(reg.check("pg_sql", { sql: 1 })).toBeTruthy();
		expect(
			reg.check("pg_branch_create", {
				tenant: "t",
				name: "n",
				ancestorTimelineId: "a",
			}),
		).toBeUndefined();
		const ctx = { signal: new AbortController().signal, chatId: "c" };
		await reg.get("wesql_sql")?.run(ctx, { sql: "SELECT 1" });
		expect(calls).toEqual([["SELECT 1", { agent: true }]]);
		await expect(
			reg.get("pg_sql")?.run(ctx, { sql: "DROP TABLE t" }),
		).rejects.toMatchObject({ code: "read_only" });
	});
});
