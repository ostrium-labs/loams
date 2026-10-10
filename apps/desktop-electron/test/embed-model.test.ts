import { describe, expect, it } from "vitest";
import {
	clampBounds,
	EmbedController,
	isFocusConsoleKey,
} from "../src/main/factory/embed-model";
import type { FactoryAppId } from "../src/shared/contracts";

const CONTENT = { width: 1000, height: 700 };

describe("embed model", () => {
	it("bounds_clamped_and_validated", () => {
		expect(
			clampBounds({ x: 248, y: 52.4, width: 700.6, height: 300 }, CONTENT),
		).toEqual({ x: 248, y: 52, width: 701, height: 300 });
		// Overflows the content on both axes.
		expect(
			clampBounds({ x: 900, y: 600, width: 500, height: 500 }, CONTENT),
		).toEqual({ x: 900, y: 600, width: 100, height: 100 });
		// Entirely outside, or no area: hide.
		expect(
			clampBounds({ x: 1200, y: 0, width: 10, height: 10 }, CONTENT),
		).toBeNull();
		expect(
			clampBounds({ x: 0, y: 0, width: 0, height: 10 }, CONTENT),
		).toBeNull();
		for (const bad of [
			{ x: 0, y: 0, width: Number.NaN, height: 10 },
			{ x: 0, y: 0, width: 10, height: Number.POSITIVE_INFINITY },
			{ x: "0", y: 0, width: 10, height: 10 },
			{ x: 0, y: 0, width: 10 },
			null,
			"rect",
		])
			expect(clampBounds(bad, CONTENT)).toBeUndefined();
	});

	it("negative_origin_clamped_and_shrunk", () => {
		expect(
			clampBounds({ x: -20, y: -10, width: 100, height: 50 }, CONTENT),
		).toEqual({ x: 0, y: 0, width: 80, height: 40 });
	});

	it("zero_area_hides", () => {
		// Entirely left of the window after clamping, and a negative size.
		expect(
			clampBounds({ x: -200, y: 0, width: 100, height: 50 }, CONTENT),
		).toBeNull();
		expect(
			clampBounds({ x: 0, y: 0, width: -5, height: 50 }, CONTENT),
		).toBeNull();
		const { ctl, log } = harness();
		ctl.show("forgejo", R);
		expect(ctl.show("forgejo", { ...R, x: -500 }).ok).toBe(true);
		expect(log).toContain("conceal:forgejo#1");
	});

	it("invalid_bounds_hide_current_view", () => {
		const { ctl, log } = harness();
		ctl.show("forgejo", R);
		const r = ctl.show("forgejo", { ...R, width: Number.NaN });
		expect(r).toMatchObject({ ok: false, code: "bad_request" });
		expect(log).toContain("conceal:forgejo#1");
		// A bad report for another app hides the visible one too.
		ctl.show("forgejo", R);
		log.length = 0;
		ctl.show("zulip", null);
		expect(log).toContain("conceal:forgejo#1");
	});

	it("zoom_scales_bounds", () => {
		expect(
			clampBounds({ x: 100, y: 50, width: 400, height: 200 }, CONTENT, 1.25),
		).toEqual({ x: 125, y: 63, width: 500, height: 250 });
		expect(
			clampBounds({ x: 100, y: 50, width: 400, height: 200 }, CONTENT, 0.5),
		).toEqual({ x: 50, y: 25, width: 200, height: 100 });
	});

	it("concealing_focused_view_returns_focus_to_console", () => {
		const { ctl, log, focus } = harness();
		ctl.show("forgejo", R);
		focus.on = "forgejo#1";
		ctl.hide();
		expect(log).toEqual(
			expect.arrayContaining(["focusConsole", "conceal:forgejo#1"]),
		);
		expect(log.indexOf("focusConsole")).toBeLessThan(
			log.indexOf("conceal:forgejo#1"),
		);
		// Destroying a focused view does the same.
		log.length = 0;
		ctl.show("forgejo", R);
		focus.on = "forgejo#1";
		ctl.destroy("forgejo");
		expect(log.indexOf("focusConsole")).toBeLessThan(
			log.indexOf("destroy:forgejo#1"),
		);
		// An unfocused view leaves focus alone.
		log.length = 0;
		focus.on = "";
		ctl.show("zulip", R);
		ctl.hide();
		expect(log).not.toContain("focusConsole");
	});

	it("evicts_before_creating_fifth_view", () => {
		const { ctl, log } = harness();
		for (const a of ["forgejo", "zulip", "plane", "glitchtip"] as const)
			ctl.show(a, R);
		log.length = 0;
		ctl.show("matomo", R);
		expect(log.indexOf("destroy:forgejo#1")).toBeLessThan(
			log.indexOf("create:matomo"),
		);
	});

	function harness(
		configured: FactoryAppId[] = [
			"forgejo",
			"zulip",
			"plane",
			"glitchtip",
			"matomo",
		],
	) {
		const log: string[] = [];
		const focus = { on: "" };
		let n = 0;
		const ctl = new EmbedController<string>({
			contentSize: () => CONTENT,
			zoom: () => 1,
			isFocused: (v) => v === focus.on,
			focusConsole: () => log.push("focusConsole"),
			create: (app) => {
				if (!configured.includes(app)) return undefined;
				log.push(`create:${app}`);
				return `${app}#${++n}`;
			},
			place: (v, b) => log.push(`place:${v}:${b.width}x${b.height}`),
			conceal: (v) => log.push(`conceal:${v}`),
			destroy: (v) => log.push(`destroy:${v}`),
		});
		return { ctl, log, focus };
	}
	const R = { x: 0, y: 0, width: 100, height: 100 };

	it("lru_evicts_least_recent_at_four", () => {
		const { ctl, log } = harness();
		for (const a of ["forgejo", "zulip", "plane", "glitchtip"] as const)
			expect(ctl.show(a, R).ok).toBe(true);
		expect(ctl.live()).toEqual(["forgejo", "zulip", "plane", "glitchtip"]);
		// Re-showing forgejo makes zulip the least recent.
		ctl.show("forgejo", R);
		ctl.show("matomo", R);
		expect(ctl.live()).toEqual(["plane", "glitchtip", "forgejo", "matomo"]);
		expect(log).toContain("destroy:zulip#2");
		expect(ctl.get("zulip")).toBeUndefined();
		// The shown view is never the one evicted.
		expect(ctl.get("matomo")).toBeDefined();
	});

	it("hides_previous_view_and_keeps_it_alive", () => {
		const { ctl, log } = harness();
		ctl.show("forgejo", R);
		ctl.show("zulip", R);
		expect(log).toContain("conceal:forgejo#1");
		ctl.hide();
		expect(log).toContain("conceal:zulip#2");
		expect(ctl.live()).toEqual(["forgejo", "zulip"]);
		expect(log.some((l) => l.startsWith("destroy"))).toBe(false);
	});

	it("rejects_bad_bounds_and_unconfigured", () => {
		const { ctl, log } = harness(["forgejo"]);
		expect(ctl.show("forgejo", { ...R, width: Number.NaN })).toMatchObject({
			ok: false,
			code: "bad_request",
		});
		expect(ctl.show("zulip", R)).toMatchObject({
			ok: false,
			code: "unconfigured",
		});
		expect(log).toEqual([]);
	});

	it("reconfigure_destroys_view", () => {
		const { ctl, log } = harness();
		ctl.show("forgejo", R);
		ctl.destroy("forgejo");
		expect(log).toContain("destroy:forgejo#1");
		expect(ctl.live()).toEqual([]);
		// The next show builds a fresh view.
		ctl.show("forgejo", R);
		expect(log).toContain("create:forgejo");
		expect(ctl.get("forgejo")).toBe("forgejo#2");
	});

	it("popout_moves_to_window", () => {
		const { ctl, log } = harness();
		ctl.show("forgejo", R);
		const opened: string[] = [];
		const r = ctl.popOut("forgejo", (a) => {
			opened.push(a);
			return { ok: true, value: undefined };
		});
		expect(r.ok).toBe(true);
		expect(opened).toEqual(["forgejo"]);
		expect(log).toContain("destroy:forgejo#1");
		expect(ctl.live()).toEqual([]);
		// A failed window open keeps the embedded view.
		ctl.show("zulip", R);
		const f = ctl.popOut("zulip", () => ({
			ok: false,
			code: "unconfigured",
			message: "x",
		}));
		expect(f.ok).toBe(false);
		expect(ctl.live()).toEqual(["zulip"]);
	});

	it("destroy_all_on_quit", () => {
		const { ctl } = harness();
		ctl.show("forgejo", R);
		ctl.show("zulip", R);
		ctl.destroyAll();
		expect(ctl.live()).toEqual([]);
	});

	it("focus_console_shortcut", () => {
		const k = {
			type: "keyDown",
			key: "l",
			control: true,
			meta: false,
			alt: false,
			shift: false,
		};
		expect(isFocusConsoleKey(k)).toBe(true);
		expect(
			isFocusConsoleKey({ ...k, control: false, meta: true, key: "L" }),
		).toBe(true);
		expect(isFocusConsoleKey({ ...k, control: false })).toBe(false);
		expect(isFocusConsoleKey({ ...k, type: "keyUp" })).toBe(false);
		expect(isFocusConsoleKey({ ...k, key: "j" })).toBe(false);
	});
});
