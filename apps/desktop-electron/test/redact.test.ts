import { describe, expect, it } from "vitest";
import { Secret } from "../src/main/factory/vault";
import { redact } from "../src/main/redact";

describe("redact (shared)", () => {
	it("masks_every_secret_longest_first", () => {
		expect(redact("a tok-123 and tok-1234", ["tok-123", "tok-1234"])).toBe(
			"a [redacted] and [redacted]",
		);
		expect(redact("x", [""])).toBe("x");
	});
	it("takes_secret_objects_and_a_custom_mask", () => {
		expect(redact("pw hunter2", [new Secret("hunter2")], { mask: "***" })).toBe(
			"pw ***",
		);
	});
	it("credentials_mode_strips_urls_and_password_pairs", () => {
		const o = { mask: "***", credentials: true };
		expect(redact("mysql://root:s3cr3t@h/db", [], o)).toBe(
			"mysql://root:***@h/db",
		);
		expect(redact("password=abc user=x", [], o)).toBe("password=*** user=x");
		// The user name survives when it equals the password.
		expect(
			redact(
				"failed postgres://cloud_admin:cloud_admin@h/d",
				["cloud_admin"],
				o,
			),
		).toBe("failed postgres://cloud_admin:***@h/d");
	});
	it("regex_characters_in_a_secret_are_literal", () => {
		expect(redact("k=a.b+c", ["a.b+c"])).toBe("k=[redacted]");
		expect(redact("k=aXb", ["a.b"])).toBe("k=aXb");
	});
});
