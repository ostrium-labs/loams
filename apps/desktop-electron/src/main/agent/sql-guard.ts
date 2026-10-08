// The agent's read-only check for its engine SQL tool (D675): a thin wrapper around the
// shared scanner (src/shared/sql-lex.ts), so the agent, the pages' write confirm and the
// Postgres/WeSQL tools use one lexer. It is the first of two fences: the engine's SQL
// route only runs read-only plans (`run_read_only`). On top of `isWrite` it requires
// exactly one statement and refuses EXPLAIN ANALYZE, which executes the statement.

import {
	isSingleStatement,
	isWrite,
	type SqlDialect,
	stripSql,
} from "../../shared/sql-lex";

/** Undefined when `sql` is a single read-only statement; otherwise the reason. */
export function readOnlyViolation(
	sql: string,
	dialect: SqlDialect = "postgres",
): string | undefined {
	if (!sql.trim().replace(/;\s*$/, "").trim()) return "empty statement";
	if (!isSingleStatement(sql, dialect))
		return "only one statement is allowed (or the quoting is ambiguous)";
	if (isWrite(sql, dialect))
		return "only read-only SELECT, WITH, EXPLAIN, SHOW, DESCRIBE or VALUES statements are allowed";
	const code = stripSql(sql, dialect) ?? "";
	if (/^[\s(]*explain\b/i.test(code) && /\banaly[sz]e\b/i.test(code))
		return "EXPLAIN ANALYZE runs the statement and is not allowed";
	return undefined;
}

export function assertReadOnlySql(sql: string): void {
	const why = readOnlyViolation(sql);
	if (why) throw new Error(`Refused: ${why}.`);
}
