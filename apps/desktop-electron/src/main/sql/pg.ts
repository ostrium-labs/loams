// Postgres backend for the desktop. v0.1 talks to the dev compose (deploy/loams-postgres-dev): pageserver HTTP
// for tenants/timelines/branches and the compute (55433) for SQL.
//
// SEAM: everything the UI and agent use goes through `PostgresBackend`. The production backend
// (docs/design/46: Neon + loams-wal + PgDog, `loams.postgres.v1`) will be a second implementation
// backed by the active server's control plane; `createPgBackend` is the one place that picks, so
// when the server advertises that API a `ControlPlanePostgresBackend` slots in there.
import { randomBytes, randomUUID } from "node:crypto";
import { Client } from "pg";
import Cursor from "pg-cursor";
import type {
	PgTimeline,
	PgWalStatus,
	SqlConnection,
	SqlResult,
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
import type { NeonClient, PgBranchInput } from "./neon";

/** Compose constants (deploy/loams-postgres-dev/compose.yaml, compute/config.json). */
export const PG_DEV = {
	host: "127.0.0.1",
	port: 55433,
	database: "postgres",
	user: "cloud_admin",
} as const;
const PG_PASSWORD = new Secret("cloud_admin");
/**
 * The dedicated login the agent's reads connect as: NOSUPERUSER, no role membership (so no
 * pg_read_all_data and no path to pg_authid), SELECT only on the non-system schemas.
 */
export const PG_RO_USER = "loams_ro";
/** Bounds each grant statement so a lock held elsewhere cannot stall an agent read. */
export const GRANT_STATEMENT_TIMEOUT_MS = 5000;

export interface SqlBackend {
	connection(): SqlConnection;
	/** The secret, for revealPassword only. Never logged, never in `connection()`. */
	password(): Secret;
	/** `agent` implies `readOnly` and runs as a SELECT-only login (pg: loams_ro; mysql: a SELECT-only user). */
	query(
		sql: string,
		opts?: { readOnly?: boolean; agent?: boolean },
	): Promise<SqlResult>;
}

export interface PostgresBackend extends SqlBackend {
	tenants(): Promise<string[]>;
	timelines(tenant: string): Promise<PgTimeline[]>;
	createBranch(tenant: string, b: PgBranchInput): Promise<PgTimeline>;
	walStatus(tenant: string, timeline: string): Promise<PgWalStatus>;
}

/** An opened connection plus its close. */
export type OpenSession = SqlSession & { close(): Promise<void> };
export interface PgLogin {
	user: string;
	password: Secret;
}
export type PgConnect = (as?: PgLogin) => Promise<OpenSession>;

/** An opaque, random-per-process id; the renderer hands it back, it is never derived from the password. */
export function newPasswordRef(): string {
	return randomUUID();
}

const SIMPLE_AFTER_CURSOR = /multiple commands/i;

export const connectPg: PgConnect = async (as) => {
	const c = new Client({
		...PG_DEV,
		user: as?.user ?? PG_DEV.user,
		password: (as?.password ?? PG_PASSWORD).reveal(),
		connectionTimeoutMillis: 5000,
	});
	// A dropped connection must not crash main with an unhandled 'error' event, and must fail the
	// statement in flight (e.g. `pg_terminate_backend(pg_backend_pid())`) instead of leaving it hanging.
	const gone = new Promise<never>((_, reject) => {
		c.on("error", (e) => reject(e));
		c.on("end", () =>
			reject(new SqlError("connection_closed", "the connection was closed")),
		);
	});
	gone.catch(() => {});
	try {
		await c.connect();
	} catch (e) {
		await c.end().catch(() => {});
		throw e;
	}
	const kill = () =>
		(
			c as unknown as { connection: { stream: { destroy(): void } } }
		).connection.stream.destroy();
	const simple = async (sql: string): Promise<RawResult> => {
		const res = await c.query({ text: sql, rowMode: "array" });
		const last = (Array.isArray(res) ? res[res.length - 1] : res) as
			| { fields?: { name: string }[]; rows?: unknown[] }
			| undefined;
		return {
			columns: (last?.fields ?? []).map((f) => f.name),
			rows: (last?.rows ?? []) as unknown[][],
		};
	};
	// Fetches at most `limit` rows through a portal, then closes it: a huge result never reaches memory.
	const limited = async (sql: string, limit: number): Promise<RawResult> => {
		const cur = c.query(new Cursor(sql, [], { rowMode: "array" }));
		try {
			const rows = (await cur.read(limit)) as unknown[][];
			const fields =
				(cur as unknown as { _result?: { fields?: { name: string }[] } })
					._result?.fields ?? [];
			return { columns: fields.map((f) => f.name), rows };
		} finally {
			await new Promise<void>((r) => cur.close(() => r()));
		}
	};
	return {
		dialect: "pg",
		query(sql, opts) {
			const run = async (): Promise<RawResult> => {
				if (opts?.limit === undefined) return simple(sql);
				try {
					return await limited(sql, opts.limit);
				} catch (e) {
					// Writes the user confirmed may stack statements; the extended protocol refuses those.
					if (
						!opts.single &&
						SIMPLE_AFTER_CURSOR.test(String((e as Error)?.message))
					)
						return simple(sql);
					throw e;
				}
			};
			return withDeadline(
				Promise.race([run(), gone]),
				DEFAULT_CAPS.timeoutMs + CLIENT_GRACE_MS,
				kill,
			);
		},
		close: () => c.end().catch(() => {}),
	};
};

/** 28P01 invalid_password, 28000 invalid_authorization_specification (role missing). */
export function isLoginFailure(e: unknown): boolean {
	const c = (e as { code?: unknown })?.code;
	return c === "28P01" || c === "28000";
}

/** 42501 insufficient_privilege: a schema or table created after the last grant. */
export function isPermissionMiss(e: unknown): boolean {
	return (e as { code?: unknown })?.code === "42501";
}

/**
 * A denial on a system object (pg_catalog, pg_toast, information_schema, a pg_* table or
 * function) that no grant run will ever fix: loams_ro is meant not to read those.
 */
export function isSystemObjectDenial(message: string): boolean {
	return /permission denied for \w+ "?(pg_\w+|information_schema)\b/i.test(
		message,
	);
}

