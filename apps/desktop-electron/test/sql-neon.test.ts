import { mkdtempSync, readdirSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { type FetchFn, NeonClient } from "../src/main/sql/neon";
import { createPgBackend } from "../src/main/sql/pg";
import { createWesqlBackend, wesqlDev } from "../src/main/sql/wesql";

const T = "a".repeat(32);
const ANC = "b".repeat(32);
const NEW = "c".repeat(32);

function setup(reply: (url: string, method: string) => unknown) {
	const calls: { url: string; method: string; body?: unknown }[] = [];
	const fetch: FetchFn = async (url, init) => {
		calls.push({
			url,
			method: init?.method ?? "GET",
			body: init?.body ? JSON.parse(init.body) : undefined,
		});
		return {
			ok: true,
			status: 200,
			text: async () => JSON.stringify(reply(url, init?.method ?? "GET")),
		};
	};
	const dir = mkdtempSync(join(tmpdir(), "neon-test-"));
	const file = join(dir, "postgres", "branches.json");
	return {
		calls,
		file,
		neon: new NeonClient({ fetch, branchesFile: file, newId: () => NEW }),
	};
}

describe("neon", () => {
	it("neon_create_branch_body", async () => {
		const { calls, file, neon } = setup(() => ({
			timeline_id: NEW,
			ancestor_timeline_id: ANC,
			ancestor_lsn: "0/1",
		}));
		const tl = await neon.createBranch(T, {
			name: "feature",
			ancestorTimelineId: ANC,
			ancestorStartLsn: "0/16B3748",
		});
		expect(calls).toEqual([
			{
				url: `http://127.0.0.1:9898/v1/tenant/${T}/timeline/`,
				method: "POST",
				body: {
					new_timeline_id: NEW,
					ancestor_timeline_id: ANC,
					ancestor_start_lsn: "0/16B3748",
				},
			},
		]);
		expect(tl).toMatchObject({
			timelineId: NEW,
			name: "feature",
			ancestorTimelineId: ANC,
		});
		expect(JSON.parse(readFileSync(file, "utf8"))).toEqual({
			[NEW]: "feature",
		});
		const noLsn = setup(() => ({}));
		await noLsn.neon.createBranch(T, { name: "x", ancestorTimelineId: ANC });
		expect(noLsn.calls[0]?.body).toEqual({
			new_timeline_id: NEW,
			ancestor_timeline_id: ANC,
		});
	});

	it("lists_tenants_timelines_wal_with_names_and_validates_ids", async () => {
		const { calls, neon } = setup((url) =>
			url.endsWith("/v1/tenant")
				? [{ id: T, state: { slug: "Active" } }]
				: url.includes(":7676")
					? { flush_lsn: "0/2", commit_lsn: "0/1" }
					: [{ timeline_id: ANC, last_record_lsn: "0/3" }],
		);
		expect(await neon.tenants()).toEqual([T]);
		expect((await neon.timelines(T))[0]).toMatchObject({
			timelineId: ANC,
			lastRecordLsn: "0/3",
		});
		expect(await neon.walStatus(T, ANC)).toMatchObject({
			flushLsn: "0/2",
			commitLsn: "0/1",
		});
		expect(calls.map((c) => c.url)).toEqual([
			"http://127.0.0.1:9898/v1/tenant",
			`http://127.0.0.1:9898/v1/tenant/${T}/timeline`,
			`http://127.0.0.1:7676/v1/tenant/${T}/timeline/${ANC}`,
		]);
		await expect(neon.timelines("../x")).rejects.toMatchObject({
			code: "invalid",
		});
		await expect(
			neon.createBranch(T, {
				name: "n",
				ancestorTimelineId: ANC,
				ancestorStartLsn: "zz",
			}),
		).rejects.toMatchObject({ code: "invalid" });
	});
});

describe("branches.json", () => {
	it("concurrent_creates_keep_every_name", async () => {
		let n = 0;
		const dir = mkdtempSync(join(tmpdir(), "neon-conc-"));
		const file = join(dir, "postgres", "branches.json");
		const fetch: FetchFn = async () => ({
			ok: true,
			status: 200,
			text: async () => "{}",
		});
		const neon = new NeonClient({
			fetch,
			branchesFile: file,
			newId: () => String(++n).padStart(32, "0"),
		});
		await Promise.all(
			["a", "b", "c", "d", "e"].map((name) =>
				neon.createBranch(T, { name, ancestorTimelineId: ANC }),
			),
		);
		expect(
			Object.values(JSON.parse(readFileSync(file, "utf8"))).sort(),
		).toEqual(["a", "b", "c", "d", "e"]);
		expect(readdirSync(join(dir, "postgres"))).toEqual(["branches.json"]);
	});
});

describe("backends", () => {
	it("connection_never_carries_the_password", () => {
		const neon = setup(() => ({})).neon;
		const pg = createPgBackend({ neon });
		const c = pg.connection();
		expect(Object.keys(c).sort()).toEqual([
			"database",
			"host",
			"passwordRef",
			"port",
			"user",
		]);
		expect(c.passwordRef).not.toContain("cloud_admin");
		expect(createPgBackend({ neon }).connection().passwordRef).not.toBe(
			c.passwordRef,
		);
		expect(c).toMatchObject({
			host: "127.0.0.1",
			port: 55433,
			user: "cloud_admin",
		});
		const w = createWesqlBackend({ env: { WESQL_ROOT_PASSWORD: "pw-xyz" } });
		expect(JSON.stringify(w.connection())).not.toContain("pw-xyz");
		expect(w.connection().port).toBe(13306);
		expect(w.password().reveal()).toBe("pw-xyz");
		expect(JSON.stringify(w.password())).toBe('"[redacted]"');
		expect(wesqlDev({}).password.reveal()).toBe("loams-dev");
	});

	it("query_closes_the_session_and_redacts_connect_errors", async () => {
		let closed = 0;
		const pg = createPgBackend({
			neon: setup(() => ({})).neon,
			connect: async () => ({
				dialect: "pg",
				query: async () => ({ columns: ["?column?"], rows: [[1]] }),
				close: async () => {
					closed++;
				},
			}),
		});
		expect((await pg.query("SELECT 1")).rows).toEqual([[1]]);
		expect(closed).toBe(1);
		const bad = createPgBackend({
			neon: setup(() => ({})).neon,
			connect: async () => {
				throw Object.assign(
					new Error("failed postgres://cloud_admin:cloud_admin@127.0.0.1/x"),
					{ code: "ECONNREFUSED" },
				);
			},
		});
		await expect(bad.query("SELECT 1")).rejects.toMatchObject({
			code: "ECONNREFUSED",
			message: "failed postgres://cloud_admin:***@127.0.0.1/x",
		});
	});
});
