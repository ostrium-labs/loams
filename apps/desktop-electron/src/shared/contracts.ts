export type ServerKind = "local" | "remote" | "demo";
export interface ServerEntry {
	id: string;
	name: string;
	kind: ServerKind;
	url: string;
} // url = origin
export type EngineState =
	| { phase: "stopped" }
	| { phase: "starting"; attempt: number }
	| {
			phase: "ready";
			url: string;
			esUrl: string;
			flightUrl: string;
			durableUrl: string;
			liveUrl?: string;
			pid: number;
	  }
	| { phase: "failed"; reason: string; logPath: string };
export type FactoryAppId =
	| "forgejo"
	| "zulip"
	| "plane"
	| "glitchtip"
	| "openpanel"
	| "matomo"
	| "langfuse"
	| "openobserve";
export type FactoryHealth =
	| "unconfigured"
	| "ok"
	| "auth_failed"
	| "unreachable";
export interface FactoryAppInfo {
	id: FactoryAppId;
	label: string;
	url?: string;
	health: FactoryHealth;
	hasPanels: boolean;
	credentialFields: { key: string; label: string; secret: boolean }[];
	persistent: boolean; // false when safeStorage has no backend (session-only)
}
export interface FactoryQuery {
	app: FactoryAppId;
	op: string;
	params: Record<string, unknown>;
}
export type IpcResult<T> =
	| { ok: true; value: T }
	| { ok: false; code: string; message: string };

export interface LoamsDesktopApi {
	version: string;
	platform: NodeJS.Platform;
	servers: {
		list(): Promise<{ servers: ServerEntry[]; activeId: string }>;
		add(e: Omit<ServerEntry, "id">): Promise<IpcResult<ServerEntry>>;
		remove(id: string): Promise<IpcResult<void>>;
		activate(id: string): Promise<IpcResult<void>>; // reloads the window
	};
	engine: {
		state(): Promise<EngineState>;
		start(): Promise<void>;
		stop(): Promise<void>;
		openLogs(): Promise<void>;
		onState(cb: (s: EngineState) => void): () => void;
	};
	factory: {
		list(): Promise<FactoryAppInfo[]>;
		configure(
			app: FactoryAppId,
			url: string,
			fields: Record<string, string>,
		): Promise<IpcResult<FactoryAppInfo>>;
		test(app: FactoryAppId): Promise<FactoryAppInfo>;
		remove(app: FactoryAppId): Promise<void>;
		query<T = unknown>(q: FactoryQuery): Promise<IpcResult<T>>;
		openApp(app: FactoryAppId): Promise<IpcResult<void>>;
		closeApp(app: FactoryAppId): Promise<void>;
	};
	shell: {
		openExternal(url: string): Promise<IpcResult<void>>;
		notify(title: string, body?: string): Promise<void>;
		clipboardWrite(text: string): Promise<void>;
		onNavigate(cb: (path: string) => void): () => void; // deep links
		/** Call once after subscribing with onNavigate; returns a link received before the page was ready. */
		takePendingNavigation(): Promise<string | null>;
		setBadge(count: number): Promise<void>; // pending approvals in tray
	};
	update: {
		state(): Promise<{
			phase:
				| "disabled"
				| "idle"
				| "checking"
				| "available"
				| "downloading"
				| "ready"
				| "error";
			version?: string;
			message?: string;
		}>;
		check(): Promise<void>;
		download(): Promise<void>;
		installAndRestart(): Promise<void>;
	};
}
export const CH = {
	serversList: "servers:list",
	serversAdd: "servers:add",
	serversRemove: "servers:remove",
	serversActivate: "servers:activate",
	engineState: "engine:state",
	engineStart: "engine:start",
	engineStop: "engine:stop",
	engineLogs: "engine:logs",
	engineEvent: "engine:event",
	factoryList: "factory:list",
	factoryConfigure: "factory:configure",
	factoryTest: "factory:test",
	factoryRemove: "factory:remove",
	factoryQuery: "factory:query",
	factoryOpen: "factory:open",
	factoryClose: "factory:close",
	shellOpenExternal: "shell:open-external",
	shellNotify: "shell:notify",
	shellClipboard: "shell:clipboard",
	shellNavigate: "shell:navigate",
	shellPendingNav: "shell:pending-nav",
	shellBadge: "shell:badge",
	updateState: "update:state",
	updateCheck: "update:check",
	updateDownload: "update:download",
	updateInstall: "update:install",
} as const;
