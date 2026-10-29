import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it, vi } from "vite-plus/test";
import {
	CHROME,
	ChromeSync,
	type ChromeWindow,
	macBackdrop,
	TRAFFIC_LIGHT,
	titleBarOverlay,
	windowChromeOptions,
} from "../src/main/shell/window-chrome";
import { MIN_HEIGHT, MIN_WIDTH } from "../src/main/shell/window-state";
import { CH } from "../src/shared/contracts";

function fakeWindow(
	o: { minimized?: boolean; visible?: boolean; destroyed?: boolean } = {},
) {
	const calls: [string, unknown][] = [];
	const win: ChromeWindow = {
		isDestroyed: () => o.destroyed ?? false,
		isMinimized: () => o.minimized ?? false,
		isVisible: () => o.visible ?? true,
		setVibrancy: (v) => void calls.push(["vibrancy", v]),
		setBackgroundColor: (c) => void calls.push(["background", c]),
		setTitleBarOverlay: (x) => void calls.push(["overlay", x]),
	};
	return { win, calls };
}

describe("window chrome per OS (D1083)", () => {
	it("macOS: hidden inset, traffic lights at x 16 centred in the 44px bar, sidebar vibrancy", () => {
		const o = windowChromeOptions("darwin", true);
		expect(o).toMatchObject({
			titleBarStyle: "hiddenInset",
			trafficLightPosition: { x: 16, y: 15 },
			vibrancy: "sidebar",
			visualEffectState: "active",
			backgroundColor: "#00000000",
		});
		expect(TRAFFIC_LIGHT.y + 7).toBe(22);
	});

	it.each(["win32", "linux"] as const)(
		"%s: hidden with a 44px title bar overlay in the theme's colours",
		(platform) => {
			expect(windowChromeOptions(platform, true)).toMatchObject({
				titleBarStyle: "hidden",
				titleBarOverlay: {
					color: "#111111",
					symbolColor: "#f5f5f5",
					height: 44,
				},
				backgroundColor: "#111111",
				autoHideMenuBar: true,
			});
			expect(titleBarOverlay(false)).toEqual({
				color: "#f4f2ee",
				symbolColor: "#1d1b18",
				height: 44,
			});
		},
	);

	it("keeps a minimum of 900×640", () => {
		expect([MIN_WIDTH, MIN_HEIGHT]).toEqual([900, 640]);
	});

	it("uses the same colours as the tokens", () => {
		const css = readFileSync(
			resolve(__dirname, "../../../web/packages/ui/src/tokens.css"),
			"utf8",
		);
		const light = css.slice(css.indexOf("\n:root {"), css.indexOf("\n.dark {"));
		const dark = css.slice(css.indexOf("\n.dark {"));
		const token = (block: string, name: string) =>
			new RegExp(`${name}: (#[0-9a-f]{6});`).exec(block)?.[1];
		expect(CHROME.light).toEqual({
			window: token(light, "--op-window"),
			canvas: token(light, "--op-bg"),
			symbol: token(light, "--op-ink"),
		});
		expect(CHROME.dark).toEqual({
			window: token(dark, "--op-window"),
			canvas: token(dark, "--op-bg"),
			symbol: token(dark, "--op-ink"),
		});
	});
});

describe("chrome follows every theme change (Review Focus 3)", () => {
	it("Windows and Linux: the overlay and background update on each switch", () => {
		const { win, calls } = fakeWindow();
		let choice: "system" | "dark" | "light" = "dark";
		const sync = new ChromeSync({
			platform: "linux",
			window: () => win,
			choice: () => choice,
			osDark: () => false,
		});
		sync.onTheme(true);
		sync.onTheme(false);
		expect(calls).toEqual([
			["background", "#111111"],
			["overlay", { color: "#111111", symbolColor: "#f5f5f5", height: 44 }],
			["background", "#f4f2ee"],
			["overlay", { color: "#f4f2ee", symbolColor: "#1d1b18", height: 44 }],
		]);
		calls.length = 0;
		sync.onOsThemeChanged(); // an explicit choice ignores the OS
		expect(calls).toEqual([]);
		choice = "system";
		sync.onOsThemeChanged(); // System follows it (the OS says light here)
		expect(calls.at(-1)).toEqual([
			"overlay",
			{ color: "#f4f2ee", symbolColor: "#1d1b18", height: 44 },
		]);
	});

	it("macOS: vibrancy while visible, a solid backdrop while hidden or minimised", () => {
		expect(macBackdrop({ minimized: false, visible: true }, true)).toEqual({
			vibrancy: "sidebar",
			background: "#00000000",
		});
		expect(macBackdrop({ minimized: true, visible: true }, true)).toEqual({
			vibrancy: null,
			background: "#111111",
		});
		expect(macBackdrop({ minimized: false, visible: false }, false)).toEqual({
			vibrancy: null,
			background: "#f4f2ee",
		});
		const { win, calls } = fakeWindow({ minimized: true });
		new ChromeSync({
			platform: "darwin",
			window: () => win,
			choice: () => "dark",
			osDark: () => true,
		}).onTheme(true);
		expect(calls).toEqual([
			["vibrancy", null],
			["background", "#111111"],
		]);
	});

	it("leaves a destroyed window alone", () => {
		const { win, calls } = fakeWindow({ destroyed: true });
		new ChromeSync({
			platform: "win32",
			window: () => win,
			choice: () => "dark",
			osDark: () => true,
		}).onTheme(false);
		expect(calls).toEqual([]);
	});
});

const ipc = vi.hoisted(() => {
	const listeners = new Map<string, (e: unknown, v: unknown) => void>();
	return {
		listeners,
		on: (ch: string, fn: (e: unknown, v: unknown) => void) =>
			void listeners.set(ch, fn),
		removeListener: (ch: string) => void listeners.delete(ch),
	};
});
vi.mock("electron", () => ({
	ipcRenderer: { on: ipc.on, removeListener: ipc.removeListener },
}));

describe("the preload marks <html> for the console's chrome rules", () => {
	it("sets data-platform, follows fullscreen, and stops on unmount", async () => {
		const { mountWindowChrome } = await import("../src/preload/window-chrome");
		const dataset: Record<string, string | undefined> = {};
		const doc = {
			documentElement: { dataset },
			addEventListener: () => {},
			removeEventListener: () => {},
		};
		const unmount = mountWindowChrome(doc);
		expect(dataset.platform).toBe(process.platform);
		expect(dataset.fullscreen).toBeUndefined();
		const send = ipc.listeners.get(CH.shellFullscreen);
		send?.({}, true);
		expect(dataset.fullscreen).toBe("true");
		send?.({}, "yes"); // not a boolean: ignored
		expect(dataset.fullscreen).toBe("true");
		send?.({}, false);
		expect(dataset.fullscreen).toBeUndefined();
		unmount();
		expect(ipc.listeners.has(CH.shellFullscreen)).toBe(false);
	});
});
