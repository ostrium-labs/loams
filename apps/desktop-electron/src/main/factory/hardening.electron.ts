import { shell, type WebContents } from "electron";
import type { FactoryAppId } from "../../shared/contracts";
import {
	downloadDecision,
	frameNavigation,
	viewPermission,
	webOrigin,
} from "./view-policy";

export interface AppTarget {
	url: string;
	ssoOrigin?: string;
}

interface Cfg {
	appOrigin: string;
	ssoOrigin?: string;
}

/**
 * The ONE place that hardens factory app web contents, used by both the
 * standalone window (Task 12) and the embedded view (Task 12b). Navigation
 * guards attach per webContents; permission and download handlers attach once
 * per partition session and read the current app config, so a window and a view
 * in the same partition cannot disagree.
 */
export class FactoryHardening {
	readonly #cfg = new Map<FactoryAppId, Cfg>();
	readonly #sessions = new WeakSet<object>();

	/** The partition every view of `app` shares. */
	static partition(app: FactoryAppId): string {
		return `persist:factory-${app}`;
	}

	/** Web preferences for an app view: sandboxed, no preload, no bridge. */
	static webPreferences(app: FactoryAppId) {
		return {
			partition: FactoryHardening.partition(app),
			preload: undefined,
			sandbox: true,
			contextIsolation: true,
			nodeIntegration: false,
			webSecurity: true,
			webviewTag: false,
		} as const;
	}

	/** Apply the guards. Returns false when the URL is not a web URL. */
	apply(app: FactoryAppId, wc: WebContents, target: AppTarget): boolean {
		const appOrigin = webOrigin(target.url);
		if (!appOrigin) return false;
		const sso = target.ssoOrigin;
		this.#cfg.set(app, { appOrigin, ssoOrigin: sso });
		const guard = (
			event: { preventDefault(): void },
			url: string,
			isMainFrame: boolean,
		): void => {
			const d = frameNavigation(appOrigin, url, isMainFrame, sso);
			if (d === "allow") return;
			event.preventDefault();
			if (d === "external") void shell.openExternal(url);
		};
		// will-frame-navigate covers the main frame and every subframe (will-navigate
		// would double-fire for the main frame).
		wc.on("will-frame-navigate", (e) => guard(e, e.url, e.isMainFrame));
		wc.on("will-redirect", (e) => guard(e, e.url, e.isMainFrame));
		wc.setWindowOpenHandler(({ url }) => {
			const d = frameNavigation(appOrigin, url, true, sso);
			if (d === "external") void shell.openExternal(url);
			// Allowed same-origin popups open in this view rather than a new one.
			else if (d === "allow") void wc.loadURL(url);
			return { action: "deny" };
		});
		wc.on("will-attach-webview", (event) => event.preventDefault());
		// A page's own <title> must not replace the app label.
		wc.on("page-title-updated", (event) => event.preventDefault());
		this.#hookSession(app, wc);
		return true;
	}

	/** The app was removed or reconfigured: forget its config (denies everything). */
	forget(app: FactoryAppId): void {
		this.#cfg.delete(app);
	}

	#hookSession(app: FactoryAppId, wc: WebContents): void {
		const ses = wc.session;
		if (this.#sessions.has(ses)) return;
		this.#sessions.add(ses);
		ses.on("will-download", (event, item) => {
			const c = this.#cfg.get(app);
			if (
				!c ||
				downloadDecision(c.appOrigin, item.getURL(), c.ssoOrigin) !== "allow"
			)
				event.preventDefault();
			// Allowed: Electron's default save dialog (no setSavePath).
		});
		ses.setPermissionCheckHandler(
			(_wc, permission, requestingOrigin, details) => {
				const c = this.#cfg.get(app);
				return (
					!!c &&
					details.isMainFrame !== false &&
					viewPermission(
						permission,
						details.requestingUrl ?? requestingOrigin,
						c.appOrigin,
					)
				);
			},
		);
		ses.setPermissionRequestHandler((_wc, permission, callback, details) => {
			const c = this.#cfg.get(app);
			callback(
				!!c &&
					details.isMainFrame !== false &&
					viewPermission(permission, details.requestingUrl, c.appOrigin),
			);
		});
	}
}
