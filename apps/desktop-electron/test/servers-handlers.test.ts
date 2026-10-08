import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it, vi } from "vitest";
import {
	clearSessionCookies,
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
		clearCookies: vi.fn(async (_origin: string) => {
			order.push("clear");
		}),
		reload: vi.fn(() => {
			order.push("reload");
		}),
	};
}

describe("serverHandlers", () => {
	it("activate_clears_the_departing_servers_cookies_before_reload", async () => {
		const d = deps();
		const reg = fresh();
		const h = serverHandlers(reg, d);
		const added = await h.add({ name: "a", url: "https://a.example" });
		if (!added.ok) throw new Error("add");
		await h.activate(added.value.id);
		d.order.length = 0;
		d.clearCookies.mockClear();
		expect(await h.activate("demo")).toEqual({ ok: true, value: undefined });
		expect(d.order).toEqual(["clear", "reload"]);
		expect(d.clearCookies).toHaveBeenCalledWith("https://a.example");
	});

	it("re_activating_the_same_server_keeps_its_session", async () => {
		const d = deps();
		const h = serverHandlers(fresh(), d);
		await h.activate("demo");
		d.clearCookies.mockClear();
		await h.activate("demo");
		expect(d.clearCookies).not.toHaveBeenCalled();
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

describe("clearSessionCookies", () => {
	type C = { name: string; domain?: string; path?: string; secure?: boolean };
	/** Behaves like Chromium: a `url` filter path-matches, so it misses `Path=/api` cookies. */
	function chromiumJar(cookies: C[]) {
		const removed: string[] = [];
		return {
			removed,
			get: vi.fn(async (f: { url?: string }) => {
				if (f.url) {
					const u = new URL(f.url);
					return cookies.filter(
						(c) => c.domain === u.hostname && (c.path ?? "/") === u.pathname,
					);
				}
				return cookies;
			}),
			remove: vi.fn(async (url: string, name: string) => {
				removed.push(`${url} ${name}`);
			}),
		};
	}

	it("removes_path_scoped_secure_and_domain_cookies_of_that_host_only", async () => {
		const jar = chromiumJar([
			{ name: "sid", domain: "127.0.0.1", path: "/" },
			{ name: "csrf", domain: "127.0.0.1", path: "/api" },
			{ name: "sec", domain: "127.0.0.1", path: "/", secure: true },
			{ name: "c", domain: "console", path: "/v1" },
			{ name: "other", domain: "git.example.com", path: "/" },
		]);
		await clearSessionCookies(jar, "http://127.0.0.1:8084");
		expect(jar.removed).toEqual([
			"http://127.0.0.1/ sid",
			"http://127.0.0.1/api csrf",
			"https://127.0.0.1/ sec",
			"loams-app://console/v1 c",
		]);
	});

	it("matches_parent_domain_cookies_that_would_reach_the_host", async () => {
		const jar = chromiumJar([
			{ name: "wide", domain: ".example.com", path: "/" },
			{ name: "host", domain: "api.example.com", path: "/" },
			{ name: "sibling", domain: "web.example.com", path: "/" },
			{ name: "lookalike", domain: ".ample.com", path: "/" },
		]);
		await clearSessionCookies(jar, "https://api.example.com");
		expect(jar.removed).toEqual([
			"https://example.com/ wide",
			"https://api.example.com/ host",
		]);
	});

	it("an_empty_origin_only_clears_the_console", async () => {
		const jar = chromiumJar([
			{ name: "sid", domain: "127.0.0.1", path: "/" },
			{ name: "c", domain: "console", path: "/" },
		]);
		await clearSessionCookies(jar, "");
		expect(jar.removed).toEqual(["loams-app://console/ c"]);
	});
});
