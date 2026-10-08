// Framework-free SQL scanner shared by the main process (agent read tools, UI write confirm) and the
// console (SqlConsole's confirm). Ported from Task 23's desktop-ui `sql-write.ts`; both sides should
// import this one copy so the two lexers cannot diverge. Not a security boundary on its own: the
// database enforces read-only for agent tools (see main/sql).

export type SqlDialect = "postgres" | "mysql";

/** Words that change data or schema; a statement containing one needs a confirm. */
const WRITE_WORDS =
	/\b(insert|update|delete|merge|create|drop|alter|truncate|into|for\s+update|for\s+share)\b/gi;

/** A write word used as a keyword: a qualified name (`t.update`, `update.x`) is a column, not one. */
function hasWriteWord(s: string): boolean {
	for (const m of s.matchAll(WRITE_WORDS)) {
		const before = s.slice(0, m.index).trimEnd();
		const after = s.slice(m.index + m[0].length).trimStart();
		if (before.endsWith(".") || after.startsWith(".")) continue;
		return true;
	}
	return false;
}

/**
 * Functions with side effects that a SELECT can call. A small denylist, not
 * a complete one: the check is a convenience (see `isWrite`).
 */
const SIDE_EFFECT_CALL =
	/\b(nextval|setval|set_config|pg_terminate_backend|pg_cancel_backend|pg_advisory_lock|pg_advisory_xact_lock|pg_try_advisory_lock|pg_try_advisory_xact_lock|pg_notify|pg_sleep\w*|pg_reload_conf|pg_rotate_logfile|pg_switch_wal|pg_create_restore_point|pg_create_\w+_slot|pg_drop_replication_slot|pg_stat_reset\w*|pg_read_file|pg_read_binary_file|pg_ls_dir|lo_import|lo_export|lo_unlink|lo_create|lo_from_bytea|dblink\w*|txid_current|sleep|get_lock|release_lock|release_all_locks|load_file|benchmark|master_pos_wait|sys_exec|sys_eval)\s*\(/i;

const READ_HEADS = new Set([
	"select",
	"show",
	"explain",
	"with",
	"values",
	"table",
	"describe",
	"desc",
]);

const isIdentChar = (c: string | undefined) =>
	!!c && /[A-Za-z0-9_$\u0080-￿]/.test(c);

/**
 * Remove comments and the contents of quoted strings and identifiers, so
 * keyword matching sees only code. Returns undefined when the quoting is
 * unterminated or depends on server settings we can't see (treated as a write).
 */
export function stripSql(sql: string, dialect: SqlDialect): string | undefined {
	let out = "";
	let i = 0;
	const n = sql.length;
	while (i < n) {
		const c = sql[i] as string;
		const next = sql[i + 1];
		// Line comments.
		if (c === "-" && next === "-") {
			// MySQL needs whitespace after `--`; otherwise it is not a comment.
			if (
				dialect === "mysql" &&
				next === "-" &&
				sql[i + 2] !== undefined &&
				!/\s/.test(sql[i + 2] as string)
			) {
				return undefined;
			}
			const end = sql.indexOf("\n", i);
			if (end < 0) break;
			out += " ";
			i = end + 1;
			continue;
		}
		if (c === "#" && dialect === "mysql") {
			const end = sql.indexOf("\n", i);
			if (end < 0) break;
			out += " ";
			i = end + 1;
			continue;
		}
		// Block comments. MySQL `/*! ... */` is executed code: ambiguous.
		if (c === "/" && next === "*") {
			if (dialect === "mysql" && sql[i + 2] === "!") return undefined;
			let depth = 1;
			let j = i + 2;
			while (j < n && depth > 0) {
				if (sql[j] === "*" && sql[j + 1] === "/") {
					depth--;
					j += 2;
				} else if (
					dialect === "postgres" &&
					sql[j] === "/" &&
					sql[j + 1] === "*"
				) {
					depth++;
					j += 2;
				} else j++;
			}
			if (depth > 0) return undefined;
			out += " ";
			i = j;
			continue;
		}
		// Quoted strings and identifiers.
		if (c === "'" || c === '"' || c === "`") {
			if (c === "`" && dialect !== "mysql") return undefined;
			const prev = sql[i - 1];
			const escapeString =
				dialect === "postgres" &&
				c === "'" &&
				(prev === "E" || prev === "e") &&
				!isIdentChar(sql[i - 2]);
			// MySQL's backslash handling depends on sql_mode (NO_BACKSLASH_ESCAPES), so a
			// backslash in a MySQL quote is ambiguous. E'' strings always honour backslashes.
			let j = i + 1;
			let closed = false;
			while (j < n) {
				const d = sql[j] as string;
				if (
					d === "\\" &&
					(escapeString || (dialect === "mysql" && c !== "`"))
				) {
					if (dialect === "mysql") return undefined;
					j += 2;
					continue;
				}
				if (d === c) {
					if (sql[j + 1] === c) {
						j += 2;
						continue;
					}
					closed = true;
					break;
				}
				j++;
			}
			if (!closed) return undefined;
			out += c === "'" ? "''" : c === '"' ? '""' : "``";
			i = j + 1;
			continue;
		}
		// Postgres dollar quoting: $$ ... $$ or $tag$ ... $tag$ (but not $1 or a$b).
		if (c === "$" && dialect === "postgres" && !isIdentChar(sql[i - 1])) {
			const m = /^\$([A-Za-z_\u0080-￿][A-Za-z0-9_\u0080-￿]*)?\$/.exec(
				sql.slice(i),
			);
			if (m) {
				const end = sql.indexOf(m[0], i + m[0].length);
				if (end < 0) return undefined;
				out += "''";
				i = end + m[0].length;
				continue;
			}
		}
		out += c;
		i++;
	}
	return out;
}

/**
 * True unless every statement looks like a plain SELECT, SHOW, EXPLAIN or
 * WITH-SELECT with no known side-effecting call. Unterminated or ambiguous
 * quoting counts as a write.
 *
 * This is a convenience that decides when to ask first, NOT a security
 * boundary: it can't see server settings or every side-effecting function.
 * The main process enforces read-only for agent tools.
 */
export function isWrite(sql: string, dialect: SqlDialect): boolean {
	const code = stripSql(sql, dialect);
	if (code === undefined) return true;
	const statements = code
		.split(";")
		.map((s) => s.trim())
		.filter(Boolean);
	if (statements.length === 0) return false;
	return statements.some((s) => {
		const head = (/^[\s(]*([a-z]+)/i.exec(s)?.[1] ?? "").toLowerCase();
		if (!READ_HEADS.has(head)) return true;
		if (SIDE_EFFECT_CALL.test(s)) return true;
		if (head === "show" || head === "describe" || head === "desc") return false;
		return hasWriteWord(s);
	});
}

/**
 * True when `sql` is exactly one statement (a trailing `;` is fine). Ambiguous quoting counts as
 * "not single" so callers fail closed.
 */
export function isSingleStatement(sql: string, dialect: SqlDialect): boolean {
	const code = stripSql(sql, dialect);
	if (code === undefined) return false;
	const body = code.trim().replace(/;+\s*$/, "");
	return body.length > 0 && !body.includes(";");
}
