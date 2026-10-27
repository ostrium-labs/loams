// Studio's preview view (SD1 Task 26): the WebContentsView main places for the Preview
// tab, its isolation and its policy (design §53 §5.8, D678).
import { EventEmitter } from "node:events";
import { describe, expect, it, vi } from "vite-plus/test";

const { FakeView, created, openExternal } = vi.hoisted(() => {
	const created: { options: unknown; view: unknown }[] = [];
	const openExternal = { calls: [] as string[] };
	const { EventEmitter: EE } = require("node:events");
	class FakeView {
		visible = false;
		bounds: unknown;
		loaded: string[] = [];
		session = Object.assign(new EE(), {
			setPermissionCheckHandler: (fn: unknown) => {
				this.check = fn;
			},
			setPermissionRequestHandler: (fn: unknown) => {
				this.request = fn;
			},
		});
		check: unknown;
		request: unknown;
		openHandler: unknown;
		webContents: EventEmitter & Record<string, unknown>;
		constructor(options: unknown) {
			created.push({ options, view: this });
			this.webContents = Object.assign(new EE(), {
				session: this.session,
				loadURL: async (url: string) => {
					this.loaded.push(url);
				},
				isDestroyed: () => false,
				isFocused: () => false,
				isLoading: () => false,
				getURL: () => this.loaded.at(-1) ?? "",
				close: () => undefined,
				reload: () => undefined,
				setWindowOpenHandler: (fn: unknown) => {
					this.openHandler = fn;
				},
				navigationHistory: {
					canGoBack: () => false,
					canGoForward: () => false,
					goBack: () => undefined,
					goForward: () => undefined,
				},
			});
		}
		setVisible(v: boolean) {
			this.visible = v;
		}
		setBounds(b: unknown) {
			this.bounds = b;
		}
	}
	return { FakeView, created, openExternal };
});
vi.mock("electron", () => ({
	WebContentsView: FakeView,
	shell: {
		openExternal: async (url: string) => {
			openExternal.calls.push(url);
		},
	},
}));

import {
	isPreviewUrl,
	PREVIEW_PARTITION,
	previewDownload,
	previewNavigation,
	previewPermission,
	previewWebPreferences,
} from "../src/main/preview/policy";
import { StudioPreviewView } from "../src/main/preview/view.electron";

function fakeWin() {
	const wc = Object.assign(new EventEmitter(), {
		getZoomFactor: () => 1,
		focus: () => undefined,
		send: vi.fn(),
	});
	const children: unknown[] = [];
	const win = Object.assign(new EventEmitter(), {
		webContents: wc,
		isDestroyed: () => false,
		getContentSize: () => [1200, 800],
		contentView: {
			addChildView: (v: unknown) => children.push(v),
			removeChildView: () => undefined,
		},
	});
	return { win, wc, children };
}

const rect = { x: 600, y: 80, width: 500, height: 600 };
const URL_OK = "http://web.my-app.localhost:7331/";

