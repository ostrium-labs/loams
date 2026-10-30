import { contextBridge, ipcRenderer } from "electron";
import {
	CH,
	type LoamsDesktopApi,
	PREVIEW_CH,
	type StackId,
	TRANSFER_CH,
	type StackState,
} from "../shared/contracts";
import { themeFromArgv, versionFromArgv } from "../shared/version";
import { installNotificationShim } from "./notification-shim";
import { type ChromeDocument, mountWindowChrome } from "./window-chrome";

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
	// The saved theme choice, passed like the version (D1074).
	theme: {
		initial: themeFromArgv(process.argv),
		set: (choice, dark) => invoke(CH.themeSet, choice, dark),
	},
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
		liveStore: () => invoke(CH.engineLiveStore),
		setLiveStore: (choice) => invoke(CH.engineSetLiveStore, choice),
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
		pickFolder: (title) => invoke(CH.shellPickFolder, title),
		setBadge: (count) => invoke(CH.shellBadge, count),
		onCommand: (cb) => subscribe(CH.shellCommand, cb),
		onFullscreen: (cb) => subscribe(CH.shellFullscreen, cb),
		ready: () => invoke(CH.shellReady),
	},
	// SD1 Task 28: Studio's editor hand-off and notification choices; main builds every
	// editor URL or argv itself.
	studio: {
		openInEditor: (path, at) => invoke(CH.studioOpenInEditor, path, at),
		editor: () => invoke(CH.studioEditorGet),
		setEditor: (editor) => invoke(CH.studioEditorSet, editor),
		chooseEditorCommand: () => invoke(CH.studioEditorChooseCommand),
		setNotifications: (prefs) => invoke(CH.studioNotifySet, prefs),
	},
	pgControl: {
		settings: () => invoke(CH.pgControlSettings),
		save: (input) => invoke(CH.pgControlSave, input),
		test: (input) => invoke(CH.pgControlTest, input),
		pickKeyFile: () => invoke(CH.pgControlPickKeyFile),
		onChanged: (cb) => subscribe(CH.pgControlEvent, cb),
	},
	update: {
		state: () => invoke(CH.updateState),
		check: () => invoke(CH.updateCheck),
		autoCheck: () => invoke(CH.updateAutoGet),
		setAutoCheck: (enabled) => invoke(CH.updateAutoSet, enabled),
		download: () => invoke(CH.updateDownload),
		installAndRestart: () => invoke(CH.updateInstall),
		onState: (cb) => subscribe(CH.updateState, cb),
		skip: (version) => invoke(CH.updateSkip, version),
	},
	stacks: {
		state: (id) => invoke(CH.stacksState, id),
		start: (id) => invoke(CH.stacksStart, id),
		stop: (id) => invoke(CH.stacksStop, id),
		reset: (id) => invoke(CH.stacksReset, id),
		openLogs: (id) => invoke(CH.stacksOpenLogs, id),
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
		testProvider: (id) => invoke(CH.chatTestProvider, id),
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
	pg: {
		tenants: () => invoke(CH.pgTenants),
		timelines: (t) => invoke(CH.pgTimelines, t),
		createBranch: (t, b) => invoke(CH.pgCreateBranch, t, b),
		walStatus: (t, tl) => invoke(CH.pgWalStatus, t, tl),
		connection: () => invoke(CH.pgConnection),
		revealPassword: () => invoke(CH.pgRevealPassword),
		query: (sql) => invoke(CH.pgQuery, sql),
	},
	wesql: {
		connection: () => invoke(CH.wesqlConnection),
		revealPassword: () => invoke(CH.wesqlRevealPassword),
		schemas: () => invoke(CH.wesqlSchemas),
		tables: (schema) => invoke(CH.wesqlTables, schema),
		query: (sql) => invoke(CH.wesqlQuery, sql),
	},
	// DD1: the daemon's token stays in main; these are named, validated channels.
	agentd: {
		status: () => invoke(CH.agentdStatus),
		onStatus: (cb) => subscribe(CH.agentdEvent, cb),
		sessions: () => invoke(CH.agentdSessions),
		reattach: (sessionId) => invoke(CH.agentdReattach, sessionId),
		start: () => invoke(CH.agentdStart),
		stop: (opts) => invoke(CH.agentdStop, opts),
		background: {
			get: () => invoke(CH.agentdBackgroundGet),
			set: (v) => invoke(CH.agentdBackgroundSet, v),
		},
	},
	preview: {
		show: (url, rect) => invoke(PREVIEW_CH.show, url, rect),
		hide: () => invoke(PREVIEW_CH.hide),
		reload: () => invoke(PREVIEW_CH.reload),
		back: () => invoke(PREVIEW_CH.back),
		forward: () => invoke(PREVIEW_CH.forward),
		onState: (cb) => subscribe(PREVIEW_CH.state, cb),
	},
	transfers: {
		snapshot: () => invoke(TRANSFER_CH.snapshot),
		onChange: (cb) => subscribe(TRANSFER_CH.event, cb),
		hasKey: (serverId) => invoke(TRANSFER_CH.hasToken, serverId),
		saveKey: (serverId, key, settings) =>
			invoke(TRANSFER_CH.setToken, serverId, key, settings),
		upload: (target) => invoke(TRANSFER_CH.upload, target),
		download: (target, tid, name) =>
			invoke(TRANSFER_CH.download, target, tid, name),
		pause: (id) => invoke(TRANSFER_CH.pause, id),
		resume: (id) => invoke(TRANSFER_CH.resume, id),
		cancel: (id) => invoke(TRANSFER_CH.cancel, id),
		dismiss: (id) => invoke(TRANSFER_CH.dismiss, id),
		remoteList: (target) => invoke(TRANSFER_CH.remoteList, target),
		startRun: (target, job, params) =>
			invoke(TRANSFER_CH.startRun, target, job, params),
		followRun: (target, rid, job) =>
			invoke(TRANSFER_CH.followRun, target, rid, job),
	},
};

// html[data-platform] and data-fullscreen for the console's window chrome rules (D1083).
// The preload compiles without the DOM library; in the renderer these globals exist.
const page = globalThis as unknown as {
	document: ChromeDocument;
	addEventListener(type: "unload", fn: () => void, o: { once: boolean }): void;
};
const unmountChrome = mountWindowChrome(page.document);
page.addEventListener("unload", unmountChrome, { once: true });
contextBridge.exposeInMainWorld("loamsDesktop", api);
// Page code's `new Notification(...)` posts through the bridge above (CF1 Task 16, D1085).
installNotificationShim();
