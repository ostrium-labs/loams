import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Context } from "cordis";
import { afterEach, describe, expect, it, vi } from "vitest";
import { type AdapterCtor, FACTORY_APPS } from "../src/main/factory/apps";
import { FactoryHost } from "../src/main/factory/host";
import { Vault } from "../src/main/factory/vault";

const crypto = {
	available: () => true,
	encrypt: (s: string) => Buffer.from(s),
	decrypt: (b: Buffer) => b.toString(),
};
const mkVault = () =>
	new Vault(join(mkdtempSync(join(tmpdir(), "fh-")), "v.json"), crypto);

class Fake {
	static made: Fake[] = [];
	static behaviour: { search?: () => unknown; version?: () => unknown } = {};
	detached = false;
	constructor(
		_ctx: unknown,
		public config: Record<string, unknown>,
	) {
		Fake.made.push(this);
	}
	detach() {
		this.detached = true;
	}
	async getVersion() {
		return (Fake.behaviour.version?.() as object) ?? { version: "1.0" };
	}
	async searchRepositories() {
		return (
			(Fake.behaviour.search?.() as object) ?? {
				ok: true,
				data: [{ full_name: "a/b", stars_count: 3, extra: "dropped" }],
			}
		);
	}
}
const fakeApps = (ctor: unknown = Fake) => ({
	...FACTORY_APPS,
	forgejo: {
		...FACTORY_APPS.forgejo,
		adapter: async () => ctor as AdapterCtor,
	},
});
const mk = (ctor?: unknown) =>
	new FactoryHost(mkVault(), new Context(), fakeApps(ctor));

afterEach(() => {
	Fake.made = [];
	Fake.behaviour = {};
	vi.unstubAllGlobals();
});

const SECRET = "tok-S3CRET/+x";

