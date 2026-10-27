// Electron main's factory host is a client of loams-agentd's FactoryService (SF1
// amendment F1-F4): it maps the daemon's answers to the console's IPC shapes, keeps
// the apps' non-secret URLs for the embedded views and never holds a secret.

import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { create } from "@bufbuild/protobuf";
import { createRouterTransport, type Transport } from "@connectrpc/connect";
import {
	type FactoryApp,
	FactoryAppSchema,
	FactoryHealth,
	FactoryService,
} from "@loams/proto/agentd";
import { describe, expect, it } from "vite-plus/test";
import {
	DAEMON_DOWN,
	FactoryHost,
	redact,
	secretForms,
} from "../src/main/factory/host";
import { Vault } from "../src/main/factory/vault";

const SECRET = "tok-S3CRET/+x";

function plane(extra: { url?: string } = {}): FactoryApp {
	return create(FactoryAppSchema, {
		id: "plane",
		label: "Plane (ItsAPlan)",
		url: "http://api.plan.test",
		health: FactoryHealth.OK,
		hasPanels: true,
		credentialFields: [
			{ key: "apiKey", label: "API key", secret: true },
			{ key: "appUrl", label: "App URL (optional)", secret: false },
		],
		fields: { appUrl: "http://plan.test", ssoOrigin: "https://sso.test" },
		persistent: true,
		...extra,
	});
}

/** A daemon whose FactoryService answers from `handlers`; records the requests. */
function daemon(seen: unknown[] = []): Transport {
	return createRouterTransport(({ service }) => {
		service(FactoryService, {
			listFactoryApps: () => ({
				apps: [
					plane(),
					create(FactoryAppSchema, {
						id: "zulip",
						label: "Zulip",
						health: FactoryHealth.UNCONFIGURED,
						hasPanels: true,
						persistent: true,
					}),
				],
			}),
			testFactoryApp: (r) => {
				seen.push(r);
				return { result: { case: "app", value: plane() } };
			},
			queryFactory: (r) => {
				seen.push(r);
				return r.op === "stats"
					? { result: { case: "valueJson", value: '{"open":2}' } }
					: {
							result: {
								case: "error",
								value: { code: "unknown_op", message: "Unknown operation: x" },
							},
						};
			},
			configureFactoryApp: (r) => {
				seen.push(r);
				return r.url.startsWith("ftp:")
					? {
							result: {
								case: "error",
								value: { code: "invalid_url", message: "Enter an http(s) URL" },
							},
						}
					: { result: { case: "app", value: plane({ url: r.url }) } };
			},
			removeFactoryApp: (r) => {
				seen.push(r);
				return {};
			},
		});
	});
}

