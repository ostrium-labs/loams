import { describe, expect, it } from "vitest";
import { Secret } from "../src/main/factory/vault";
import {
	isPlainRead,
	type RawResult,
	redact,
	runCapped,
	SqlError,
	type SqlSession,
	toResult,
} from "../src/main/sql/caps";
import { pgTools } from "../src/main/sql/pg";
import { createWesqlBackend, wesqlTools } from "../src/main/sql/wesql";

function fake(
	dialect: "pg" | "mysql",
	reply: (sql: string) => RawResult | Error = () => ({ columns: [], rows: [] }),
) {
	const log: { sql: string; single?: boolean }[] = [];
	const s: SqlSession = {
		dialect,
		async query(sql, opts) {
			log.push({ sql, single: opts?.single });
			const r = reply(sql);
			if (r instanceof Error) throw r;
			return r;
		},
	};
	return { s, log };
}

describe("sql caps", () => {
	it("caps_truncate_and_flag", async () => {
		const rows = Array.from({ length: 1500 }, (_, i) => [i, 10n ** 20n]);
		const { s } = fake("pg", (q) =>
			q.startsWith("SELECT")
				? { columns: ["a", "b"], rows }
				: { columns: [], rows: [] },
		);
		const r = await runCapped(s, "SELECT * FROM t", {
			maxRows: 1000,
			timeoutMs: 30000,
		});
		expect(r.rows).toHaveLength(1000);
		expect(r.rowCount).toBe(1000);
		expect(r.truncated).toBe(true);
		expect(r.rows[0]?.[1]).toBe("100000000000000000000");
		const small = await runCapped(
			fake("pg", () => ({ columns: ["x"], rows: [[1]] })).s,
			"SELECT 1",
		);
		expect(small.truncated).toBe(false);
	});

	it("timeout_maps_to_error", async () => {
		const pg = fake("pg", (q) =>
			q.startsWith("SELECT")
				? Object.assign(
						new Error("canceling statement due to statement timeout"),
						{ code: "57014" },
					)
				: { columns: [], rows: [] },
		);
		await expect(
			runCapped(pg.s, "SELECT pg_sleep(60)", { timeoutMs: 50 }),
		).rejects.toMatchObject({ code: "timeout" });
		expect(pg.log[0]?.sql).toBe("SET statement_timeout = 50");
		const my = fake("mysql", (q) =>
			q.startsWith("SELECT")
				? Object.assign(new Error("Query execution was interrupted"), {
						errno: 3024,
						code: "ER_QUERY_TIMEOUT",
					})
				: { columns: [], rows: [] },
		);
		await expect(
			runCapped(my.s, "SELECT SLEEP(60)", { timeoutMs: 50 }),
		).rejects.toMatchObject({ code: "timeout" });
		expect(my.log[0]?.sql).toBe("SET SESSION MAX_EXECUTION_TIME = 50");
	});

	it("read_only_wrapper_blocks_insert", async () => {
		// pg: the sequence is BEGIN READ ONLY ... ROLLBACK, and the failing statement still rolls back.
		const pg = fake("pg", (q) =>
			q.startsWith("INSERT")
				? Object.assign(
						new Error("cannot execute INSERT in a read-only transaction"),
						{ code: "25006" },
					)
				: { columns: [], rows: [] },
		);
		await expect(
			runCapped(pg.s, "INSERT INTO t VALUES (1)", { readOnly: true }),
		).rejects.toMatchObject({ code: "25006" });
		expect(pg.log.map((l) => l.sql)).toEqual([
			"BEGIN READ ONLY",
			"SET LOCAL statement_timeout = 30000",
			"INSERT INTO t VALUES (1)",
			"ROLLBACK",
		]);
		expect(pg.log[2]?.single).toBe(true);
		const my = fake("mysql", (q) =>
			q.startsWith("INSERT")
				? Object.assign(new Error("read only"), {
						errno: 1792,
						code: "ER_CANT_EXECUTE_IN_READ_ONLY_TRANSACTION",
					})
				: { columns: [], rows: [] },
		);
		await expect(
			runCapped(my.s, "INSERT INTO t VALUES (1)", { readOnly: true }),
		).rejects.toMatchObject({
			code: "ER_CANT_EXECUTE_IN_READ_ONLY_TRANSACTION",
		});
		expect(my.log.map((l) => l.sql)).toEqual([
			"SET SESSION MAX_EXECUTION_TIME = 30000",
			"START TRANSACTION READ ONLY",
			"INSERT INTO t VALUES (1)",
			"ROLLBACK",
		]);
		// A stacked statement never reaches the server in read-only mode.
		const stacked = fake("pg");
		await expect(
			runCapped(stacked.s, "SELECT 1; COMMIT; INSERT INTO t VALUES (1)", {
				readOnly: true,
			}),
		).rejects.toMatchObject({ code: "multi_statement" });
		expect(stacked.log).toHaveLength(0);
	});

	it("agent_tools_refuse_writes_and_run_read_only", async () => {
		const calls: { sql: string; ro?: boolean; agent?: boolean }[] = [];
		const be = {
			query: async (
				sql: string,
				o?: { readOnly?: boolean; agent?: boolean },
			) => {
				calls.push({ sql, ro: o?.readOnly, agent: o?.agent });
				return {
					columns: [],
					rows: [],
					rowCount: 0,
					truncated: false,
					elapsedMs: 0,
				};
			},
		};
		const pgSql = pgTools.find((t) => t.name === "pg_sql");
		const wesqlSql = wesqlTools.find((t) => t.name === "wesql_sql");
		expect(pgTools.map((t) => [t.name, t.risk])).toEqual([
			["pg_sql", "read"],
			["pg_branch_create", "write"],
		]);
		await pgSql?.run({ pg: be } as never, { sql: "select 1" });
		expect(calls[0]).toEqual({ sql: "select 1", ro: undefined, agent: true });
		await expect(
			pgSql?.run({ pg: be } as never, { sql: "drop table t" }),
		).rejects.toMatchObject({ code: "read_only" });
		await expect(
			wesqlSql?.run({ wesql: be } as never, { sql: "delete from t" }),
		).rejects.toMatchObject({ code: "read_only" });
	});

	it("classifies_plain_reads", () => {
		for (const q of [
			"SELECT 1",
			" select replace(a,'x','y') from t;",
			"SHOW TABLES",
			"EXPLAIN SELECT 1",
			"WITH c AS (SELECT 1) SELECT * FROM c",
			"SELECT 'insert; drop' -- delete",
		])
			expect(isPlainRead(q, "pg"), q).toBe(true);
		expect(isPlainRead("SELECT 1 # c\n", "mysql")).toBe(true);
		for (const q of [
			"INSERT INTO t VALUES (1)",
			"EXPLAIN ANALYZE DELETE FROM t",
			"WITH d AS (DELETE FROM t RETURNING *) SELECT * FROM d",
			"SELECT 1; DROP TABLE t",
			"SELECT * INTO u FROM t",
			"SELECT * FROM t FOR UPDATE",
			"SET x = 1",
			"",
			"SELECT set_config('role','none',true)",
			"SELECT E'a\\'' INTO OUTFILE '/x'",
		])
			expect(isPlainRead(q, "pg"), q).toBe(false);
		// MySQL: executable comments and `--` without whitespace are ambiguous, so they count as writes.
		for (const q of [
			"SELECT 1 /*!80000 INTO OUTFILE '/tmp/x' */",
			"SELECT a --1 INTO OUTFILE '/tmp/x' FROM t",
			"SELECT 'a\\' INTO OUTFILE '/x'",
		])
			expect(isPlainRead(q, "mysql"), q).toBe(false);
	});

	it("errors_redact_password", async () => {
		expect(
			redact(
				"connect failed postgres://cloud_admin:hunter2@127.0.0.1:55433/postgres",
			),
		).not.toContain("hunter2");
		expect(redact("mysql://root:s3cr3t@h/db")).toBe("mysql://root:***@h/db");
		expect(redact("bad password=abc123 here")).not.toContain("abc123");
		expect(redact("auth failed for loams-dev", ["loams-dev"])).toBe(
			"auth failed for ***",
		);
		// The user name survives even when it equals the password.
		expect(
			redact("failed postgres://cloud_admin:cloud_admin@h/d", [
				new Secret("cloud_admin"),
			]),
		).toBe("failed postgres://cloud_admin:***@h/d");
		const r = await toResult(async () => {
			throw Object.assign(
				new Error("password authentication failed: postgres://u:pw9@h/d"),
				{ code: "28P01" },
			);
		}, ["pw9"]);
		expect(r).toEqual({
			ok: false,
			code: "28P01",
			message: "password authentication failed: postgres://u:***@h/d",
		});
		expect(JSON.stringify(r)).not.toContain("pw9");
		expect(new SqlError("x", "y").code).toBe("x");
	});

	it("agent_role_sequence_and_early_stop", async () => {
		const seen: { sql: string; limit?: number }[] = [];
		const s: SqlSession = {
			dialect: "pg",
			async query(sql, o) {
				seen.push({ sql, limit: o?.limit });
				if (!sql.startsWith("SELECT")) return { columns: [], rows: [] };
				// Behaves like the real sessions: never returns more than `limit` rows.
				const n = Math.min(o?.limit ?? Infinity, 5_000_000);
				return {
					columns: ["n"],
					rows: Array.from({ length: n }, (_, i) => [i]),
				};
			},
		};
		const r = await runCapped(s, "SELECT n FROM big", {
			readOnly: true,
			role: "pg_read_all_data",
		});
		expect(seen.map((x) => x.sql)).toEqual([
			"BEGIN READ ONLY",
			"SET LOCAL ROLE pg_read_all_data",
			"SET LOCAL statement_timeout = 30000",
			"SELECT n FROM big",
			"ROLLBACK",
		]);
		// The row bound reaches the session as maxRows + 1, so it can stop reading there.
		expect(seen[3]?.limit).toBe(1001);
		expect(r.rowCount).toBe(1000);
		expect(r.truncated).toBe(true);
		await expect(
			runCapped(fake("pg").s, "SELECT 1", { readOnly: true, role: "x; DROP" }),
		).rejects.toMatchObject({ code: "invalid" });
	});

	it("mysql_agent_reads_as_the_select_only_user", async () => {
		const logins: (string | undefined)[] = [];
		const log: string[] = [];
		const connect = async (as?: { user: string; password: Secret }) => {
			logins.push(as?.user);
			return {
				dialect: "mysql" as const,
				query: async (sql: string) => {
					log.push(`${as?.user ?? "root"}: ${sql}`);
					return { columns: ["1"], rows: [[1]] };
				},
				params: async () => ({ columns: [], rows: [] }),
				close: async () => {},
			};
		};
		const be = createWesqlBackend({ connect });
		await be.query("SELECT 1", { agent: true });
		await be.query("SELECT 2", { agent: true });
		expect(logins).toEqual([undefined, "loams_ro", "loams_ro"]);
		const admin = log.filter((l) => l.startsWith("root:"));
		expect(admin).toHaveLength(5); // provisioned once, not per query
		expect(admin[0]).toMatch(
			/CREATE USER IF NOT EXISTS 'loams_ro'@'%' IDENTIFIED BY '[\w-]{32}'/,
		);
		expect(admin[3]).toBe("root: GRANT SELECT ON *.* TO 'loams_ro'@'%'");
		expect(
			log
				.filter((l) => l.startsWith("loams_ro:"))
				.slice(0, 3)
				.map((l) => l.slice(10)),
		).toEqual([
			"SET SESSION MAX_EXECUTION_TIME = 30000",
			"START TRANSACTION READ ONLY",
			"SELECT 1",
		]);
	});
});
