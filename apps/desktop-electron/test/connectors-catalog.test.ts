import { readdirSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
// @ts-expect-error plain ESM script without types
import { buildCatalog, REPO_ROOT } from "../scripts/connectors-catalog.mjs";

const catalog = buildCatalog() as {
	connectors: {
		id: string;
		stub: boolean;
		status: string;
		source: string[] | null;
	}[];
	details: Record<string, { manifest: { id: string }; schema: unknown }>;
};

describe("connectors catalog script", () => {
	it("catalog_script_counts_match_registry", () => {
		const yamls = readdirSync(join(REPO_ROOT, "connectors", "registry")).filter(
			(n) => n.endsWith(".yaml"),
		);
		expect(yamls.length).toBeGreaterThan(100);
		expect(catalog.connectors).toHaveLength(yamls.length);
		expect(Object.keys(catalog.details)).toHaveLength(yamls.length);
		expect(new Set(catalog.connectors.map((c) => c.id)).size).toBe(
			yamls.length,
		);
		for (const c of catalog.connectors)
			expect(catalog.details[c.id]?.manifest.id).toBe(c.id);
	});

	it("stub_flagged", () => {
		const by = (id: string) => catalog.connectors.find((c) => c.id === id);
		expect(by("adyen")?.stub).toBe(true);
		expect(by("kafka")?.stub).toBe(false);
		expect(by("kafka")?.source).toEqual(["streaming"]);
		// A stub is always a planned row (grafeo is planned but has a real schema, so it is not one).
		for (const c of catalog.connectors) {
			if (c.stub) expect(c.status).toBe("planned");
		}
		expect(by("grafeo")?.stub).toBe(false);
	});
});
