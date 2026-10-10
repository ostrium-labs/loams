import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { createLocalShim } from "../src/main/protocol/local-shim";

const fixture = (n: string) =>
	JSON.parse(
		readFileSync(new URL(`./fixtures/${n}`, import.meta.url), "utf8"),
	) as unknown;

/** Key tree: object keys recursively; leaves are ignored (nullable and empty values differ locally). */
function shape(v: unknown): unknown {
	if (v && typeof v === "object" && !Array.isArray(v))
		return Object.fromEntries(
			Object.entries(v)
				.sort(([a], [b]) => a.localeCompare(b))
				.map(([k, x]) => [k, shape(x)]),
		);
	return "leaf";
}

const shim = createLocalShim(
	{ version: "1.2.3", username: "dina" },
	() => "local",
);

describe("local shim", () => {
	it("instance_and_session_shapes_match_mock", async () => {
		const inst = shim("/api/v1/instance", "GET");
		expect(inst?.status).toBe(200);
		const i = (await inst?.json()) as Record<string, unknown>;
		const mi = fixture("apps-mock-instance.json") as Record<string, unknown>;
		const { oidc: _a, ...iSign } = i.sign_in as Record<string, unknown>;
		const { oidc: _b, ...mSign } = mi.sign_in as Record<string, unknown>;
		expect(shape(iSign)).toEqual(shape(mSign));
		expect(Array.isArray((i.sign_in as { oidc: unknown }).oidc)).toBe(true);
		const { features: f, sign_in: _s, ...iRest } = i;
		const { features: mf, sign_in: _t, ...mRest } = mi;
		expect(shape(iRest)).toEqual(shape(mRest));
		expect(Object.keys(f as object)).toEqual(
			expect.arrayContaining(Object.keys(mf as object)),
		);
		expect(i).toMatchObject({
			edition: "oss",
			version: "1.2.3",
			features: { local: true, desktop: true },
		});

		const ses = shim("/api/v1/session", "GET");
		expect(ses?.status).toBe(200);
		const s = (await ses?.json()) as Record<string, unknown>;
		expect(shape(s)).toEqual(shape(fixture("apps-mock-session.json")));
		expect(s).toMatchObject({
			user: { id: "local", name: "dina", email: null },
			org: { id: "local", name: "This computer" },
			role: "owner",
		});
	});

	it("other_api_v1_404", async () => {
		for (const [p, m] of [
			["/api/v1/orgs", "GET"],
			["/api/v1/instance", "POST"],
			["/api/v1/session/logout", "POST"],
		] as const) {
			const r = shim(p, m);
			expect(r?.status).toBe(404);
			expect(await r?.json()).toEqual({
				code: "not_in_local_edition",
				message: "This needs a Loams control plane. Add a server in Servers.",
			});
		}
	});

	it("non_api_returns_null", () => {
		expect(shim("/loams.instance.v1.InstanceService/GetInstance", "POST")).toBe(
			null,
		);
		expect(shim("/api/v10/x", "GET")).toBe(null);
		expect(shim("/", "GET")).toBe(null);
	});

	it("shim_inactive_for_remote", () => {
		const remote = createLocalShim(
			{ version: "1", username: "u" },
			() => "remote",
		);
		expect(remote("/api/v1/instance", "GET")).toBe(null);
		expect(remote("/api/v1/orgs", "GET")).toBe(null);
	});
});
