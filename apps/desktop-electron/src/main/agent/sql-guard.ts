// A lexical read-only check for the agent's SQL tools (D675). It is the first
// of two fences: the engine's own SQL route only runs read-only plans
// (`run_read_only`), and the Postgres/WeSQL tools also run inside a read-only
// transaction (Task 22). It is deliberately strict: one statement, starting
// with SELECT, WITH, EXPLAIN (not ANALYZE), SHOW, DESCRIBE/DESC, VALUES or TABLE,
// no row locks, and no write keyword outside string literals, quoted identifiers
// and comments. It cannot see side effects inside functions; the fences above do.

const FIRST = new Set([
	"select",
	"with",
	"explain",
	"show",
	"describe",
	"desc",
	"values",
	"table",
]);
// Reserved words that write, lock or execute. A word used as a function (`replace(`) or a
// qualified name (`t.update`) is not a keyword and is let through.
const WRITE = new Set([
	"insert",
	"update",
	"delete",
	"merge",
	"upsert",
	"create",
	"drop",
	"alter",
	"truncate",
	"grant",
	"revoke",
	"copy",
	"call",
	"into",
	"lock",
	"vacuum",
	"analyze",
	"analyse",
	"attach",
	"detach",
	"pragma",
	"commit",
	"rollback",
	"savepoint",
	"prepare",
	"execute",
	"listen",
	"notify",
	"refresh",
]);

/** The statement with comments, literals and quoted identifiers blanked; throws on an unterminated one. */
export function stripSql(sql: string): string {
	let out = "";
	let i = 0;
	const n = sql.length;
	while (i < n) {
		const c = sql[i] as string;
		const next = sql[i + 1];
		if (c === "-" && next === "-") {
			while (i < n && sql[i] !== "\n") i++;
			out += " ";
		} else if (c === "#") {
			// MySQL line comment
			while (i < n && sql[i] !== "\n") i++;
			out += " ";
		} else if (c === "/" && next === "*") {
			const end = sql.indexOf("*/", i + 2);
			if (end < 0) throw new Error("unterminated comment");
			i = end + 2;
			out += " ";
		} else if (c === "'" || c === '"' || c === "`") {
			let j = i + 1;
			for (;;) {
				if (j >= n) throw new Error("unterminated quoted text");
				if (sql[j] === "\\" && c === "'") {
					j += 2;
					continue;
				}
				if (sql[j] === c) {
					if (sql[j + 1] === c) {
						j += 2;
						continue;
					}
					break;
				}
				j++;
			}
			out += c === "'" ? " '' " : " q ";
			i = j + 1;
		} else if (c === "$") {
			// Postgres dollar quoting: $tag$ ... $tag$
			const m = /^\$([A-Za-z_][A-Za-z0-9_]*)?\$/.exec(sql.slice(i));
			if (m) {
				const tag = m[0];
				const end = sql.indexOf(tag, i + tag.length);
				if (end < 0) throw new Error("unterminated dollar-quoted text");
				out += " '' ";
				i = end + tag.length;
			} else {
				out += c;
				i++;
			}
		} else {
			out += c;
			i++;
		}
	}
	return out;
}

/** Undefined when `sql` is a single read-only statement; otherwise the reason. */
export function readOnlyViolation(sql: string): string | undefined {
	let bare: string;
	try {
		bare = stripSql(sql);
	} catch (e) {
		return (e as Error).message;
	}
	const body = bare.trim().replace(/;\s*$/, "");
	if (!body) return "empty statement";
	if (body.includes(";")) return "only one statement is allowed";
	const lower = body.toLowerCase();
	const words = lower.match(/[a-z_][a-z0-9_]*/g) ?? [];
	const first = words[0] ?? "";
	if (!FIRST.has(first))
		return `only SELECT, WITH, EXPLAIN, SHOW, DESCRIBE or VALUES statements are allowed (got ${first.toUpperCase() || "nothing"})`;
	// SELECT ... FOR UPDATE / FOR SHARE lock rows; EXPLAIN ANALYZE runs the statement;
	// WITH x AS (DELETE ... RETURNING ...) writes; SELECT ... INTO creates a table.
	if (/\bfor\s+(no\s+key\s+update|update|key\s+share|share)\b/.test(lower))
		return "row locks (FOR UPDATE / FOR SHARE) are not allowed";
	for (const m of lower.matchAll(/[a-z_][a-z0-9_]*/g)) {
		const w = m[0];
		if (!WRITE.has(w)) continue;
		const before = lower.slice(0, m.index).trimEnd();
		const after = lower.slice(m.index + w.length).trimStart();
		if (before.endsWith(".") || after.startsWith(".") || after.startsWith("("))
			continue;
		return `${w.toUpperCase()} is not allowed in a read-only query`;
	}
	return undefined;
}

export function assertReadOnlySql(sql: string): void {
	const why = readOnlyViolation(sql);
	if (why) throw new Error(`Refused: ${why}.`);
}
