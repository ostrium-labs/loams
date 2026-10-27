import { inspect } from "node:util";
import { describe, expect, it } from "vite-plus/test";
import { AgentdToken, upgradeHeaders } from "../src/main/agentd/client";
import {
	modeFor,
	needsConsent,
	parseAnswer,
	parseBackground,
	stopConfirmed,
} from "../src/main/agentd/consent";

describe("agentd consent", () => {
	it("unknown_values_mean_ask", () => {
		expect(parseBackground(undefined)).toBe("ask");
		expect(parseBackground("yes")).toBe("ask");
		expect(parseBackground("on")).toBe("on");
		expect(parseBackground("off")).toBe("off");
	});

	it("ask_needs_consent_and_has_no_mode", () => {
		expect(needsConsent("ask")).toBe(true);
		expect(modeFor("ask")).toBeNull();
		expect(needsConsent("on")).toBe(false);
		expect(modeFor("on")).toBe("service");
		expect(modeFor("off")).toBe("child");
	});

	it("answers_are_validated", () => {
		expect(parseAnswer("on")).toBe("on");
		expect(parseAnswer("ask")).toBeNull();
		expect(parseAnswer(1)).toBeNull();
	});

	it("stop_requires_explicit_true", () => {
		expect(stopConfirmed({ confirmed: true })).toBe(true);
		expect(stopConfirmed({ confirmed: "true" })).toBe(false);
		expect(stopConfirmed({})).toBe(false);
		expect(stopConfirmed(null)).toBe(false);
	});
});

describe("agentd token", () => {
	it("token_not_in_strings_json_or_inspect", () => {
		const secret = "c2VjcmV0LXRva2VuLWZvci10ZXN0cw";
		const t = new AgentdToken(secret);
		expect(String(t)).not.toContain(secret);
		expect(JSON.stringify({ t })).not.toContain(secret);
		expect(inspect(t)).not.toContain(secret);
		expect(inspect({ nested: t })).not.toContain(secret);
		expect(Object.keys(t)).toEqual([]);
	});

	it("upgrade_headers_carry_bearer_and_no_origin", () => {
		const h = upgradeHeaders(new AgentdToken("abc"));
		expect(h.authorization).toBe("Bearer abc");
		expect(Object.keys(h).map((k) => k.toLowerCase())).not.toContain("origin");
	});
});
