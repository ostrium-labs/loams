// Shared SQL plumbing for the pg and mysql backends: row/time caps, the read-only wrapper,
// statement classification, error mapping and secret redaction. Pure: no driver or electron imports.
import type { IpcResult, SqlResult } from "../../shared/contracts";

export type Dialect = "pg" | "mysql";

export interface RawResult {
	columns: string[];
	rows: unknown[][];
}

/** One connection's worth of SQL. Implemented over `pg` and `mysql2`, and by fakes in tests. */
export interface SqlSession {
	readonly dialect: Dialect;
	/**
	 * `single` asks the driver to accept exactly one statement (pg: extended protocol), which is
	 * what keeps `COMMIT; INSERT ...` from escaping a read-only transaction.
	 */
	query(sql: string, opts?: { single?: boolean }): Promise<RawResult>;
}

export interface CapOpts {
	maxRows: number;
	timeoutMs: number;
	/** Run inside a READ ONLY transaction that is always rolled back. */
	readOnly?: boolean;
}

export const DEFAULT_CAPS = { maxRows: 1000, timeoutMs: 30_000 } as const;

/** An error with a stable `code`, safe to show to the user (already redacted). */
export class SqlError extends Error {
	constructor(
		readonly code: string,
		message: string,
	) {
		super(message);
		this.name = "SqlError";
	}
}

/** pg: 57014 query_canceled (statement_timeout). mysql: 3024 ER_QUERY_TIMEOUT, 1317 ER_QUERY_INTERRUPTED. */
function isTimeout(e: { code?: unknown; errno?: unknown }): boolean {
	return (
		e.code === "57014" ||
		e.code === "ER_QUERY_TIMEOUT" ||
		e.code === "ER_QUERY_INTERRUPTED" ||
		e.errno === 3024 ||
		e.errno === 1317 ||
		e.code === "ETIMEDOUT_QUERY"
	);
}

/** Makes a cell safe for structured clone and JSON (bigint, Buffer, Date, nested objects). */
export function normalizeCell(v: unknown): unknown {
	if (v === null || v === undefined) return null;
	if (typeof v === "bigint") return v.toString();
	if (v instanceof Date)
		return Number.isNaN(v.getTime()) ? null : v.toISOString();
	if (v instanceof Uint8Array) return `\\x${Buffer.from(v).toString("hex")}`;
	if (Array.isArray(v)) return v.map(normalizeCell);
	if (typeof v === "object") {
		return Object.fromEntries(
			Object.entries(v as Record<string, unknown>).map(([k, x]) => [
				k,
				normalizeCell(x),
			]),
		);
	}
	return v;
}

/**
 * Runs `sql` on `exec` with a statement timeout and a row cap. Rows beyond `maxRows` are dropped
 * and `truncated` is set; `rowCount` is the number of rows kept.
 */
export async function runCapped(
	exec: SqlSession,
	sql: string,
	opts: Partial<CapOpts> = {},
): Promise<SqlResult> {
	const maxRows = opts.maxRows ?? DEFAULT_CAPS.maxRows;
	const timeoutMs = opts.timeoutMs ?? DEFAULT_CAPS.timeoutMs;
	const ro = opts.readOnly === true;
	const t0 = Date.now();
	const pg = exec.dialect === "pg";
	if (ro) assertSingleStatement(sql);
	let inTx = false;
	try {
		if (pg) {
			if (ro) {
				await exec.query("BEGIN READ ONLY");
				inTx = true;
				await exec.query(`SET LOCAL statement_timeout = ${timeoutMs | 0}`);
			} else {
				await exec.query(`SET statement_timeout = ${timeoutMs | 0}`);
			}
		} else {
			await exec.query(`SET SESSION MAX_EXECUTION_TIME = ${timeoutMs | 0}`);
			if (ro) {
				await exec.query("START TRANSACTION READ ONLY");
				inTx = true;
			}
		}
		const raw = await exec.query(sql, { single: ro });
		const truncated = raw.rows.length > maxRows;
		const rows = (truncated ? raw.rows.slice(0, maxRows) : raw.rows).map((r) =>
			r.map(normalizeCell),
		);
		return {
			columns: raw.columns,
			rows,
			rowCount: rows.length,
			truncated,
			elapsedMs: Date.now() - t0,
		};
	} catch (e) {
		throw toSqlError(e);
	} finally {
		if (inTx) {
			try {
				await exec.query("ROLLBACK");
			} catch {
				// the connection is closed by the caller; nothing more to do
			}
		}
	}
}