/** At most one grant refresh per window, per server (one backend per server). */
export const REGRANT_MIN_INTERVAL_MS = 10_000;

export function createPgBackend(deps: {
	neon: NeonClient;
	connect?: PgConnect;
	now?: () => number;
}): PostgresBackend {
	const connect = deps.connect ?? connectPg;
	const now = deps.now ?? Date.now;
	/** When a 42501 last triggered a grant refresh. */
	let lastRefresh = Number.NEGATIVE_INFINITY;
	const ref = newPasswordRef();
	// Per app run, in memory only; reset on every run via ALTER ROLE.
	const roPassword = new Secret(randomBytes(24).toString("base64url"));
	const secrets = [PG_PASSWORD, roPassword];
	/** True once the login exists with this run's password and holds no memberships. */
	let provisioned = false;
	/** Schemas already granted to the login; cleared when a read is denied. */
	const granted = new Set<string>();
	/** Single flight: concurrent reads share one admin session and one grant run. */
	let inflight: Promise<void> | undefined;

	/**
	 * Creates (or refreshes) the agent's login once per run and grants SELECT on every
	 * non-system schema not granted yet. The session user of an agent read is this role,
	 * never the admin: a superuser session could `set_config('role', ...)` its way back.
	 * Nothing runs while the cache is warm; a read denied with 42501 clears it (see query).
	 */
	async function grantRun(): Promise<void> {
		let s: OpenSession;
		try {
			s = await connect();
		} catch (e) {
			throw toSqlError(e, secrets);
		}
		try {
			await s.query(`SET statement_timeout = ${GRANT_STATEMENT_TIMEOUT_MS}`);
			if (!provisioned) {
				const exists = await s.query(
					`SELECT 1 FROM pg_roles WHERE rolname = '${PG_RO_USER}'`,
				);
				const attrs =
					"LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS INHERIT";
				await s.query(
					`${exists.rows.length ? "ALTER" : "CREATE"} ROLE ${PG_RO_USER} ${attrs} PASSWORD '${roPassword.reveal()}'`,
				);
				const revokes = await s.query(
					`SELECT format('REVOKE %s FROM ${PG_RO_USER}', roleid::regrole) FROM pg_auth_members WHERE member = '${PG_RO_USER}'::regrole`,
				);
				for (const row of revokes.rows) await s.query(String(row[0]));
				provisioned = true;
			}
			// Built server-side with %I so identifiers are always quoted. Default privileges make
			// tables the admin creates later readable without another grant run.
			const schemas = await s.query(
				`SELECT nspname, format('GRANT USAGE ON SCHEMA %1$I TO ${PG_RO_USER}; GRANT SELECT ON ALL TABLES IN SCHEMA %1$I TO ${PG_RO_USER}; GRANT SELECT ON ALL SEQUENCES IN SCHEMA %1$I TO ${PG_RO_USER}; ALTER DEFAULT PRIVILEGES FOR ROLE ${PG_DEV.user} IN SCHEMA %1$I GRANT SELECT ON TABLES TO ${PG_RO_USER}', nspname) FROM pg_namespace WHERE nspname <> 'information_schema' AND nspname NOT LIKE 'pg\\_%'`,
			);
			for (const [name, stmt] of schemas.rows) {
				if (granted.has(String(name))) continue;
				await s.query(String(stmt));
				granted.add(String(name));
			}
		} catch (e) {
			throw toSqlError(e, secrets);
		} finally {
			await s.close();
		}
	}

	/** Runs a grant pass unless the cache is warm; concurrent callers share it. */
	function ensureReadOnlyRole(): Promise<void> {
		if (provisioned && granted.size > 0 && !inflight) return Promise.resolve();
		inflight ??= grantRun().finally(() => {
			inflight = undefined;
		});
		return inflight;
	}

	return {
		connection: () => ({
			host: PG_DEV.host,
			port: PG_DEV.port,
			database: PG_DEV.database,
			user: PG_DEV.user,
			passwordRef: ref,
		}),
		password: () => PG_PASSWORD,
		async query(sql, opts) {
			const run = async (as?: PgLogin): Promise<SqlResult> => {
				let s: OpenSession;
				try {
					s = await connect(as);
				} catch (e) {
					throw toSqlError(e, secrets);
				}
				try {
					return await runCapped(s, sql, {
						readOnly: opts?.readOnly || opts?.agent,
					});
				} catch (e) {
					throw toSqlError(e, secrets);
				} finally {
					await s.close();
				}
			};
			if (!opts?.agent) return run();
			const login = { user: PG_RO_USER, password: roPassword };
			try {
				await ensureReadOnlyRole();
				return await run(login);
			} catch (e) {
				// Login refused: the stack was recreated since the role was made. Denied: a schema
				// or table appeared since the last grant. Either way refresh, then retry once.
				if (isLoginFailure(e)) provisioned = false;
				else if (
					!isPermissionMiss(e) ||
					isSystemObjectDenial(String((e as Error)?.message)) ||
					now() - lastRefresh < REGRANT_MIN_INTERVAL_MS
				)
					throw toSqlError(e, secrets);
				else lastRefresh = now();
				granted.clear();
				await ensureReadOnlyRole();
				return run(login);
			}
		},
		tenants: () => deps.neon.tenants(),
		timelines: (t) => deps.neon.timelines(t),
		createBranch: (t, b) => deps.neon.createBranch(t, b),
		walStatus: (t, tl) => deps.neon.walStatus(t, tl),
	};
}

