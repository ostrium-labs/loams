// Adapted from dataelement/dsh-desktop (MIT), src/main/index.ts.
// One window style per OS (design 59, D1083): macOS gets a hidden inset title bar with the
// traffic lights centred in the 44px top bar and sidebar vibrancy; Windows and Linux get a
// hidden title bar with a 44px overlay whose colours follow the page theme. Upstream's
// createWindow options, windowsTitleBarOverlay and applyWindowChromeTheme, made pure; the
// values are Loams' (the tokens, a test compares them). Linux needs an opaque overlay colour
// (as T3 Code notes, #00000000 does not work there), so both get the window colour.

import type {
	BrowserWindow,
	BrowserWindowConstructorOptions,
	TitleBarOverlayOptions,
} from "electron";
import type { ThemeChoice } from "../../shared/contracts";
import {
	applyMacosWindowBackdrop,
	type MacBackdrop,
	macosBackdrop,
} from "./macos-window-backdrop";

/** The console's top bar (`--op-topbar`), which the OS title bar controls sit inside. */
export const TOPBAR_HEIGHT = 44;
const MAC_BUTTON_RADIUS = 7;
export const TRAFFIC_LIGHT = {
	x: 16,
	y: TOPBAR_HEIGHT / 2 - MAC_BUTTON_RADIUS,
} as const;

/** --op-window, --op-bg and --op-ink from web/packages/ui/src/tokens.css (test/window-chrome.test.ts). */
export const CHROME = {
	dark: { window: "#111111", canvas: "#0a0a0a", symbol: "#f5f5f5" },
	light: { window: "#f4f2ee", canvas: "#ffffff", symbol: "#1d1b18" },
} as const;

const colours = (dark: boolean) => (dark ? CHROME.dark : CHROME.light);

export function titleBarOverlay(dark: boolean): TitleBarOverlayOptions {
	const c = colours(dark);
	return { color: c.window, symbolColor: c.symbol, height: TOPBAR_HEIGHT };
}

export function windowChromeOptions(
	platform: NodeJS.Platform,
	dark: boolean,
): BrowserWindowConstructorOptions {
	if (platform === "darwin") {
		return {
			titleBarStyle: "hiddenInset",
			trafficLightPosition: { ...TRAFFIC_LIGHT },
			vibrancy: "sidebar",
			visualEffectState: "active",
			backgroundColor: "#00000000",
		};
	}
	return {
		titleBarStyle: "hidden",
		titleBarOverlay: titleBarOverlay(dark),
		backgroundColor: colours(dark).window,
		autoHideMenuBar: true,
	};
}

/** dsh-desktop's backdrop swap: vibrancy while shown, the theme's window colour while hidden. */
export function macBackdrop(
	s: { minimized: boolean; visible: boolean },
	dark: boolean,
): MacBackdrop {
	return macosBackdrop(s, colours(dark).window);
}

export type ChromeWindow = Pick<
	BrowserWindow,
	| "isDestroyed"
	| "isMinimized"
	| "isVisible"
	| "setVibrancy"
	| "setBackgroundColor"
	| "setTitleBarOverlay"
>;

export function applyWindowChrome(
	win: ChromeWindow,
	platform: NodeJS.Platform,
	dark: boolean,
): void {
	if (win.isDestroyed()) return;
	if (platform === "darwin") {
		applyMacosWindowBackdrop(win, colours(dark).window);
		return;
	}
	win.setBackgroundColor(colours(dark).window);
	win.setTitleBarOverlay(titleBarOverlay(dark));
}

export interface ChromeSyncOptions {
	platform: NodeJS.Platform;
	window: () => ChromeWindow | undefined;
	choice: () => ThemeChoice;
	osDark: () => boolean;
}

/** Keeps the native chrome on the page theme: each `theme:set`, and each OS flip under System. */
export class ChromeSync {
	#dark: boolean;
	readonly #o: ChromeSyncOptions;

	constructor(o: ChromeSyncOptions) {
		this.#o = o;
		const choice = o.choice();
		this.#dark = choice === "dark" || (choice === "system" && o.osDark());
	}

	get dark(): boolean {
		return this.#dark;
	}

	onTheme(dark: boolean): void {
		this.#dark = dark;
		this.apply();
	}

	onOsThemeChanged(): void {
		if (this.#o.choice() !== "system") return;
		this.onTheme(this.#o.osDark());
	}

	/** Re-applies the current theme (the macOS backdrop on hide, show, minimise and restore). */
	apply(): void {
		const win = this.#o.window();
		if (win) applyWindowChrome(win, this.#o.platform, this.#dark);
	}
}
