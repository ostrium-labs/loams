// Adapted from dataelement/dsh-desktop (MIT), src/preload/macos-window-chrome.ts.
// Marks <html> with the OS and the fullscreen state, so the console's CSS reserves the traffic
// lights' or the title bar controls' space only when it is there (design 59, D1083). Upstream
// marks macOS only and injects a sidebar style; Loams marks every OS, and console.css holds
// the rules.
import { ipcRenderer } from "electron";
import { CH } from "../shared/contracts";

/** The part of `Document` this needs (the preload compiles without the DOM library). */
export interface ChromeDocument {
	documentElement: { dataset: Record<string, string | undefined> } | null;
	addEventListener(
		type: "DOMContentLoaded",
		fn: () => void,
		o?: { once: boolean },
	): void;
	removeEventListener(type: "DOMContentLoaded", fn: () => void): void;
}

export function mountWindowChrome(doc: ChromeDocument): () => void {
	let fullscreen = false;
	const mark = (): void => {
		const root = doc.documentElement;
		if (!root) return;
		root.dataset.platform = process.platform;
		if (fullscreen) root.dataset.fullscreen = "true";
		else delete root.dataset.fullscreen;
	};
	mark();
	doc.addEventListener("DOMContentLoaded", mark, { once: true });
	const receive = (_e: unknown, value: unknown): void => {
		if (typeof value !== "boolean") return;
		fullscreen = value;
		mark();
	};
	ipcRenderer.on(CH.shellFullscreen, receive);
	return () => {
		doc.removeEventListener("DOMContentLoaded", mark);
		ipcRenderer.removeListener(CH.shellFullscreen, receive);
	};
}
