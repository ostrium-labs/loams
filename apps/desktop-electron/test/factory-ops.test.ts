import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { FACTORY_APPS } from "../src/main/factory/apps";
import { OP_NAME_ALLOWLIST, OPS } from "../src/main/factory/ops";

const src = readFileSync(
	new URL("../src/main/factory/ops.ts", import.meta.url),
	"utf8",
);
// Per-app section of the OPS table: `\t<app>: {` at depth 1 up to the next one.
function sections(): Record<string, string> {
	const body = src.slice(src.indexOf("export const OPS"));
	const out: Record<string, string> = {};
	const re = /^\t(\w+): \{/gm;
	const marks = [...body.matchAll(re)];
	marks.forEach((m, i) => {
		out[m[1] ?? ""] = body.slice(m.index, marks[i + 1]?.index ?? body.length);
	});
	return out;
}
const names = (text: string) =>
	[...text.matchAll(/call\(a, "([A-Za-z]+)"/g)].map((m) => m[1] ?? "");

describe("ops", () => {
	it("ops_are_read_only_names", () => {
		const called = names(src);
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
		for (const m of called)
			expect(OP_NAME_ALLOWLIST.test(m) || allowedExtra.has(m), m).toBe(true);
		expect(
			called.some((m) =>
				/^(create|delete|update|set|post|put|patch|send)/i.test(m),
			),
		).toBe(false);
	});

	it("ops_invoke_adapters_only_through_call", () => {
		const body = src.slice(src.indexOf("export const OPS"));
		expect(body).not.toMatch(/\ba\.[A-Za-z]/);
		expect(body).not.toMatch(/\ba\[/);
		expect(body).not.toMatch(/\.(apply|call|bind)\(/);
	});

	it("every_method_exists_on_the_real_adapter", async () => {
		const secs = sections();
		for (const [id, def] of Object.entries(FACTORY_APPS)) {
			if (!def.adapter) continue;
			const Ctor = await def.adapter();
			for (const m of names(secs[id] ?? "")) {
				expect(typeof Ctor.prototype[m], `${id}.${m}`).toBe("function");
			}
			expect(names(secs[id] ?? "").length, id).toBeGreaterThan(0);
		}
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
