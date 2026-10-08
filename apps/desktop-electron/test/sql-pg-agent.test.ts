import { describe, expect, it } from "vitest";
import { NeonClient } from "../src/main/sql/neon";
import {
	createPgBackend,
	isSystemObjectDenial,
	type PgLogin,
} from "../src/main/sql/pg";

const neon = new NeonClient({ branchesFile: "/nonexistent/b.json" });

const grantRow = (schema: string) => [
	schema,
	`GRANT USAGE ON SCHEMA "${schema}" TO loams_ro`,
];

function fakePg(
	opts: {
		failLoginOnce?: boolean;
		/** Agent reads fail with 42501 this many times. */
		denyReads?: number;
		schemas?: string[];
		/** Holds every admin connect until released. */
		gate?: Promise<void>;
		denyMessage?: string;
	} = {},
) {
	const log: string[] = [];
	let failLogin = opts.failLoginOnce === true;
	const state = {
		schemas: opts.schemas ?? ["shop"],
		adminConnects: 0,
		deny: opts.denyReads ?? 0,
	};
	const connect = async (as?: PgLogin) => {
		const who = as?.user ?? "admin";
		if (!as) {
			state.adminConnects++;
			await opts.gate;
		}
		if (as && failLogin) {
			failLogin = false;
			throw Object.assign(new Error("password authentication failed"), {
				code: "28P01",
			});
		}
		return {
			dialect: "pg" as const,
			query: async (sql: string) => {
				log.push(`${who}: ${sql}`);
				if (as && /^SELECT \d/.test(sql) && state.deny > 0) {
					state.deny--;
					throw Object.assign(
						new Error(opts.denyMessage ?? "permission denied for table t"),
						{
							code: "42501",
						},
					);
				}
				if (sql.startsWith("SELECT 1 FROM pg_roles"))
					return { columns: ["?column?"], rows: [] };
				if (sql.includes("FROM pg_auth_members"))
					return {
						columns: ["f"],
						rows: [["REVOKE pg_read_all_data FROM loams_ro"]],
					};
				if (sql.includes("FROM pg_namespace"))
					return { columns: ["n", "f"], rows: state.schemas.map(grantRow) };
				return { columns: ["n"], rows: [[1]] };
			},
			close: async () => {},
		};
	};
	return { connect, log, state };
}

const adminLines = (log: string[]) =>
	log.filter((l) => l.startsWith("admin:")).map((l) => l.slice(7));
const grants = (log: string[]) =>
	adminLines(log).filter((l) => l.startsWith("GRANT USAGE ON SCHEMA"));

