// SD1 T1-7 (Task 28): the desktop bundle ships its NOTICE and T3 Code's licence.
import { existsSync, readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vite-plus/test";

const app = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const config = createRequire(import.meta.url)(
	resolve(app, "electron-builder.config.cjs"),
) as {
	extraResources: { from: string; to: string }[];
};

describe("desktop bundle attribution", () => {
	it("desktop_bundle_ships_notice_and_t3code_licence", () => {
		const shipped = new Map(
			config.extraResources.map((r) => [r.to, resolve(app, r.from)]),
		);
		const notice = shipped.get("NOTICE");
		const licence = shipped.get("LICENSE.t3code");
		expect(notice && existsSync(notice)).toBe(true);
		expect(licence && existsSync(licence)).toBe(true);
		expect(readFileSync(licence as string, "utf8")).toMatch(
			/MIT License[\s\S]*T3 Tools Inc\./,
		);
		const text = readFileSync(notice as string, "utf8");
		expect(text).toContain("T3 Code");
		expect(text).toContain("LICENSE.t3code");
	});

	it("ships the dsh-desktop licence and the splash", () => {
		const shipped = new Map(
			config.extraResources.map((r) => [r.to, resolve(app, r.from)]),
		);
		for (const to of [
			"LICENSE.dsh-desktop",
			"splash/splash.html",
			"splash/loams-loader.gif",
			"splash/loams-loader-dark.gif",
		]) {
			expect(existsSync(shipped.get(to) ?? ""), to).toBe(true);
		}
	});
});
