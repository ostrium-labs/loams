// Postgres backend for the desktop. v0.1 talks to the dev compose (deploy/neon): pageserver HTTP
// for tenants/timelines/branches and the compute (55433) for SQL.
//
// SEAM: everything the UI and agent use goes through `PostgresBackend`. The production backend
// (docs/design/46: Neon + loams-wal + PgDog, `loams.postgres.v1`) will be a second implementation
// backed by the active server's control plane; `createPgBackend` is the one place that picks, so
// when the server advertises that API a `ControlPlanePostgresBackend` slots in there.
import { randomUUID } from "node:crypto";
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

/** Compose constants (deploy/neon/compose.yaml, compute/config.json). */
export const PG_DEV = {
	host: "127.0.0.1",
	port: 55433,
	database: "postgres",
	user: "cloud_admin",
} as const;
const PG_PASSWORD = new Secret("cloud_admin");
/** pg >= 14 predefined role; assumed by agent reads so admin functions fail in the database itself. */
export const PG_AGENT_ROLE = "pg_read_all_data";

export interface SqlBackend {
	connection(): SqlConnection;
	/** The secret, for revealPassword only. Never logged, never in `connection()`. */
	password(): Secret;
	/** `agent` implies `readOnly` and also drops privileges (pg: SET LOCAL ROLE; mysql: a SELECT-only user). */
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
export type PgConnect = () => Promise<OpenSession>;

/** An opaque, random-per-process id; the renderer hands it back, it is never derived from the password. */
export function newPasswordRef(): string {
	return randomUUID();
}

const SIMPLE_AFTER_CURSOR = /multiple commands/i;

export const connectPg: PgConnect = async () => {
	const c = new Client({
		...PG_DEV,
		password: PG_PASSWORD.reveal(),
		connectionTimeoutMillis: 5000,
	});
	// A dropped idle connection must not crash main with an unhandled 'error' event.
	c.on("error", () => {});
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
				run(),
				DEFAULT_CAPS.timeoutMs + CLIENT_GRACE_MS,
				kill,
			);
		},
		close: () => c.end().catch(() => {}),
	};
};

export function createPgBackend(deps: {
	neon: NeonClient;
	connect?: PgConnect;
}): PostgresBackend {
	const connect = deps.connect ?? connectPg;
	const ref = newPasswordRef();
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
			let s: OpenSession;
			try {
				s = await connect();
			} catch (e) {
				throw toSqlError(e, [PG_PASSWORD]);
			}
			try {
				return await runCapped(s, sql, {
					readOnly: opts?.readOnly || opts?.agent,
					role: opts?.agent ? PG_AGENT_ROLE : undefined,
				});
			} catch (e) {
				throw toSqlError(e, [PG_PASSWORD]);
			} finally {
				await s.close();
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

/** Role switching inside a read-only agent query; the lexer-level belt to the database-level braces. */
const ROLE_ESCAPE = /\b(set_config|reset|role|authorization)\b/i;

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
			if (!isPlainRead(sql, "pg") || ROLE_ESCAPE.test(sql))
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
