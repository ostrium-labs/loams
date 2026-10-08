// Postgres backend for the desktop. v0.1 talks to the dev compose (deploy/neon): pageserver HTTP
// for tenants/timelines/branches and the compute (55433) for SQL.
//
// SEAM: everything the UI and agent use goes through `PostgresBackend`. The production backend
// (docs/design/46: Neon + loams-wal + PgDog, `loams.postgres.v1`) will be a second implementation
// backed by the active server's control plane; `createPgBackend` is the one place that picks, so
// when the server advertises that API a `ControlPlanePostgresBackend` slots in there.
import { createHash } from "node:crypto";
import { Client } from "pg";
import type {
	PgTimeline,
	PgWalStatus,
	SqlConnection,
	SqlResult,
} from "../../shared/contracts";
import {
	isPlainRead,
	type RawResult,
	runCapped,
	SqlError,
	type SqlSession,
	type ToolDef,
	toSqlError,
} from "./caps";
import type { NeonClient, PgBranchInput } from "./neon";

/** Compose constants (deploy/neon/compose.yaml, compute/config.json). */
export const PG_DEV = {
	host: "127.0.0.1",
	port: 55433,
	database: "postgres",
	user: "cloud_admin",
	password: "cloud_admin",
} as const;

export interface SqlBackend {
	connection(): SqlConnection;
	/** The secret, for revealPassword only. Never logged, never in `connection()`. */
	password(): string;
	query(sql: string, opts?: { readOnly?: boolean }): Promise<SqlResult>;
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

export function passwordRef(kind: string, password: string): string {
	return createHash("sha256")
		.update(`${kind}:${password}`)
		.digest("hex")
		.slice(0, 16);
}

export const connectPg: PgConnect = async () => {
	const c = new Client({ ...PG_DEV, connectionTimeoutMillis: 5000 });
	// A dropped idle connection must not crash main with an unhandled 'error' event.
	c.on("error", () => {});
	try {
		await c.connect();
	} catch (e) {
		await c.end().catch(() => {});
		throw e;
	}
	return {
		dialect: "pg",
		async query(sql, opts): Promise<RawResult> {
			const res = await c.query({
				text: sql,
				rowMode: "array",
				...(opts?.single ? { queryMode: "extended" as const } : {}),
			});
			const last = (Array.isArray(res) ? res[res.length - 1] : res) as
				| { fields?: { name: string }[]; rows?: unknown[] }
				| undefined;
			return {
				columns: (last?.fields ?? []).map((f) => f.name),
				rows: (last?.rows ?? []) as unknown[][],
			};
		},
		close: () => c.end().catch(() => {}),
	};
};

export function createPgBackend(deps: {
	neon: NeonClient;
	connect?: PgConnect;
}): PostgresBackend {
	const connect = deps.connect ?? connectPg;
	return {
		connection: () => ({
			host: PG_DEV.host,
			port: PG_DEV.port,
			database: PG_DEV.database,
			user: PG_DEV.user,
			passwordRef: passwordRef("pg", PG_DEV.password),
		}),
		password: () => PG_DEV.password,
		async query(sql, opts) {
			let s: OpenSession;
			try {
				s = await connect();
			} catch (e) {
				throw toSqlError(e, [PG_DEV.password]);
			}
			try {
				return await runCapped(s, sql, { readOnly: opts?.readOnly });
			} catch (e) {
				throw toSqlError(e, [PG_DEV.password]);
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
			if (!isPlainRead(sql))
				throw new SqlError(
					"read_only",
					"pg_sql only runs a single SELECT, SHOW, EXPLAIN or WITH ... SELECT",
				);
			return ctx.pg.query(sql, { readOnly: true });
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
