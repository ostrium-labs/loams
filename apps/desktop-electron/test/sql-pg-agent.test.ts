import { describe, expect, it } from "vitest";
import { NeonClient } from "../src/main/sql/neon";
import { createPgBackend, type PgLogin } from "../src/main/sql/pg";

const neon = new NeonClient({ branchesFile: "/nonexistent/b.json" });

function fakePg(opts: { failLoginOnce?: boolean } = {}) {
	const log: string[] = [];
	let failLogin = opts.failLoginOnce === true;
	const connect = async (as?: PgLogin) => {
		const who = as?.user ?? "admin";
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
				return sql.startsWith("SELECT 1 FROM pg_roles")
					? { columns: ["?column?"], rows: [] }
					: { columns: ["n"], rows: [[1]] };
			},
			close: async () => {},
		};
	};
	return { connect, log };
}

describe("postgres agent reads", () => {
	it("agent_reads_connect_as_a_non_superuser_login", async () => {
		const { connect, log } = fakePg();
		const pg = createPgBackend({ neon, connect });
		await pg.query("SELECT 1", { agent: true });
		await pg.query("SELECT 2", { agent: true });
		const admin = log
			.filter((l) => l.startsWith("admin:"))
			.map((l) => l.slice(7));
		expect(admin[1]).toMatch(
			/^CREATE ROLE loams_ro LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS INHERIT PASSWORD '[\w-]{32}'$/,
		);
		expect(admin[2]).toBe("GRANT pg_read_all_data TO loams_ro");
		expect(admin).toHaveLength(3); // provisioned once
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

	it("reprovisions_once_when_the_login_is_refused", async () => {
		const { connect, log } = fakePg({ failLoginOnce: true });
		const pg = createPgBackend({ neon, connect });
		await expect(pg.query("SELECT 1", { agent: true })).resolves.toMatchObject({
			rowCount: 1,
		});
		expect(log.filter((l) => l.includes("ROLE loams_ro"))).toHaveLength(2);
	});
});
