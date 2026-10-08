import { describe, expect, it, vi } from "vitest";

vi.mock("electron", () => ({ shell: { openExternal: vi.fn() } }));

import { FactoryHardening } from "../src/main/factory/hardening.electron";

type Fn = (...a: never[]) => unknown;
function fakeWc() {
	const sessionOn: Record<string, Fn> = {};
	const state: { check?: Fn; request?: Fn; sessionOnCalls: number } = {
		sessionOnCalls: 0,
	};
	const session = {
		on: (ev: string, fn: Fn) => {
			state.sessionOnCalls++;
			sessionOn[ev] = fn;
		},
		setPermissionCheckHandler: (fn: Fn) => {
			state.check = fn;
		},
		setPermissionRequestHandler: (fn: Fn) => {
			state.request = fn;
		},
	};
	const wc = {
		session,
		on: vi.fn(),
		setWindowOpenHandler: vi.fn(),
		loadURL: vi.fn(),
	};
	return { wc: wc as never, state, sessionOn };
}

describe("FactoryHardening", () => {
	it("hooks_session_once_and_reads_current_config", () => {
		const h = new FactoryHardening();
		const a = fakeWc();
		expect(h.apply("forgejo", a.wc, { url: "https://git.example.com/" })).toBe(
			true,
		);
		// A second contents on the same session does not re-hook it.
		const calls = a.state.sessionOnCalls;
		h.apply("forgejo", a.wc, { url: "https://git.example.com/" });
		expect(a.state.sessionOnCalls).toBe(calls);

		const check = a.state.check as unknown as (
			wc: unknown,
			p: string,
			o: string,
			d: object,
		) => boolean;
		const d = { isMainFrame: true, requestingUrl: "https://git.example.com/x" };
		expect(check(null, "clipboard-sanitized-write", "", d)).toBe(true);
		expect(check(null, "media", "", d)).toBe(false);
		expect(
			check(null, "clipboard-sanitized-write", "", {
				isMainFrame: true,
				requestingUrl: "https://evil.example/",
			}),
		).toBe(false);

		const dl = a.sessionOn["will-download"] as unknown as (
			e: { preventDefault(): void },
			i: { getURL(): string },
		) => void;
		const blocked = vi.fn();
		dl({ preventDefault: blocked }, { getURL: () => "https://evil.example/f" });
		expect(blocked).toHaveBeenCalledOnce();
		const ok = vi.fn();
		dl({ preventDefault: ok }, { getURL: () => "https://git.example.com/f" });
		expect(ok).not.toHaveBeenCalled();

		// Removing the app denies everything, even from its own origin.
		h.forget("forgejo");
		expect(check(null, "clipboard-sanitized-write", "", d)).toBe(false);
		const after = vi.fn();
		dl(
			{ preventDefault: after },
			{ getURL: () => "https://git.example.com/f" },
		);
		expect(after).toHaveBeenCalledOnce();
	});

	it("rejects_non_web_urls", () => {
		expect(
			new FactoryHardening().apply("forgejo", fakeWc().wc, {
				url: "file:///etc/passwd",
			}),
		).toBe(false);
	});
});
