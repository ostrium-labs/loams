import { spawn } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { userInfo } from "node:os";
import { join } from "node:path";
import {
	app,
	BrowserWindow,
	crashReporter,
	safeStorage,
	screen,
	session,
	shell,
} from "electron";
import type { EngineState } from "../shared/contracts";
import { VERSION_ARG } from "../shared/version";
import { registerChatIpc } from "./agent/ipc.electron";
import { registerTools } from "./agent/tools";
import type { ChatService } from "./agent/service";
import { appPaths } from "./app-paths";
import { ConnectorCatalog, catalogPath } from "./connectors/catalog";
import { registerConnectorsIpc } from "./connectors/ipc.electron";
import { findEngineBinary, probeLiveSupport } from "./engine/binary";
import { registerEngineIpc } from "./engine/ipc.electron";
import { EngineSupervisor } from "./engine/supervisor";
import { FactoryEmbed } from "./factory/embed.electron";
import { FactoryHardening } from "./factory/hardening.electron";
import { FactoryHost } from "./factory/host";
import { registerFactoryIpc } from "./factory/ipc.electron";
import { Vault } from "./factory/vault";
import { FactoryViews } from "./factory/views.electron";
import {
	installAppProtocol,
	registerAppScheme,
} from "./protocol/handler.electron";
import { createLocalShim } from "./protocol/local-shim";
import { secureWindow } from "./security/install.electron";
import { registerServerIpc } from "./servers/ipc.electron";
import { ServerRegistry } from "./servers/registry";
import { readSetting } from "./settings";
import { getMainWindow, setMainWindow } from "./shell/main-window";
import { installAppMenu } from "./shell/menu.electron";
import { DOCS_URL } from "./shell/menu-model";
import { reveal } from "./shell/reveal";
import { registerShellIpc } from "./shell/shell-ipc.electron";
import { initSingleInstance } from "./shell/single-instance.electron";
import { createTray, type TrayHandle } from "./shell/tray.electron";
import { closeAction, TRAY_ROUTES } from "./shell/tray-model";
import {
	loadWindowState,
	MIN_HEIGHT,
	MIN_WIDTH,
	saveWindowState,
	type WindowState,
} from "./shell/window-state";
import { createSqlServices, registerSqlIpc } from "./sql/ipc.electron";
import { sqlAgentTools } from "./sql/tools";
import { createStackManager, registerStacksIpc } from "./stacks/ipc.electron";
import { bindLiveToTikv } from "./stacks/stacks";
import { startUpdater, type UpdaterHandle } from "./update/updater.electron";

// Test hook (unpackaged builds only): the e2e smoke runs against a scratch profile, which
// also gives it its own single-instance lock.
if (!app.isPackaged && process.env.LOAMS_DESKTOP_USER_DATA)
	app.setPath("userData", process.env.LOAMS_DESKTOP_USER_DATA);

registerAppScheme();
// D663: crash dumps stay on this machine (userData/Crashpad); nothing is uploaded.
crashReporter.start({ uploadToServer: false });

let registry: ServerRegistry;
let engine: EngineSupervisor | undefined;
let updater: UpdaterHandle | undefined;
let installingOnQuit = false;
let factory: FactoryHost | undefined;
let factoryViews: FactoryViews | undefined;
let factoryEmbed: FactoryEmbed | undefined;
let chat: ChatService | undefined;
let tray: TrayHandle | undefined;
let isQuitting = false;
const settingsFile = (): string =>
	join(app.getPath("userData"), "settings.json");

/** `engine.autoStart` in userData/settings.json; default true. */
function engineAutoStart(): boolean {
	return (
		readSetting<unknown>(settingsFile(), "engine.autoStart", true) !== false
	);
}

/** `shell.closeToTray` in settings.json; default: true on Windows, false elsewhere. */
function closeToTray(): boolean | undefined {
	const v = readSetting<unknown>(
		settingsFile(),
		"shell.closeToTray",
		undefined,
	);
	return typeof v === "boolean" ? v : undefined;
}

function showMainWindow(create: () => BrowserWindow): void {
	const win = getMainWindow() ?? create();
	reveal(win);
}

function createWindow(): BrowserWindow {
	const stateFile = join(app.getPath("userData"), "window-state.json");
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
		title: "Loams Desktop",
		webPreferences: {
			preload: join(__dirname, "../preload/index.cjs"),
			contextIsolation: true,
			sandbox: true,
			nodeIntegration: false,
			webSecurity: true,
			webviewTag: false,
			additionalArguments: [`${VERSION_ARG}${app.getVersion()}`],
		},
	});
	secureWindow(win);
	// The console sets document.title per page; the native title stays the product name.
	win.on("page-title-updated", (e) => e.preventDefault());
	setMainWindow(win);
	win.on("closed", () => setMainWindow(undefined));
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
	win.on("close", (e) => {
		clearTimeout(timer);
		persist();
		const action = closeAction({
			platform: process.platform,
			hasTray: tray !== undefined,
			closeToTray: closeToTray(),
			quitting: isQuitting,
		});
		if (action === "hide") {
			e.preventDefault();
			win.hide();
		}
	});
	win.once("ready-to-show", () => win.show());
	void win.loadURL("loams-app://console/ui/cordis.html");
	return win;
}

