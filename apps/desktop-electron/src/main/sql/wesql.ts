// WeSQL (MySQL 8.0 compatible) backend for the desktop, against the dev compose (deploy/wesql).
//
// SEAM: the UI and agent only see `MySqlBackend`. The production backend (docs/design/47: MySQL 8.4
// + mywal + Vitess, `loams.sqldb.v1`) is a second implementation backed by the active server's
// control plane, selected where `createWesqlBackend` is called.
import { createConnection } from "mysql2/promise";
import type { SqlConnection, WesqlSchema, WesqlTable } from "../../shared/contracts";
import {
	isPlainRead,
	type RawResult,
	runCapped,
	SqlError,
	type SqlSession,
	type ToolDef,
	toSqlError,
} from "./caps";
import { passwordRef, type SqlBackend } from "./pg";

/** Compose constants (deploy/wesql/compose.yaml). The root password is the compose default unless overridden the same way. */
export function wesqlDev(env: NodeJS.ProcessEnv = process.env) {
	return {
		host: "127.0.0.1",
		port: 13306,
		database: "",
		user: "root",
		password: env.WESQL_ROOT_PASSWORD || "loams-dev",
	};
}

export interface MySqlBackend extends SqlBackend {
	schemas(): Promise<WesqlSchema[]>;
	tables(schema: string): Promise<WesqlTable[]>;
}

export type MySqlSession = SqlSession & {
	params(sql: string, params: unknown[]): Promise<RawResult>;
	close(): Promise<void>;
};
export type MySqlConnect = () => Promise<MySqlSession>;

export function connectMySql(
	env: NodeJS.ProcessEnv = process.env,
): MySqlConnect {
	return async () => {
		const d = wesqlDev(env);
		const c = await createConnection({
			host: d.host,
			port: d.port,
			user: d.user,
			password: d.password,
			connectTimeout: 5000,
			rowsAsArray: true,
			supportBigNumbers: true,
			dateStrings: false,
		});
		c.on("error", () => {});
		const run = async (sql: string, params?: unknown[]): Promise<RawResult> => {
			const [rows, fields] = params
				? await c.execute({ sql, rowsAsArray: true }, params as never[])
				: await c.query({ sql, rowsAsArray: true });
			if (!Array.isArray(fields)) return { columns: [], rows: [] }; // OK packet from a write
			return {
				columns: fields.map((f) => f.name),
				rows: rows as unknown as unknown[][],
			};
		};
		return {
			dialect: "mysql",
			query: (sql) => run(sql),
			params: run,
			close: () => c.end().catch(() => {}),
		};
	};
}

export function createWesqlBackend(
	deps: { connect?: MySqlConnect; env?: NodeJS.ProcessEnv } = {},
): MySqlBackend {
	const dev = wesqlDev(deps.env);
	const connect = deps.connect ?? connectMySql(deps.env);
	const secrets = [dev.password];
	async function withSession<T>(
		fn: (s: MySqlSession) => Promise<T>,
	): Promise<T> {
		let s: MySqlSession;
		try {
			s = await connect();
		} catch (e) {
			throw toSqlError(e, secrets);
		}
		try {
			return await fn(s);
		} catch (e) {
			throw toSqlError(e, secrets);
		} finally {
			await s.close();
		}
	}
	return {
		connection: (): SqlConnection => ({
			host: dev.host,
			port: dev.port,
			database: dev.database,
			user: dev.user,
			passwordRef: passwordRef("wesql", dev.password),
		}),
		password: () => dev.password,
		query: (sql, opts) =>
			withSession((s) => runCapped(s, sql, { readOnly: opts?.readOnly })),
		schemas: () =>
			withSession(async (s) => {
				const r = await s.query(
					"SELECT SCHEMA_NAME FROM information_schema.SCHEMATA ORDER BY SCHEMA_NAME",
				);
				return r.rows.map((x) => ({ name: String(x[0]) }));
			}),
		tables: (schema) =>
			withSession(async (s) => {
				if (typeof schema !== "string" || !schema)
					throw new SqlError("invalid", "schema is required");
				const r = await s.params(
					"SELECT TABLE_NAME, ENGINE, TABLE_ROWS FROM information_schema.TABLES WHERE TABLE_SCHEMA = ? ORDER BY TABLE_NAME",
					[schema],
				);
				return r.rows.map((x) => ({
					name: String(x[0]),
					engine: x[1] == null ? "" : String(x[1]),
					rows: Number(x[2] ?? 0),
				}));
			}),
	};
}

export interface WesqlToolCtx {
	wesql: MySqlBackend;
}

/** Pure tool definitions for the agent registry (Tasks 28/29). SQL runs read-only, always. */
export const wesqlTools: ToolDef<WesqlToolCtx>[] = [
	{
		name: "wesql_sql",
		description:
			"Run one read-only SQL statement (SELECT, SHOW, EXPLAIN, WITH ... SELECT) on the local WeSQL. Runs in START TRANSACTION READ ONLY, 30 s timeout, at most 1000 rows.",
		risk: "read",
		schema: {
			type: "object",
			properties: { sql: { type: "string" } },
			required: ["sql"],
			additionalProperties: false,
		},
		async run(ctx, args) {
			const sql = (args as { sql?: unknown })?.sql;
			if (typeof sql !== "string" || !sql)
				throw new SqlError("invalid", "sql is required");
			if (!isPlainRead(sql))
				throw new SqlError(
					"read_only",
					"wesql_sql only runs a single SELECT, SHOW, EXPLAIN or WITH ... SELECT",
				);
			return ctx.wesql.query(sql, { readOnly: true });
		},
	},
];
