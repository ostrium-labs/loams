import { app, type BrowserWindow } from "electron";
import { CH } from "../../shared/contracts";
import { parseDeepLink } from "../../shared/deeplink";

export interface SingleInstanceDeps {
	getWindow: () => BrowserWindow | undefined;
	/** Starts the app; called only in the primary instance. */
	onPrimary: () => void;
}

function linkFromArgv(argv: string[]): string | undefined {
	return argv.find((a) => /^loams:\/\//i.test(a));
}

export function initSingleInstance(deps: SingleInstanceDeps): void {
	if (!app.requestSingleInstanceLock()) {
		app.quit();
		return;
	}
	let pending: string | undefined;

	const deliver = (raw: string): void => {
		const win = deps.getWindow();
		if (!win) {
			pending = raw;
			return;
		}
		if (win.isMinimized()) win.restore();
		win.focus();
		const link = parseDeepLink(raw);
		if (!link) {
			console.warn("dropped unparseable loams:// link");
			return;
		}
		win.webContents.send(CH.shellNavigate, link.path);
	};

	app.setAsDefaultProtocolClient("loams");
	app.on("second-instance", (_e, argv) => {
		const raw = linkFromArgv(argv);
		if (raw) return deliver(raw);
		const win = deps.getWindow();
		if (win) {
			if (win.isMinimized()) win.restore();
			win.focus();
		}
	});
	app.on("open-url", (event, url) => {
		event.preventDefault();
		deliver(url);
	});
	pending = linkFromArgv(process.argv);
	// Links that arrived before the window existed are delivered once it loads.
	app.on("browser-window-created", (_e, w) => {
		w.webContents.once("did-finish-load", () => {
			if (pending) {
				const p = pending;
				pending = undefined;
				deliver(p);
			}
		});
	});
	deps.onPrimary();
}
