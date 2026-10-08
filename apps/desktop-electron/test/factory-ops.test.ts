import { describe, expect, it } from "vitest";
import { FACTORY_APPS } from "../src/main/factory/apps";
import { OP_NAME_ALLOWLIST, OPS } from "../src/main/factory/ops";

// Methods each op may call on its adapter; every one must be read-only by name.
const src = (await import("node:fs")).readFileSync(
	new URL("../src/main/factory/ops.ts", import.meta.url),
	"utf8",
);

describe("ops", () => {
	it("ops_are_read_only_names", () => {
		const called = [...src.matchAll(/a\["([A-Za-z]+)"\]\?\./g)].map(
			(m) => m[1] ?? "",
		);
		expect(called.length).toBeGreaterThan(15);
		const allowedExtra = new Set([
			"me",
			"root",
			"status",
			"overview",
			"topPages",
			"live",
			"metricsDaily",
		]);
		for (const m of called) {
			const ok = OP_NAME_ALLOWLIST.test(m) || allowedExtra.has(m);
			expect(ok, m).toBe(true);
		}
		expect(
			called.some((m) =>
				/^(create|delete|update|set|post|put|patch|send)/i.test(m),
			),
		).toBe(false);
	});

	it("every_adapter_app_has_health", () => {
		for (const [id, def] of Object.entries(FACTORY_APPS))
			expect(Object.keys(OPS[id as keyof typeof OPS]).includes("health")).toBe(
				!!def.adapter,
			);
	});

	it("every_app_has_optional_sso_origin", () => {
		for (const def of Object.values(FACTORY_APPS))
			expect(def.credentialFields).toContainEqual({
				key: "ssoOrigin",
				label: "SSO origin (optional)",
				secret: false,
			});
	});
});
