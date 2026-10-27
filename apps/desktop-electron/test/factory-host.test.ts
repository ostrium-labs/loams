import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Context } from "cordis";
import { afterEach, describe, expect, it, vi } from "vite-plus/test";
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
	static behaviour: {
		search?: () => unknown;
		searchAsync?: () => Promise<unknown>;
		version?: () => unknown;
	} = {};
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
		if (Fake.behaviour.searchAsync) return Fake.behaviour.searchAsync();
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
		const signoz = list.find((a) => a.id === "signoz");
		expect(signoz?.label).toBe("SigNoz");
		expect(signoz?.hasPanels).toBe(false);
		// SigNoz on the House is a preview until it passes the House gate (D1571, D1666).
		expect(signoz?.preview).toBe("Preview: SigNoz on Loams House");
		expect(list.filter((a) => a.preview)).toHaveLength(1);
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
		// The info carries non-secret fields only, never a secret value.
		expect(
			(replies[1] as { fields?: Record<string, string> }[]).find(
				(a) => a.fields,
			)?.fields,
		).toEqual({});
		expect((replies[1] as { url?: string }[]).find((a) => a.url)?.url).toBe(
			"https://f.example",
		);
	});

	it("locked_vault_entry_reports_locked", async () => {
		const file = join(mkdtempSync(join(tmpdir(), "fh-")), "v.json");
		new Vault(file, crypto).set("forgejo", "https://f.example", {
			token: SECRET,
		});
		const broken = {
			...crypto,
			decrypt: () => {
				throw new Error("keychain changed");
			},
		};
		const h = new FactoryHost(
			new Vault(file, broken),
			new Context(),
			fakeApps(),
		);
		const f = (await h.list()).find((a) => a.id === "forgejo");
		expect(f?.health).toBe("locked");
		expect(f?.url).toBeUndefined();
		expect(
			await h.query({ app: "forgejo", op: "searchRepositories" } as never),
		).toMatchObject({
			ok: false,
		});
		// Re-entering the credentials unlocks it.
		const r = await h.configure("forgejo", "https://f.example", {
			token: "t2",
		});
		expect(r.ok && r.value.health).toBe("ok");
	});

	it("all_secrets_lists_every_app_secret_form_but_no_plain_field", async () => {
		const h = mk();
		await h.configure("forgejo", "https://f.example", {
			token: SECRET,
			ssoOrigin: "https://sso.example",
		});
		const all = h.allSecrets();
		expect(all).toContain(SECRET);
		expect(all).toContain(encodeURIComponent(SECRET));
		expect(all).not.toContain("https://sso.example");
	});

	it("reconfigure_same_origin_keeps_secrets", async () => {
		const h = mk();
		await h.configure("forgejo", "https://f.example", { token: SECRET });
		const r = await h.configure("forgejo", "https://f.example/git/", {});
		expect(r.ok).toBe(true);
		expect(Fake.made.at(-1)?.config["token"]).toBe(SECRET);
		expect(Fake.made.at(-1)?.config["baseUrl"]).toBe("https://f.example/git");
		const first = await mk().configure("forgejo", "https://f.example", {});
		expect(first).toMatchObject({ ok: false, code: "bad_fields" });
	});

	it("reconfigure_new_origin_requires_secrets_again", async () => {
		const h = mk();
		await h.configure("forgejo", "https://f.example", {
			token: SECRET,
			ssoOrigin: "https://sso.example",
		});
		const made = Fake.made.length;
		const r = await h.configure("forgejo", "https://evil.example", {});
		expect(r).toMatchObject({ ok: false, code: "bad_fields" });
		expect(!r.ok && r.message).toMatch(/Access token/);
		// The old configuration stays in place; nothing was built for the new origin.
		expect(h.appUrls("forgejo")?.url).toBe("https://f.example");
		expect(Fake.made.length).toBe(made);
		// http -> https on the same host is a different origin too.
		expect((await h.configure("forgejo", "http://f.example", {})).ok).toBe(
			false,
		);
		const ok = await h.configure("forgejo", "https://g.example", {
			token: "new",
		});
		expect(ok.ok).toBe(true);
		expect(Fake.made.at(-1)?.config["token"]).toBe("new");
		// Non-secret fields are still carried over.
		expect(ok.ok && ok.value.fields).toEqual({
			ssoOrigin: "https://sso.example",
		});
	});

	it("reconfigure_keeps_unretyped_non_secret_fields", async () => {
		const h = mk();
		await h.configure("forgejo", "https://f.example", {
			token: SECRET,
			ssoOrigin: "https://sso.example",
		});
		const r = await h.configure("forgejo", "https://f.example", {
			token: "new-token",
		});
		expect(r.ok && r.value.fields).toEqual({
			ssoOrigin: "https://sso.example",
		});
		expect(h.appUrls("forgejo")?.ssoOrigin).toBe("https://sso.example");
		expect(Fake.made.at(-1)?.config["token"]).toBe("new-token");
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

	it("adapterless_app_health_is_probed_not_assumed", async () => {
		// SigNoz has no adapter: its health comes from its unauthenticated
		// /api/v2/healthz at the configured origin, never from "configured means ok".
		const real = new FactoryHost(mkVault(), new Context());
		const seen: string[] = [];
		const cases: [() => Response | Promise<Response>, string][] = [
			[() => Response.json({ status: "ok" }), "ok"],
			[() => new Response("bad gateway", { status: 502 }), "unreachable"],
			[
				() => {
					throw new TypeError("fetch failed");
				},
				"unreachable",
			],
		];
		for (const [respond, health] of cases) {
			vi.stubGlobal(
				"fetch",
				vi.fn(async (u: string | URL) => {
					seen.push(String(u));
					return respond();
				}),
			);
			await real.remove("signoz");
			const r = await real.configure(
				"signoz",
				"http://signoz.example:8080/services/",
				{},
			);
			expect(r).toMatchObject({ ok: true, value: { health } });
		}
		expect(
			seen.every((u) => u === "http://signoz.example:8080/api/v2/healthz"),
		).toBe(true);
	});

	it("plane_app_url_is_the_embedded_ui_and_the_api_base_stays", async () => {
		// ItsAPlan serves its API and its web UI on different origins.
		const seen: string[] = [];
		vi.stubGlobal(
			"fetch",
			vi.fn(async (u: string | URL | Request) => {
				seen.push(String(u instanceof Request ? u.url : u));
				return Response.json({ authenticated: true, name: "x", status: "ok" });
			}),
		);
		const h = new FactoryHost(mkVault(), new Context());
		const r = await h.configure("plane", "http://api.plan.test", {
			apiKey: "itp_k",
			appUrl: "http://plan.test/",
		});
		expect(r).toMatchObject({
			ok: true,
			value: { url: "http://api.plan.test" },
		});
		expect(h.appUrls("plane")).toEqual({ url: "http://plan.test/" });
		expect(seen.every((u) => u.startsWith("http://api.plan.test/"))).toBe(true);
		// Not an http(s) URL: refused, nothing stored.
		const bad = await h.configure("plane", "http://api.plan.test", {
			apiKey: "itp_k",
			appUrl: "javascript:alert(1)",
		});
		expect(bad).toMatchObject({ ok: false, code: "invalid_url" });
		// Without one, the embedded UI is the configured URL, as before.
		await h.configure("plane", "http://api.plan.test", { apiKey: "itp_k" });
		expect(h.appUrls("plane")?.url).toBe("http://plan.test/");
		await h.remove("plane");
		await h.configure("plane", "http://api.plan.test", { apiKey: "itp_k" });
		expect(h.appUrls("plane")?.url).toBe("http://api.plan.test");
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
		).toMatchObject({ code: "invalid_url" });
		expect(await h.configure("forgejo", "https://f.example", {})).toMatchObject(
			{ code: "bad_fields" },
		);
	});

	it("reconfigure_during_query_still_redacts_old_secret", async () => {
		const h = mk();
		await h.configure("forgejo", "https://f.example", {
			token: "AAAA-old-secret",
		});
		let fail: (e: Error) => void = () => undefined;
		Fake.behaviour.searchAsync = () =>
			new Promise((_, rej) => {
				fail = rej;
			});
		const inflight = h.query({ app: "forgejo", op: "repos", params: {} });
		await new Promise((r) => setTimeout(r, 5));
		Fake.behaviour.searchAsync = undefined;
		await h.configure("forgejo", "https://f.example", {
			token: "BBBB-new-secret",
		});
		fail(new Error("upstream said AAAA-old-secret is bad"));
		const r = await inflight;
		expect(JSON.stringify(r)).not.toContain("AAAA-old-secret");
		expect(r).toMatchObject({ ok: false });
	});

	it("redacts_basic_and_form_encoded_forms", async () => {
		const h = mk();
		await h.configure("forgejo", "https://f.example", { token: "p@ss w/rd" });
		const forms = [
			"p%40ss%20w%2Frd",
			"p%40ss+w%2Frd",
			Buffer.from("p@ss w/rd").toString("base64"),
		];
		Fake.behaviour.search = () => {
			throw new Error(forms.join(" | "));
		};
		const r = JSON.stringify(
			await h.query({ app: "forgejo", op: "repos", params: {} }),
		);
		for (const f of forms) expect(r).not.toContain(f);
	});

	it("concurrent_first_queries_build_one_adapter", async () => {
		const h = mk(Fake);
		await h.configure("forgejo", "https://f.example", { token: "t" });
		await h.remove("forgejo");
		Fake.made = [];
		let loads = 0;
		const apps = fakeApps();
		apps.forgejo.adapter = async () => {
			loads++;
			await new Promise((r) => setTimeout(r, 10));
			return Fake as unknown as AdapterCtor;
		};
		const v2 = mkVault();
		v2.set("forgejo", "https://f.example", { token: "t" });
		const h2 = new FactoryHost(v2, new Context(), apps);
		const rs = await Promise.all(
			[1, 2, 3].map(() =>
				h2.query({ app: "forgejo", op: "version", params: {} }),
			),
		);
		expect(rs.every((r) => r.ok)).toBe(true);
		expect(Fake.made).toHaveLength(1);
		expect(loads).toBe(1);
	});

	it("configure_during_build_discards_stale_adapter", async () => {
		let release: () => void = () => undefined;
		const gate = new Promise<void>((r) => {
			release = r;
		});
		const apps = fakeApps();
		apps.forgejo.adapter = async () => {
			await gate;
			return Fake as unknown as AdapterCtor;
		};
		const h = new FactoryHost(mkVault(), new Context(), apps);
		const first = h.configure("forgejo", "https://f.example", { token: "one" });
		await new Promise((r) => setTimeout(r, 5));
		const second = h.configure("forgejo", "https://f.example", {
			token: "two",
		});
		release();
		await Promise.all([first, second]);
		expect(Fake.made).toHaveLength(2);
		expect(Fake.made[0]?.config).toMatchObject({ token: "one" });
		expect(Fake.made[0]?.detached).toBe(true);
		expect(Fake.made[1]?.detached).toBe(false);
		const r = await h.query({ app: "forgejo", op: "version", params: {} });
		expect(r.ok).toBe(true);
		expect(Fake.made).toHaveLength(2);
	});

	it("configure_rejects_credentials_in_url", async () => {
		expect(
			await mk().configure("forgejo", "https://u:p@f.example", { token: "t" }),
		).toMatchObject({ ok: false, code: "invalid_url" });
	});

	it("missing_adapter_method_is_unsupported", async () => {
		class Bare {
			constructor(
				_c: unknown,
				public config: unknown,
			) {}
		}
		const h = mk(Bare);
		await h.configure("forgejo", "https://f.example", { token: "t" });
		expect(
			await h.query({ app: "forgejo", op: "version", params: {} }),
		).toMatchObject({
			ok: false,
			code: "unsupported",
		});
	});

	it("health_maps_matomo_and_glitchtip_auth", async () => {
		const h = new FactoryHost(mkVault(), new Context());
		vi.stubGlobal(
			"fetch",
			vi.fn(async () =>
				Response.json({
					result: "error",
					message: "token_auth is not valid, unable to authenticate",
				}),
			),
		);
		const m = await h.configure("matomo", "https://m.example", {
			apiToken: "bad",
		});
		expect(m).toMatchObject({ ok: true, value: { health: "auth_failed" } });
		vi.stubGlobal(
			"fetch",
			vi.fn(async () => Response.json("5.1.0")),
		);
		await h.remove("matomo");
		expect(
			await h.configure("matomo", "https://m.example", { apiToken: "ok" }),
		).toMatchObject({
			value: { health: "ok" },
		});
		vi.stubGlobal(
			"fetch",
			vi.fn(async () =>
				Response.json({ version: "1", user: null, auth: null }),
			),
		);
		expect(
			await h.configure("glitchtip", "https://g.example", { token: "x" }),
		).toMatchObject({
			value: { health: "auth_failed" },
		});
		vi.stubGlobal(
			"fetch",
			vi.fn(async () =>
				Response.json({ version: "1", user: {}, auth: { id: 1 } }),
			),
		);
		await h.remove("glitchtip");
		expect(
			await h.configure("glitchtip", "https://g.example", { token: "x" }),
		).toMatchObject({
			value: { health: "ok" },
		});
	});

	it("stale_configure_does_not_overwrite_health", async () => {
		let release: () => void = () => undefined;
		const gate = new Promise<void>((r) => {
			release = r;
		});
		let n = 0;
		const apps = fakeApps();
		apps.forgejo.adapter = async () => {
			if (n++ === 0) await gate;
			return Fake as unknown as AdapterCtor;
		};
		const h = new FactoryHost(mkVault(), new Context(), apps);
		const first = h.configure("forgejo", "https://f.example", { token: "one" });
		await new Promise((r) => setTimeout(r, 5));
		const second = await h.configure("forgejo", "https://f.example", {
			token: "two",
		});
		expect(second).toMatchObject({ ok: true, value: { health: "ok" } });
		release();
		const stale = await first;
		expect(stale).toMatchObject({ ok: true, value: { health: "ok" } });
		expect((await h.list()).find((a) => a.id === "forgejo")?.health).toBe("ok");
	});
});
