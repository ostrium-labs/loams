import { describe, expect, it } from "vitest";
import {
	viewNavigation,
	viewPermission,
} from "../src/main/factory/view-policy";

const APP = "https://git.example.com:3000";

describe("factory_view_navigation_policy", () => {
	const rows: [string, string, "allow" | "external" | "deny"][] = [
		["same origin path", "https://git.example.com:3000/a/b?x=1", "allow"],
		["same origin root", "https://git.example.com:3000", "allow"],
		["other port", "https://git.example.com:3001/", "external"],
		["other scheme", "http://git.example.com:3000/", "external"],
		["other host", "https://example.com/", "external"],
		[
			"subdomain lookalike",
			"https://git.example.com.evil.io:3000/",
			"external",
		],
		["userinfo spoof", "https://git.example.com:3000@evil.io/", "external"],
		["userinfo on app origin", "https://u:p@git.example.com:3000/", "external"],
		["file", "file:///etc/passwd", "deny"],
		["javascript", "javascript:alert(1)", "deny"],
		["data", "data:text/html,hi", "deny"],
		["custom scheme", "loams-app://console/x", "deny"],
		["mailto", "mailto:a@b.c", "deny"],
		["garbage", "not a url", "deny"],
		["empty", "", "deny"],
	];
	for (const [name, to, want] of rows)
		it(name, () => expect(viewNavigation(APP, to)).toBe(want));

	it("permissions: only sanitized clipboard write from the app origin", () => {
		expect(viewPermission("clipboard-sanitized-write", `${APP}/x`, APP)).toBe(
			true,
		);
		expect(viewPermission("clipboard-read", `${APP}/x`, APP)).toBe(false);
		expect(viewPermission("notifications", `${APP}/x`, APP)).toBe(false);
		expect(viewPermission("media", `${APP}/x`, APP)).toBe(false);
		expect(
			viewPermission("clipboard-sanitized-write", "https://evil.io/", APP),
		).toBe(false);
	});
});

describe("sso_origin_allowed_only_when_configured", () => {
	const IDP = "https://auth.example.com";
	it("external without config", () =>
		expect(viewNavigation(APP, `${IDP}/login`)).toBe("external"));
	it("allowed when configured", () =>
		expect(viewNavigation(APP, `${IDP}/login`, IDP)).toBe("allow"));
	it("configured value may carry a path", () =>
		expect(viewNavigation(APP, `${IDP}/login`, `${IDP}/application/o`)).toBe(
			"allow",
		));
	it("does not widen to other origins", () => {
		expect(viewNavigation(APP, "https://other.example.com/", IDP)).toBe(
			"external",
		);
		expect(viewNavigation(APP, "https://auth.example.com:8443/", IDP)).toBe(
			"external",
		);
	});
	it("a non-http sso value allows nothing", () =>
		expect(viewNavigation(APP, "file:///x", "file:///x")).toBe("deny"));
});
