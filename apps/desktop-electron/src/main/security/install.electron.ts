// Adapted from dataelement/dsh-desktop (MIT), src/main/security.ts.
import { type BrowserWindow, shell } from "electron";
import { navigationDecision, permissionDecision } from "./policy";

export { assertTrustedSender } from "./policy";

function openIfHttp(url: string): void {
	if (navigationDecision("", url) === "external") void shell.openExternal(url);
}

export function secureWindow(window: Pick<BrowserWindow, "webContents">): void {
	const wc = window.webContents;
	wc.setWindowOpenHandler(({ url }) => {
		openIfHttp(url);
		return { action: "deny" };
	});
	/** Main frame: console stays, http(s) opens in the browser. Subframes: console only. */
	const guard = (
		event: { preventDefault(): void },
		url: string,
		isMainFrame: boolean,
	): void => {
		const d = navigationDecision(wc.getURL(), url);
		if (d === "allow") return;
		event.preventDefault();
		if (d === "external" && isMainFrame) void shell.openExternal(url);
	};
	wc.on("will-navigate", (event, url) => guard(event, url, true));
	// A server-side 3xx does not pass will-navigate again.
	wc.on("will-redirect", (event, url, _inPlace, isMainFrame) =>
		guard(event, url, isMainFrame),
	);
	// Subframes (will-navigate covers the main frame, so it is not handled twice).
	wc.on("will-frame-navigate", (event) => {
		if (!event.isMainFrame) guard(event, event.url, false);
	});
	wc.on("will-attach-webview", (event) => event.preventDefault());
	wc.session.setPermissionCheckHandler(
		(_wc, permission, requestingOrigin, details) =>
			details.isMainFrame !== false &&
			permissionDecision(permission, details.requestingUrl ?? requestingOrigin),
	);
	wc.session.setPermissionRequestHandler((_wc, permission, callback, details) =>
		callback(
			details.isMainFrame !== false &&
				permissionDecision(permission, details.requestingUrl),
		),
	);
}
