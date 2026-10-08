import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
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
import { appPaths } from "./app-paths";
import { findEngineBinary, probeLiveSupport } from "./engine/binary";
import { registerEngineIpc } from "./engine/ipc.electron";
import { EngineSupervisor } from "./engine/supervisor";
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
import { registerShellIpc } from "./shell/shell-ipc.electron";
import { initSingleInstance } from "./shell/single-instance.electron";
import { createTray, type TrayHandle } from "./shell/tray.electron";
import { closeAction } from "./shell/tray-model";
import {
	loadWindowState,
	MIN_HEIGHT,
	MIN_WIDTH,
	saveWindowState,
	type WindowState,
} from "./shell/window-state";

registerAppScheme();
// D663: crash dumps stay on this machine (userData/Crashpad); nothing is uploaded.
crashReporter.start({ uploadToServer: false });

let registry: ServerRegistry;
let engine: EngineSupervisor | undefined;
let factory: FactoryHost | undefined;
let factoryViews: FactoryViews | undefined;
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

/** `shell.closeToTray` in settings.json; default true. */
function closeToTray(): boolean {
	return (
		readSetting<unknown>(settingsFile(), "shell.closeToTray", true) !== false
	);
}

function showMainWindow(create: () => BrowserWindow): void {
	const win = getMainWindow() ?? create();
	if (win.isMinimized()) win.restore();
	win.show();
	win.focus();
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
		},
	});
	secureWindow(win);
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
			factory = new FactoryHost(
				new Vault(join(app.getPath("userData"), "factory", "credentials.bin"), {
					// On Linux, basic_text means no keyring: the "encryption" is a fixed key.
					available: () =>
						safeStorage.isEncryptionAvailable() &&
						(process.platform !== "linux" ||
							safeStorage.getSelectedStorageBackend() !== "basic_text"),
					encrypt: (v) => safeStorage.encryptString(v),
					decrypt: (b) => safeStorage.decryptString(b),
				}),
			);
			factoryViews = new FactoryViews(factory);
			registerFactoryIpc(factory, factoryViews);
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
			if (engineAutoStart()) engine.start();
			installAppProtocol(session.defaultSession, {
				distRoot: paths.consoleDist,
				activeServer: () => registry.active(),
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
				onNotifyClick: () => showMainWindow(open),
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
						singleInstance.navigate("/approvals");
					} else if (id === "servers") {
						showMainWindow(open);
						singleInstance.navigate("/servers");
					} else if (id === "quit") app.quit();
				},
			});
			open();
			app.on("activate", () => showMainWindow(open));
		});
		let quitting = false;
		app.on("before-quit", (e) => {
			isQuitting = true;
			tray?.destroy();
			factoryViews?.closeAll();
			if (quitting || !engine) return;
			e.preventDefault();
			quitting = true;
			void Promise.race([
				engine.stop(),
				new Promise((r) => setTimeout(r, 6000)),
			]).finally(() => app.quit());
		});
		app.on("window-all-closed", () => {
			if (process.platform !== "darwin") app.quit();
		});
	},
});
