import { describe, expect, it } from "vitest";
import { ConnectorCatalog, catalogPath } from "../src/main/connectors/catalog";
import { saveYaml } from "../src/main/connectors/save";

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
		const c = new ConnectorCatalog(() => {
			n++;
			return file;
		});
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
		expect(c.validate("zz", {})).toMatchObject({
			ok: false,
			code: "not_found",
		});
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

describe("validate guards", () => {
	const mk = (schema: object, secrets: string[] = []) =>
		new ConnectorCatalog(() =>
			JSON.stringify({
				version: 1,
				connectors: [],
				details: { k: { manifest: { secrets }, schema } },
			}),
		);
	const schema = {
		type: "object",
		required: ["token", "nested"],
		properties: {
			token: {
				type: "string",
				minLength: 40,
				pattern: "^[a-f0-9]+$",
				writeOnly: true,
			},
			nested: {
				type: "object",
				required: ["pw"],
				properties: { pw: { type: "string", minLength: 30, format: "uuid" } },
			},
		},
	};

	it("rejects non-object, oversized and too deep configs", () => {
		const c = mk(schema);
		for (const bad of [null, [], "x", 3, Object.create({ a: 1 })])
			expect(c.validate("k", bad)).toMatchObject({
				ok: false,
				code: "bad_request",
			});
		expect(c.validate("k", { a: "x".repeat(300 * 1024) })).toMatchObject({
			ok: false,
			code: "bad_request",
		});
		let deep: Record<string, unknown> = {};
		for (let i = 0; i < 40; i++) deep = { d: deep };
		expect(c.validate("k", deep)).toMatchObject({
			ok: false,
			code: "bad_request",
		});
	});

	it("does not apply minLength, pattern or format to secret placeholders", () => {
		const c = mk(schema, ["nested.pw"]);
		const ok = c.validate("k", {
			token: "${secret:token}",
			nested: { pw: "${secret:nested.pw}" },
		});
		expect(ok).toEqual({ ok: true, value: { valid: true, errors: [] } });
		// A missing required secret is still an error.
		const miss = c.validate("k", { nested: {} });
		expect(miss).toMatchObject({ ok: true, value: { valid: false } });
	});
});

describe("saveYaml", () => {
	const deps = (path: string | undefined, written: string[][] = []) => ({
		pick: async () => path,
		write: async (p: string, t: string) => void written.push([p, t]),
	});
	it("writes to the picked path, reports cancel, and caps size", async () => {
		const w: string[][] = [];
		expect(await saveYaml(deps("/x/a.yaml", w), "a/../b", "k: v")).toEqual({
			ok: true,
			value: { saved: true },
		});
		expect(w).toEqual([["/x/a.yaml", "k: v"]]);
		expect(await saveYaml(deps(undefined), "a", "k: v")).toEqual({
			ok: true,
			value: { saved: false },
		});
		expect(
			await saveYaml(deps("/x"), "a", "x".repeat(300 * 1024)),
		).toMatchObject({
			ok: false,
			code: "too_large",
		});
		expect(await saveYaml(deps("/x"), 1, "x")).toMatchObject({
			code: "bad_request",
		});
	});
	it("surfaces write errors", async () => {
		const r = await saveYaml(
			{
				pick: async () => "/x",
				write: async () => {
					throw new Error("EACCES");
				},
			},
			"a",
			"k",
		);
		expect(r).toMatchObject({
			ok: false,
			code: "write_failed",
			message: "EACCES",
		});
	});
});