export function toSqlError(e: unknown, secrets: string[] = []): SqlError {
	if (e instanceof SqlError) return e;
	const o = (e ?? {}) as { code?: unknown; errno?: unknown; message?: unknown };
	const message = redact(
		typeof o.message === "string" ? o.message : String(e),
		secrets,
	);
	if (isTimeout(o)) return new SqlError("timeout", message);
	const code =
		typeof o.code === "string" && o.code
			? o.code
			: typeof o.errno === "number"
				? String(o.errno)
				: "error";
	return new SqlError(code, message);
}

/** Strips passwords from connection strings, `password=` pairs and any known secret value. */
export function redact(text: string, secrets: string[] = []): string {
	let out = text
		.replace(/\b([a-z][a-z0-9+.-]*:\/\/[^\s:/@]*):[^\s@/]*@/gi, "$1:***@")
		.replace(/(password\s*[=:]\s*)("[^"]*"|'[^']*'|[^\s,;&]+)/gi, "$1***");
	for (const s of secrets) if (s) out = out.split(s).join("***");
	return out;
}

/** Wraps a handler body as an IpcResult; thrown errors become `{code, message}` (redacted). */
export async function toResult<T>(
	fn: () => Promise<T>,
	secrets: string[] = [],
): Promise<IpcResult<T>> {
	try {
		return { ok: true, value: await fn() };
	} catch (e) {
		const err = toSqlError(e, secrets);
		return { ok: false, code: err.code, message: err.message };
	}
}

/** Replaces comments and quoted literals with spaces so keyword scans see only code. */
export function stripLiterals(sql: string): string {
	let out = "";
	let i = 0;
	while (i < sql.length) {
		const c = sql[i];
		const n = sql[i + 1];
		if (c === "-" && n === "-") {
			while (i < sql.length && sql[i] !== "\n") i++;
			out += " ";
		} else if (c === "#") {
			while (i < sql.length && sql[i] !== "\n") i++;
			out += " ";
		} else if (c === "/" && n === "*") {
			const end = sql.indexOf("*/", i + 2);
			i = end < 0 ? sql.length : end + 2;
			out += " ";
		} else if (c === "'" || c === '"' || c === "`") {
			i++;
			// '' and "" are escaped quotes; a backslash escapes in mysql strings. Treating a
			// backslash as an escape can only hide more text, never expose a `;` that is a literal.
			while (i < sql.length) {
				if (sql[i] === "\\" && c !== "`") i += 2;
				else if (sql[i] === c) {
					if (sql[i + 1] === c) i += 2;
					else break;
				} else i++;
			}
			i++;
			out += " ";
		} else if (c === "$") {
			const m = /^\$[A-Za-z_]*\$/.exec(sql.slice(i));
			if (m) {
				const end = sql.indexOf(m[0], i + m[0].length);
				i = end < 0 ? sql.length : end + m[0].length;
				out += " ";
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

function assertSingleStatement(sql: string): void {
	const code = stripLiterals(sql)
		.trim()
		.replace(/;+\s*$/, "");
	if (code.includes(";"))
		throw new SqlError(
			"multi_statement",
			"read-only queries must be a single statement",
		);
}

const WRITE_WORDS =
	/\b(insert|update|delete|merge|replace(?!\s*\()|into|create|drop|alter|truncate|grant|revoke|call|do|copy|analyze|analyse|vacuum|reindex|cluster|refresh|lock|set|load|handler|prepare|execute|begin|commit|rollback|start)\b/i;

/**
 * True for a single plain SELECT, SHOW, EXPLAIN (without ANALYZE) or WITH ... SELECT. Anything else,
 * including data-modifying CTEs, SELECT INTO, FOR UPDATE and multi-statement text, needs the confirm
 * dialog in the UI console. Conservative: a false "not plain" only costs one click.
 */
export function isPlainRead(sql: string): boolean {
	const code = stripLiterals(sql)
		.trim()
		.replace(/;+\s*$/, "");
	if (!code || code.includes(";")) return false;
	const first = /^[(\s]*([a-z]+)/i.exec(code)?.[1]?.toLowerCase();
	if (
		!first ||
		!["select", "show", "explain", "with", "values", "table"].includes(first)
	)
		return false;
	return !WRITE_WORDS.test(code);
}

export type ToolRisk = "read" | "write";

/** A tool the agent loop can register; defined here so Task 28/29 need no dependency on this module's backends. */
export interface ToolDef<Ctx> {
	name: string;
	description: string;
	risk: ToolRisk;
	/** JSON Schema for `args`. */
	schema: Record<string, unknown>;
	run(ctx: Ctx, args: unknown): Promise<unknown>;
}