describe("factory host over the daemon", () => {
	it("lists_the_daemons_apps_as_the_console_shape", async () => {
		const host = new FactoryHost(() => daemon());
		const apps = await host.list();
		expect(apps[0]).toEqual({
			id: "plane",
			label: "Plane (ItsAPlan)",
			url: "http://api.plan.test",
			health: "ok",
			hasPanels: true,
			credentialFields: [
				{ key: "apiKey", label: "API key", secret: true },
				{ key: "appUrl", label: "App URL (optional)", secret: false },
			],
			fields: { appUrl: "http://plan.test", ssoOrigin: "https://sso.test" },
			persistent: true,
		});
		// Unconfigured: no URL and no fields.
		expect(apps[1]).toMatchObject({ id: "zulip", health: "unconfigured" });
		expect(apps[1]).not.toHaveProperty("url");
		expect(apps[1]).not.toHaveProperty("fields");
	});

	it("embedded_views_load_the_app_url_with_its_sso_origin", async () => {
		const host = new FactoryHost(() => daemon());
		expect(host.appUrls("plane")).toBeUndefined();
		await host.list();
		expect(host.appUrls("plane")).toEqual({
			url: "http://plan.test",
			ssoOrigin: "https://sso.test",
		});
		expect(host.appUrls("zulip")).toBeUndefined();
	});

	it("configure_and_test_answer_ipc_results", async () => {
		const seen: unknown[] = [];
		const host = new FactoryHost(() => daemon(seen));
		const ok = await host.configure("plane", "http://api2.plan.test", {
			apiKey: SECRET,
		});
		expect(ok).toMatchObject({
			ok: true,
			value: { url: "http://api2.plan.test" },
		});
		expect(JSON.stringify(ok)).not.toContain("S3CRET");
		expect(seen[0]).toMatchObject({ app: "plane", fields: { apiKey: SECRET } });
		const bad = await host.configure("plane", "ftp://x", {});
		expect(bad).toEqual({
			ok: false,
			code: "invalid_url",
			message: "Enter an http(s) URL",
		});
		expect(await host.test("plane")).toMatchObject({
			ok: true,
			value: { health: "ok" },
		});
	});

	it("query_sends_params_as_json_and_parses_the_value", async () => {
		const seen: unknown[] = [];
		const host = new FactoryHost(() => daemon(seen));
		expect(
			await host.query({
				app: "plane",
				op: "stats",
				params: { projectKey: "FAC" },
			}),
		).toEqual({
			ok: true,
			value: { open: 2 },
		});
		expect(seen[0]).toMatchObject({
			app: "plane",
			op: "stats",
			paramsJson: '{"projectKey":"FAC"}',
		});
		expect(
			await host.query({ app: "plane", op: "x", params: {} }),
		).toMatchObject({
			ok: false,
			code: "unknown_op",
		});
		expect(
			await host.query({ app: "nope" as never, op: "x", params: {} }),
		).toMatchObject({
			code: "unknown_app",
		});
	});

	it("says_so_when_the_daemon_is_down", async () => {
		const host = new FactoryHost(() => null);
		await expect(host.list()).rejects.toThrow(DAEMON_DOWN);
		expect(await host.query({ app: "plane", op: "stats", params: {} })).toEqual(
			{
				ok: false,
				code: "daemon_down",
				message: DAEMON_DOWN,
			},
		);
		expect(await host.configure("plane", "http://x", {})).toMatchObject({
			code: "daemon_down",
		});
		expect(host.allSecrets()).toEqual([]);
	});

	it("imports_the_old_vaults_entries_into_the_daemon_then_forgets_them", async () => {
		const seen: unknown[] = [];
		const vault = new Vault(
			join(mkdtempSync(join(tmpdir(), "fh-")), "v.json"),
			{
				available: () => true,
				encrypt: (s: string) => Buffer.from(s),
				decrypt: (b: Buffer) => b.toString(),
			},
		);
		vault.set("plane", "http://api.plan.test", {
			apiKey: SECRET,
			projectKey: "FAC",
		});
		vault.set("agent:anthropic", "https://api.anthropic.com", { apiKey: "k" });
		const host = new FactoryHost(() => daemon(seen));
		expect(await host.importLegacy(vault)).toBe(1);
		expect(seen[0]).toMatchObject({
			app: "plane",
			url: "http://api.plan.test",
			fields: { apiKey: SECRET, projectKey: "FAC" },
		});
		expect(vault.get("plane")).toBeUndefined();
		// Not a factory app: left alone.
		expect(vault.get("agent:anthropic")).toBeDefined();
		// Without the daemon nothing moves and nothing is lost.
		vault.set("plane", "http://api.plan.test", { apiKey: SECRET });
		expect(await new FactoryHost(() => null).importLegacy(vault)).toBe(0);
		expect(vault.get("plane")).toBeDefined();
	});
});

describe("secret forms", () => {
	it("redacts_basic_and_form_encoded_forms", () => {
		const forms = secretForms([SECRET], ["bot@example.com"]);
		const basic = Buffer.from(`bot@example.com:${SECRET}`).toString("base64");
		for (const text of [
			SECRET,
			encodeURIComponent(SECRET),
			basic,
			basic.replace(/=+$/, ""),
		])
			expect(redact(`x ${text} y`, forms)).not.toContain(text);
	});
});
