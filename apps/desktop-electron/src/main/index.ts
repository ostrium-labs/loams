import { spawn } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { app, BrowserWindow, screen, session } from "electron";
import type { EngineState } from "../shared/contracts";
import { appPaths } from "./app-paths";
import { findEngineBinary, probeLiveSupport } from "./engine/binary";
import { registerEngineIpc } from "./engine/ipc.electron";
import { EngineSupervisor } from "./engine/supervisor";
import {
	installAppProtocol,
	registerAppScheme,
} from "./protocol/handler.electron";
import { secureWindow } from "./security/install.electron";
import { registerServerIpc } from "./servers/ipc.electron";
import { ServerRegistry } from "./servers/registry";
import { getMainWindow, setMainWindow } from "./shell/main-window";
import { initSingleInstance } from "./shell/single-instance.electron";
import {
	loadWindowState,
	MIN_HEIGHT,
	MIN_WIDTH,
	saveWindowState,
	type WindowState,
} from "./shell/window-state";

registerAppScheme();

let registry: ServerRegistry;
let engine: EngineSupervisor | undefined;

/** `engine.autoStart` in userData/settings.json; default true. */
function engineAutoStart(): boolean {
	try {
		const raw = JSON.parse(
			readFileSync(join(app.getPath("userData"), "settings.json"), "utf8"),
		) as Record<string, unknown>;
		return raw["engine.autoStart"] !== false;
	} catch {
		return true;
	}
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
	win.on("close", () => {
		clearTimeout(timer);
		persist();
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
			engine.on("state", (st: EngineState) =>
				registry.setLocalUrl(st.phase === "ready" ? st.url : ""),
			);
			registerEngineIpc(engine, logFile);
			if (engineAutoStart()) engine.start();
			installAppProtocol(session.defaultSession, {
				distRoot: paths.consoleDist,
				activeServer: () => registry.active(),
				localShim: () => null, // Task 6
			});
			singleInstance.watchMainWindow(createWindow());
			app.on("activate", () => {
				if (!getMainWindow()) singleInstance.watchMainWindow(createWindow());
			});
		});
		let quitting = false;
		app.on("before-quit", (e) => {
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
