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
	wc.on("will-navigate", (event, url) => {
		const d = navigationDecision(wc.getURL(), url);
		if (d === "allow") return;
		event.preventDefault();
		if (d === "external") void shell.openExternal(url);
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
