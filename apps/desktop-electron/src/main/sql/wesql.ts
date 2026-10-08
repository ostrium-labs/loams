// WeSQL (MySQL 8.0 compatible) backend for the desktop, against the dev compose (deploy/wesql).
//
// SEAM: the UI and agent only see `MySqlBackend`. The production backend (docs/design/47: MySQL 8.4
// + mywal + Vitess, `loams.sqldb.v1`) is a second implementation backed by the active server's
// control plane, selected where `createWesqlBackend` is called.
import { randomBytes } from "node:crypto";
import { createConnection } from "mysql2/promise";
import type {
	SqlConnection,
	WesqlSchema,
	WesqlTable,
} from "../../shared/contracts";
import { Secret } from "../factory/vault";
import {
	CLIENT_GRACE_MS,
	DEFAULT_CAPS,
	isPlainRead,
	type RawResult,
	runCapped,
	SqlError,
	type SqlSession,
	type ToolDef,
	toSqlError,
	withDeadline,
} from "./caps";
import { newPasswordRef, type SqlBackend } from "./pg";

/** Compose constants (deploy/wesql/compose.yaml). The root password is the compose default unless overridden the same way. */
export function wesqlDev(env: NodeJS.ProcessEnv = process.env) {
	return {
		host: "127.0.0.1",
		port: 13306,
		database: "",
		user: "root",
		password: new Secret(env.WESQL_ROOT_PASSWORD || "loams-dev"),
	};
}

/** The dedicated SELECT-only account used by agent reads. */
export const RO_USER = "loams_ro";

/** Never granted to `loams_ro`. information_schema stays visible by MySQL design but lists only granted objects. */
export const SYSTEM_SCHEMAS = new Set([
	"mysql",
	"sys",
	"performance_schema",
	"information_schema",
]);

/** A schema name as a GRANT target: quoted, with `_` and `%` escaped so it cannot act as a pattern. */
export function grantTarget(db: string): string {
	return `\`${db.replace(/\\/g, "\\\\").replace(/`/g, "``").replace(/[_%]/g, "\\$&")}\``;
}

export interface MySqlBackend extends SqlBackend {
	schemas(): Promise<WesqlSchema[]>;
	tables(schema: string): Promise<WesqlTable[]>;
}

export type MySqlSession = SqlSession & {
	params(sql: string, params: unknown[]): Promise<RawResult>;
	close(): Promise<void>;
};
export interface MySqlLogin {
	user: string;
	password: Secret;
}
export type MySqlConnect = (as?: MySqlLogin) => Promise<MySqlSession>;

export function connectMySql(
	env: NodeJS.ProcessEnv = process.env,
): MySqlConnect {
	return async (as) => {
		const d = wesqlDev(env);
		const c = await createConnection({
			host: d.host,
			port: d.port,
			user: as?.user ?? d.user,
			password: (as?.password ?? d.password).reveal(),
			connectTimeout: 5000,
			rowsAsArray: true,
			supportBigNumbers: true,
			multipleStatements: false,
		});
		c.on("error", () => {});
		let dead = false;
		const kill = () => {
			dead = true;
			c.destroy();
		};
		// Streams rows and, at `limit`, drops the connection: a huge result never reaches memory.
		const streamed = (sql: string, limit: number) =>
			new Promise<RawResult>((resolve, reject) => {
				const rows: unknown[][] = [];
				let columns: string[] = [];
				let settled = false;
				const q = (
					c as unknown as { connection: { query(o: never): unknown } }
				).connection.query({
					sql,
					rowsAsArray: true,
				} as never) as unknown as {
					on(ev: string, cb: (...a: never[]) => void): void;
				};
				const finish = () => {
					if (settled) return;
					settled = true;
					resolve({ columns, rows });
				};
				q.on("fields", ((f: { name: string }[]) => {
					if (Array.isArray(f)) columns = f.map((x) => x.name); // null for writes
				}) as never);
				q.on("result", ((row: unknown) => {
					if (settled) return;
					if (!Array.isArray(row)) {
						// OK packet from a write: the driver emits no 'end' for it.
						finish();
						return;
					}
					rows.push(row);
					if (rows.length >= limit) {
						kill();
						finish();
					}
				}) as never);
				q.on("error", ((e: Error) => {
					if (settled) return;
					settled = true;
					reject(e);
				}) as never);
				q.on("end", finish as never);
			});
		const plain = async (
			sql: string,
			params?: unknown[],
		): Promise<RawResult> => {
			const [rows, fields] = params
				? await c.execute({ sql, rowsAsArray: true }, params as never[])
				: await c.query({ sql, rowsAsArray: true });
			if (!Array.isArray(fields)) return { columns: [], rows: [] };
			return {
				columns: fields.map((f) => f.name),
				rows: rows as unknown as unknown[][],
			};
		};
		const deadline = <T>(p: Promise<T>) =>
			withDeadline(p, DEFAULT_CAPS.timeoutMs + CLIENT_GRACE_MS, kill);
		return {
			dialect: "mysql",
			query: (sql, opts) => {
				if (dead) return Promise.resolve({ columns: [], rows: [] });
				return deadline(
					opts?.limit === undefined ? plain(sql) : streamed(sql, opts.limit),
				);
			},
			params: (sql, params) => deadline(plain(sql, params)),
			close: () => c.end().catch(() => c.destroy()),
		};
	};
}

