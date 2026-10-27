// Adapted from dataelement/dsh-desktop (MIT), src/main/macos-window-backdrop.ts.
// Match the sidebar's native material and avoid transparent restore flashes (design 59,
// D1083). Upstream picks its own solid colours; here the caller passes the theme's window
// colour (window-chrome.ts `CHROME`), so this file has no colour of its own.
import type { BrowserWindow } from "electron";

export type MacBackdrop = { vibrancy: "sidebar" | null; background: string };

/** Vibrancy while shown; a solid `solid` while hidden or minimised, so a restore does not flash. */
export function macosBackdrop(
	s: { minimized: boolean; visible: boolean },
	solid: string,
): MacBackdrop {
	if (s.minimized || !s.visible) return { vibrancy: null, background: solid };
	return { vibrancy: "sidebar", background: "#00000000" };
}

export function applyMacosWindowBackdrop(
	window: Pick<
		BrowserWindow,
		| "isDestroyed"
		| "isMinimized"
		| "isVisible"
		| "setVibrancy"
		| "setBackgroundColor"
	>,
	solid: string,
): void {
	if (window.isDestroyed()) return;
	const b = macosBackdrop(
		{ minimized: window.isMinimized(), visible: window.isVisible() },
		solid,
	);
	window.setVibrancy(b.vibrancy);
	window.setBackgroundColor(b.background);
}
