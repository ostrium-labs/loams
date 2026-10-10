import { EventEmitter } from "node:events";
import { beforeEach, describe, expect, it, vi } from "vitest";

const { openExternal } = vi.hoisted(() => ({ openExternal: vi.fn() }));
vi.mock("electron", () => ({ shell: { openExternal } }));

import { secureWindow } from "../src/main/security/install.electron";

function fakeWindow() {
	const wc = Object.assign(new EventEmitter(), {
		getURL: () => "loams-app://console/ui/cordis.html",
		setWindowOpenHandler: vi.fn(),
		session: {
			setPermissionCheckHandler: vi.fn(),
			setPermissionRequestHandler: vi.fn(),
		},
	});
	secureWindow({ webContents: wc as never });
	return wc;
}

const ev = (over: { url: string; isMainFrame: boolean }) => ({
	preventDefault: vi.fn(),
	...over,
});

describe("secureWindow navigation guards", () => {
	beforeEach(() => openExternal.mockReset());

	it("will_redirect_main_frame_matches_will_navigate", () => {
		const wc = fakeWindow();
		const ok = ev({ url: "loams-app://console/ui/x", isMainFrame: true });
		wc.emit("will-redirect", ok, ok.url, false, true);
		expect(ok.preventDefault).not.toHaveBeenCalled();

		const ext = ev({ url: "https://idp.example/auth", isMainFrame: true });
		wc.emit("will-redirect", ext, ext.url, false, true);
		expect(ext.preventDefault).toHaveBeenCalled();
		expect(openExternal).toHaveBeenCalledWith("https://idp.example/auth");

		const bad = ev({ url: "file:///etc/passwd", isMainFrame: true });
		wc.emit("will-redirect", bad, bad.url, false, true);
		expect(bad.preventDefault).toHaveBeenCalled();
		expect(openExternal).toHaveBeenCalledTimes(1);
	});

	it("will_redirect_subframe_is_denied_without_opening", () => {
		const wc = fakeWindow();
		const e = ev({ url: "https://evil.example/", isMainFrame: false });
		wc.emit("will-redirect", e, e.url, false, false);
		expect(e.preventDefault).toHaveBeenCalled();
		expect(openExternal).not.toHaveBeenCalled();
		const ok = ev({
			url: "loams-app://console/ui/sandbox/frame.html",
			isMainFrame: false,
		});
		wc.emit("will-redirect", ok, ok.url, false, false);
		expect(ok.preventDefault).not.toHaveBeenCalled();
	});

	it("will_frame_navigate_guards_subframes", () => {
		const wc = fakeWindow();
		const e = ev({ url: "https://evil.example/", isMainFrame: false });
		wc.emit("will-frame-navigate", e);
		expect(e.preventDefault).toHaveBeenCalled();
		expect(openExternal).not.toHaveBeenCalled();
		const js = ev({ url: "javascript:alert(1)", isMainFrame: false });
		wc.emit("will-frame-navigate", js);
		expect(js.preventDefault).toHaveBeenCalled();
		const ok = ev({
			url: "loams-app://console/ui/sandbox/frame.html",
			isMainFrame: false,
		});
		wc.emit("will-frame-navigate", ok);
		expect(ok.preventDefault).not.toHaveBeenCalled();
	});

	it("will_frame_navigate_main_frame_left_to_will_navigate", () => {
		const wc = fakeWindow();
		const e = ev({ url: "https://example.com/", isMainFrame: true });
		wc.emit("will-frame-navigate", e);
		wc.emit("will-navigate", e, e.url);
		// one decision, one external open
		expect(openExternal).toHaveBeenCalledTimes(1);
		expect(e.preventDefault).toHaveBeenCalled();
	});
});