const singleInstance = initSingleInstance({
	getWindow: getMainWindow,
	onPrimary: () => {
		void app.whenReady().then(async () => {
			app.setAppUserModelId("dev.loams.desktop");
			registry = new ServerRegistry(
				join(app.getPath("userData"), "servers.json"),
				{ devDemo: !app.isPackaged },
			);
			registerServerIpc(registry);
			const connectorsFile = catalogPath({
				isPackaged: app.isPackaged,
				resourcesPath: process.resourcesPath,
				appRoot: app.getAppPath(),
			});
			const connectors = new ConnectorCatalog(() =>
				readFileSync(connectorsFile, "utf8"),
			);
			registerConnectorsIpc(connectors);
			const vault = new Vault(
				join(app.getPath("userData"), "factory", "credentials.bin"),
				{
					// On Linux, basic_text means no keyring: the "encryption" is a fixed key.
					available: () =>
						safeStorage.isEncryptionAvailable() &&
						(process.platform !== "linux" ||
							safeStorage.getSelectedStorageBackend() !== "basic_text"),
					encrypt: (v) => safeStorage.encryptString(v),
					decrypt: (b) => safeStorage.decryptString(b),
				},
			);
			factory = new FactoryHost(vault);
			// Postgres and WeSQL services: the pages (IPC) and the agent tools share them.
			const sql = createSqlServices(app.getPath("userData"));
			registerTools(sqlAgentTools(sql));
			chat = registerChatIpc({
				session: session.defaultSession,
				vault,
				factory,
				connectors,
			});
			const hardening = new FactoryHardening();
			const views = new FactoryViews(factory, hardening);
			factoryViews = views;
			factoryEmbed = new FactoryEmbed(factory, hardening, (a) => views.open(a));
			registerFactoryIpc(factory, views, factoryEmbed);
			const paths = await appPaths();
			const logFile = join(paths.logs, "engine.log");
			engine = new EngineSupervisor({
				spawn,
				binary: () => findEngineBinary(paths.engineBin, existsSync),
				dataDir: paths.engineData,
				logFile,
				fetch,
				now: Date.now,
				sleep: (ms) => new Promise((r) => setTimeout(r, ms)),
				liveSupported: probeLiveSupport,
			});
			engine.on("state", (st: EngineState) => {
				registry.setLocalUrl(st.phase === "ready" ? st.url : "");
				tray?.refresh();
			});
			registerEngineIpc(engine, logFile);
			const stacks = createStackManager({
				stacksDir: paths.stacksDir,
				logsDir: paths.logs,
			});
			bindLiveToTikv(stacks, engine, (e) =>
				console.error("[stacks] setLivePd failed:", e),
			);
			registerStacksIpc(stacks, paths.logs);
			registerSqlIpc(sql);
			// A tikv stack left running from last time starts the engine with Live directly,
			// but a slow or absent runtime must not hold the engine back for long.
			await Promise.race([
				stacks.state("tikv"),
				new Promise((r) => setTimeout(r, 2000)),
			]);
			if (engineAutoStart()) engine.start();
			installAppProtocol(session.defaultSession, {
				distRoot: paths.consoleDist,
				activeServer: () => registry.active(),
				engineState: () => engine?.state() ?? { phase: "stopped" },
				localShim: createLocalShim(
					{ version: app.getVersion(), username: userInfo().username },
					() => registry.active().kind,
				),
			});
			const open = (): BrowserWindow => {
				const w = createWindow();
				singleInstance.watchMainWindow(w);
				return w;
			};
			registerShellIpc({
				onBadge: (n) => tray?.setBadge(n),
				onNotifyClick: (route) => {
					showMainWindow(open);
					if (route) singleInstance.navigate(route);
				},
			});
			updater = startUpdater({
				prepareToInstall: async () => {
					await engine?.stop();
				},
				restartEngine: async () => {
					engine?.start();
				},
			});
			app.setAboutPanelOptions({
				applicationName: "Loams Desktop",
				applicationVersion: app.getVersion(),
				website: "https://loams.dev",
			});
			const openLogs = (): void => void shell.openPath(paths.logs);
			installAppMenu({
				dev: !app.isPackaged,
				onAction: (id) => {
					if (id === "quit") app.quit();
					else if (id === "docs") void shell.openExternal(DOCS_URL);
					else if (id === "logs") openLogs();
					else if (id === "about") app.showAboutPanel();
				},
			});
			const eng = engine;
			tray = createTray({
				getEngine: () => eng.state(),
				getServer: () => registry.active(),
				onAction: (id) => {
					if (id === "open") showMainWindow(open);
					else if (id === "engine")
						void (eng.state().phase === "ready" ? eng.stop() : eng.start());
					else if (id === "approvals") {
						showMainWindow(open);
						singleInstance.navigate(TRAY_ROUTES.approvals);
					} else if (id === "servers") {
						showMainWindow(open);
						singleInstance.navigate(TRAY_ROUTES.servers);
					} else if (id === "quit") app.quit();
				},
			});
			open();
			app.on("activate", () => showMainWindow(open));
		});
		let quitting = false;
		let chatStopping: Promise<void> | undefined;
		app.on("before-quit", (e) => {
			isQuitting = true;
			// Stop running agent turns and let their chat files land, bounded (below).
			chatStopping ??= chat?.dispose().catch(() => undefined);
			tray?.destroy();
			factoryEmbed?.closeAll();
			factoryViews?.closeAll();
			// Quit-time install of a hash-verified download (D661); runs prepareToInstall first.
			if (updater?.hasVerifiedDownload() && !installingOnQuit) {
				installingOnQuit = true;
				e.preventDefault();
				void updater.installOnQuit().finally(() => app.quit());
				return;
			}
			if (quitting || (!engine && !chatStopping)) return;
			e.preventDefault();
			quitting = true;
			void Promise.race([
				Promise.all([engine?.stop(), chatStopping]),
				new Promise((r) => setTimeout(r, 6000)),
			]).finally(() => app.quit());
		});
		app.on("window-all-closed", () => {
			if (process.platform !== "darwin") app.quit();
		});
	},
});
