import { describe, expect, it, vi } from "vite-plus/test";
import {
	autoCheckEnabled,
	decideUpdateMode,
	UPDATE_AUTO_KEY,
} from "../src/main/update/policy";
import { createCheckScheduler } from "../src/main/update/scheduler";

const base = {
	packaged: true,
	feed: "https://example.com/feed",
	pubkeyHex: "a".repeat(64),
	platform: "win32" as const,
	appImage: undefined,
};

describe("automatic update checks", () => {
	it("default_is_on_and_only_false_turns_it_off", () => {
		expect(UPDATE_AUTO_KEY).toBe("update.autoCheck");
		expect(autoCheckEnabled(true)).toBe(true);
		expect(autoCheckEnabled(undefined)).toBe(true);
		expect(autoCheckEnabled("junk")).toBe(true);
		expect(autoCheckEnabled(false)).toBe(false);
	});

	it("env_var_forces_off_and_disables_the_mode", () => {
		expect(autoCheckEnabled(true, "1")).toBe(false);
		expect(autoCheckEnabled(true, "0")).toBe(true);
		expect(decideUpdateMode(base)).toBe("self");
		expect(decideUpdateMode({ ...base, noUpdateCheckEnv: "1" })).toBe(
			"disabled",
		);
		expect(decideUpdateMode({ ...base, noUpdateCheckEnv: "" })).toBe("self");
	});

	it("off_stops_the_scheduler_and_on_restarts_it", () => {
		vi.useFakeTimers();
		try {
			const check = vi.fn();
			let listeners = 0;
			const s = createCheckScheduler({
				check,
				rand: () => 0,
				onResume: () => {
					listeners++;
					return () => void listeners--;
				},
			});
			s.setEnabled(true);
			expect(s.running()).toBe(true);
			expect(listeners).toBe(1);
			vi.advanceTimersByTime(15_000);
			expect(check).toHaveBeenCalledTimes(1);
			vi.advanceTimersByTime(6 * 60 * 60 * 1000);
			expect(check).toHaveBeenCalledTimes(2);

			s.setEnabled(false);
			expect(s.running()).toBe(false);
			expect(listeners).toBe(0);
			vi.advanceTimersByTime(24 * 60 * 60 * 1000);
			expect(check).toHaveBeenCalledTimes(2);

			s.setEnabled(true);
			vi.advanceTimersByTime(15_000);
			expect(check).toHaveBeenCalledTimes(3);
		} finally {
			vi.useRealTimers();
		}
	});

	it("never_started_when_disabled_from_the_start", () => {
		vi.useFakeTimers();
		try {
			const check = vi.fn();
			const s = createCheckScheduler({ check, onResume: () => () => {} });
			s.setEnabled(false);
			vi.advanceTimersByTime(48 * 60 * 60 * 1000);
			expect(check).not.toHaveBeenCalled();
		} finally {
			vi.useRealTimers();
		}
	});
});
