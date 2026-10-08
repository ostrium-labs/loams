import { EventEmitter } from "node:events";
import { describe, expect, it, vi } from "vitest";

const { FakeView } = vi.hoisted(() => {
	class FakeView {
		visible = false;
		webContents = Object.assign(new (require("node:events").EventEmitter)(), {
			loadURL: async () => undefined,
			isDestroyed: () => false,
			isFocused: () => false,
			close: () => undefined,
		});
		setVisible(v: boolean) {
			this.visible = v;
		}
		setBounds() {}
	}
	return { FakeView };
});
vi.mock("electron", () => ({ WebContentsView: FakeView, shell: {} }));

import { FactoryEmbed } from "../src/main/factory/embed.electron";

function fakeWin() {
	const wc = Object.assign(new EventEmitter(), {
		getZoomFactor: () => 1,
		focus: () => undefined,
	});
	const views: InstanceType<typeof FakeView>[] = [];
	const win = Object.assign(new EventEmitter(), {
		webContents: wc,
		isDestroyed: () => false,
		getContentSize: () => [1200, 800],
		contentView: {
			addChildView: (v: InstanceType<typeof FakeView>) => views.push(v),
			removeChildView: () => undefined,
		},
	});
	return { win, wc, views };
}

function embed() {
	return new FactoryEmbed(
		{ appUrls: () => ({ url: "https://git.example.com/" }) },
		{ apply: () => true, forget: () => undefined } as never,
		() => ({ ok: true, value: undefined }),
	);
}
const rect = { x: 10, y: 10, width: 300, height: 200 };

describe("FactoryEmbed", () => {
	it("hides_the_view_when_the_console_renderer_crashes", () => {
		const { win, wc, views } = fakeWin();
		const e = embed();
		expect(e.show(win as never, "forgejo", rect).ok).toBe(true);
		expect(views[0]?.visible).toBe(true);
		wc.emit("render-process-gone", {}, { reason: "crashed" });
		expect(views[0]?.visible).toBe(false);
	});

	it("hides_the_view_when_the_console_reloads_or_navigates", () => {
		const { win, wc, views } = fakeWin();
		const e = embed();
		e.show(win as never, "forgejo", rect);
		// In-page (hash route) navigation keeps it: the page hides it itself on unmount.
		wc.emit("did-start-navigation", {
			isMainFrame: true,
			isSameDocument: true,
		});
		expect(views[0]?.visible).toBe(true);
		// A subframe navigating does not count.
		wc.emit("did-start-navigation", {
			isMainFrame: false,
			isSameDocument: false,
		});
		expect(views[0]?.visible).toBe(true);
		wc.emit("did-start-navigation", {
			isMainFrame: true,
			isSameDocument: false,
		});
		expect(views[0]?.visible).toBe(false);
	});

	it("hooks_the_window_once", () => {
		const { win, wc } = fakeWin();
		const e = embed();
		e.show(win as never, "forgejo", rect);
		e.show(win as never, "forgejo", rect);
		expect(wc.listenerCount("render-process-gone")).toBe(1);
		expect(wc.listenerCount("did-start-navigation")).toBe(1);
	});
});
