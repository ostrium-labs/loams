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
		expect((await w.schemas()).map((s) => s.name)).toContain("information_schema");
	});
});
