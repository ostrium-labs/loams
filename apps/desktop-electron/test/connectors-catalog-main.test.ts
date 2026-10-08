import { describe, expect, it } from "vitest";
import { ConnectorCatalog, catalogPath } from "../src/main/connectors/catalog";

const file = JSON.stringify({
	version: 1,
	connectors: [{ id: "kafka", name: "Kafka" }],
	details: { kafka: { manifest: { id: "kafka" }, schema: { type: "object" } } },
});

describe("ConnectorCatalog", () => {
	it("serves the list and one detail", () => {
		const c = new ConnectorCatalog(() => file);
		expect(c.catalog()).toHaveLength(1);
		expect(c.get("kafka")).toMatchObject({ ok: true });
	});
	it("rejects unknown and non-string ids, including prototype keys", () => {
		const c = new ConnectorCatalog(() => file);
		expect(c.get("nope")).toMatchObject({ ok: false, code: "not_found" });
		expect(c.get("__proto__")).toMatchObject({ ok: false, code: "not_found" });
		expect(c.get(42)).toMatchObject({ ok: false, code: "bad_request" });
	});
	it("reads the file once", () => {
		let n = 0;
		const c = new ConnectorCatalog(() => { n++; return file; });
		c.catalog();
		c.get("kafka");
		expect(n).toBe(1);
	});
	it("validates a config against the schema (2020-12, unknown keywords tolerated)", () => {
		const f = JSON.stringify({
			version: 1,
			connectors: [],
			details: {
				k: {
					manifest: {},
					schema: {
						$schema: "https://json-schema.org/draft/2020-12/schema",
						$id: "https://loams.dev/schemas/k.config.json",
						"x-loams-generated-by": "gen",
						type: "object",
						required: ["brokers"],
						properties: { brokers: { type: "string", minLength: 1 } },
					},
				},
			},
		});
		const c = new ConnectorCatalog(() => f);
		expect(c.validate("k", { brokers: "a:1" })).toEqual({
			ok: true,
			value: { valid: true, errors: [] },
		});
		const bad = c.validate("k", {});
		expect(bad).toMatchObject({ ok: true, value: { valid: false } });
		expect(c.validate("zz", {})).toMatchObject({ ok: false, code: "not_found" });
	});
	it("resolves the path for packaged and dev", () => {
		expect(
			catalogPath({ isPackaged: true, resourcesPath: "/r", appRoot: "/a" }),
		).toBe("/r/connectors.json");
		expect(
			catalogPath({ isPackaged: false, resourcesPath: "/r", appRoot: "/a" }),
		).toBe("/a/resources/connectors.json");
	});
});
