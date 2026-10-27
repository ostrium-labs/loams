import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it, vi } from "vite-plus/test";
import { SPLASH_TIMEOUT_MS, SplashGate } from "../src/main/shell/splash";

const build = resolve(__dirname, "../build");

describe("the splash (D1084)", () => {
	it("shows the main window once, on ready or after the timeout", () => {
		let fire: () => void = () => {};
		const onShow = vi.fn();
		const gate = new SplashGate({ timeoutMs: SPLASH_TIMEOUT_MS, onShow, schedule: (fn) => ((fire = fn), () => {}) });
		gate.ready();
		gate.ready();
		fire();
		expect(onShow).toHaveBeenCalledOnce();
		const late = vi.fn();
		const g2 = new SplashGate({ timeoutMs: 10, onShow: late, schedule: (fn) => ((fire = fn), () => {}) });
		fire();
		g2.ready();
		expect(late).toHaveBeenCalledOnce();
	});

	it("is a local page with a strict CSP and both loaders", () => {
		const html = readFileSync(resolve(build, "splash.html"), "utf8");
		expect(html).toContain(`http-equiv="Content-Security-Policy"`);
		expect(html).toContain("default-src 'none'");
		expect(html).not.toMatch(/https?:\/\//);
		expect(html).toContain("Starting Loams Desktop");
		for (const gif of ["loams-loader.gif", "loams-loader-dark.gif"]) {
			expect(html).toContain(gif);
			expect(existsSync(resolve(build, gif))).toBe(true);
			expect(readFileSync(resolve(build, gif)).subarray(0, 6).toString()).toBe("GIF89a");
		}
	});
});