describe("postgres agent reads", () => {
	it("agent_reads_connect_as_a_non_superuser_login", async () => {
		const { connect, log } = fakePg();
		const pg = createPgBackend({ neon, connect });
		await pg.query("SELECT 1", { agent: true });
		const admin = adminLines(log);
		expect(admin[0]).toBe("SET statement_timeout = 5000");
		expect(admin.find((l) => l.startsWith("CREATE ROLE"))).toMatch(
			/^CREATE ROLE loams_ro LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS INHERIT PASSWORD '[\w-]{32}'$/,
		);
		// No membership is ever granted; existing ones are revoked.
		expect(admin.some((l) => /GRANT pg_read_all_data/.test(l))).toBe(false);
		expect(
			admin.filter((l) => l.startsWith("REVOKE pg_read_all_data")),
		).toHaveLength(1);
		expect(grants(log)).toEqual(['GRANT USAGE ON SCHEMA "shop" TO loams_ro']);
		const ns = admin.find((l) => l.includes("FROM pg_namespace")) ?? "";
		expect(ns).toContain("NOT LIKE 'pg\\_%'");
		expect(ns).toContain("ALTER DEFAULT PRIVILEGES");
		expect(
			log
				.filter((l) => l.startsWith("loams_ro:"))
				.slice(0, 3)
				.map((l) => l.slice(10)),
		).toEqual([
			"BEGIN READ ONLY",
			"SET LOCAL statement_timeout = 30000",
			"SELECT 1",
		]);
		expect(log.join("\n")).not.toMatch(/SET LOCAL ROLE/);
	});

	it("does_not_regrant_per_read_when_nothing_changed", async () => {
		const { connect, log, state } = fakePg();
		const pg = createPgBackend({ neon, connect });
		await pg.query("SELECT 1", { agent: true });
		const after = log.length;
		const connects = state.adminConnects;
		await pg.query("SELECT 2", { agent: true });
		await pg.query("SELECT 3", { agent: true });
		expect(state.adminConnects).toBe(connects);
		expect(adminLines(log.slice(after))).toEqual([]);
	});

	it("permission_miss_regrants_once_and_retries", async () => {
		const { connect, log, state } = fakePg();
		const pg = createPgBackend({ neon, connect });
		await pg.query("SELECT 1", { agent: true });
		expect(grants(log)).toEqual(['GRANT USAGE ON SCHEMA "shop" TO loams_ro']);
		// A schema created since: the next read is denied once, then the grants refresh.
		state.schemas.push("sales");
		state.deny = 1;
		await expect(pg.query("SELECT 2", { agent: true })).resolves.toMatchObject({
			rowCount: 1,
		});
		expect(grants(log).slice(1)).toEqual([
			'GRANT USAGE ON SCHEMA "shop" TO loams_ro',
			'GRANT USAGE ON SCHEMA "sales" TO loams_ro',
		]);
		// And it is quiet again afterwards.
		const n = log.length;
		await pg.query("SELECT 3", { agent: true });
		expect(adminLines(log.slice(n))).toEqual([]);
	});

	it("a_denial_on_a_system_object_does_not_regrant", async () => {
		const sys = fakePg({
			denyMessage: "permission denied for table pg_authid",
		});
		const pg = createPgBackend({ neon, connect: sys.connect });
		await pg.query("SELECT 1", { agent: true });
		const n = grants(sys.log).length;
		sys.state.deny = 1;
		await expect(pg.query("SELECT 2", { agent: true })).rejects.toMatchObject({
			code: "42501",
		});
		expect(grants(sys.log).length).toBe(n);
		for (const m of [
			"permission denied for table pg_authid",
			"permission denied for schema pg_toast",
			"permission denied for schema information_schema",
			'permission denied for view "pg_shadow"',
			"permission denied for function pg_read_file",
		])
			expect(isSystemObjectDenial(m), m).toBe(true);
		for (const m of [
			"permission denied for table orders",
			"permission denied for schema sales",
			"permission denied for table page_views",
		])
			expect(isSystemObjectDenial(m), m).toBe(false);
	});

	it("regrants_at_most_once_per_10s", async () => {
		let t = 1_000_000;
		const { connect, log, state } = fakePg();
		const pg = createPgBackend({ neon, connect, now: () => t });
		await pg.query("SELECT 1", { agent: true });
		state.deny = 1;
		await pg.query("SELECT 2", { agent: true });
		const after = grants(log).length;
		// A second miss within 10 s is returned, not re-granted.
		t += 5_000;
		state.deny = 1;
		await expect(pg.query("SELECT 3", { agent: true })).rejects.toMatchObject({
			code: "42501",
		});
		expect(grants(log).length).toBe(after);
		// After 10 s it refreshes again.
		t += 6_000;
		state.deny = 1;
		await expect(pg.query("SELECT 4", { agent: true })).resolves.toMatchObject({
			rowCount: 1,
		});
		expect(grants(log).length).toBeGreaterThan(after);
	});

	it("a_second_miss_is_returned_not_looped", async () => {
		const { connect } = fakePg({ denyReads: 5 });
		const pg = createPgBackend({ neon, connect });
		await expect(pg.query("SELECT 1", { agent: true })).rejects.toMatchObject({
			code: "42501",
		});
	});

	it("concurrent_first_reads_share_one_grant_run", async () => {
		let release: () => void = () => {};
		const gate = new Promise<void>((r) => {
			release = r;
		});
		const { connect, log, state } = fakePg({ gate });
		const pg = createPgBackend({ neon, connect });
		const reads = Promise.all([
			pg.query("SELECT 1", { agent: true }),
			pg.query("SELECT 2", { agent: true }),
			pg.query("SELECT 3", { agent: true }),
		]);
		await new Promise((r) => setTimeout(r, 10));
		release();
		await reads;
		expect(state.adminConnects).toBe(1);
		expect(
			adminLines(log).filter((l) => l.startsWith("CREATE ROLE")),
		).toHaveLength(1);
		expect(grants(log)).toHaveLength(1);
	});

	it("reprovisions_once_when_the_login_is_refused", async () => {
		const { connect, log } = fakePg({ failLoginOnce: true });
		const pg = createPgBackend({ neon, connect });
		await expect(pg.query("SELECT 1", { agent: true })).resolves.toMatchObject({
			rowCount: 1,
		});
		expect(log.filter((l) => l.includes("ROLE loams_ro"))).toHaveLength(2);
	});

	it("provisioning_failure_is_a_redacted_sql_error", async () => {
		const pg = createPgBackend({
			neon,
			connect: async (as) => {
				if (as)
					throw Object.assign(new Error("password authentication failed"), {
						code: "28P01",
					});
				return {
					dialect: "pg" as const,
					query: async (sql: string) => {
						if (sql.startsWith("CREATE") || sql.startsWith("ALTER"))
							throw new Error(`boom ${sql}`);
						return { columns: [], rows: [] };
					},
					close: async () => {},
				};
			},
		});
		const err = await pg.query("SELECT 1", { agent: true }).catch((e) => e);
		expect(err?.name).toBe("SqlError");
		expect(String(err?.message)).not.toMatch(/PASSWORD '[\w-]{32}'/);
	});
});
