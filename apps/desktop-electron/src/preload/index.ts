import { contextBridge, ipcRenderer } from "electron";
import { CH, type LoamsDesktopApi } from "../shared/contracts";

const invoke = <T>(ch: string, ...args: unknown[]): Promise<T> =>
	ipcRenderer.invoke(ch, ...args) as Promise<T>;

function subscribe<T>(ch: string, cb: (v: T) => void): () => void {
	const listener = (_e: unknown, v: T) => cb(v);
	ipcRenderer.on(ch, listener);
	return () => {
		ipcRenderer.removeListener(ch, listener);
	};
}

const api: LoamsDesktopApi = {
	version: process.env.LOAMS_DESKTOP_VERSION ?? "0.0.0",
	platform: process.platform,
	servers: {
		list: () => invoke(CH.serversList),
		add: (e) => invoke(CH.serversAdd, e),
		remove: (id) => invoke(CH.serversRemove, id),
		activate: (id) => invoke(CH.serversActivate, id),
	},
	engine: {
		state: () => invoke(CH.engineState),
		start: () => invoke(CH.engineStart),
		stop: () => invoke(CH.engineStop),
		openLogs: () => invoke(CH.engineLogs),
		onState: (cb) => subscribe(CH.engineEvent, cb),
	},
	factory: {
		list: () => invoke(CH.factoryList),
		configure: (app, url, fields) =>
			invoke(CH.factoryConfigure, app, url, fields),
		test: (app) => invoke(CH.factoryTest, app),
		remove: (app) => invoke(CH.factoryRemove, app),
		query: (q) => invoke(CH.factoryQuery, q),
		openApp: (app) => invoke(CH.factoryOpen, app),
		closeApp: (app) => invoke(CH.factoryClose, app),
	},
	shell: {
		openExternal: (url) => invoke(CH.shellOpenExternal, url),
		notify: (title, body) => invoke(CH.shellNotify, title, body),
		clipboardWrite: (text) => invoke(CH.shellClipboard, text),
		onNavigate: (cb) => subscribe(CH.shellNavigate, cb),
		takePendingNavigation: () => invoke(CH.shellPendingNav),
		setBadge: (count) => invoke(CH.shellBadge, count),
	},
	update: {
		state: () => invoke(CH.updateState),
		check: () => invoke(CH.updateCheck),
		download: () => invoke(CH.updateDownload),
		installAndRestart: () => invoke(CH.updateInstall),
	},
};

contextBridge.exposeInMainWorld("loamsDesktop", api);
