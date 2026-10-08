import { join } from "node:path";
import { BrowserWindow, app as electronApp, screen, shell } from "electron";
import type { FactoryAppId, IpcResult } from "../../shared/contracts";
import {
	loadWindowState,
	MIN_HEIGHT,
	MIN_WIDTH,
	saveWindowState,
	type WindowState,
} from "../shell/window-state";
import { FACTORY_APPS } from "./apps";
import { viewNavigation, viewPermission, webOrigin } from "./view-policy";

interface UrlSource {
	appUrls(app: FactoryAppId): { url: string; ssoOrigin?: string } | undefined;
}

/**
 * One full-UI window per factory app. No preload and no IPC bridge: the page is
 * untrusted web content, and no credential is ever injected into it.
 * These windows are never registered as the main window, so the deep-link
 * navigation queue never touches them.
 */
export class FactoryViews {
	readonly #wins = new Map<FactoryAppId, BrowserWindow>();

	constructor(private readonly source: UrlSource) {}

	open(app: FactoryAppId): IpcResult<void> {
		const existing = this.#wins.get(app);
		if (existing && !existing.isDestroyed()) {
			if (existing.isMinimized()) existing.restore();
			existing.focus();
			return { ok: true, value: undefined };
		}
		const cfg = this.source.appUrls(app);
		const appOrigin = cfg ? webOrigin(cfg.url) : undefined;
		if (!cfg || !appOrigin)
			return {
				ok: false,
				code: "unconfigured",
				message: "This app is not configured yet",
			};
		const label = FACTORY_APPS[app].label;
		const stateFile = join(
			electronApp.getPath("userData"),
			"factory",
			`window-${app}.json`,
		);
		const state = loadWindowState(
			stateFile,
			screen.getAllDisplays().map((d) => d.workArea),
		);
		const win = new BrowserWindow({
			width: state.width,
			height: state.height,
			x: state.x,
			y: state.y,
			minWidth: MIN_WIDTH,
			minHeight: MIN_HEIGHT,
			show: false,
			title: `${label} — Loams Desktop`,
			webPreferences: {
				partition: `persist:factory-${app}`,
				preload: undefined,
				sandbox: true,
				contextIsolation: true,
				nodeIntegration: false,
				webSecurity: true,
				webviewTag: false,
			},
		});
		this.#wins.set(app, win);
		const wc = win.webContents;
		const decide = (to: string) => viewNavigation(appOrigin, to, cfg.ssoOrigin);
		const guard = (event: { preventDefault(): void }, url: string): void => {
			const d = decide(url);
			if (d === "allow") return;
			event.preventDefault();
			if (d === "external") void shell.openExternal(url);
		};
		wc.on("will-navigate", (event, url) => guard(event, url));
		wc.on("will-redirect", (event, url) => guard(event, url));
		wc.setWindowOpenHandler(({ url }) => {
			if (decide(url) === "external") void shell.openExternal(url);
			// Allowed same-origin popups also open in this window rather than a new one.
			else if (decide(url) === "allow") void wc.loadURL(url);
			return { action: "deny" };
		});
		wc.on("will-attach-webview", (event) => event.preventDefault());
		// A page's own <title> must not replace the app label.
		wc.on("page-title-updated", (event) => event.preventDefault());
		const ses = wc.session;
		ses.setPermissionCheckHandler(
			(_wc, permission, requestingOrigin, details) =>
				details.isMainFrame !== false &&
				viewPermission(
					permission,
					details.requestingUrl ?? requestingOrigin,
					appOrigin,
				),
		);
		ses.setPermissionRequestHandler((_wc, permission, callback, details) =>
			callback(
				details.isMainFrame !== false &&
					viewPermission(permission, details.requestingUrl, appOrigin),
			),
		);

		if (state.isMaximized) win.maximize();
		let last: WindowState = state;
		const persist = (): void => {
			if (win.isDestroyed()) return;
			const maximized = win.isMaximized();
			if (!maximized && !win.isMinimized())
				last = { ...win.getBounds(), isMaximized: false };
			else last = { ...last, isMaximized: maximized };
			saveWindowState(stateFile, last);
		};
		let timer: NodeJS.Timeout | undefined;
		const schedule = (): void => {
			clearTimeout(timer);
			timer = setTimeout(persist, 400);
		};
		win.on("resize", schedule);
		win.on("move", schedule);
		win.on("maximize", schedule);
		win.on("unmaximize", schedule);
		win.on("close", () => {
			clearTimeout(timer);
			persist();
		});
		win.on("closed", () => {
			if (this.#wins.get(app) === win) this.#wins.delete(app);
		});
		win.once("ready-to-show", () => win.show());
		void win.loadURL(cfg.url);
		return { ok: true, value: undefined };
	}

	close(app: FactoryAppId): void {
		const w = this.#wins.get(app);
		this.#wins.delete(app);
		if (w && !w.isDestroyed()) w.close();
	}

	closeAll(): void {
		for (const a of [...this.#wins.keys()]) this.close(a);
	}
}
