// LOAMS_IT_PG=1: needs a running postgres stack (podman/docker compose up of deploy/neon) with one tenant
// and timeline, and LOAMS_IT_TENANT / LOAMS_IT_TIMELINE set. Optional LOAMS_IT_WESQL=1 for the wesql stack.
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { branchesFile, NeonClient } from "../src/main/sql/neon";
import { createPgBackend } from "../src/main/sql/pg";
import { createWesqlBackend } from "../src/main/sql/wesql";

describe.skipIf(!process.env.LOAMS_IT_PG)("postgres stack", () => {
	const neon = new NeonClient({
		branchesFile: branchesFile(mkdtempSync(join(tmpdir(), "it-pg-"))),
	});
	const pg = createPgBackend({ neon });
	it("SELECT 1", async () => {
		const r = await pg.query("SELECT 1 AS one", { readOnly: true });
		expect(r.rows).toEqual([[1]]);
	});
	it("agent reads are least-privilege and memory-bounded", async () => {
		const r = await pg.query("SELECT current_user, session_user", {
			agent: true,
		});
		expect(r.rows[0]?.[0]).toBe("pg_read_all_data");
		for (const q of [
			"SELECT pg_reload_conf()",
			"SELECT pg_terminate_backend(pg_backend_pid())",
			"SELECT pg_rotate_logfile()",
		])
			await expect(pg.query(q, { agent: true }), q).rejects.toBeTruthy();
		const big = await pg.query(
			"SELECT g FROM generate_series(1, 200000000) g",
			{ agent: true },
		);
		expect(big.rowCount).toBe(1000);
		expect(big.truncated).toBe(true);
		await expect(
			pg.query("CREATE TABLE it_x(a int)", { agent: true }),
		).rejects.toBeTruthy();
		await expect(
			pg.query("SELECT pg_sleep(60)", { agent: true }),
		).rejects.toMatchObject({ code: "timeout" });
	}, 60_000);
	it("lists tenants and creates a branch", async () => {
		const tenant = process.env.LOAMS_IT_TENANT as string;
		const timeline = process.env.LOAMS_IT_TIMELINE as string;
		expect(await pg.tenants()).toContain(tenant);
		const b = await pg.createBranch(tenant, {
			name: `it-${Date.now()}`,
			ancestorTimelineId: timeline,
		});
		expect((await pg.timelines(tenant)).map((t) => t.timelineId)).toContain(
			b.timelineId,
		);
		expect((await pg.walStatus(tenant, timeline)).flushLsn).toBeTruthy();
	});
});

describe.skipIf(!process.env.LOAMS_IT_WESQL)("wesql stack", () => {
	it("SELECT 1", async () => {
		const w = createWesqlBackend();
		expect((await w.query("SELECT 1 AS one", { readOnly: true })).rows).toEqual(
			[[1]],
		);
		expect(
			Number((await w.query("SELECT 1 + 1", { agent: true })).rows[0]?.[0]),
		).toBe(2);
		expect(
			(await w.query("SELECT CURRENT_USER()", { agent: true })).rows[0]?.[0],
		).toBe("loams_ro@%");
		expect(
			(await w.query("SELECT @@secure_file_priv", { agent: true }))
				.rows[0]?.[0],
		).toBe("NULL");
		await expect(
			w.query("CREATE TABLE test.it_x (a int)", { agent: true }),
		).rejects.toBeTruthy();
		await expect(
			w.query("SELECT user FROM mysql.user", { agent: true }),
		).rejects.toMatchObject({ code: "ER_TABLEACCESS_DENIED_ERROR" });
		// A schema created after loams_ro exists is readable on the next agent read.
		await w.query("DROP DATABASE IF EXISTS it_new_db");
		await w.query("CREATE DATABASE it_new_db");
		await w.query("CREATE TABLE IF NOT EXISTS it_new_db.t (a int)");
		await w.query("INSERT INTO it_new_db.t VALUES (42)");
		expect(
			(await w.query("SELECT a FROM it_new_db.t", { agent: true })).rows,
		).toEqual([[42]]);
		await w.query("DROP DATABASE it_new_db");
		const big = await w.query(
			"SELECT a.ID FROM information_schema.COLLATIONS a CROSS JOIN information_schema.COLLATIONS b CROSS JOIN information_schema.COLLATIONS c",
			{ agent: true },
		);
		expect(big.rowCount).toBe(1000);
		expect(big.truncated).toBe(true);
		expect((await w.schemas()).map((s) => s.name)).toContain(
			"information_schema",
		);
	}, 60_000);
});
