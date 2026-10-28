import { createHash } from "node:crypto";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative, resolve, sep } from "node:path";
import { describe, expect, it } from "vite-plus/test";

const app = resolve(__dirname, "..");
const HEADER = /^\/\/ Adapted from dataelement\/dsh-desktop \(MIT\), (\S+)\.$|^<!-- Adapted from dataelement\/dsh-desktop \(MIT\), (\S+)\. -->$/m;
const prov = JSON.parse(readFileSync(join(app, "provenance.json"), "utf8")) as {
	upstream: string;
	license: string;
	copyright: string;
	files: { path: string; upstreamPath: string; commit: string; upstreamSha256: string; localSha256: string; verdict: string }[];
};

function* files(dir: string): Generator<string> {
	for (const name of readdirSync(dir)) {
		if (["node_modules", "out", "dist", "test-results"].includes(name) || name.startsWith(".")) continue;
		const p = join(dir, name);
		if (statSync(p).isDirectory()) yield* files(p);
		else if (/\.(ts|mjs|cjs|html)$/.test(name)) yield p;
	}
}

describe("dsh-desktop provenance (§10)", () => {
	it("names dsh-desktop, its licence and copyright", () => {
		expect(prov.upstream).toBe("https://github.com/dataelement/dsh-desktop");
		expect(prov.license).toBe("MIT");
		expect(prov.copyright).toBe("Copyright (c) 2026 DataElement");
		expect(readFileSync(join(app, "LICENSE.dsh-desktop"), "utf8")).toMatch(/MIT License[\s\S]*Copyright \(c\) 2026 DataElement/);
		const notice = readFileSync(join(app, "NOTICE"), "utf8");
		expect(notice).toContain("LICENSE.dsh-desktop");
		expect(notice).toContain("provenance.json");
	});

	it("has a row for every adapted file, and every row's file says where it came from", () => {
		const rows = new Map(prov.files.map((r) => [r.path, r]));
		const headed: string[] = [];
		for (const p of [...files(join(app, "src")), ...files(join(app, "scripts")), ...files(join(app, "build")), join(app, "electron.vite.config.ts")]) {
			const m = HEADER.exec(readFileSync(p, "utf8"));
			if (!m) continue;
			// provenance.json keys use '/', whatever the platform's separator.
			const rel = relative(app, p).split(sep).join("/");
			headed.push(rel);
			expect(rows.get(rel)?.upstreamPath, rel).toBe(m[1] ?? m[2]);
		}
		expect([...rows.keys()].sort()).toEqual(headed.sort());
		for (const r of prov.files) {
			expect(r.commit).toBe("bc732f5dc4c5465915d5b3f28d9b0b32703fc664");
			expect(r.upstreamSha256).toMatch(/^[0-9a-f]{64}$/);
			expect(["as-is", "adapt", "rewrite"]).toContain(r.verdict);
			if (r.verdict === "as-is") expect(createHash("sha256").update(readFileSync(join(app, r.path))).digest("hex")).toBe(r.localSha256);
		}
	});

	it("never depends on @deepseek-ai packages", () => {
		const manifest = readFileSync(join(app, "package.json"), "utf8");
		expect(manifest).not.toContain("@deepseek-ai");
		for (const p of files(join(app, "src"))) expect(readFileSync(p, "utf8"), p).not.toContain("@deepseek-ai");
	});
});
