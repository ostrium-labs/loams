import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import {
	CONSOLE_CSP,
	ensureCspMeta,
	resolveStatic,
} from "../src/main/protocol/static";

const root = "/srv/dist";
const u = (s: string) => new URL(s);

describe("resolveStatic", () => {
	it("serves_index_and_assets_with_mime", () => {
		expect(resolveStatic(root, u("loams-app://console/ui/index.html"))).toEqual(
			{
				file: join(root, "index.html"),
				mime: "text/html; charset=utf-8",
			},
		);
		expect(
			resolveStatic(root, u("loams-app://console/ui/assets/a-1.js")),
		).toEqual({
			file: join(root, "assets", "a-1.js"),
			mime: "text/javascript; charset=utf-8",
		});
		expect(
			resolveStatic(root, u("loams-app://console/ui/config.json")),
		).toMatchObject({
			mime: "application/json; charset=utf-8",
		});
		expect(
			resolveStatic(root, u("loams-app://console/ui/a.css")),
		).toMatchObject({
			mime: "text/css; charset=utf-8",
		});
		expect(
			resolveStatic(root, u("loams-app://console/ui/a.woff2")),
		).toMatchObject({
			mime: "font/woff2",
		});
	});

	it("spa_fallback_for_routes", () => {
		expect(resolveStatic(root, u("loams-app://console/ui/projects/x"))).toEqual(
			{
				spaFallback: join(root, "index.html"),
			},
		);
		expect(resolveStatic(root, u("loams-app://console/ui/"))).toEqual({
			spaFallback: join(root, "index.html"),
		});
		expect(
			resolveStatic(root, u("loams-app://console/ui/cordis/data")),
		).toEqual({
			spaFallback: join(root, "cordis.html"),
		});
	});

	it("rejects_traversal_variants", () => {
		for (const s of [
			"loams-app://console/ui/..%2f..%2fetc/passwd",
			"loams-app://console/ui/%2e%2e/%2e%2e/etc/passwd",
			"loams-app://console/ui/..%2fsecret.txt",
			"loams-app://console/ui/..\\..\\x.txt",
			"loams-app://console/ui/a%00.js",
			"loams-app://console/etc/passwd",
			"loams-app://console//etc/passwd",
			"loams-app://evil/ui/index.html",
		]) {
			const r = resolveStatic(root, u(s));
			expect(r, s).toBeNull();
		}
	});
});

describe("ensureCspMeta", () => {
	it("injects_console_csp_after_charset_once", () => {
		const html =
			'<!doctype html><html><head>\n<meta charset="utf-8" />\n<title>x</title></head></html>';
		const out = ensureCspMeta(html);
		expect(out).toContain(
			`<meta charset="utf-8" />\n    <meta http-equiv="Content-Security-Policy" content="${CONSOLE_CSP}" />`,
		);
		expect(ensureCspMeta(out)).toBe(out);
	});
	it("keeps_an_existing_csp_meta", () => {
		const html =
			'<head><meta charset="utf-8"><meta http-equiv="content-security-policy" content="default-src \'none\'"></head>';
		expect(ensureCspMeta(html)).toBe(html);
	});
	it("allows_existing_inline_scripts_by_hash_only", () => {
		const body = "document.documentElement.classList.add('dark');";
		const out = ensureCspMeta(
			`<head><meta charset="utf-8" /><script>${body}</script><script src="/a.js"></script></head>`,
		);
		const hash = createHash("sha256").update(body).digest("base64");
		expect(out).toContain(`script-src 'self' 'sha256-${hash}'; style-src`);
		expect(out.match(/sha256-/g)?.length).toBe(1);
	});
	it("injects_at_head_start_without_charset", () => {
		const out = ensureCspMeta("<html><head><title>x</title></head></html>");
		expect(out).toMatch(
			/^<html><head><meta http-equiv="Content-Security-Policy"/,
		);
	});
	it("csp_equals_the_vite_build_policy", () => {
		const vite = readFileSync(
			join(__dirname, "../../../web/apps/console/vite.config.ts"),
			"utf8",
		);
		const decl = /const CSP =([\s\S]*?);\n/.exec(vite)?.[1] ?? "";
		const parts = [...decl.matchAll(/"([^"]*)"/g)].map((m) => m[1]).join("");
		expect(parts).toBe(CONSOLE_CSP);
	});
	it("csp_matches_the_console_build_policy", () => {
		expect(CONSOLE_CSP).toContain("script-src 'self'");
		expect(CONSOLE_CSP).not.toContain("unsafe-eval");
		expect(CONSOLE_CSP).toContain("object-src 'none'");
	});
});
