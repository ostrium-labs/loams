import { describe, expect, it } from "vite-plus/test";
import { parseDeepLink } from "../src/shared/deeplink";

describe("deeplink_allowlist", () => {
	it.each([
		["loams://open/console/approvals/apr_1", "/approvals/apr_1"],
		["loams://open/console/servers", "/servers"],
		["loams://open/data", "/data"],
		["loams://open/data/ns.one/coll-2", "/data/ns.one/coll-2"],
		["loams://open/factory", "/studio/factory"],
		["loams://open/factory/forgejo", "/studio/factory/forgejo"],
		["loams://open/servers", "/settings/servers"],
		// No per-server route exists: a server id lands on the servers page.
		["loams://open/servers/abc", "/settings/servers"],
	])("accepts %s", (raw, path) => {
		expect(parseDeepLink(raw)).toEqual({ path });
	});
	it.each([
		"loams://open/../x",
		"loams://open/console/../x",
		"loams://open/console/./x",
		"loams://open/data/%2e%2e/x",
		"loams://run/rm",
		"loams://open/data?x=javascript:",
		"loams://open/data#/x",
		"loams://open/console",
		"loams://open/console/",
		"loams://open/data/",
		"loams://open/data//x",
		"loams://open/factory/a/b",
		"loams://open/unknown",
		"loams://open",
		"loams://open/",
		"loams://evil/open/data",
		"loams://user@open/data",
		"loams://open:80/data",
		"loams://open/data\\x",
		"loams://open/data/a b",
		"loams://open/data/\u0000",
		"https://open/data",
		"javascript:alert(1)",
		"LOAMS-APP://console/",
		"",
	])("rejects %j", (raw) => {
		expect(parseDeepLink(raw)).toBeNull();
	});
	it("deeplink_allows_studio_thread_and_group", () => {
		expect(parseDeepLink("loams://open/studio/t/c_01hq7x")).toEqual({
			path: "/studio/t/c_01hq7x",
		});
		expect(parseDeepLink("loams://open/studio/g/g-2")).toEqual({
			path: "/studio/g/g-2",
		});
		for (const raw of [
			"loams://open/studio",
			"loams://open/studio/t",
			"loams://open/studio/t/",
			"loams://open/studio/t/a/b",
			"loams://open/studio/p/x",
			"loams://open/studio/settings",
			"loams://open/studio/t/..",
			"loams://open/studio/t/a?panel=diff",
		])
			expect(parseDeepLink(raw)).toBeNull();
	});
	it("deeplink_opens_thread_and_group", () => {
		// SD1 Task 28 (§53 §4.4): a thread or run group opens its Studio route, which
		// switches the console to Studio (syncModeWithRoute).
		expect(parseDeepLink("loams://open/studio/t/chat_1")).toEqual({
			path: "/studio/t/chat_1",
		});
		expect(parseDeepLink("loams://open/studio/g/grp-1")).toEqual({
			path: "/studio/g/grp-1",
		});
		expect(parseDeepLink("loams://open/studio/x/chat_1")).toBeNull();
		expect(parseDeepLink("loams://open/studio/t/chat_1/extra")).toBeNull();
	});
	it("agent_deeplink_maps_to_thread", () => {
		// DD1's agent links name a thread (a chat id): Studio shows it (Shared contracts).
		expect(parseDeepLink("loams://open/agent/c_01hq7x")).toEqual({
			path: "/studio/t/c_01hq7x",
		});
		for (const raw of [
			"loams://open/agent",
			"loams://open/agent/",
			"loams://open/agent/a/b",
			"loams://open/agent/..",
			"loams://open/agent/a%20b",
			"loams://open/agent/a?x=1",
		])
			expect(parseDeepLink(raw)).toBeNull();
	});
	it("is case-insensitive on the scheme only", () => {
		expect(parseDeepLink("LOAMS://open/data")).toEqual({ path: "/data" });
	});
});
