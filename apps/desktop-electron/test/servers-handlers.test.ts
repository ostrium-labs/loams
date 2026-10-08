import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it, vi } from "vitest";
import {
	clearConsoleCookies,
	serverHandlers,
} from "../src/main/servers/handlers";
import { ServerRegistry } from "../src/main/servers/registry";

const fresh = () =>
	new ServerRegistry(join(mkdtempSync(join(tmpdir(), "srv-")), "s.json"), {
		devDemo: true,
	});

function deps() {
	const order: string[] = [];
	return {
		order,
		clearCookies: vi.fn(async () => {
			order.push("clear");
		}),
		reload: vi.fn(() => {
			order.push("reload");
		}),
	};
}

describe("serverHandlers", () => {
	it("activate_clears_console_cookies_before_reload", async () => {
		const d = deps();
		const h = serverHandlers(fresh(), d);
		expect(await h.activate("demo")).toEqual({ ok: true, value: undefined });
		expect(d.order).toEqual(["clear", "reload"]);
	});

	it("activate_unknown_does_not_touch_cookies", async () => {
		const d = deps();
		const h = serverHandlers(fresh(), d);
		expect(await h.activate("nope")).toMatchObject({ code: "not_found" });
		expect(d.clearCookies).not.toHaveBeenCalled();
		expect(d.reload).not.toHaveBeenCalled();
	});

	it("activate_still_reloads_when_cookie_clear_fails", async () => {
		const d = deps();
		d.clearCookies.mockRejectedValueOnce(new Error("x"));
		const h = serverHandlers(fresh(), d);
		expect((await h.activate("demo")).ok).toBe(true);
		expect(d.reload).toHaveBeenCalled();
	});

	it("rejects_bad_args_with_structured_error", async () => {
		const h = serverHandlers(fresh(), deps());
		for (const bad of [
			undefined,
			null,
			42,
			"x",
			[],
			{ name: 1, url: "https://a.example" },
			{ name: "a" },
			{ name: "a", url: 5 },
			{ name: "a".repeat(300), url: "https://a.example" },
			{ name: "a", url: `https://${"a".repeat(3000)}.example` },
		])
			expect(await h.add(bad), JSON.stringify(bad)).toMatchObject({
				ok: false,
				code: "bad_request",
			});
		for (const bad of [undefined, 3, {}, "", "x".repeat(200)]) {
			expect(await h.remove(bad)).toMatchObject({ code: "bad_request" });
			expect(await h.activate(bad)).toMatchObject({ code: "bad_request" });
		}
	});

	it("add_ignores_a_caller_supplied_kind", async () => {
		const h = serverHandlers(fresh(), deps());
		const r = await h.add({
			name: "x",
			kind: "local",
			url: "https://a.example/",
		});
		expect(r).toMatchObject({ ok: true, value: { kind: "remote" } });
	});
});

describe("clearConsoleCookies", () => {
	it("removes_every_cookie_of_the_console_origin_by_path", async () => {
		const removed: string[] = [];
		const jar = {
			get: vi.fn(async () => [
				{ name: "sid", path: "/" },
				{ name: "csrf", path: "/api" },
				{ name: "x" },
			]),
			remove: vi.fn(async (url: string, name: string) => {
				removed.push(`${url} ${name}`);
			}),
		};
		await clearConsoleCookies(jar);
		expect(jar.get).toHaveBeenCalledWith({ url: "loams-app://console/" });
		expect(removed).toEqual([
			"loams-app://console/ sid",
			"loams-app://console/api csrf",
			"loams-app://console/ x",
		]);
	});
});