describe("Studio preview view", () => {
	it("desktop_view_has_no_preload_and_own_partition", () => {
		created.length = 0;
		const { win, children } = fakeWin();
		const preview = new StudioPreviewView();
		expect(preview.show(win as never, URL_OK, rect).ok).toBe(true);
		expect(created).toHaveLength(1);
		const options = created[0]?.options as
			| { webPreferences: Record<string, unknown> }
			| undefined;
		const prefs = options?.webPreferences ?? {};
		expect(prefs).toEqual(previewWebPreferences());
		expect(prefs.partition).toBe("persist:studio-preview");
		expect(prefs.partition).toBe(PREVIEW_PARTITION);
		// Not the console's partition, not a factory app's.
		expect(String(prefs.partition)).not.toMatch(/factory|console/);
		expect(prefs.preload).toBeUndefined();
		expect(prefs.sandbox).toBe(true);
		expect(prefs.contextIsolation).toBe(true);
		expect(prefs.nodeIntegration).toBe(false);
		expect(prefs.nodeIntegrationInSubFrames).toBe(false);
		expect(prefs.webviewTag).toBe(false);
		expect(prefs.webSecurity).toBe(true);
		const view = created[0]?.view as InstanceType<typeof FakeView>;
		expect(children).toEqual([view]);
		expect(view.loaded).toEqual([URL_OK]);
		expect(view.visible).toBe(true);
		expect(view.bounds).toEqual(rect);

		// The same address again does not reload; a hide keeps the page.
		preview.show(win as never, URL_OK, rect);
		expect(view.loaded).toHaveLength(1);
		preview.hide();
		expect(view.visible).toBe(false);
	});

	it("refuses_addresses_off_this_machine", () => {
		created.length = 0;
		const { win } = fakeWin();
		const preview = new StudioPreviewView();
		for (const url of [
			"https://example.com/",
			"file:///etc/passwd",
			"loams-app://console/",
			"javascript:alert(1)",
			"http://user:pw@localhost:5173/",
			"http://localhost.evil.example/",
			42,
		]) {
			const r = preview.show(win as never, url, rect);
			expect(r.ok, String(url)).toBe(false);
		}
		expect(created).toHaveLength(0);
	});

	it("hardens_navigation_popups_permissions_and_downloads", () => {
		created.length = 0;
		openExternal.calls.length = 0;
		const { win } = fakeWin();
		new StudioPreviewView().show(win as never, URL_OK, rect);
		const view = created[0]?.view as InstanceType<typeof FakeView>;
		const wc = view.webContents;

		const nav = (url: string, isMainFrame: boolean) => {
			const preventDefault = vi.fn();
			wc.emit("will-frame-navigate", { url, isMainFrame, preventDefault });
			return preventDefault.mock.calls.length > 0;
		};
		expect(nav("http://localhost:3000/api/login", true)).toBe(false);
		expect(nav("https://docs.example.com/", true)).toBe(true);
		expect(openExternal.calls).toEqual(["https://docs.example.com/"]);
		expect(nav("https://ads.example.com/", false)).toBe(true);
		expect(nav("file:///etc/passwd", true)).toBe(true);
		expect(openExternal.calls).toHaveLength(1);

		const open = view.openHandler as (d: { url: string }) => { action: string };
		expect(open({ url: "http://localhost:5173/other" }).action).toBe("deny");
		expect(view.loaded.at(-1)).toBe("http://localhost:5173/other");

		const attach = vi.fn();
		wc.emit("will-attach-webview", { preventDefault: attach });
		expect(attach).toHaveBeenCalled();

		const check = view.check as (
			wc: unknown,
			p: string,
			o: string,
			d: object,
		) => boolean;
		const main = { isMainFrame: true, requestingUrl: URL_OK };
		expect(check(null, "clipboard-sanitized-write", "", main)).toBe(true);
		for (const p of ["media", "geolocation", "notifications", "clipboard-read"])
			expect(check(null, p, "", main)).toBe(false);

		const dl = (url: string) => {
			const preventDefault = vi.fn();
			view.session.emit(
				"will-download",
				{ preventDefault },
				{ getURL: () => url },
			);
			return preventDefault.mock.calls.length === 0;
		};
		expect(dl("http://localhost:5173/report.csv")).toBe(true);
		expect(dl("https://evil.example/x.exe")).toBe(false);
	});

	it("reports_navigation_state_and_load_failures_to_the_console", () => {
		created.length = 0;
		const { win, wc: consoleWc } = fakeWin();
		new StudioPreviewView().show(win as never, URL_OK, rect);
		const view = created[0]?.view as InstanceType<typeof FakeView>;
		view.webContents.emit("did-navigate");
		expect(consoleWc.send).toHaveBeenLastCalledWith("preview:state", {
			url: URL_OK,
			canGoBack: false,
			canGoForward: false,
			loading: false,
		});
		view.webContents.emit(
			"did-fail-load",
			{},
			-102,
			"ERR_CONNECTION_REFUSED",
			URL_OK,
			true,
		);
		expect(consoleWc.send).toHaveBeenLastCalledWith(
			"preview:state",
			expect.objectContaining({
				error: {
					code: -102,
					description: "ERR_CONNECTION_REFUSED",
					url: URL_OK,
				},
			}),
		);
	});

	it("hides_when_the_console_renderer_goes", () => {
		created.length = 0;
		const { win, wc } = fakeWin();
		new StudioPreviewView().show(win as never, URL_OK, rect);
		const view = created[0]?.view as InstanceType<typeof FakeView>;
		wc.emit("render-process-gone", {}, { reason: "crashed" });
		expect(view.visible).toBe(false);
	});
});

describe("preview policy", () => {
	it("allows_only_this_machine", () => {
		expect(isPreviewUrl("http://web.app.localhost:7331/")).toBe(true);
		expect(isPreviewUrl("http://127.0.0.1:8080/x")).toBe(true);
		expect(isPreviewUrl("http://[::1]:3000/")).toBe(true);
		expect(isPreviewUrl("https://localhost:5173/")).toBe(true);
		expect(isPreviewUrl("http://localhost.example.com/")).toBe(false);
		expect(isPreviewUrl("http://10.0.0.1/")).toBe(false);
		expect(isPreviewUrl("ws://localhost:5173/")).toBe(false);
		expect(previewNavigation("https://x.example/", true)).toBe("external");
		expect(previewNavigation("https://x.example/", false)).toBe("deny");
		expect(previewNavigation("data:text/html,hi", true)).toBe("deny");
		expect(
			previewPermission("clipboard-sanitized-write", "https://x.example/"),
		).toBe(false);
		expect(previewDownload("http://localhost/a")).toBe("allow");
	});
});