export function createWesqlBackend(
	deps: { connect?: MySqlConnect; env?: NodeJS.ProcessEnv } = {},
): MySqlBackend {
	const dev = wesqlDev(deps.env);
	const connect = deps.connect ?? connectMySql(deps.env);
	const ref = newPasswordRef();
	// Per app run, in memory only; reset on every run via ALTER USER.
	const roPassword = new Secret(randomBytes(24).toString("base64url"));
	let roReady: Promise<void> | undefined;
	const secrets = [dev.password, roPassword];

	/** Schemas `loams_ro` already holds SELECT on in this run. */
	const granted = new Set<string>();

	/**
	 * Creates the SELECT-only account on first use, then on every agent read grants SELECT on each
	 * non-system schema it does not hold yet (diff of SHOW DATABASES against `granted`). There is no
	 * global grant, so mysql.* (account hashes) stays unreadable.
	 */
	async function ensureReadOnlyUser(): Promise<void> {
		const s = await connect();
		try {
			const u = `'${RO_USER}'@'%'`;
			if (!roReady) {
				const pw = roPassword.reveal();
				roReady = (async () => {
					await s.query(`CREATE USER IF NOT EXISTS ${u} IDENTIFIED BY '${pw}'`);
					await s.query(`ALTER USER ${u} IDENTIFIED BY '${pw}'`);
					await s.query(`REVOKE ALL PRIVILEGES, GRANT OPTION FROM ${u}`);
				})().catch((e) => {
					roReady = undefined;
					throw e;
				});
			}
			await roReady;
			const dbs = (await s.query("SHOW DATABASES")).rows.map((r) =>
				String(r[0]),
			);
			for (const db of dbs) {
				if (SYSTEM_SCHEMAS.has(db.toLowerCase()) || granted.has(db)) continue;
				await s.query(`GRANT SELECT ON ${grantTarget(db)}.* TO ${u}`);
				granted.add(db);
			}
		} finally {
			await s.close();
		}
	}

	async function withSession<T>(
		fn: (s: MySqlSession) => Promise<T>,
		as?: MySqlLogin,
	): Promise<T> {
		let s: MySqlSession;
		try {
			s = await connect(as);
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
			passwordRef: ref,
		}),
		password: () => dev.password,
		async query(sql, opts) {
			if (opts?.agent) {
				try {
					await ensureReadOnlyUser();
				} catch (e) {
					throw toSqlError(e, secrets);
				}
				return withSession((s) => runCapped(s, sql, { readOnly: true }), {
					user: RO_USER,
					password: roPassword,
				});
			}
			return withSession((s) =>
				runCapped(s, sql, { readOnly: opts?.readOnly }),
			);
		},
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

/** Pure tool definitions for the agent registry (Tasks 28/29). SQL runs read-only, as a SELECT-only user. */
export const wesqlTools: ToolDef<WesqlToolCtx>[] = [
	{
		name: "wesql_sql",
		description:
			"Run one read-only SQL statement (SELECT, SHOW, EXPLAIN, WITH ... SELECT) on the local WeSQL. Runs as a SELECT-only user in START TRANSACTION READ ONLY, 30 s timeout, at most 1000 rows.",
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
			if (!isPlainRead(sql, "mysql"))
				throw new SqlError(
					"read_only",
					"wesql_sql only runs a single SELECT, SHOW, EXPLAIN or WITH ... SELECT",
				);
			return ctx.wesql.query(sql, { agent: true });
		},
	},
];
