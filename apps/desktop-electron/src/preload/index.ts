import { contextBridge, ipcRenderer } from "electron";
import {
	CH,
	type LoamsDesktopApi,
	type StackId,
	type StackState,
} from "../shared/contracts";
import { versionFromArgv } from "../shared/version";

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
	// The main process passes app.getVersion() as an additional argument; a sandboxed
	// preload has no app.getVersion() and process.env does not carry it.
	version: versionFromArgv(process.argv),
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
		showEmbedded: (app, rect) => invoke(CH.factoryShowEmbedded, app, rect),
		hideEmbedded: () => invoke(CH.factoryHideEmbedded),
		reloadEmbedded: (app) => invoke(CH.factoryReloadEmbedded, app),
		popOut: (app) => invoke(CH.factoryPopOut, app),
	},
	shell: {
		openExternal: (url) => invoke(CH.shellOpenExternal, url),
		notify: (title, body, route) => invoke(CH.shellNotify, title, body, route),
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
	stacks: {
		state: (id) => invoke(CH.stacksState, id),
		start: (id) => invoke(CH.stacksStart, id),
		stop: (id) => invoke(CH.stacksStop, id),
		onState: (cb) => {
			const listener = (_e: unknown, id: StackId, s: StackState) => cb(id, s);
			ipcRenderer.on(CH.stacksEvent, listener);
			return () => {
				ipcRenderer.removeListener(CH.stacksEvent, listener);
			};
		},
	},
	connectors: {
		catalog: () => invoke(CH.connectorsCatalog),
		get: (id) => invoke(CH.connectorsGet, id),
		validate: (id, config) => invoke(CH.connectorsValidate, id, config),
		saveYaml: (name, text) => invoke(CH.connectorsSaveYaml, name, text),
	},
	chat: {
		providers: () => invoke(CH.chatProviders),
		configureProvider: (id, cfg) => invoke(CH.chatConfigureProvider, id, cfg),
		list: () => invoke(CH.chatList),
		get: (chatId) => invoke(CH.chatGet, chatId),
		create: (opts) => invoke(CH.chatCreate, opts),
		send: (chatId, text, opts) => invoke(CH.chatSend, chatId, text, opts),
		cancel: (chatId) => invoke(CH.chatCancel, chatId),
		approve: (chatId, callId, decision) =>
			invoke(CH.chatApprove, chatId, callId, decision),
		remove: (chatId) => invoke(CH.chatRemove, chatId),
		onEvent: (cb) => subscribe(CH.chatEvent, cb),
	},
};

contextBridge.exposeInMainWorld("loamsDesktop", api);
