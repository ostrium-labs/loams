import {
	type BrowserWindow,
	type WebContents,
	WebContentsView,
} from "electron";
import type { FactoryAppId, IpcResult } from "../../shared/contracts";
import { EmbedController, isFocusConsoleKey } from "./embed-model";
import { FactoryHardening } from "./hardening.electron";
import { webOrigin } from "./view-policy";

interface UrlSource {
	appUrls(app: FactoryAppId): { url: string; ssoOrigin?: string } | undefined;
}

/**
 * D678: each factory app as a WebContentsView inside the console's own window.
 * Same partition, preload-less sandbox and navigation/permission/download
 * policy as the standalone window, applied by the shared FactoryHardening.
 * The pure lifecycle (LRU of 4, bounds, destroy) lives in EmbedController.
 */
export class FactoryEmbed {
	#win: BrowserWindow | undefined;
	readonly #ctl: EmbedController<WebContentsView>;

	constructor(
		private readonly source: UrlSource,
		private readonly hardening: FactoryHardening,
		private readonly openWindow: (app: FactoryAppId) => IpcResult<void>,
	) {
		this.#ctl = new EmbedController<WebContentsView>({
			contentSize: () => {
				const [width, height] = this.#win?.getContentSize() ?? [0, 0];
				return { width: width ?? 0, height: height ?? 0 };
			},
			zoom: () => this.#win?.webContents.getZoomFactor() ?? 1,
			isFocused: (v) =>
				!v.webContents.isDestroyed() && v.webContents.isFocused(),
			focusConsole: () => {
				if (this.#win && !this.#win.isDestroyed())
					this.#win.webContents.focus();
			},
			create: (app) => this.#create(app),
			place: (v, b) => {
				v.setBounds(b);
				v.setVisible(true);
			},
			conceal: (v) => v.setVisible(false),
			destroy: (v) => this.#destroyView(v),
		});
	}

	#create(app: FactoryAppId): WebContentsView | undefined {
		const win = this.#win;
		const cfg = this.source.appUrls(app);
		if (!win || win.isDestroyed() || !cfg || !webOrigin(cfg.url))
			return undefined;
		const view = new WebContentsView({
			webPreferences: FactoryHardening.webPreferences(app),
		});
		const wc = view.webContents;
		this.hardening.apply(app, wc, cfg);
		// The view is a separate webContents, so the console's shortcuts never
		// reach it. Ctrl/Cmd+L is the one way back: focus the console.
		wc.on("before-input-event", (event, input) => {
			if (!isFocusConsoleKey(input)) return;
			event.preventDefault();
			if (!win.isDestroyed()) win.webContents.focus();
		});
		view.setVisible(false);
		win.contentView.addChildView(view);
		void wc.loadURL(cfg.url);
		return view;
	}

	#destroyView(view: WebContentsView): void {
		const win = this.#win;
		if (win && !win.isDestroyed()) win.contentView.removeChildView(view);
		const wc: WebContents = view.webContents;
		if (!wc.isDestroyed()) wc.close();
	}

	show(win: BrowserWindow, app: FactoryAppId, rect: unknown): IpcResult<void> {
		if (this.#win !== win) {
			this.#ctl.destroyAll();
			this.#win = win;
			win.once("closed", () => {
				if (this.#win === win) {
					this.#ctl.destroyAll();
					this.#win = undefined;
				}
			});
		}
		return this.#ctl.show(app, rect);
	}

	hide(): void {
		this.#ctl.hide();
	}

	reload(app: FactoryAppId): void {
		const wc = this.#ctl.get(app)?.webContents;
		if (wc && !wc.isDestroyed()) wc.reload();
	}

	destroy(app: FactoryAppId): void {
		this.#ctl.destroy(app);
	}

	/** The app was removed: drop its view and its hardening config. */
	remove(app: FactoryAppId): void {
		this.#ctl.destroy(app);
		this.hardening.forget(app);
	}

	popOut(app: FactoryAppId): IpcResult<void> {
		return this.#ctl.popOut(app, this.openWindow);
	}

	closeAll(): void {
		this.#ctl.destroyAll();
	}
}
