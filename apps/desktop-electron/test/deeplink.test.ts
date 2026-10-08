import { describe, expect, it } from "vitest";
import { parseDeepLink } from "../src/shared/deeplink";

describe("deeplink_allowlist", () => {
	it.each([
		["loams://open/console/approvals/apr_1", "/approvals/apr_1"],
		["loams://open/console/servers", "/servers"],
		["loams://open/data", "/data"],
		["loams://open/data/ns.one/coll-2", "/data/ns.one/coll-2"],
		["loams://open/factory", "/factory"],
		["loams://open/factory/forgejo", "/factory/forgejo"],
		["loams://open/servers", "/settings/servers"],
		["loams://open/servers/abc", "/settings/servers/abc"],
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
	it("is case-insensitive on the scheme only", () => {
		expect(parseDeepLink("LOAMS://open/data")).toEqual({ path: "/data" });
	});
});