export interface PgToolCtx {
	pg: PostgresBackend;
}

const str = (v: unknown, name: string): string => {
	if (typeof v !== "string" || !v)
		throw new SqlError("invalid", `${name} is required`);
	return v;
};

/** Pure tool definitions for the agent registry (Tasks 28/29). SQL runs read-only, always. */
export const pgTools: ToolDef<PgToolCtx>[] = [
	{
		name: "pg_sql",
		description:
			"Run one read-only SQL statement (SELECT, SHOW, EXPLAIN, WITH ... SELECT) on the local Postgres. Runs in a READ ONLY transaction, 30 s timeout, at most 1000 rows.",
		risk: "read",
		schema: {
			type: "object",
			properties: { sql: { type: "string" } },
			required: ["sql"],
			additionalProperties: false,
		},
		async run(ctx, args) {
			const sql = str((args as { sql?: unknown })?.sql, "sql");
			if (!isPlainRead(sql, "pg"))
				throw new SqlError(
					"read_only",
					"pg_sql only runs a single SELECT, SHOW, EXPLAIN or WITH ... SELECT",
				);
			return ctx.pg.query(sql, { agent: true });
		},
	},
	{
		name: "pg_branch_create",
		description:
			"Create a copy-on-write Postgres branch (a new Neon timeline) from an existing timeline, optionally at a past LSN.",
		risk: "write",
		schema: {
			type: "object",
			properties: {
				tenant: { type: "string", description: "tenant id, 32 hex" },
				name: { type: "string" },
				ancestorTimelineId: { type: "string", description: "32 hex" },
				ancestorStartLsn: { type: "string", description: "e.g. 0/16B3748" },
			},
			required: ["tenant", "name", "ancestorTimelineId"],
			additionalProperties: false,
		},
		async run(ctx, args) {
			const a = (args ?? {}) as Record<string, unknown>;
			return ctx.pg.createBranch(str(a.tenant, "tenant"), {
				name: str(a.name, "name"),
				ancestorTimelineId: str(a.ancestorTimelineId, "ancestorTimelineId"),
				ancestorStartLsn:
					typeof a.ancestorStartLsn === "string"
						? a.ancestorStartLsn
						: undefined,
			});
		},
	},
];
