import { join } from "node:path";
import { app, BrowserWindow, screen, session } from "electron";
import { appPaths } from "./app-paths";
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
			installAppProtocol(session.defaultSession, {
				distRoot: (await appPaths()).consoleDist,
				activeServer: () => registry.active(),
				localShim: () => null, // Task 6
			});
			singleInstance.watchMainWindow(createWindow());
			app.on("activate", () => {
				if (!getMainWindow()) singleInstance.watchMainWindow(createWindow());
			});
		});
		app.on("window-all-closed", () => {
			if (process.platform !== "darwin") app.quit();
		});
	},
});
