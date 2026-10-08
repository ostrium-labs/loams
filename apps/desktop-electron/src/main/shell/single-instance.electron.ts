import { app, type BrowserWindow, ipcMain } from "electron";
import { CH } from "../../shared/contracts";
import { parseDeepLink } from "../../shared/deeplink";
import { assertTrustedSender } from "../security/policy";
import { NavQueue } from "./nav-queue";

export interface SingleInstanceDeps {
	getWindow: () => BrowserWindow | undefined;
	/** Starts the app; called only in the primary instance. */
	onPrimary: () => void;
}

function linkFromArgv(argv: string[]): string | undefined {
	return argv.find((a) => /^loams:\/\//i.test(a));
}

export interface SingleInstanceHandle {
	/** Call for the main window only: resets the pull handshake on each new page load. */
	watchMainWindow(win: BrowserWindow): void;
}

export function initSingleInstance(
	deps: SingleInstanceDeps,
): SingleInstanceHandle {
	if (!app.requestSingleInstanceLock()) {
		app.quit();
		return { watchMainWindow: () => undefined };
	}
	const queue = new NavQueue();

	const focus = (): BrowserWindow | undefined => {
		const win = deps.getWindow();
		if (win) {
			if (win.isMinimized()) win.restore();
			win.focus();
		}
		return win;
	};

	const deliver = (raw: string): void => {
		const win = focus();
		const link = parseDeepLink(raw);
		if (!link) {
			console.warn("dropped unparseable loams:// link");
			return;
		}
		// Held links are picked up by the renderer via CH.shellPendingNav.
		if (queue.submit(link.path) === "push" && win) {
			win.webContents.send(CH.shellNavigate, link.path);
		}
	};

	ipcMain.handle(CH.shellPendingNav, (event) => {
		assertTrustedSender(event);
		const win = deps.getWindow();
		if (!win || event.sender !== win.webContents) return null;
		return queue.take();
	});
	app.setAsDefaultProtocolClient("loams");
	app.on("second-instance", (_e, argv) => {
		const raw = linkFromArgv(argv);
		if (raw) deliver(raw);
		else focus();
	});
	app.on("open-url", (event, url) => {
		event.preventDefault();
		deliver(url);
	});
	const initial = linkFromArgv(process.argv);
	if (initial) deliver(initial);
	deps.onPrimary();
	return {
		watchMainWindow(win) {
			// A new page load has not subscribed yet; hash-only changes keep the page.
			win.webContents.on(
				"did-start-navigation",
				(details: { isMainFrame: boolean; isSameDocument: boolean }) => {
					if (details.isMainFrame && !details.isSameDocument) queue.reset();
				},
			);
		},
	};
}