describe("factory host", () => {
	it("lists_eight_apps_unconfigured", async () => {
		const list = await mk().list();
		expect(list).toHaveLength(8);
		expect(list.every((a) => a.health === "unconfigured")).toBe(true);
		expect(list.find((a) => a.id === "plane")?.label).toBe("Plane (ItsAPlan)");
		expect(list.find((a) => a.id === "openobserve")?.hasPanels).toBe(false);
	});

	it("ipc_replies_never_contain_secret", async () => {
		const h = mk();
		Fake.behaviour.search = () => {
			throw new Error(`boom ${SECRET} and ${encodeURIComponent(SECRET)}`);
		};
		const replies = [
			await h.configure("forgejo", "https://f.example", { token: SECRET }),
			await h.list(),
			await h.test("forgejo"),
			await h.query({ app: "forgejo", op: "repos", params: {} }),
			await h.query({ app: "forgejo", op: "version", params: {} }),
		];
		const blob = JSON.stringify(replies);
		expect(blob).not.toContain(SECRET);
		expect(blob).not.toContain(encodeURIComponent(SECRET));
		expect(blob).toContain("[redacted]");
		expect((replies[1] as { url?: string }[]).find((a) => a.url)?.url).toBe(
			"https://f.example",
		);
	});

	it("adapter_error_is_redacted", async () => {
		const h = mk();
		await h.configure("forgejo", "https://f.example", { token: "abc123" });
		Fake.behaviour.search = () => {
			throw new Error("bad token abc123");
		};
		const r = await h.query({ app: "forgejo", op: "repos", params: {} });
		expect(r).toEqual({
			ok: false,
			code: "upstream_error",
			message: "bad token [redacted]",
		});
	});

	it("query_projects_to_dto", async () => {
		const h = mk();
		await h.configure("forgejo", "https://f.example", { token: "t" });
		const r = await h.query({
			app: "forgejo",
			op: "repos",
			params: { limit: 500 },
		});
		expect(r).toEqual({
			ok: true,
			value: [{ fullName: "a/b", stars: 3 }].map((x) =>
				expect.objectContaining(x),
			),
		});
		expect(JSON.stringify(r)).not.toContain("dropped");
	});

	it("unknown_op_rejected", async () => {
		const h = mk();
		await h.configure("forgejo", "https://f.example", { token: "t" });
		for (const op of ["deleteRepo", "__proto__", "constructor", "toString"]) {
			const r = await h.query({ app: "forgejo", op, params: {} });
			expect(r).toMatchObject({ ok: false, code: "unknown_op" });
		}
		expect(
			await h.query({ app: "nope" as never, op: "x", params: {} }),
		).toMatchObject({
			ok: false,
			code: "unknown_app",
		});
	});

	it("params_validated", async () => {
		const h = mk();
		await h.configure("forgejo", "https://f.example", { token: "t" });
		for (const params of [
			{ limit: "5" },
			{ q: 1 },
			{ admin: true },
			{ limit: -1 },
		]) {
			const r = await h.query({ app: "forgejo", op: "repos", params });
			expect(r).toMatchObject({ ok: false, code: "bad_params" });
		}
		const pulls = await h.query({
			app: "forgejo",
			op: "issues",
			params: { type: "pulls" },
		});
		expect(pulls).toMatchObject({ ok: false, code: "bad_params" });
	});

	it("unconfigured_query_is_friendly", async () => {
		expect(
			await mk().query({ app: "forgejo", op: "repos", params: {} }),
		).toMatchObject({
			ok: false,
			code: "unconfigured",
		});
	});

	it("reconfigure_disposes_old_adapter", async () => {
		const h = mk();
		await h.configure("forgejo", "https://f.example", { token: "one" });
		const first = Fake.made[0];
		expect(first?.config).toMatchObject({
			baseUrl: "https://f.example",
			token: "one",
		});
		await h.configure("forgejo", "https://f.example", { token: "two" });
		expect(first?.detached).toBe(true);
		expect(Fake.made).toHaveLength(2);
		expect(Fake.made[1]?.config).toMatchObject({ token: "two" });
		await h.remove("forgejo");
		expect(Fake.made[1]?.detached).toBe(true);
		expect((await h.list()).find((a) => a.id === "forgejo")?.health).toBe(
			"unconfigured",
		);
	});

	it("health_maps_status_codes", async () => {
		const real = new FactoryHost(mkVault(), new Context());
		const cases: [() => Response | Promise<Response>, string][] = [
			[() => new Response("{}", { status: 401 }), "auth_failed"],
			[() => new Response("{}", { status: 403 }), "auth_failed"],
			[() => new Response("{}", { status: 500 }), "unreachable"],
			[() => Response.json({ version: "9.0.0" }), "ok"],
		];
		for (const [respond, health] of cases) {
			vi.stubGlobal(
				"fetch",
				vi.fn(async () => respond()),
			);
			await real.remove("forgejo");
			const r = await real.configure("forgejo", "https://f.example", {
				token: "t",
			});
			expect(r).toMatchObject({ ok: true, value: { health } });
		}
		vi.stubGlobal(
			"fetch",
			vi.fn(async () => {
				throw new TypeError("fetch failed");
			}),
		);
		await real.remove("forgejo");
		const r = await real.configure("forgejo", "https://f.example", {
			token: "t",
		});
		expect(r).toMatchObject({ ok: true, value: { health: "unreachable" } });
	});

	it("error_codes_map", async () => {
		const h = mk();
		await h.configure("forgejo", "https://f.example", { token: "t" });
		for (const [err, code] of [
			[Object.assign(new Error("x"), { status: 401 }), "auth_failed"],
			[new TypeError("fetch failed"), "unreachable"],
			[Object.assign(new Error("x"), { status: 500 }), "upstream_error"],
		] as const) {
			Fake.behaviour.search = () => {
				throw err;
			};
			expect(
				await h.query({ app: "forgejo", op: "repos", params: {} }),
			).toMatchObject({ code });
		}
	});

	it("configure_validates_input", async () => {
		const h = mk();
		expect(
			await h.configure("forgejo", "javascript:alert(1)", { token: "t" }),
		).toMatchObject({ code: "bad_url" });
		expect(await h.configure("forgejo", "https://f.example", {})).toMatchObject(
			{ code: "bad_fields" },
		);
	});
});
