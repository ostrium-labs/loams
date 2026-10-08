import { describe, expect, it } from "vitest";
import { isSingleStatement, isWrite } from "../src/shared/sql-lex";

// Cases ported from the agent's former sql-guard lexer (I8): the guard is now a
// wrapper around this scanner, so these pin the behaviour it relied on.
describe("sql-lex (ported guard cases)", () => {
	const pg = (sql: string) => isWrite(sql, "postgres");
	it.each([
		["SELECT * FROM docs", false],
		["select count(*) from docs;", false],
		["WITH t AS (SELECT 1) SELECT * FROM t", false],
		["EXPLAIN SELECT 1", false],
		["SHOW TABLES", false],
		["select replace(title, 'a', 'b') from docs", false],
		[
			"select 'insert into x' as s, \"delete\" from docs -- drop table docs",
			false,
		],
		// A qualified name is not the keyword.
		["select t.update from docs t", false],
		["select docs.delete, x.into.y from docs", false],
		["/* update */ select 1", false],
		["INSERT INTO docs VALUES (1)", true],
		["update docs set a = 1", true],
		["DELETE FROM docs", true],
		["drop table docs", true],
		["select 1; drop table docs", true],
		["WITH x AS (DELETE FROM docs RETURNING *) SELECT * FROM x", true],
		["SELECT * INTO copy FROM docs", true],
		["SELECT * FROM docs FOR UPDATE", true],
		["SELECT * FROM docs FOR SHARE", true],
		["EXPLAIN ANALYZE DELETE FROM docs", true],
		["select 'unterminated", true],
		["CALL proc()", true],
		["SET x = 1", true],
		["VACUUM docs", true],
		["LOCK TABLE docs", true],
		["COPY docs TO '/tmp/x'", true],
	])("%s -> %s", (sql, want) => expect(pg(sql)).toBe(want));

	it("single_statement", () => {
		expect(isSingleStatement("select 1;", "postgres")).toBe(true);
		expect(isSingleStatement("select 1; select 2", "postgres")).toBe(false);
		expect(isSingleStatement("", "postgres")).toBe(false);
		expect(isSingleStatement("   ;", "postgres")).toBe(false);
	});
});
