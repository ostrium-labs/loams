import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { resolveStatic } from "../src/main/protocol/static";

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
