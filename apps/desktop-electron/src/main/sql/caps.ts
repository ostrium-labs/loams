// Shared SQL plumbing for the pg and mysql backends: row/time caps, the read-only wrapper,
// statement classification, error mapping and secret redaction. Pure: no driver or electron imports.
import type { IpcResult, SqlResult } from "../../shared/contracts";
import {
	isSingleStatement,
	isWrite,
	type SqlDialect,
} from "../../shared/sql-lex";
import type { Secret } from "../factory/vault";
import { redact as redactSecrets } from "../redact";

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
	 * `limit` is a memory bound: the session must stop reading after `limit` rows (pg: cursor
	 * fetch; mysql: stream, then destroy the connection) rather than buffer the whole result.
	 */
	query(
		sql: string,
		opts?: { single?: boolean; limit?: number },
	): Promise<RawResult>;
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
	if (ro && !isSingleStatement(sql, lexDialect(exec.dialect)))
		throw new SqlError(
			"multi_statement",
			"read-only queries must be a single statement with unambiguous quoting",
		);
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
		const raw = await exec.query(sql, { single: ro, limit: maxRows + 1 });
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

/**
 * Client-side deadline for one statement: when the server-side timeout does not fire (hung network,
 * stuck handshake), `kill` drops the connection and the call rejects with code `timeout`.
 */
export function withDeadline<T>(
	p: Promise<T>,
	ms: number,
	kill: () => void,
): Promise<T> {
	let timer: ReturnType<typeof setTimeout> | undefined;
	const t = new Promise<never>((_, reject) => {
		timer = setTimeout(() => {
			try {
				kill();
			} catch {
				// already closed
			}
			reject(new SqlError("timeout", `no answer within ${ms} ms`));
		}, ms);
	});
	return Promise.race([p, t]).finally(() => clearTimeout(timer));
}

/** Client deadline = server timeout plus grace, so the server's own error wins when it works. */
export const CLIENT_GRACE_MS = 5000;

export function toSqlError(
	e: unknown,
	secrets: (string | Secret)[] = [],
): SqlError {
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
export function redact(
	text: string,
	secrets: (string | Secret)[] = [],
): string {
	return redactSecrets(text, secrets, { mask: "***", credentials: true });
}

/** Wraps a handler body as an IpcResult; thrown errors become `{code, message}` (redacted). */
export async function toResult<T>(
	fn: () => Promise<T>,
	secrets: (string | Secret)[] = [],
): Promise<IpcResult<T>> {
	try {
		return { ok: true, value: await fn() };
	} catch (e) {
		const err = toSqlError(e, secrets);
		return { ok: false, code: err.code, message: err.message };
	}
}

const lexDialect = (d: Dialect): SqlDialect =>
	d === "pg" ? "postgres" : "mysql";

/** True for a single plain SELECT, SHOW, EXPLAIN or WITH-SELECT without known side effects (see shared/sql-lex). */
export function isPlainRead(sql: string, dialect: Dialect): boolean {
	return sql.trim() !== "" && !isWrite(sql, lexDialect(dialect));
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
