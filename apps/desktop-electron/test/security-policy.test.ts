import { describe, expect, it } from "vitest";
import {
	assertTrustedSender,
	navigationDecision,
	permissionDecision,
} from "../src/main/security/policy";

const HOME = "loams-app://console/ui/cordis.html";

describe("navigation_matrix", () => {
	const rows: [string, string, "allow" | "external" | "deny"][] = [
		[HOME, "loams-app://console/ui/cordis.html#/servers", "allow"],
		[HOME, "loams-app://console/ui/projects/x", "allow"],
		[HOME, "loams-app://console/", "allow"],
		[HOME, "loams-app://evil/ui/", "deny"],
		[HOME, "loams-app://console.evil.com/ui/", "deny"],
		[HOME, "loams-app://console@evil.com/ui/", "deny"],
		[HOME, "https://example.com/x", "external"],
		[HOME, "http://127.0.0.1:8084/ui/", "external"],
		[HOME, "file:///etc/passwd", "deny"],
		[HOME, "javascript:alert(1)", "deny"],
		[HOME, "data:text/html,<script>1</script>", "deny"],
		[HOME, "blob:https://example.com/x", "deny"],
		[HOME, "ftp://example.com/x", "deny"],
		[HOME, "loams://open/data", "deny"],
		[HOME, "not a url", "deny"],
		[HOME, "", "deny"],
	];
	it.each(rows)("%s -> %s = %s", (from, to, want) => {
		expect(navigationDecision(from, to)).toBe(want);
	});
});

describe("permission_matrix", () => {
	const ok = "loams-app://console";
	it("grants only clipboard write and notifications from the console", () => {
		expect(permissionDecision("clipboard-sanitized-write", ok)).toBe(true);
		expect(permissionDecision("notifications", ok)).toBe(true);
		expect(permissionDecision("notifications", `${ok}/`)).toBe(true);
		expect(permissionDecision("notifications", `${ok}/ui/cordis.html`)).toBe(
			true,
		);
	});
	it.each([
		"media",
		"geolocation",
		"clipboard-read",
		"openExternal",
		"midi",
		"fullscreen",
		"",
	])("denies %s", (p) => {
		expect(permissionDecision(p, ok)).toBe(false);
	});
	it.each([
		"https://example.com",
		"loams-app://other",
		"loams-app://console.evil.com",
		"file:///x",
		"",
	])("denies origin %s", (o) => {
		expect(permissionDecision("notifications", o)).toBe(false);
	});
});

describe("assert_trusted_sender_rejects_other_frames", () => {
	const top = { url: "loams-app://console/ui/cordis.html", parent: null };
	it("accepts the top console frame", () => {
		expect(() => assertTrustedSender({ senderFrame: top })).not.toThrow();
	});
	it("rejects a child frame even with a console url", () => {
		expect(() =>
			assertTrustedSender({
				senderFrame: { url: "loams-app://console/ui/x", parent: top },
			}),
		).toThrow(/untrusted/);
	});
	it.each([
		"https://example.com/",
		"loams-app://evil/ui/",
		"loams-app://console.evil.com/ui/",
		"file:///x",
		"",
	])("rejects url %s", (url) => {
		expect(() =>
			assertTrustedSender({ senderFrame: { url, parent: null } }),
		).toThrow(/untrusted/);
	});
	it("rejects a missing or null frame", () => {
		expect(() => assertTrustedSender({ senderFrame: null })).toThrow();
		expect(() => assertTrustedSender({})).toThrow();
	});
});
